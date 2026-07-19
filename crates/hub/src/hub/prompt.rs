use std::sync::Arc;

use super::state::{CoreHub, OperationKind, PromptOperation};
use super::types::{CancelResult, ConfigSnapshot, PromptResult, SendPromptParams};
use crate::acp::AgentCommand;
use crate::error::HubError;
use crate::runtime::{RunLease, SessionState};
use crate::store::{MessageSource, NewMessage, RunStatus, search_body};
use agent_client_protocol::schema::v1::{CancelNotification, ContentBlock, SessionId, StopReason};
use tokio::sync::oneshot;
use uuid::Uuid;

impl CoreHub {
    /// Send a prompt turn through the live ACP connection.
    pub async fn send_prompt(&self, params: SendPromptParams) -> Result<PromptResult, HubError> {
        let conv = self
            .store()
            .conversation(&params.conv_id)?
            .ok_or_else(|| HubError::not_found("conversation", &params.conv_id))?;
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

        let handle = self.agent_handle(&conv.agent_id).await?;
        self.ensure_live_session(&conv, &agent_cfg, &handle, Some(Arc::clone(&operation)))
            .await?;
        let permit = handle.cmd_tx.clone().reserve_owned().await.map_err(|_| {
            HubError::other(format!("agent {} command loop is closed", conv.agent_id))
        })?;

        self.store().create_run(&run_id, &conv.id)?;
        let Some(run_lease) = RunLease::acquire(Arc::clone(&self.runtime), &conv.id) else {
            let _ = self.finalize_run(&conv.id, &run_id, RunStatus::Failed, None)?;
            return Err(HubError::other("could not acquire run lease"));
        };
        self.ctx
            .set_current_run(&conv.agent_id, &conv.agent_session_id, &run_id);
        let prompt_seq = match self.store_prompt_message(&conv.id, &run_id, &params.prompt) {
            Ok(prompt_seq) => prompt_seq,
            Err(error) => {
                self.ctx
                    .clear_current_run(&conv.agent_id, &conv.agent_session_id);
                let _ = self.finalize_run(&conv.id, &run_id, RunStatus::Failed, None)?;
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
            let result = match command_result {
                Ok(done) => {
                    let stop_reason = stop_reason_string(done.stop_reason);
                    let status = if done.stop_reason == StopReason::Cancelled {
                        RunStatus::Cancelled
                    } else {
                        RunStatus::Completed
                    };
                    ctx.store()
                        .finalize_run_cas(&run_id, &conv_id, status, Some(&stop_reason))
                        .map(|_| PromptResult {
                            conv_id: conv_id.clone(),
                            run_id: run_id.clone(),
                            prompt_seq,
                            stop_reason,
                        })
                }
                Err(command_error) => {
                    match ctx
                        .store()
                        .finalize_run_cas(&run_id, &conv_id, RunStatus::Failed, None)
                    {
                        Ok(_) => Err(command_error),
                        Err(finalize_error) => Err(finalize_error),
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
                self.ensure_conversation(conv_id)?;
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: None,
                    requested: false,
                });
            };
            let OperationKind::Prompt(active) = &entry.kind else {
                return Ok(CancelResult {
                    conv_id: conv_id.to_string(),
                    run_id: None,
                    requested: false,
                });
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

            current.cancel_requested = true;
            if let Err(error) =
                handle
                    .cx
                    .send_notification(CancelNotification::new(SessionId::new(
                        active.agent_session_id.as_str(),
                    )))
            {
                current.cancel_requested = false;
                return Err(error.into());
            }
            if let Some((_, generation)) = self.runtime.get(conv_id) {
                self.runtime.transition(
                    conv_id,
                    SessionState::Live,
                    SessionState::Cancelling,
                    generation,
                );
            }
            let _ = self.finalize_run(conv_id, &active.run_id, RunStatus::Cancelling, None)?;
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
