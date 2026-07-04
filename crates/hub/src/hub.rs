//! In-process Hub engine plus embedded-library RPC client.
//!
//! [`CoreHub`] is daemon-internal: it owns the registry, runtime cache, agent
//! handles, and the single [`Store`] owned by [`HubCtx`]. [`HubClient`] is the
//! public embedded-library entry point; it discovers the singleton daemon and
//! forwards every method over JSON-RPC.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::acp::{AgentCommand, AgentHandle, PromptDone, SessionCreated, spawn_agent_connection};
use crate::callbacks::{HubCtx, SessionBinding};
use crate::conductor;
use crate::daemon::{self, ActivityTracker};
use crate::endpoint::{AgentEndpointConfig, ProxyEndpointConfig, Registry};
use crate::error::HubError;
use crate::rpc::RpcClient;
use crate::runtime::{RunLease, RuntimeCache, SessionState};
use crate::store::{
    ConvStatus, ConversationRow, MessageRow, MessageSource, NewConversation, NewMessage, RunStatus,
    SearchPage, Store, search_body,
};
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, McpServer, SessionId, StopReason,
};
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

/// Parameters for `hub/conv/create`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationParams {
    pub agent_id: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    #[serde(default)]
    pub additional_directories: Vec<PathBuf>,
}

/// Result for `hub/conv/create`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCreated {
    pub conv_id: String,
    pub agent_id: String,
    pub agent_session_id: String,
    pub status: String,
}

/// A config/mode parameter applied before a prompt turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigParam {
    pub config_id: String,
    pub value: String,
}

/// Parameters for `hub/conv/send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPromptParams {
    pub conv_id: String,
    pub prompt: Vec<ContentBlock>,
    #[serde(default)]
    pub params: Vec<ConfigParam>,
    #[serde(default)]
    pub mode_id: Option<String>,
}

/// Result for `hub/conv/send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub conv_id: String,
    pub run_id: String,
    pub stop_reason: String,
}

/// Result for `hub/conv/cancel`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResult {
    pub conv_id: String,
    pub run_id: Option<String>,
    pub requested: bool,
}

