use std::sync::Arc;

use super::state::{CoreHub, OperationKind, PromptOperation};
use super::types::{CancelResult, ConfigSnapshot, PromptResult, SendPromptParams};
use crate::acp::{AgentCommand, validate_prompt_capabilities};
use crate::error::HubError;
use crate::runtime::{RunLease, SessionState};
use crate::store::{
    ConversationRow, Interaction, MessageSource, NewMessage, RunStatus, search_body,
};
use agent_client_protocol::schema::v1::{CancelNotification, ContentBlock, SessionId, StopReason};
use tokio::sync::oneshot;
use uuid::Uuid;

impl CoreHub {
    /// PHASE1 write gate: interaction must be writable; phase must not be closed/deleted.
    pub fn assert_write_gate(conv: &ConversationRow) -> Result<(), HubError> {
        use crate::store::{ConvPhase, SessionSpace, parse_session_meta};
        if conv.phase == ConvPhase::Closed {
            return Err(HubError::ConversationClosed {
                conv_id: conv.id.clone(),
            });
        }
        if conv.phase == ConvPhase::Deleted {
            return Err(HubError::not_found("conversation", &conv.id));
        }
        if conv.interaction != Interaction::Writable {
            let (_, space) = parse_session_meta(conv.session_meta.as_ref());
            return Err(HubError::read_only_conversation(
                &conv.id,
                conv.origin.as_str(),
                conv.interaction.as_str(),
                space == SessionSpace::Ide,
            ));
        }
        Ok(())
    }

    /// Send a prompt turn through the live ACP connection.
    pub async fn send_prompt(&self, params: SendPromptParams) -> Result<PromptResult, HubError> {
        let conv = self
            .store()
            .conversation(&params.conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", &params.conv_id))?;
        Self::assert_write_gate(&conv)?;
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        let operation = Arc::new(self.reserve_operation(
            &conv.id,
            &conv.agent_id,
            OperationKind::Prompt(PromptOperation {
                run_id: run_id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                cancel_requested: false,
            }),
        )?);
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        // Note: PermissionPolicy::Reject still allows send; it only auto-denies
        // permission callbacks. Do not map it to a hard send gate.

        let handle = self.agent_handle(&conv.agent_id).await?;
        validate_prompt_capabilities(&conv.agent_id, &handle.capabilities, &params.prompt)?;
        self.ensure_live_session(&conv, &agent_cfg, &handle, Some(Arc::clone(&operation)))
            .await?;
        let permit = handle.cmd_tx.clone().reserve_owned().await.map_err(|_| {
            HubError::other(format!("agent {} command loop is closed", conv.agent_id))
        })?;

        self.store().create_run(&run_id, &conv.id)?;
        let Some(run_lease) = RunLease::acquire(Arc::clone(&self.runtime), &conv.id) else {
            if !self
                .store()
                .finalize_run_cas(&run_id, &conv.id, RunStatus::Failed, None)?
            {
                return Err(HubError::Conflict(conv.id));
            }
            return Err(HubError::other("could not acquire run lease"));
        };
        self.ctx
            .set_current_run(&conv.agent_id, &conv.agent_session_id, &run_id);
        let prompt_seq = match self.store_prompt_message(&conv.id, &run_id, &params.prompt) {
            Ok(prompt_seq) => prompt_seq,
            Err(error) => {
                self.ctx
                    .clear_current_run(&conv.agent_id, &conv.agent_session_id);
                if !self
                    .store()
                    .finalize_run_cas(&run_id, &conv.id, RunStatus::Failed, None)?
                {
                    return Err(HubError::Conflict(conv.id));
                }
                run_lease.complete();
                return Err(error);
            }
        };
        let config_params = params
            .params
            .iter()
            .map(|param| (param.config_id.clone(), param.value.clone()))
            .collect::<Vec<_>>();
        let (reply, response) = oneshot::channel();
        permit.send(AgentCommand::SendPrompt {
            conv_id: conv.id.clone(),
            agent_session_id: conv.agent_session_id.clone(),
            prompt: params.prompt,
            params: config_params,
            mode_id: params.mode_id,
            reply,
        });

        let ctx = Arc::clone(&self.ctx);
        let activity = Arc::clone(&self.activity);
        let operations = Arc::clone(&self.operations);
        let conv_id = conv.id;
        let agent_id = conv.agent_id;
        let agent_session_id = conv.agent_session_id;
        let worker = tokio::spawn(async move {
            let _operation = operation;
            let _activity_lease = activity.run_lease();
            let command_result = match response.await {
                Ok(result) => result,
                Err(_) => Err(HubError::other(format!(
                    "agent {agent_id} command response dropped"
                ))),
            };
            let result = {
                let _finalization = operations.lock();
                match command_result {
                    Ok(done) => {
                        let stop_reason = stop_reason_string(done.stop_reason);
                        let status = if done.stop_reason == StopReason::Cancelled {
                            RunStatus::Cancelled
                        } else {
                            RunStatus::Completed
                        };
                        match ctx.store().finalize_run_cas(
                            &run_id,
                            &conv_id,
                            status,
                            Some(&stop_reason),
                        ) {
                            Ok(true) => Ok(PromptResult {
                                conv_id: conv_id.clone(),
                                run_id: run_id.clone(),
                                prompt_seq,
                                stop_reason,
                            }),
                            Ok(false) => Err(HubError::Conflict(conv_id.clone())),
                            Err(error) => Err(error),
                        }
                    }
                    Err(command_error) => {
                        match ctx.store().finalize_run_cas(
                            &run_id,
                            &conv_id,
                            RunStatus::Failed,
                            None,
                        ) {
                            Ok(true) => Err(command_error),
                            Ok(false) => Err(HubError::Conflict(conv_id.clone())),
                            Err(finalize_error) => Err(finalize_error),
                        }
                    }
                }
            };
            ctx.clear_current_run(&agent_id, &agent_session_id);
            run_lease.complete();
            result
        });
        worker
            .await
            .map_err(|error| HubError::other(format!("prompt worker failed: {error}")))?
    }

    /// Request cancellation for the active turn by sending ACP `session/cancel`
    /// directly through the cloned connection handle.
    pub async fn cancel(&self, conv_id: &str) -> Result<CancelResult, HubError> {
        let (operation_token, agent_id, active) = {
            let operations = self.operations.lock();
            let Some(entry) = operations.get(conv_id) else {
                drop(operations);
                // Ensure the conversation exists, then PHASE1 §4.3 not_busy.
                self.ensure_conversation(conv_id)?;
                return Err(HubError::not_busy(conv_id));
            };
            let OperationKind::Prompt(active) = &entry.kind else {
                return Err(HubError::not_busy(conv_id));
            };
            if active.cancel_requested {
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: Some(active.run_id.clone()),
                    requested: false,
                });
            }
            (entry.token, entry.agent_id.clone(), active.clone())
        };

