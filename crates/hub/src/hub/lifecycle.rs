use std::sync::Arc;

use super::state::{CoreHub, OperationKind};
use crate::acp::AgentCommand;
use crate::error::HubError;
use crate::store::ConvStatus;
use tokio::sync::oneshot;

impl CoreHub {
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
        let operation =
            Arc::new(self.reserve_operation(conv_id, &conv.agent_id, OperationKind::Delete)?);
        if local_only {
            self.ctx
                .unbind_session(&conv.agent_id, &conv.agent_session_id);
            self.runtime.remove(conv_id);
            return self.store().delete_conversation(conv_id);
        }

        let handle = self.agent_handle(&conv.agent_id).await?;
        let permit = handle.cmd_tx.clone().reserve_owned().await.map_err(|_| {
            HubError::other(format!("agent {} command loop is closed", conv.agent_id))
        })?;
        let (reply, response) = oneshot::channel();
        permit.send(AgentCommand::DeleteSession {
            conv_id: conv.id.clone(),
            agent_session_id: conv.agent_session_id.clone(),
            local_only,
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
                    ctx.store().delete_conversation(&conv_id)
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
        let worker = tokio::spawn(async move {
            let _operation = operation;
            match response.await {
                Ok(Ok(())) => {
                    ctx.unbind_session(&agent_id, &agent_session_id);
                    runtime.remove(&conv_id);
                    ctx.store().set_conv_status(&conv_id, ConvStatus::Idle)
                }
                Ok(Err(error)) => Err(error),
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
