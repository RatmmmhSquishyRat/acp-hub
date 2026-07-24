use super::state::CoreHub;
use super::types::*;
use crate::error::HubError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

impl CoreHub {
    /// Dispatch daemon JSON-RPC method names to CoreHub methods.
    pub async fn handle_rpc(&self, method: &str, params: Value) -> Result<Value, HubError> {
        match method {
            "hub/agent/list" => to_value(self.list_agents()),
            "hub/agent/inspect" => {
                let p: InspectAgentParams = from_params(params)?;
                to_value(self.inspect_agent(&p.agent_id)?)
            }
            "hub/agent/register" => {
                let p: RegisterAgentParams = from_params(params)?;
                self.register_agent(p.agent_id, p.config).await?;
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
            "hub/proxy/list" => to_value(self.list_proxies()),
            "hub/proxy/register" => {
                let p: RegisterProxyParams = from_params(params)?;
                self.register_proxy(p.proxy_id, p.config).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/proxy/remove" => {
                let p: RemoveProxyParams = from_params(params)?;
                self.remove_proxy(&p.proxy_id).await?;
                Ok(json!({ "ok": true }))
            }
            "hub/conv/create" => {
                let p: CreateConversationParams = from_params(params)?;
                to_value(self.create_conversation(p).await?)
            }
            "hub/conv/list" => {
                let p: ListConversationsParams = from_params(params)?;
                let force_workbench = p.workbench && (p.include_imported || p.status.is_some());
                let filter = crate::store::ListConversationsFilter {
                    agent_id: p.agent_id,
                    workbench: p.workbench,
                    include_imported: p.include_imported,
                    status: p.status,
                    interaction: p.interaction,
                    limit: if p.limit == 0 { 100 } else { p.limit },
                    offset: p.offset,
                    force_workbench,
                };
                to_value(self.list_conversations_filtered(&filter)?)
            }
            "hub/conv/messages_page" => {
                let p: MessagesPageParams = from_params(params)?;
                to_value(self.messages_page(&p)?)
            }
            "hub/conv/message_cursor" => {
                let p: ConversationIdParams = from_params(params)?;
                to_value(self.max_message_seq(&p.conv_id)?)
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
                to_value(self.create_run(&p.conv_id)?)
            }
            "hub/conv/finalize_run" => {
                let p: FinalizeRunParams = from_params(params)?;
                to_value(self.finalize_run(
                    &p.conv_id,
                    &p.run_id,
                    &p.owner_token,
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
}

fn from_params<T: DeserializeOwned>(params: Value) -> Result<T, HubError> {
    serde_json::from_value(if params.is_null() { json!({}) } else { params })
        .map_err(HubError::Json)
}

pub(super) fn to_value(value: impl Serialize) -> Result<Value, HubError> {
    serde_json::to_value(value).map_err(HubError::Json)
}