        #[cfg(test)]
        let cancel_snapshot_gate = { self.cancel_snapshot_gate.lock().take() };
        #[cfg(test)]
        if let Some((reached, release)) = cancel_snapshot_gate {
            let _ = reached.send(());
            let _ = release.await;
        }

        let handle = self.agent_handle(&agent_id).await?;
        {
            let mut operations = self.operations.lock();
            let Some(entry) = operations.get_mut(conv_id) else {
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: Some(active.run_id),
                    requested: false,
                });
            };
            let OperationKind::Prompt(current) = &mut entry.kind else {
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: Some(active.run_id),
                    requested: false,
                });
            };
            if entry.token != operation_token
                || entry.agent_id != agent_id
                || current.run_id != active.run_id
                || current.agent_session_id != active.agent_session_id
                || current.cancel_requested
            {
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: Some(active.run_id),
                    requested: false,
                });
            }

            if !self
                .store()
                .request_run_cancel_cas(&active.run_id, conv_id)?
            {
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: Some(active.run_id),
                    requested: false,
                });
            }
            let runtime_generation = self.runtime.get(conv_id).and_then(|(state, generation)| {
                self.runtime
                    .transition(
                        conv_id,
                        SessionState::Live,
                        SessionState::Cancelling,
                        generation,
                    )
                    .then_some((state, generation))
            });
            if runtime_generation.is_none() {
                let rollback = self
                    .store()
                    .rollback_run_cancel_request_cas(&active.run_id, conv_id);
                return match rollback {
                    Ok(true) => Err(HubError::Conflict(conv_id.to_string())),
                    Ok(false) => Err(HubError::other(format!(
                        "run {} entered cancelling but runtime ownership was lost and persisted rollback lost ownership",
                        active.run_id
                    ))),
                    Err(cleanup) => Err(HubError::other(format!(
                        "run {} entered cancelling but runtime ownership was lost; persisted rollback failed: {cleanup}",
                        active.run_id
                    ))),
                };
            }
            current.cancel_requested = true;
            #[cfg(test)]
            let forced_failure = self
                .cancel_notification_fail_once
                .swap(false, std::sync::atomic::Ordering::SeqCst);
            #[cfg(not(test))]
            let forced_failure = false;
            let notification = if forced_failure {
                Err(HubError::other("forced cancel notification failure"))
            } else {
                handle
                    .cx
                    .send_notification(CancelNotification::new(SessionId::new(
                        active.agent_session_id.as_str(),
                    )))
                    .map_err(HubError::from)
            };
            if let Err(error) = notification {
                #[cfg(test)]
                let rollback = if self
                    .cancel_rollback_fail_once
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    Err(HubError::other("forced cancel rollback failure"))
                } else {
                    self.store()
                        .rollback_run_cancel_request_cas(&active.run_id, conv_id)
                };
                #[cfg(not(test))]
                let rollback = self
                    .store()
                    .rollback_run_cancel_request_cas(&active.run_id, conv_id);
                match rollback {
                    Ok(true) => {
                        let (_, generation) = runtime_generation.expect("checked above");
                        if !self.runtime.transition(
                            conv_id,
                            SessionState::Cancelling,
                            SessionState::Live,
                            generation,
                        ) {
                            return Err(HubError::other(format!(
                                "cancel notification failed ({error}); persisted rollback succeeded but runtime rollback lost ownership"
                            )));
                        }
                        current.cancel_requested = false;
                        return Err(error);
                    }
                    Ok(false) => {
                        return Err(HubError::other(format!(
                            "cancel notification failed ({error}) and persisted rollback lost ownership"
                        )));
                    }
                    Err(cleanup) => {
                        return Err(HubError::other(format!(
                            "cancel notification failed ({error}); persisted rollback failed: {cleanup}"
                        )));
                    }
                }
            }
        }

        Ok(CancelResult {
            conv_id: conv_id.to_string(),
            run_id: Some(active.run_id),
            requested: true,
        })
    }

    /// Read the config and mode snapshot for a conversation.
    pub fn get_config(&self, conv_id: &str) -> Result<ConfigSnapshot, HubError> {
        self.ensure_conversation(conv_id)?;
        Ok(ConfigSnapshot {
            config_options: self.store().config_snapshot(conv_id)?,
            modes: self.store().modes_snapshot(conv_id)?,
            updated_at: None,
        })
    }

    /// Set one ACP session config option for an existing conversation.
    pub async fn set_param(
        &self,
        conv_id: &str,
        config_id: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), HubError> {
        let conv = self.ensure_conversation(conv_id)?;
        Self::assert_write_gate(&conv)?;
        let operation =
            Arc::new(self.reserve_operation(conv_id, &conv.agent_id, OperationKind::SetParam)?);
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        let handle = self.agent_handle(&conv.agent_id).await?;
        self.ensure_live_session(&conv, &agent_cfg, &handle, Some(Arc::clone(&operation)))
            .await?;
        let agent_session_id = conv.agent_session_id;
        let config_id = config_id.into();
        let value = value.into();
        self.enqueue_operation(&handle, operation, move |reply| AgentCommand::SetConfig {
            agent_session_id,
            config_id,
            value,
            reply,
        })
        .await
    }

    /// Set the ACP session mode for an existing conversation.
    pub async fn set_mode(
        &self,
        conv_id: &str,
        mode_id: impl Into<String>,
    ) -> Result<(), HubError> {
        let conv = self.ensure_conversation(conv_id)?;
        Self::assert_write_gate(&conv)?;
        let operation =
            Arc::new(self.reserve_operation(conv_id, &conv.agent_id, OperationKind::SetMode)?);
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        let handle = self.agent_handle(&conv.agent_id).await?;
        self.ensure_live_session(&conv, &agent_cfg, &handle, Some(Arc::clone(&operation)))
            .await?;
        let agent_session_id = conv.agent_session_id;
        let mode_id = mode_id.into();
        self.enqueue_operation(&handle, operation, move |reply| AgentCommand::SetMode {
            agent_session_id,
            mode_id,
            reply,
        })
        .await
    }

    fn store_prompt_message(
        &self,
        conv_id: &str,
        run_id: &str,
        prompt: &[ContentBlock],
    ) -> Result<i64, HubError> {
        let content = serde_json::to_value(prompt)?;
        let body_text = search_body(&content);
        self.store().append_message(&NewMessage {
            id: format!("msg-{}", Uuid::new_v4().simple()),
            conv_id: conv_id.to_string(),
            run_id: Some(run_id.to_string()),
            source: MessageSource::LocalTurn,
            role: "user".to_string(),
            kind: Some("prompt".to_string()),
            content_json: content,
            body_text,
        })
    }
}

fn stop_reason_string(stop: StopReason) -> String {
    serde_json::to_value(stop)
        .ok()
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{stop:?}"))
}