/// Read surface for the config/mode snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub config_options: Option<Value>,
    pub modes: Option<Value>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListConversationsParams {
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagesParams {
    pub conv_id: String,
    #[serde(default)]
    pub include_audit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub query: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub conv_id: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationIdParams {
    pub conv_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConversationParams {
    pub conv_id: String,
    #[serde(default)]
    pub local_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterAgentParams {
    #[serde(rename = "agentId", alias = "id")]
    pub agent_id: String,
    pub config: AgentEndpointConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveAgentParams {
    #[serde(rename = "agentId", alias = "id")]
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectAgentParams {
    #[serde(rename = "agentId", alias = "id")]
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInspection {
    pub agent_id: String,
    pub config: AgentEndpointConfig,
    pub agent_info: Option<Value>,
    pub capabilities: Option<Value>,
    pub cache_populated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterProxyParams {
    #[serde(rename = "proxyId", alias = "id")]
    pub proxy_id: String,
    pub config: ProxyEndpointConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProxyParams {
    #[serde(rename = "proxyId", alias = "id")]
    pub proxy_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticateAgentParams {
    #[serde(rename = "agentId", alias = "id")]
    pub agent_id: String,
    pub method_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetParamParams {
    pub conv_id: String,
    pub config_id: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModeParams {
    pub conv_id: String,
    pub mode_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunParams {
    pub conv_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCreated {
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeRunParams {
    pub conv_id: String,
    pub run_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct ActiveRun {
    run_id: String,
    agent_id: String,
    agent_session_id: String,
}

/// Daemon-internal Hub engine.
///
/// The projection store has one owner: [`HubCtx`]. CoreHub reaches it through
/// [`HubCtx::store`] so callback-captured updates and direct RPC reads/writes
/// always use the same SQLite connection.
pub struct CoreHub {
    home: PathBuf,
    registry: RwLock<Registry>,
    runtime: Arc<RuntimeCache>,
    ctx: Arc<HubCtx>,
    handles: Mutex<HashMap<String, Arc<AgentHandle>>>,
    active_runs: Mutex<HashMap<String, ActiveRun>>,
    activity: Arc<ActivityTracker>,
}

impl CoreHub {
    /// Build a CoreHub from already-loaded registry and store state.
    pub fn new(
        home: impl AsRef<Path>,
        registry: Registry,
        store: Store,
        activity: Arc<ActivityTracker>,
    ) -> Self {
        Self {
            home: home.as_ref().to_path_buf(),
            registry: RwLock::new(registry),
            runtime: RuntimeCache::new(),
            ctx: HubCtx::new(store),
            handles: Mutex::default(),
            active_runs: Mutex::default(),
            activity,
        }
    }

    /// Load registry and store from `home`.
    pub fn open(home: impl AsRef<Path>) -> Result<Self, HubError> {
        let home = home.as_ref();
        let registry = Registry::load(home)?;
        let store = Store::open(home)?;
        Ok(Self::new(
            home,
            registry,
            store,
            Arc::new(ActivityTracker::new()),
        ))
    }

    /// Access the callback context used by agent connections.
    pub fn ctx(&self) -> Arc<HubCtx> {
        Arc::clone(&self.ctx)
    }

    /// Access the runtime cache.
    pub fn runtime(&self) -> Arc<RuntimeCache> {
        Arc::clone(&self.runtime)
    }

    /// Access the single projection store owned by the callback context.
    pub fn store(&self) -> &Store {
        self.ctx.store()
    }

    /// Current in-memory registry snapshot.
    pub fn registry(&self) -> Registry {
        self.registry.read().clone()
    }

    /// Register or replace an agent endpoint and persist `agents.json`.
    pub fn register_agent(
        &self,
        agent_id: impl Into<String>,
        config: AgentEndpointConfig,
    ) -> Result<(), HubError> {
        let mut next = self.registry.read().clone();
        next.register_agent(agent_id.into(), config)?;
        next.save(&self.home)?;
        *self.registry.write() = next;
        Ok(())
    }

    /// Remove an agent endpoint and persist `agents.json`.
    pub async fn remove_agent(&self, agent_id: &str) -> Result<(), HubError> {
        let mut next = self.registry.read().clone();
        next.remove_agent(agent_id)?;
        next.save(&self.home)?;
        *self.registry.write() = next;
        self.handles.lock().await.remove(agent_id);
        Ok(())
    }

    /// Register or replace a proxy endpoint and persist `agents.json`.
    pub fn register_proxy(
        &self,
        proxy_id: impl Into<String>,
        config: ProxyEndpointConfig,
    ) -> Result<(), HubError> {
        let mut next = self.registry.read().clone();
        next.register_proxy(proxy_id.into(), config)?;
        next.save(&self.home)?;
        *self.registry.write() = next;
        Ok(())
    }

    /// Remove a proxy endpoint and persist `agents.json`.
    pub fn remove_proxy(&self, proxy_id: &str) -> Result<(), HubError> {
        let mut next = self.registry.read().clone();
        next.remove_proxy(proxy_id)?;
        next.save(&self.home)?;
        *self.registry.write() = next;
        Ok(())
    }

    /// List registered agents.
    pub fn list_agents(&self) -> Registry {
        self.registry()
    }

    /// Inspect a registered agent endpoint without opening a new ACP connection.
    pub fn inspect_agent(&self, agent_id: &str) -> Result<AgentInspection, HubError> {
        let config = self.agent_config(agent_id)?;
        let cache = self.store().agent_cache(agent_id)?;
        let cache_populated = cache.is_some();
        let (agent_info, capabilities) = match cache {
            Some((agent_info, capabilities)) => (
                Some(serde_json::from_str(&agent_info)?),
                Some(serde_json::from_str(&capabilities)?),
            ),
            None => (None, None),
        };
        Ok(AgentInspection {
            agent_id: agent_id.to_string(),
            config,
            agent_info,
            capabilities,
            cache_populated,
        })
    }

    /// Authenticate an agent using one of its advertised auth methods.
    pub async fn authenticate_agent(
        &self,
        agent_id: &str,
        method_id: &str,
    ) -> Result<(), HubError> {
        let handle = self.agent_handle(agent_id).await?;
        if !handle
            .auth_methods
            .iter()
            .any(|method| method.id == method_id)
        {
            return Err(HubError::not_found("auth method", method_id));
        }
        self.request_agent(&handle, |reply| AgentCommand::Authenticate {
            method_id: method_id.to_string(),
            reply,
        })
        .await
    }

    /// Logout an agent through its live ACP connection.
    pub async fn logout_agent(&self, agent_id: &str) -> Result<(), HubError> {
        let handle = self.agent_handle(agent_id).await?;
        self.request_agent(&handle, |reply| AgentCommand::Logout { reply })
            .await
    }

    /// List sessions known to the agent (ACP `session/list`) and auto-import
    /// discovered sessions into the projection (FAQ: "全量记录静态资源snapshot").
    pub async fn list_agent_sessions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<agent_client_protocol::schema::v1::SessionInfo>, HubError> {
        let handle = self.agent_handle(agent_id).await?;
        let result = self
            .request_agent(&handle, |reply| AgentCommand::ListSessions {
                cwd: None,
                reply,
            })
            .await?;
        // Auto-import each discovered session into the projection.
        for info in &result.sessions {
            let sid = info.session_id.to_string();
            let title = info.title.as_deref();
            let cwd = info.cwd.to_str();
            let dirs: Vec<String> = info
                .additional_directories
                .iter()
                .map(|d| d.to_string_lossy().into_owned())
                .collect();
            match self
                .store()
                .upsert_agent_session(agent_id, &sid, title, cwd, &dirs)
            {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(?e, "failed to upsert agent session");
                    continue;
                }
            }
            // Load messages via session/load (Layer 1) if supported.
            if handle.capabilities.load_session {
                if let Ok(Some(conv)) = self.store().conversation_by_agent_session(agent_id, &sid) {
                    let existing = self.store().messages(&conv.id, false).unwrap_or_default();
                    if existing.is_empty() {
                        let _ = self
                            .request_agent(&handle, |reply| AgentCommand::LoadSession {
                                conv_id: conv.id.clone(),
                                agent_id: agent_id.to_string(),
                                agent_session_id: sid.clone(),
                                cwd: info.cwd.clone(),
                                reply,
                            })
                            .await;
                    }
                }
            }
        }
        Ok(result.sessions)
    }

    /// Create a Hub conversation, issuing ACP `session/new` or `session/load`.
    pub async fn create_conversation(
        &self,
        params: CreateConversationParams,
    ) -> Result<ConversationCreated, HubError> {
        let agent_cfg = self.agent_config(&params.agent_id)?;
        let cwd = params.cwd.unwrap_or(std::env::current_dir()?);
        let additional = params.additional_directories;
        if let Some(existing_session) = &params.agent_session_id {
            if let Some(existing) = self
                .store()
                .conversation_by_agent_session(&params.agent_id, existing_session)?
            {
                self.bind_session(&existing, &agent_cfg);
                return Ok(ConversationCreated {
                    conv_id: existing.id,
                    agent_id: existing.agent_id,
                    agent_session_id: existing.agent_session_id,
                    status: conv_status_string(existing.status),
                });
            }
        }

        let conv_id = format!("conv-{}", Uuid::new_v4().simple());
        let handle = self.agent_handle(&params.agent_id).await?;
        let created = if let Some(agent_session_id) = params.agent_session_id {
            self.request_agent(&handle, |reply| AgentCommand::LoadSession {
                conv_id: conv_id.clone(),
                agent_id: params.agent_id.clone(),
                agent_session_id,
                cwd: cwd.clone(),
                reply,
            })
            .await
            .map_err(|source| HubError::ResumeLoadFailed {
                attempted_method: "session/load",
                endpoint: params.agent_id.clone(),
                conv_id: conv_id.clone(),
                agent_session_id: String::new(),
                source: Box::new(source),
            })?
        } else {
            self.request_agent(&handle, |reply| AgentCommand::CreateSession {
                conv_id: conv_id.clone(),
                agent_id: params.agent_id.clone(),
                cwd: cwd.clone(),
                additional_directories: additional.clone(),
                mcp_servers: params.mcp_servers,
                reply,
            })
            .await?
        };

        let additional_strings = additional
            .iter()
            .map(|p| path_to_string(p))
            .collect::<Result<Vec<_>, _>>()?;
        self.store().create_conversation(&NewConversation {
            id: conv_id.clone(),
            agent_id: params.agent_id.clone(),
            agent_session_id: created.agent_session_id.clone(),
            cwd: Some(path_to_string(&cwd)?),
            additional_directories: additional_strings,
            title: None,
        })?;
        self.persist_session_snapshots(&conv_id, &created)?;
        let row = self
            .store()
            .conversation(&conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", &conv_id))?;
        self.bind_session(&row, &agent_cfg);
        self.runtime
            .insert(&conv_id, SessionState::Live, self.runtime.next_generation());
        Ok(ConversationCreated {
            conv_id,
            agent_id: params.agent_id,
            agent_session_id: created.agent_session_id,
            status: "idle".to_string(),
        })
    }

    /// List Hub conversations from the projection.
    pub fn list_conversations(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<ConversationRow>, HubError> {
        self.store().list_conversations(agent_id)
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
    pub fn create_run(&self, conv_id: &str) -> Result<String, HubError> {
        self.ensure_conversation(conv_id)?;
        let run_id = format!("run-{}", Uuid::new_v4().simple());
        self.store().create_run(&run_id, conv_id)?;
        Ok(run_id)
    }

    /// Compare-and-set run finalization.
    pub fn finalize_run(
        &self,
        conv_id: &str,
        run_id: &str,
        status: RunStatus,
        stop_reason: Option<&str>,
    ) -> Result<bool, HubError> {
        self.store()
            .finalize_run_cas(run_id, conv_id, status, stop_reason)
    }

    /// Send a prompt turn through the live ACP connection.
    pub async fn send_prompt(&self, params: SendPromptParams) -> Result<PromptResult, HubError> {
        let conv = self
            .store()
            .conversation(&params.conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", &params.conv_id))?;
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        let run_id = format!("run-{}", Uuid::new_v4().simple());

        {
            let mut active = self.active_runs.lock().await;
            if active.contains_key(&conv.id) {
                return Err(HubError::Conflict(conv.id));
            }
            active.insert(
                conv.id.clone(),
                ActiveRun {
                    run_id: run_id.clone(),
                    agent_id: conv.agent_id.clone(),
                    agent_session_id: conv.agent_session_id.clone(),
                },
            );
        }

        let handle = match self.agent_handle(&conv.agent_id).await {
            Ok(handle) => handle,
            Err(err) => {
                self.active_runs.lock().await.remove(&conv.id);
                return Err(err);
            }
        };
        if let Err(err) = self.ensure_live_session(&conv, &agent_cfg, &handle).await {
            self.active_runs.lock().await.remove(&conv.id);
            return Err(err);
        }
        if let Err(err) = self.store().create_run(&run_id, &conv.id) {
            self.active_runs.lock().await.remove(&conv.id);
            return Err(err);
        }

        let _activity_lease = self.activity.run_lease();
        let Some(lease) = RunLease::acquire(Arc::clone(&self.runtime), &conv.id) else {
            self.active_runs.lock().await.remove(&conv.id);
            let _ = self.finalize_run(&conv.id, &run_id, RunStatus::Failed, None)?;
            return Err(HubError::other("could not acquire run lease"));
        };
        self.ctx.set_current_run(&conv.agent_session_id, &run_id);
        if let Err(err) = self.store_prompt_message(&conv.id, &run_id, &params.prompt) {
            self.ctx.clear_current_run(&conv.agent_session_id);
            self.active_runs.lock().await.remove(&conv.id);
            let _ = self.finalize_run(&conv.id, &run_id, RunStatus::Failed, None)?;
            lease.complete();
            return Err(err);
        }

        let config_params = params
            .params
            .iter()
            .map(|p| (p.config_id.clone(), p.value.clone()))
            .collect::<Vec<_>>();
        let command_result = self
            .request_agent(&handle, |reply| AgentCommand::SendPrompt {
                conv_id: conv.id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                prompt: params.prompt,
                params: config_params,
                mode_id: params.mode_id,
                reply,
            })
            .await;

        self.ctx.clear_current_run(&conv.agent_session_id);
        self.active_runs.lock().await.remove(&conv.id);

        match command_result {
            Ok(done) => self.finish_prompt(&conv.id, &run_id, &lease, done),
            Err(err) => {
                let _ = self.finalize_run(&conv.id, &run_id, RunStatus::Failed, None)?;
                lease.complete();
                Err(err)
            }
        }
    }

    /// Request cancellation for the active turn by sending ACP `session/cancel`
    /// directly through the cloned connection handle.
    pub async fn cancel(&self, conv_id: &str) -> Result<CancelResult, HubError> {
        let Some(active) = self.active_runs.lock().await.get(conv_id).cloned() else {
            self.ensure_conversation(conv_id)?;
            return Ok(CancelResult {
                conv_id: conv_id.to_string(),
                run_id: None,
                requested: false,
            });
        };
        let handle = self.agent_handle(&active.agent_id).await?;
        let cx = handle.cx.clone();
        cx.send_notification(CancelNotification::new(SessionId::new(
            active.agent_session_id.as_str(),
        )))?;
        if let Some((_, generation)) = self.runtime.get(conv_id) {
            self.runtime.transition(
                conv_id,
                SessionState::Live,
                SessionState::Cancelling,
                generation,
            );
        }
        let _ = self.finalize_run(conv_id, &active.run_id, RunStatus::Cancelling, None)?;
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
        if self.active_runs.lock().await.contains_key(conv_id) {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        let conv = self.ensure_conversation(conv_id)?;
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        let handle = self.agent_handle(&conv.agent_id).await?;
        self.ensure_live_session(&conv, &agent_cfg, &handle).await?;
        self.request_agent(&handle, |reply| AgentCommand::SetConfig {
            agent_session_id: conv.agent_session_id.clone(),
            config_id: config_id.into(),
            value: value.into(),
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
        if self.active_runs.lock().await.contains_key(conv_id) {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        let conv = self.ensure_conversation(conv_id)?;
        let agent_cfg = self.agent_config(&conv.agent_id)?;
        let handle = self.agent_handle(&conv.agent_id).await?;
        self.ensure_live_session(&conv, &agent_cfg, &handle).await?;
        self.request_agent(&handle, |reply| AgentCommand::SetMode {
            agent_session_id: conv.agent_session_id.clone(),
            mode_id: mode_id.into(),
            reply,
        })
        .await
    }

    /// Delete a conversation projection and optionally the remote ACP session.
    pub async fn delete_conversation(
        &self,
        conv_id: &str,
        local_only: bool,
    ) -> Result<(), HubError> {
        let conv = self
            .store()
            .conversation(conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))?;
        if !local_only {
            let handle = self.agent_handle(&conv.agent_id).await?;
            self.request_agent(&handle, |reply| AgentCommand::DeleteSession {
                conv_id: conv.id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                local_only,
                reply,
            })
            .await?;
        }
        self.ctx.unbind_session(&conv.agent_session_id);
        self.runtime.remove(conv_id);
        self.store().delete_conversation(conv_id)
    }

    /// Close the remote ACP session and evict the runtime entry; projection is retained.
    pub async fn close_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        if self.active_runs.lock().await.contains_key(conv_id) {
            let _ = self.cancel(conv_id).await?;
        }
        let conv = self
            .store()
            .conversation(conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))?;
        let handle = self.agent_handle(&conv.agent_id).await?;
        self.request_agent(&handle, |reply| AgentCommand::CloseSession {
            conv_id: conv.id.clone(),
            agent_session_id: conv.agent_session_id.clone(),
            reply,
        })
        .await?;
        self.ctx.unbind_session(&conv.agent_session_id);
        self.runtime.remove(conv_id);
        self.store().set_conv_status(conv_id, ConvStatus::Idle)?;
        Ok(())
    }

    /// Dispatch daemon JSON-RPC method names to CoreHub methods.
    pub async fn handle_rpc(&self, method: &str, params: Value) -> Result<Value, HubError> {
        match method {
            "hub/agent/list" => to_value(self.registry().agents),
            "hub/agent/inspect" => {
                let p: InspectAgentParams = from_params(params)?;
                to_value(self.inspect_agent(&p.agent_id)?)
            }
            "hub/agent/register" => {
                let p: RegisterAgentParams = from_params(params)?;
                self.register_agent(p.agent_id, p.config)?;
                Ok(json!({ "ok": true }))
            }
            "hub/agent/remove" => {
                let p: RemoveAgentParams = from_params(params)?;
                self.remove_agent(&p.agent_id).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/agent/authenticate" => {
                let p: AuthenticateAgentParams = from_params(params)?;
                self.authenticate_agent(&p.agent_id, &p.method_id).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/agent/logout" => {
                let p: RemoveAgentParams = from_params(params)?;
                self.logout_agent(&p.agent_id).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/agent/sessions" => {
                let p: InspectAgentParams = from_params(params)?;
                to_value(self.list_agent_sessions(&p.agent_id).await?)
            }
            "hub/proxy/list" => to_value(self.registry().proxies),
            "hub/proxy/register" => {
                let p: RegisterProxyParams = from_params(params)?;
                self.register_proxy(p.proxy_id, p.config)?;
                Ok(json!({ "ok": true }))
            }
            "hub/proxy/remove" => {
                let p: RemoveProxyParams = from_params(params)?;
                self.remove_proxy(&p.proxy_id)?;
                Ok(json!({ "ok": true }))
            }
            "hub/conv/create" => {
                let p: CreateConversationParams = from_params(params)?;
                to_value(self.create_conversation(p).await?)
            }
            "hub/conv/list" => {
                let p: ListConversationsParams = from_params(params)?;
                to_value(self.list_conversations(p.agent_id.as_deref())?)
            }
            "hub/conv/messages" => {
                let p: MessagesParams = from_params(params)?;
                to_value(self.messages(&p.conv_id, p.include_audit)?)
            }
            "hub/conv/search" => {
                let p: SearchParams = from_params(params)?;
                to_value(self.search(
                    &p.query,
                    p.agent_id.as_deref(),
                    p.conv_id.as_deref(),
                    p.limit,
                    p.offset,
                )?)
            }
            "hub/conv/send" => {
                let p: SendPromptParams = from_params(params)?;
                to_value(self.send_prompt(p).await?)
            }
            "hub/conv/cancel" => {
                let p: ConversationIdParams = from_params(params)?;
                to_value(self.cancel(&p.conv_id).await?)
            }
            "hub/conv/config" => {
                let p: ConversationIdParams = from_params(params)?;
                to_value(self.get_config(&p.conv_id)?)
            }
            "hub/conv/create_run" => {
                let p: CreateRunParams = from_params(params)?;
                to_value(RunCreated {
                    run_id: self.create_run(&p.conv_id)?,
                })
            }
            "hub/conv/finalize_run" => {
                let p: FinalizeRunParams = from_params(params)?;
                to_value(self.finalize_run(
                    &p.conv_id,
                    &p.run_id,
                    p.status,
                    p.stop_reason.as_deref(),
                )?)
            }
            "hub/conv/set_param" => {
                let p: SetParamParams = from_params(params)?;
                self.set_param(&p.conv_id, p.config_id, p.value).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/conv/set_mode" => {
                let p: SetModeParams = from_params(params)?;
                self.set_mode(&p.conv_id, p.mode_id).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/conv/delete" => {
                let p: DeleteConversationParams = from_params(params)?;
                self.delete_conversation(&p.conv_id, p.local_only).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/conv/close" => {
                let p: ConversationIdParams = from_params(params)?;
                self.close_conversation(&p.conv_id).await?;
                Ok(json!({ "ok": true }))
            }
            other => Err(HubError::other(format!("unknown RPC method {other}"))),
        }
    }

    async fn agent_handle(&self, agent_id: &str) -> Result<Arc<AgentHandle>, HubError> {
        let mut handles = self.handles.lock().await;
        if let Some(handle) = handles.get(agent_id) {
            return Ok(Arc::clone(handle));
        }
        let registry = self.registry.read().clone();
        let component = conductor::build_endpoint_component(&registry, agent_id)?;
        let rx = spawn_agent_connection(component, agent_id.to_string(), Arc::clone(&self.ctx));
        let handle =
            Arc::new(rx.await.map_err(|_| {
                HubError::other(format!("agent {agent_id} connection task ended"))
            })??);
        let capabilities = serde_json::to_string(&handle.capabilities)?;
        self.store()
            .upsert_agent_cache(agent_id, "{}", &capabilities)?;
        handles.insert(agent_id.to_string(), Arc::clone(&handle));
        Ok(handle)
    }

    async fn request_agent<T>(
        &self,
        handle: &AgentHandle,
        f: impl FnOnce(oneshot::Sender<Result<T, HubError>>) -> AgentCommand,
    ) -> Result<T, HubError>
    where
        T: Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        handle
            .cmd_tx
            .send(f(reply))
            .await
            .map_err(|_| HubError::other("agent command loop is closed"))?;
        rx.await
            .map_err(|_| HubError::other("agent command response dropped"))?
    }

    fn agent_config(&self, agent_id: &str) -> Result<AgentEndpointConfig, HubError> {
        self.registry
            .read()
            .agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| HubError::not_found("agent", agent_id))
    }

    fn ensure_conversation(&self, conv_id: &str) -> Result<ConversationRow, HubError> {
        self.store()
            .conversation(conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))
    }

    fn bind_session(&self, conv: &ConversationRow, agent_cfg: &AgentEndpointConfig) {
        self.ctx.bind_session(
            &conv.agent_session_id,
            SessionBinding {
                conv_id: conv.id.clone(),
                agent_id: conv.agent_id.clone(),
                permission_policy: agent_cfg.permission_policy,
                fs: agent_cfg.client_capabilities.fs.clone(),
                cwd: conv.cwd.as_deref().map(PathBuf::from).unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }),
            },
        );
    }

    async fn ensure_live_session(
        &self,
        conv: &ConversationRow,
        agent_cfg: &AgentEndpointConfig,
        handle: &AgentHandle,
    ) -> Result<(), HubError> {
        if matches!(self.runtime.get(&conv.id), Some((SessionState::Live, _))) {
            self.bind_session(conv, agent_cfg);
            return Ok(());
        }

        let cwd = conv
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);

        if handle.capabilities.session_capabilities.resume.is_some() {
            match self
                .request_agent(handle, |reply| AgentCommand::ResumeSession {
                    conv_id: conv.id.clone(),
                    agent_id: conv.agent_id.clone(),
                    agent_session_id: conv.agent_session_id.clone(),
                    cwd: cwd.clone(),
                    reply,
                })
                .await
            {
                Ok(created) => {
                    self.persist_session_snapshots(&conv.id, &created)?;
                    self.bind_session(conv, agent_cfg);
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

        let created = self
            .request_agent(handle, |reply| AgentCommand::LoadSession {
                conv_id: conv.id.clone(),
                agent_id: conv.agent_id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                cwd,
                reply,
            })
            .await
            .map_err(|source| HubError::ResumeLoadFailed {
                attempted_method: "session/load",
                endpoint: conv.agent_id.clone(),
                conv_id: conv.id.clone(),
                agent_session_id: conv.agent_session_id.clone(),
                source: Box::new(source),
            })?;
        self.persist_session_snapshots(&conv.id, &created)?;
        self.bind_session(conv, agent_cfg);
        self.runtime
            .insert(&conv.id, SessionState::Live, self.runtime.next_generation());
        Ok(())
    }

    fn persist_session_snapshots(
        &self,
        conv_id: &str,
        created: &SessionCreated,
    ) -> Result<(), HubError> {
        if let Some(config) = &created.config_options {
            self.store()
                .set_config_snapshot(conv_id, config, created.modes.as_ref())?;
        }
        Ok(())
    }

    fn store_prompt_message(
        &self,
        conv_id: &str,
        run_id: &str,
        prompt: &[ContentBlock],
    ) -> Result<(), HubError> {
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
        })?;
        Ok(())
    }

    fn finish_prompt(
        &self,
        conv_id: &str,
        run_id: &str,
        lease: &RunLease,
        done: PromptDone,
    ) -> Result<PromptResult, HubError> {
        let stop = stop_reason_string(done.stop_reason);
        let status = if done.stop_reason == StopReason::Cancelled {
            RunStatus::Cancelled
        } else {
            RunStatus::Completed
        };
        let _ = self.finalize_run(conv_id, run_id, status, Some(&stop))?;
        lease.complete();
        Ok(PromptResult {
            conv_id: conv_id.to_string(),
            run_id: run_id.to_string(),
            stop_reason: stop,
        })
    }
}

/// Embedded-library client. All methods go through the singleton daemon's
/// JSON-RPC surface rather than bypassing it.
pub struct HubClient {
    rpc: RpcClient,
}

impl HubClient {
    /// Discover or spawn the singleton daemon rooted at `home`, then connect.
    pub async fn connect_or_spawn(home: impl AsRef<Path>) -> Result<Self, HubError> {
        Ok(Self {
            rpc: daemon::ensure_daemon(home.as_ref()).await?,
        })
    }

    pub async fn list_agents(&self) -> Result<Value, HubError> {
        self.call_value("hub/agent/list", Value::Null).await
    }

    pub async fn inspect_agent(&self, agent_id: impl Into<String>) -> Result<Value, HubError> {
        self.call_value(
            "hub/agent/inspect",
            InspectAgentParams {
                agent_id: agent_id.into(),
            },
        )
        .await
    }

    pub async fn list_agent_sessions(
        &self,
        agent_id: impl Into<String>,
    ) -> Result<Value, HubError> {
        self.call_value(
            "hub/agent/sessions",
            InspectAgentParams {
                agent_id: agent_id.into(),
            },
        )
        .await
    }

    pub async fn list_proxies(&self) -> Result<Value, HubError> {
        self.call_value("hub/proxy/list", Value::Null).await
    }

    pub async fn create_conversation(
        &self,
        params: CreateConversationParams,
    ) -> Result<ConversationCreated, HubError> {
        self.call_typed("hub/conv/create", params).await
    }

    pub async fn send_prompt(&self, params: SendPromptParams) -> Result<PromptResult, HubError> {
        self.call_typed("hub/conv/send", params).await
    }

    pub async fn list_conversations(&self, agent_id: Option<String>) -> Result<Value, HubError> {
        self.call_value("hub/conv/list", ListConversationsParams { agent_id })
            .await
    }

    pub async fn messages(
        &self,
        conv_id: impl Into<String>,
        include_audit: bool,
    ) -> Result<Value, HubError> {
        self.call_value(
            "hub/conv/messages",
            MessagesParams {
                conv_id: conv_id.into(),
                include_audit,
            },
        )
        .await
    }

    pub async fn search(&self, params: SearchParams) -> Result<Value, HubError> {
        self.call_value("hub/conv/search", params).await
    }

    pub async fn cancel(&self, conv_id: impl Into<String>) -> Result<CancelResult, HubError> {
        self.call_typed(
            "hub/conv/cancel",
            ConversationIdParams {
                conv_id: conv_id.into(),
            },
        )
        .await
    }

    pub async fn get_config(&self, conv_id: impl Into<String>) -> Result<ConfigSnapshot, HubError> {
        self.call_typed(
            "hub/conv/config",
            ConversationIdParams {
                conv_id: conv_id.into(),
            },
        )
        .await
    }

    pub async fn create_run(&self, conv_id: impl Into<String>) -> Result<String, HubError> {
        let created: RunCreated = self
            .call_typed(
                "hub/conv/create_run",
                CreateRunParams {
                    conv_id: conv_id.into(),
                },
            )
            .await?;
        Ok(created.run_id)
    }

    pub async fn finalize_run(
        &self,
        conv_id: impl Into<String>,
        run_id: impl Into<String>,
        status: RunStatus,
        stop_reason: Option<String>,
    ) -> Result<bool, HubError> {
        self.call_typed(
            "hub/conv/finalize_run",
            FinalizeRunParams {
                conv_id: conv_id.into(),
                run_id: run_id.into(),
                status,
                stop_reason,
            },
        )
        .await
    }

    pub async fn set_param(
        &self,
        conv_id: impl Into<String>,
        config_id: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/conv/set_param",
                SetParamParams {
                    conv_id: conv_id.into(),
                    config_id: config_id.into(),
                    value: value.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn set_mode(
        &self,
        conv_id: impl Into<String>,
        mode_id: impl Into<String>,
    ) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/conv/set_mode",
                SetModeParams {
                    conv_id: conv_id.into(),
                    mode_id: mode_id.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn delete_conversation(
        &self,
        conv_id: impl Into<String>,
        local_only: bool,
    ) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/conv/delete",
                DeleteConversationParams {
                    conv_id: conv_id.into(),
                    local_only,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn close_conversation(&self, conv_id: impl Into<String>) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/conv/close",
                ConversationIdParams {
                    conv_id: conv_id.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn register_agent(
        &self,
        agent_id: impl Into<String>,
        config: AgentEndpointConfig,
    ) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/agent/register",
                RegisterAgentParams {
                    agent_id: agent_id.into(),
                    config,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn remove_agent(&self, agent_id: impl Into<String>) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/agent/remove",
                RemoveAgentParams {
                    agent_id: agent_id.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn authenticate_agent(
        &self,
        agent_id: impl Into<String>,
        method_id: impl Into<String>,
    ) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/agent/authenticate",
                AuthenticateAgentParams {
                    agent_id: agent_id.into(),
                    method_id: method_id.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn logout_agent(&self, agent_id: impl Into<String>) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/agent/logout",
                RemoveAgentParams {
                    agent_id: agent_id.into(),
                },
            )
            .await?;
        Ok(())
    }
    pub async fn register_proxy(
        &self,
        proxy_id: impl Into<String>,
        config: ProxyEndpointConfig,
    ) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/proxy/register",
                RegisterProxyParams {
                    proxy_id: proxy_id.into(),
                    config,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn remove_proxy(&self, proxy_id: impl Into<String>) -> Result<(), HubError> {
        let _ = self
            .call_value(
                "hub/proxy/remove",
                RemoveProxyParams {
                    proxy_id: proxy_id.into(),
                },
            )
            .await?;
        Ok(())
    }

    async fn call_value<P: Serialize>(&self, method: &str, params: P) -> Result<Value, HubError> {
        self.rpc
            .request_value(method, serde_json::to_value(params)?)
            .await
    }

    async fn call_typed<P, T>(&self, method: &str, params: P) -> Result<T, HubError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        let value = self.call_value(method, params).await?;
        Ok(serde_json::from_value(value)?)
    }
}

fn default_search_limit() -> usize {
    50
}

fn from_params<T: DeserializeOwned>(params: Value) -> Result<T, HubError> {
    serde_json::from_value(if params.is_null() { json!({}) } else { params })
        .map_err(HubError::Json)
}

fn to_value(value: impl Serialize) -> Result<Value, HubError> {
    serde_json::to_value(value).map_err(HubError::Json)
}

fn path_to_string(path: &Path) -> Result<String, HubError> {
    path.to_str()
        .map(|s| s.to_owned())
        .ok_or_else(|| HubError::other(format!("path is not valid UTF-8: {}", path.display())))
}

fn conv_status_string(status: ConvStatus) -> String {
    match status {
        ConvStatus::Idle => "idle",
        ConvStatus::Running => "running",
        ConvStatus::Cancelling => "cancelling",
        ConvStatus::Cancelled => "cancelled",
        ConvStatus::Failed => "failed",
        ConvStatus::Completed => "completed",
        ConvStatus::Deleted => "deleted",
    }
    .to_string()
}

fn stop_reason_string(stop: StopReason) -> String {
    serde_json::to_value(stop)
        .ok()
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{stop:?}"))
}
