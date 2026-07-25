use std::sync::Arc;

use super::state::{CoreHub, OperationKind};
use crate::acp::AgentCommand;
use crate::error::HubError;

use tokio::sync::oneshot;

/// How a conversation delete was applied (operator messaging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// Explicit `--local-only` or remote capability used local path only.
    Local,
    /// Agent advertised session/delete and remote delete succeeded.
    Remote,
    /// Agent has no session/delete (or rejected capability); hub projection removed.
    LocalFallback,
}

impl DeleteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::LocalFallback => "local_fallback",
        }
    }
}

fn capabilities_advertise_session_delete(caps_json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(caps_json) else {
        return false;
    };
    // Live AgentCapabilities serialize with sessionCapabilities.delete; cache
    // may also nest under "session" / "session_capabilities".
    v.pointer("/sessionCapabilities/delete").is_some()
        || v.pointer("/session_capabilities/delete").is_some()
        || v.get("delete").is_some()
}

impl CoreHub {
    /// Delete a conversation projection and optionally the remote ACP session.
    ///
    /// Operator baseline B-DEL-01: default path **always succeeds** at removing
    /// the hub projection. When the agent has no `session/delete`, we soft-delete
    /// locally and report [`DeleteMode::LocalFallback`] (no cold-start required
    /// when the capability cache already says so).
    pub async fn delete_conversation(
        &self,
        conv_id: &str,
        local_only: bool,
    ) -> Result<DeleteMode, HubError> {
        let conv = self
            .store()
            .conversation(conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))?;
        let operation =
            Arc::new(self.reserve_operation(conv_id, &conv.agent_id, OperationKind::Delete)?);

        let finish_local = |mode: DeleteMode| -> Result<DeleteMode, HubError> {
            self.ctx
                .unbind_session(&conv.agent_id, &conv.agent_session_id);
            self.runtime.remove(conv_id);
            self.store().delete_conversation(conv_id)?;
            Ok(mode)
        };

        if local_only {
            let _operation = operation;
            return finish_local(DeleteMode::Local);
        }

        // Known-no-delete from cache → local fallback without agent connect.
        if let Some((_, caps_json)) = self.store().agent_cache(&conv.agent_id)?
            && !capabilities_advertise_session_delete(&caps_json)
        {
            let _operation = operation;
            return finish_local(DeleteMode::LocalFallback);
        }

        let handle = self.agent_handle(&conv.agent_id).await?;
        // Live capability gate (handles may be warmer than cache).
        if handle.capabilities.session_capabilities.delete.is_none() {
            let _operation = operation;
            return finish_local(DeleteMode::LocalFallback);
        }

        let permit = handle.cmd_tx.clone().reserve_owned().await.map_err(|_| {
            HubError::other(format!("agent {} command loop is closed", conv.agent_id))
        })?;
        let (reply, response) = oneshot::channel();
        permit.send(AgentCommand::DeleteSession {
            conv_id: conv.id.clone(),
            agent_session_id: conv.agent_session_id.clone(),
            local_only: false,
            reply,
        });

        let ctx = Arc::clone(&self.ctx);
        let runtime = Arc::clone(&self.runtime);
        let agent_id = conv.agent_id;
        let agent_session_id = conv.agent_session_id;
        let conv_id = conv.id;
        let worker = tokio::spawn(async move {
            let _operation = operation;
            match response.await {
                Ok(Ok(())) => {
                    ctx.unbind_session(&agent_id, &agent_session_id);
                    runtime.remove(&conv_id);
                    ctx.store().delete_conversation(&conv_id)?;
                    Ok(DeleteMode::Remote)
                }
                Ok(Err(HubError::UnsupportedCapability { .. })) => {
                    // Race: live caps lost delete mid-flight — still clean hub.
                    ctx.unbind_session(&agent_id, &agent_session_id);
                    runtime.remove(&conv_id);
                    ctx.store().delete_conversation(&conv_id)?;
                    Ok(DeleteMode::LocalFallback)
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err(HubError::other(format!(
                    "agent {agent_id} command response dropped"
                ))),
            }
        });
        worker
            .await
            .map_err(|error| HubError::other(format!("delete worker failed: {error}")))?
    }

    /// Close the remote ACP session and evict the runtime entry; projection is retained.
    pub async fn close_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        let conv = self
            .store()
            .conversation(conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))?;
        let operation =
            Arc::new(self.reserve_operation(conv_id, &conv.agent_id, OperationKind::Close)?);
        let handle = self.agent_handle(&conv.agent_id).await?;
        let permit = handle.cmd_tx.clone().reserve_owned().await.map_err(|_| {
            HubError::other(format!("agent {} command loop is closed", conv.agent_id))
        })?;
        let (reply, response) = oneshot::channel();
        permit.send(AgentCommand::CloseSession {
            conv_id: conv.id.clone(),
            agent_session_id: conv.agent_session_id.clone(),
            reply,
        });

        let ctx = Arc::clone(&self.ctx);
        let runtime = Arc::clone(&self.runtime);
        let agent_id = conv.agent_id;
        let agent_session_id = conv.agent_session_id;
        let conv_id = conv.id;
        let was_busy = conv.busy.is_busy();
        let worker = tokio::spawn(async move {
            let _operation = operation;
            match response.await {
                Ok(Ok(())) => {
                    ctx.unbind_session(&agent_id, &agent_session_id);
                    runtime.remove(&conv_id);
                    if was_busy {
                        // Finalize in-flight run as failed with stop reason closed.
                        if let Ok(Some(run_id)) = ctx.store().active_run_id(&conv_id) {
                            let _ = ctx.store().finalize_run_cas(
                                &run_id,
                                &conv_id,
                                crate::store::RunStatus::Failed,
                                Some("closed"),
                            );
                        }
                    }
                    ctx.store().close_conversation_local(&conv_id, was_busy)
                }
                Ok(Err(error)) => {
                    // Remote unsupported/fail → still local close (PHASE1 §7).
                    let msg = error.to_string().to_ascii_lowercase();
                    if msg.contains("unsupported") || msg.contains("not support") {
                        ctx.unbind_session(&agent_id, &agent_session_id);
                        runtime.remove(&conv_id);
                        ctx.store().close_conversation_local(&conv_id, was_busy)?;
                        return Ok(());
                    }
                    Err(error)
                }
                Err(_) => Err(HubError::other(format!(
                    "agent {agent_id} command response dropped"
                ))),
            }
        });
        worker
            .await
            .map_err(|error| HubError::other(format!("close worker failed: {error}")))?
    }
}
