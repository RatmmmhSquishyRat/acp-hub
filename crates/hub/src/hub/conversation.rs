use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::state::{
    CoreHub, OperationKind, OperationLease, ReplayPruneGuard, SessionIdentityLease,
};
use super::types::{ConversationCreated, CreateConversationParams, MessagesPageParams, RunCreated};
use crate::acp::{AgentCommand, AgentHandle, SessionCreated};
use crate::callbacks::SessionBinding;
use crate::endpoint::AgentEndpointConfig;
use crate::error::HubError;
use crate::runtime::SessionState;
use crate::store::{
    ConvOrigin, ConvStatus, ConversationRow, ListConversationsFilter, MessagePageQuery, MessageRow,
    NewConversation, NewConversationOptions, ReplayRefresh, RunStatus, SearchPage,
};
use tokio::sync::oneshot;
use uuid::Uuid;

pub(super) fn wrap_load_failure(
    endpoint: String,
    conv_id: String,
    agent_session_id: String,
    source: HubError,
) -> HubError {
    match source {
        source @ HubError::Conflict(_) => source,
        source => HubError::ResumeLoadFailed {
            attempted_method: "session/load",
            endpoint,
            conv_id,
            agent_session_id,
            source: Box::new(source),
        },
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ReplayMethod {
    Load,
    Resume,
}

struct RefreshPublication {
    binding: SessionBinding,
    remove_conversation_on_error: bool,
}

struct RefreshProjectionOptions {
    method: ReplayMethod,
    operation_keepalive: Option<Arc<OperationLease>>,
    publication: Option<RefreshPublication>,
    prepared_refresh: Option<ReplayRefresh>,
}

impl CoreHub {
    /// Create a Hub conversation, issuing ACP `session/new` or `session/load`.
    pub async fn create_conversation(
        &self,
        params: CreateConversationParams,
    ) -> Result<ConversationCreated, HubError> {
        let cwd = require_absolute_cwd(params.cwd)?;
        let additional = params.additional_directories;
        let supplied_identity = if let Some(agent_session_id) = &params.agent_session_id {
            let existing = self
                .store()
                .conversation_by_agent_session(&params.agent_id, agent_session_id)?;
            let conv_id = existing.as_ref().map_or_else(
                || format!("conv-{}", Uuid::new_v4().simple()),
                |row| row.id.clone(),
            );
            let lease =
                self.reserve_session_identity(&params.agent_id, agent_session_id, &conv_id)?;
            Some((existing, conv_id, lease))
        } else {
            None
        };
        // Bind path: existing row for agent_session_id
        if let Some((Some(existing), _, _)) = &supplied_identity {
            let existing = existing.clone();
            // Promote imported_list → bound; keep hub_created; revive deleted as bound.
            self.store()
                .promote_conversation_bind(&existing.id, existing.session_meta.as_ref())?;
            let refreshed = self
                .store()
                .conversation(&existing.id)?
                .ok_or_else(|| HubError::not_found("conversation", &existing.id))?;
            if let Err(source) = self
                .refresh_session_projection_external(&refreshed, cwd.clone(), ReplayMethod::Load)
                .await
            {
                // Keep bound/hub_created row; never session/new (PHASE1 §4.2).
                return Err(wrap_load_failure(
                    params.agent_id.clone(),
                    existing.id.clone(),
                    existing.agent_session_id.clone(),
                    source,
                ));
            }
            let row = self
                .store()
                .conversation(&existing.id)?
                .ok_or_else(|| HubError::not_found("conversation", &existing.id))?;
            return Ok(conversation_created_from_row(&row));
        }

        let conv_id = supplied_identity.as_ref().map_or_else(
            || format!("conv-{}", Uuid::new_v4().simple()),
            |(_, conv_id, _)| conv_id.clone(),
        );
        let operation =
            Arc::new(self.reserve_operation(&conv_id, &params.agent_id, OperationKind::Refresh)?);
        let agent_cfg = self.agent_config(&params.agent_id)?;
        let handle = self.agent_handle(&params.agent_id).await?;
        let created = if let Some(agent_session_id) = params.agent_session_id {
            // Bind create: no existing row → insert origin=bound; load-fail keeps row.
            let additional_strings = additional
                .iter()
                .map(|p| path_to_string(p))
                .collect::<Result<Vec<_>, _>>()?;
            self.store().create_conversation_with_options(
                &NewConversation {
                    id: conv_id.clone(),
                    agent_id: params.agent_id.clone(),
                    agent_session_id: agent_session_id.clone(),
                    cwd: Some(path_to_string(&cwd)?),
                    additional_directories: additional_strings,
                    title: None,
                },
                &NewConversationOptions {
                    origin: ConvOrigin::Bound,
                    session_meta: None,
                },
            )?;
            let row = self
                .store()
                .conversation(&conv_id)?
                .ok_or_else(|| HubError::not_found("conversation", &conv_id))?;
            match self
                .refresh_session_projection_owned(
                    &handle,
                    &row,
                    cwd.clone(),
                    RefreshProjectionOptions {
                        method: ReplayMethod::Load,
                        operation_keepalive: Some(Arc::clone(&operation)),
                        publication: Some(RefreshPublication {
                            binding: SessionBinding {
                                conv_id: conv_id.clone(),
                                agent_id: params.agent_id.clone(),
                                permission_policy: agent_cfg.permission_policy,
                                fs: agent_cfg.client_capabilities.fs.clone(),
                                cwd: cwd.clone(),
                            },
                            // Keep bound row on load failure (PHASE1).
                            remove_conversation_on_error: false,
                        }),
                        prepared_refresh: None,
                    },
                )
                .await
            {
                Ok(created) => created,
                Err(source) => {
                    self.ctx.unbind_session(&params.agent_id, &agent_session_id);
                    self.runtime.remove(&conv_id);
                    return Err(wrap_load_failure(
                        params.agent_id.clone(),
                        conv_id.clone(),
                        agent_session_id,
                        source,
                    ));
                }
            }
        } else {
            let additional_strings = additional
                .iter()
                .map(|path| path_to_string(path))
                .collect::<Result<Vec<_>, _>>()?;
            let cwd_string = path_to_string(&cwd)?;
            let publication_generation = self
                .ctx
                .acquire_connection_lease(&params.agent_id, &handle.connection_id)
                .await?;
            let permit = handle.cmd_tx.clone().reserve_owned().await.map_err(|_| {
                HubError::other(format!("agent {} command loop is closed", params.agent_id))
            })?;
            let mut creation_capture = self
                .ctx
                .begin_session_creation_capture(&params.agent_id, &handle.connection_id)?;
            let (reply, response) = oneshot::channel();
            permit.send(AgentCommand::CreateSession {
                conv_id: conv_id.clone(),
                agent_id: params.agent_id.clone(),
                cwd: cwd.clone(),
                additional_directories: additional.clone(),
                mcp_servers: params.mcp_servers,
                reply,
            });

            let ctx = Arc::clone(&self.ctx);
            let runtime = Arc::clone(&self.runtime);
            let session_identities = Arc::clone(&self.session_identities);
            let agent_id = params.agent_id;
            let worker_conv_id = conv_id;
            let worker = tokio::spawn(async move {
                let _operation = operation;
                let _publication_generation = publication_generation;
                let created = match response.await {
                    Ok(result) => result?,
                    Err(_) => {
                        return Err(HubError::other(format!(
                            "agent {agent_id} command response dropped"
                        )));
                    }
                };
                let _identity = match SessionIdentityLease::acquire(
                    session_identities,
                    &agent_id,
                    &created.agent_session_id,
                    &worker_conv_id,
                ) {
                    Ok(identity) => identity,
                    Err(error) => {
                        return match creation_capture.reject(&created.agent_session_id) {
                            Ok(()) => Err(error),
                            Err(cleanup_error) => Err(HubError::other(format!(
                                "session/new identity publication failed ({error}) and capture rollback failed ({cleanup_error})"
                            ))),
                        };
                    }
                };
                let mut local_claimed = false;
                let mut row_created = false;
                let publication = (|| {
                    if let Some(existing) = ctx
                        .store()
                        .conversation_by_agent_session(&agent_id, &created.agent_session_id)?
                    {
                        return Err(HubError::Conflict(existing.id));
                    }
                    if ctx.is_session_bound(&agent_id, &created.agent_session_id) {
                        return Err(HubError::Conflict(created.agent_session_id.clone()));
                    }
                    local_claimed = true;
                    ctx.store().create_conversation(&NewConversation {
                        id: worker_conv_id.clone(),
                        agent_id: agent_id.clone(),
                        agent_session_id: created.agent_session_id.clone(),
                        cwd: Some(cwd_string),
                        additional_directories: additional_strings,
                        title: None,
                    })?;
                    row_created = true;
                    ctx.store().replace_static_snapshots(
                        &worker_conv_id,
                        created.config_options.as_ref(),
                        created.modes.as_ref(),
                    )?;
                    creation_capture.publish(&created.agent_session_id)?;
                    ctx.bind_session(
                        &created.agent_session_id,
                        SessionBinding {
                            conv_id: worker_conv_id.clone(),
                            agent_id: agent_id.clone(),
                            permission_policy: agent_cfg.permission_policy,
                            fs: agent_cfg.client_capabilities.fs,
                            cwd,
                        },
                    )?;
                    runtime.insert(
                        &worker_conv_id,
                        SessionState::Live,
                        runtime.next_generation(),
                    );
                    Ok(ConversationCreated {
                        conv_id: worker_conv_id.clone(),
                        agent_id: agent_id.clone(),
                        agent_session_id: created.agent_session_id.clone(),
                        status: "idle".to_string(),
                        origin: "hub_created".to_string(),
                        interaction: "writable".to_string(),
                    })
                })();
                match publication {
                    Ok(created) => Ok(created),
                    Err(publication_error) => {
                        if local_claimed {
                            ctx.unbind_session_if_conversation(
                                &agent_id,
                                &created.agent_session_id,
                                &worker_conv_id,
                            );
                            runtime.remove(&worker_conv_id);
                        }
                        let cleanup = if row_created {
                            ctx.store().hard_delete_conversation(&worker_conv_id)
                        } else {
                            Ok(())
                        };
                        let capture_cleanup = creation_capture.reject(&created.agent_session_id);
                        match (capture_cleanup, cleanup) {
                            (Ok(()), Ok(())) => Err(publication_error),
                            (Err(capture_error), Ok(())) => Err(HubError::other(format!(
                                "session/new publication failed ({publication_error}) and capture rollback failed ({capture_error})"
                            ))),
                            (Ok(()), Err(cleanup_error)) => Err(HubError::other(format!(
                                "session/new publication failed ({publication_error}) and local rollback failed ({cleanup_error})"
                            ))),
                            (Err(capture_error), Err(cleanup_error)) => {
                                Err(HubError::other(format!(
                                    "session/new publication failed ({publication_error}); capture rollback failed ({capture_error}); local rollback failed ({cleanup_error})"
                                )))
                            }
                        }
                    }
                }
            });
            return worker.await.map_err(|error| {
                HubError::other(format!("create-session worker failed: {error}"))
            })?;
        };

        if self.store().conversation(&conv_id)?.is_none() {
            let additional_strings = additional
                .iter()
                .map(|p| path_to_string(p))
                .collect::<Result<Vec<_>, _>>()?;
            self.store().create_conversation_with_options(
                &NewConversation {
                    id: conv_id.clone(),
                    agent_id: params.agent_id.clone(),
                    agent_session_id: created.agent_session_id.clone(),
                    cwd: Some(path_to_string(&cwd)?),
                    additional_directories: additional_strings,
                    title: None,
                },
                &NewConversationOptions {
                    origin: ConvOrigin::HubCreated,
                    session_meta: None,
                },
            )?;
            self.persist_session_snapshots(&conv_id, &created)?;
        }
        let row = self
            .store()
            .conversation(&conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", &conv_id))?;
        Ok(conversation_created_from_row(&row))
    }

    /// List Hub conversations (default workbench when using filtered API).
    pub fn list_conversations(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<ConversationRow>, HubError> {
        let mut filter = ListConversationsFilter::workbench_default();
        filter.agent_id = agent_id.map(str::to_string);
        Ok(self.store().list_conversations_filtered(&filter)?.items)
    }

    pub fn list_conversations_filtered(
        &self,
        filter: &ListConversationsFilter,
    ) -> Result<crate::store::ConversationListPage, HubError> {
        self.store().list_conversations_filtered(filter)
    }

    /// Return stored conversation messages.
    pub fn messages(
        &self,
        conv_id: &str,
        include_audit: bool,
    ) -> Result<Vec<MessageRow>, HubError> {
        self.ensure_conversation(conv_id)?;
        self.store().messages(conv_id, include_audit)
    }

    pub fn messages_page(
        &self,
        params: &MessagesPageParams,
    ) -> Result<crate::store::MessagePage, HubError> {
        self.ensure_conversation(&params.conv_id)?;
        self.store().messages_page_query(MessagePageQuery {
            conv_id: &params.conv_id,
            include_audit: params.include_audit,
            run_id: params.run_id.as_deref(),
            after_seq: params.after_seq,
            cursor: params.cursor.as_deref(),
            limit: params.limit,
            offset: params.offset,
        })
    }

    pub fn max_message_seq(&self, conv_id: &str) -> Result<i64, HubError> {
        self.ensure_conversation(conv_id)?;
        self.store().max_message_seq(conv_id)
    }

    /// Search current and audit projection text.
    pub fn search(
        &self,
        query: &str,
        agent_id: Option<&str>,
        conv_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<SearchPage, HubError> {
        self.store().search(query, agent_id, conv_id, limit, offset)
    }

    /// Create and persist a new running row.
    pub fn create_run(&self, conv_id: &str) -> Result<RunCreated, HubError> {
        let conv = self.ensure_conversation(conv_id)?;
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        let owner_token = self.reserve_external_run(conv_id, &conv.agent_id, &run_id)?;
        if let Err(error) = self.store().create_run(&run_id, conv_id) {
            self.release_operation(conv_id, owner_token);
            return Err(error);
        }
        Ok(RunCreated {
            run_id,
            owner_token: owner_token.to_string(),
        })
    }

    /// Compare-and-set run finalization.
    pub fn finalize_run(
        &self,
        conv_id: &str,
        run_id: &str,
        owner_token: &str,
        status: RunStatus,
        stop_reason: Option<&str>,
    ) -> Result<bool, HubError> {
        let owner_token =
            Uuid::parse_str(owner_token).map_err(|_| HubError::Conflict(conv_id.to_string()))?;
        {
            let operations = self.operations.lock();
            let Some(entry) = operations.get(conv_id) else {
                return Err(HubError::Conflict(conv_id.to_string()));
            };
            if entry.token != owner_token
                || !matches!(
                    &entry.kind,
                    super::state::OperationKind::ExternalRun {
                        run_id: owned_run_id
                    } if owned_run_id == run_id
                )
            {
                return Err(HubError::Conflict(conv_id.to_string()));
            }
        }
        let updated = self
            .store()
            .finalize_run_cas(run_id, conv_id, status, stop_reason)?;
        if !updated {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        if status != RunStatus::Cancelling {
            self.release_operation(conv_id, owner_token);
        }
        Ok(true)
    }

    pub(super) fn ensure_conversation(&self, conv_id: &str) -> Result<ConversationRow, HubError> {
        self.store()
            .conversation(conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))
    }

    pub(super) fn bind_session(
        &self,
        conv: &ConversationRow,
        agent_cfg: &AgentEndpointConfig,
    ) -> Result<(), HubError> {
        let cwd = conv.cwd.as_deref().map(PathBuf::from).ok_or_else(|| {
            HubError::other(format!(
                "conversation {} has no cwd; refusing to inherit the daemon working directory",
                conv.id
            ))
        })?;
        self.ctx.bind_session(
            &conv.agent_session_id,
            SessionBinding {
                conv_id: conv.id.clone(),
                agent_id: conv.agent_id.clone(),
                permission_policy: agent_cfg.permission_policy,
                fs: agent_cfg.client_capabilities.fs.clone(),
                cwd,
            },
        )
    }

    pub(super) async fn refresh_session_projection_external(
        &self,
        conv: &ConversationRow,
        cwd: PathBuf,
        method: ReplayMethod,
    ) -> Result<SessionCreated, HubError> {
        let operation =
            Arc::new(self.reserve_operation(&conv.id, &conv.agent_id, OperationKind::Refresh)?);
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        let handle = self.agent_handle(&conv.agent_id).await?;
        let publication = RefreshPublication {
            binding: SessionBinding {
                conv_id: conv.id.clone(),
                agent_id: conv.agent_id.clone(),
                permission_policy: agent_cfg.permission_policy,
                fs: agent_cfg.client_capabilities.fs,
                cwd: cwd.clone(),
            },
            remove_conversation_on_error: false,
        };
        self.refresh_session_projection_owned(
            &handle,
            conv,
            cwd,
            RefreshProjectionOptions {
                method,
                operation_keepalive: Some(operation),
                publication: Some(publication),
                prepared_refresh: None,
            },
        )
        .await
    }

    #[allow(dead_code)] // kept for optional Layer1 import outside discover (Phase 2+)
    pub(super) async fn refresh_agent_session_import(
        &self,
        handle: &Arc<AgentHandle>,
        conv: &ConversationRow,
        cwd: PathBuf,
        operation: Arc<OperationLease>,
        refresh: ReplayRefresh,
    ) -> Result<SessionCreated, HubError> {
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        self.refresh_session_projection_owned(
            handle,
            conv,
            cwd.clone(),
            RefreshProjectionOptions {
                method: ReplayMethod::Load,
                operation_keepalive: Some(operation),
                publication: Some(RefreshPublication {
                    binding: SessionBinding {
                        conv_id: conv.id.clone(),
                        agent_id: conv.agent_id.clone(),
                        permission_policy: agent_cfg.permission_policy,
                        fs: agent_cfg.client_capabilities.fs,
                        cwd,
                    },
                    remove_conversation_on_error: false,
                }),
                prepared_refresh: Some(refresh),
            },
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn refresh_session_projection(
        &self,
        handle: &Arc<AgentHandle>,
        conv: &ConversationRow,
        cwd: PathBuf,
        method: ReplayMethod,
    ) -> Result<SessionCreated, HubError> {
        self.refresh_session_projection_owned(
            handle,
            conv,
            cwd,
            RefreshProjectionOptions {
                method,
                operation_keepalive: None,
                publication: None,
                prepared_refresh: None,
            },
        )
        .await
    }

    async fn refresh_session_projection_owned(
        &self,
        handle: &Arc<AgentHandle>,
        conv: &ConversationRow,
        cwd: PathBuf,
        options: RefreshProjectionOptions,
    ) -> Result<SessionCreated, HubError> {
        let RefreshProjectionOptions {
            method,
            operation_keepalive,
            publication,
            prepared_refresh,
        } = options;
        #[cfg(test)]
        let refresh_publish_gate = {
            let mut gate = self.refresh_publish_gate.lock();
            if publication.is_some() {
                gate.take()
            } else {
                None
            }
        };
        let replay_prune = ReplayPruneGuard::acquire(&conv.id, Arc::clone(&self.replay_locks));
        let refreshing = Arc::clone(&replay_prune.replay_lock).lock_owned().await;
        let permit = match handle.cmd_tx.clone().reserve_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                if let Some(refresh) = prepared_refresh {
                    self.store().rollback_load_replay(refresh)?;
                }
                return Err(HubError::other(format!(
                    "agent {} command loop is closed",
                    conv.agent_id
                )));
            }
        };
        let load_id = format!("load-{}", Uuid::new_v4().simple());
        let refresh = match prepared_refresh {
            Some(refresh) => refresh,
            None => self.store().begin_load_replay(&conv.id, &load_id)?,
        };
        let (reply, response) = oneshot::channel();
        let command = match method {
            ReplayMethod::Load => AgentCommand::LoadSession {
                conv_id: conv.id.clone(),
                agent_id: conv.agent_id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                cwd,
                reply,
            },
            ReplayMethod::Resume => AgentCommand::ResumeSession {
                conv_id: conv.id.clone(),
                agent_id: conv.agent_id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                cwd,
                reply,
            },
        };
        permit.send(command);

        let ctx = Arc::clone(&self.ctx);
        let runtime = Arc::clone(&self.runtime);
        let agent_id = conv.agent_id.clone();
        let conv_id = conv.id.clone();
        let expected_session_id = conv.agent_session_id.clone();
        let worker = tokio::spawn(async move {
            let _operation = operation_keepalive;
            let replay_prune = replay_prune;
            let refreshing = refreshing;
            let command_result = match response.await {
                Ok(result) => result,
                Err(_) => Err(HubError::other(format!(
                    "agent {agent_id} command response dropped"
                ))),
            };
            let result = match command_result {
                Ok(created) if created.agent_session_id == expected_session_id => ctx
                    .store()
                    .commit_load_replay_with_static(
                        refresh,
                        created.config_options.as_ref(),
                        created.modes.as_ref(),
                    )
                    .map(|_| created),
                Ok(created) => {
                    let _ = ctx.store().rollback_load_replay(refresh);
                    Err(HubError::other(format!(
                        "agent returned session id {:?} while loading {:?}",
                        created.agent_session_id, expected_session_id
                    )))
                }
                Err(error) => match ctx.store().rollback_load_replay(refresh) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(rollback_error),
                },
            };
            #[cfg(test)]
            if result.is_ok()
                && publication.is_some()
                && let Some((reached, release)) = refresh_publish_gate
            {
                let _ = reached.send(());
                let _ = release.await;
            }
            let result = match (result, publication) {
                (Ok(created), Some(publication)) => {
                    let RefreshPublication {
                        binding,
                        remove_conversation_on_error,
                    } = publication;
                    match ctx.bind_session(&expected_session_id, binding) {
                        Ok(()) => {
                            runtime.insert(&conv_id, SessionState::Live, runtime.next_generation());
                            Ok(created)
                        }
                        Err(error) if remove_conversation_on_error => {
                            ctx.unbind_session(&agent_id, &expected_session_id);
                            runtime.remove(&conv_id);
                            match ctx.store().hard_delete_conversation(&conv_id) {
                                Ok(()) => Err(error),
                                Err(cleanup_error) => Err(cleanup_error),
                            }
                        }
                        Err(error) => Err(error),
                    }
                }
                (Err(error), Some(publication)) if publication.remove_conversation_on_error => {
                    ctx.unbind_session(&agent_id, &expected_session_id);
                    runtime.remove(&conv_id);
                    match ctx.store().hard_delete_conversation(&conv_id) {
                        Ok(()) => Err(error),
                        Err(cleanup_error) => Err(cleanup_error),
                    }
                }
                (Err(error), Some(_)) => Err(error),
                (result, None) => result,
            };
            drop(refreshing);
            drop(replay_prune);
            result
        });
        worker
            .await
            .map_err(|error| HubError::other(format!("refresh worker failed: {error}")))?
    }

    pub(super) async fn ensure_live_session(
        &self,
        conv: &ConversationRow,
        agent_cfg: &AgentEndpointConfig,
        handle: &Arc<AgentHandle>,
        operation_keepalive: Option<Arc<OperationLease>>,
    ) -> Result<(), HubError> {
        if matches!(self.runtime.get(&conv.id), Some((SessionState::Live, _))) {
            if self
                .ctx
                .is_session_bound(&conv.agent_id, &conv.agent_session_id)
            {
                return Ok(());
            }
            self.runtime.remove(&conv.id);
        }

        let cwd = conv.cwd.as_deref().map(PathBuf::from).ok_or_else(|| {
            HubError::other(format!(
                "conversation {} has no cwd; refusing to inherit the daemon working directory",
                conv.id
            ))
        })?;

        if handle.capabilities.session_capabilities.resume.is_some() {
            match self
                .refresh_session_projection_owned(
                    handle,
                    conv,
                    cwd.clone(),
                    RefreshProjectionOptions {
                        method: ReplayMethod::Resume,
                        operation_keepalive: operation_keepalive.clone(),
                        publication: None,
                        prepared_refresh: None,
                    },
                )
                .await
            {
                Ok(_) => {
                    self.bind_session(conv, agent_cfg)?;
                    self.runtime.insert(
                        &conv.id,
                        SessionState::Live,
                        self.runtime.next_generation(),
                    );
                    return Ok(());
                }
                Err(source) if !handle.capabilities.load_session => {
                    return Err(HubError::ResumeLoadFailed {
                        attempted_method: "session/resume",
                        endpoint: conv.agent_id.clone(),
                        conv_id: conv.id.clone(),
                        agent_session_id: conv.agent_session_id.clone(),
                        source: Box::new(source),
                    });
                }
                Err(_) => {}
            }
        }

        self.refresh_session_projection_owned(
            handle,
            conv,
            cwd,
            RefreshProjectionOptions {
                method: ReplayMethod::Load,
                operation_keepalive,
                publication: None,
                prepared_refresh: None,
            },
        )
        .await
        .map_err(|source| HubError::ResumeLoadFailed {
            attempted_method: "session/load",
            endpoint: conv.agent_id.clone(),
            conv_id: conv.id.clone(),
            agent_session_id: conv.agent_session_id.clone(),
            source: Box::new(source),
        })?;
        self.bind_session(conv, agent_cfg)?;
        self.runtime
            .insert(&conv.id, SessionState::Live, self.runtime.next_generation());
        Ok(())
    }

    fn persist_session_snapshots(
        &self,
        conv_id: &str,
        created: &SessionCreated,
    ) -> Result<(), HubError> {
        self.store().replace_static_snapshots(
            conv_id,
            created.config_options.as_ref(),
            created.modes.as_ref(),
        )
    }
}

fn path_to_string(path: &Path) -> Result<String, HubError> {
    path.to_str()
        .map(|s| s.to_owned())
        .ok_or_else(|| HubError::other(format!("path is not valid UTF-8: {}", path.display())))
}

pub(super) fn require_absolute_cwd(cwd: Option<PathBuf>) -> Result<PathBuf, HubError> {
    let cwd = cwd.ok_or_else(|| {
        HubError::other(
            "cwd is required; callers must send their own absolute working directory instead of inheriting the daemon cwd",
        )
    })?;
    if !cwd.is_absolute() {
        return Err(HubError::other(format!(
            "cwd must be absolute, got {}",
            cwd.display()
        )));
    }
    Ok(cwd)
}

fn conversation_created_from_row(row: &ConversationRow) -> ConversationCreated {
    ConversationCreated {
        conv_id: row.id.clone(),
        agent_id: row.agent_id.clone(),
        agent_session_id: row.agent_session_id.clone(),
        status: conv_status_string(row.status),
        origin: row.origin.as_str().to_string(),
        interaction: row.interaction.as_str().to_string(),
    }
}

fn conv_status_string(status: ConvStatus) -> String {
    match status {
        ConvStatus::Idle => "idle",
        ConvStatus::Running => "running",
        ConvStatus::Cancelling => "cancelling",
        ConvStatus::Cancelled => "cancelled",
        ConvStatus::Failed => "failed",
        ConvStatus::Completed => "completed",
        ConvStatus::Closed => "closed",
        ConvStatus::Deleted => "deleted",
    }
    .to_string()
}
