use super::dispatch::to_value;
use std::{collections::HashSet, path::Path};

use super::types::*;
use crate::daemon;
use crate::endpoint::{AgentEndpointConfig, ProxyEndpointConfig};
use crate::error::HubError;
use crate::rpc::RpcClient;
use crate::store::RunStatus;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const MAX_MATERIALIZED_MESSAGE_ROWS: usize = 100_000;
const MAX_MATERIALIZED_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

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

    /// Subscribe to streamed daemon events such as `hub/conv/update`.
    pub fn subscribe_notifications(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::rpc::RpcRequest> {
        self.rpc.subscribe_notifications()
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
        const PAGE_ROWS: usize = 500;

        let conv_id = conv_id.into();
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut materialized_bytes = 0usize;
        let mut messages = Vec::new();
        loop {
            let page: crate::store::MessagePage = self
                .call_typed(
                    "hub/conv/messages_page",
                    MessagesPageParams {
                        conv_id: conv_id.clone(),
                        include_audit,
                        run_id: None,
                        cursor,
                        after_seq: None,
                        limit: PAGE_ROWS,
                        offset: 0,
                    },
                )
                .await?;
            let page_bytes = serde_json::to_vec(&page.items)?.len();
            materialized_bytes = materialized_bytes
                .checked_add(page_bytes)
                .ok_or_else(|| HubError::other("materialized message history size overflow"))?;
            if materialized_bytes > MAX_MATERIALIZED_MESSAGE_BYTES {
                return Err(HubError::ResourceLimit {
                    resource: "materialized_message_bytes",
                    limit: MAX_MATERIALIZED_MESSAGE_BYTES,
                });
            }
            if messages.len().saturating_add(page.items.len()) > MAX_MATERIALIZED_MESSAGE_ROWS {
                return Err(HubError::ResourceLimit {
                    resource: "materialized_message_rows",
                    limit: MAX_MATERIALIZED_MESSAGE_ROWS,
                });
            }
            let page_was_empty = page.items.is_empty();
            messages.extend(page.items);
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            record_message_continuation(&mut seen_cursors, &next_cursor, page_was_empty)?;
            cursor = Some(next_cursor);
        }
        to_value(messages)
    }

    pub async fn messages_page(&self, params: MessagesPageParams) -> Result<Value, HubError> {
        self.call_value("hub/conv/messages_page", params).await
    }

    pub async fn message_cursor(&self, conv_id: impl Into<String>) -> Result<i64, HubError> {
        self.call_typed(
            "hub/conv/message_cursor",
            ConversationIdParams {
                conv_id: conv_id.into(),
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

    pub async fn create_run(&self, conv_id: impl Into<String>) -> Result<RunCreated, HubError> {
        self.call_typed(
            "hub/conv/create_run",
            CreateRunParams {
                conv_id: conv_id.into(),
            },
        )
        .await
    }

    pub async fn finalize_run(
        &self,
        conv_id: impl Into<String>,
        run_id: impl Into<String>,
        owner_token: impl Into<String>,
        status: RunStatus,
        stop_reason: Option<String>,
    ) -> Result<bool, HubError> {
        self.call_typed(
            "hub/conv/finalize_run",
            FinalizeRunParams {
                conv_id: conv_id.into(),
                run_id: run_id.into(),
                owner_token: owner_token.into(),
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

fn record_message_continuation(
    seen_cursors: &mut HashSet<String>,
    next_cursor: &str,
    page_was_empty: bool,
) -> Result<(), HubError> {
    if page_was_empty {
        return Err(HubError::other(
            "message page returned a continuation without any items",
        ));
    }
    if !seen_cursors.insert(next_cursor.to_string()) {
        return Err(HubError::other(
            "message page cursor repeated without advancing",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    #[test]
    fn materializer_rejects_empty_and_repeated_continuations() {
        let mut seen = HashSet::new();
        record_message_continuation(&mut seen, "cursor-a", false).unwrap();
        let repeated = record_message_continuation(&mut seen, "cursor-a", false).unwrap_err();
        assert!(
            repeated
                .to_string()
                .contains("cursor repeated without advancing")
        );
        let empty = record_message_continuation(&mut seen, "cursor-b", true).unwrap_err();
        assert!(empty.to_string().contains("continuation without any items"));
    }
}
