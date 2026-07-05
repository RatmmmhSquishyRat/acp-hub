use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy,
    ProxyEndpointConfig, ProxyTransport,
};
use acp_hub::hub::{
    ConfigParam, CreateConversationParams, HubClient, SearchParams, SendPromptParams,
};
use agent_client_protocol::schema::v1::ContentBlock;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{
    ErrorData as McpError, Json, ServerHandler, tool, tool_handler, tool_router, transport,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};

const DEFAULT_SEARCH_LIMIT: usize = 50;

type ToolResult = Result<Json<Value>, McpError>;

/// Run the ACP Hub MCP facade over stdio.
pub async fn run(home: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let client = HubClient::connect_or_spawn(&home).await?;
    let handler = AcpHubMcp {
        client: Arc::new(client),
    };
    let server = rmcp::serve_server(handler, transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

#[derive(Clone)]
struct AcpHubMcp {
    client: Arc<HubClient>,
}

#[tool_router]
impl AcpHubMcp {
    /// List registered ACP agents.
    #[tool(description = "List registered ACP agents")]
    async fn list_agents(&self) -> ToolResult {
        structured(self.client.list_agents().await.map_err(hub_error)?)
    }

    /// Register or replace an ACP agent endpoint.
    #[tool(description = "Register or replace an ACP agent endpoint")]
    async fn register_agent(
        &self,
        Parameters(params): Parameters<RegisterAgentRequest>,
    ) -> ToolResult {
        let agent_id = params.agent_id.clone();
        let config = params.into_config()?;
        self.client
            .register_agent(agent_id, config)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Remove a registered ACP agent endpoint.
    #[tool(description = "Remove a registered ACP agent endpoint")]
    async fn remove_agent(
        &self,
        Parameters(RemoveAgentRequest { agent_id }): Parameters<RemoveAgentRequest>,
    ) -> ToolResult {
        self.client
            .remove_agent(agent_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Inspect a registered ACP agent endpoint config and cached capabilities.
    #[tool(description = "Inspect a registered ACP agent endpoint config and cached capabilities")]
    async fn inspect_agent(
        &self,
        Parameters(InspectAgentRequest { agent_id }): Parameters<InspectAgentRequest>,
    ) -> ToolResult {
        structured(
            self.client
                .inspect_agent(agent_id)
                .await
                .map_err(hub_error)?,
        )
    }

    /// List agent-side sessions via ACP session/list (Layer 1 discovery).
    #[tool(description = "Discover agent-side sessions via ACP session/list")]
    async fn list_agent_sessions(
        &self,
        Parameters(ListAgentSessionsRequest { agent_id }): Parameters<ListAgentSessionsRequest>,
    ) -> ToolResult {
        structured(
            self.client
                .list_agent_sessions(agent_id)
                .await
                .map_err(hub_error)?,
        )
    }

    /// Cancel the active run for a conversation.
    #[tool(description = "Cancel the in-flight prompt for a conversation")]
    async fn cancel_conversation(
        &self,
        Parameters(CancelConversationRequest { conv_id }): Parameters<CancelConversationRequest>,
    ) -> ToolResult {
        structured(self.client.cancel(conv_id).await.map_err(hub_error)?)
    }

    /// Authenticate an ACP agent using an advertised method id.
    #[tool(description = "Authenticate an ACP agent using an advertised method id")]
    async fn authenticate_agent(
        &self,
        Parameters(params): Parameters<AuthenticateAgentRequest>,
    ) -> ToolResult {
        self.client
            .authenticate_agent(params.agent_id, params.method_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Logout an ACP agent.
    #[tool(description = "Logout an ACP agent")]
    async fn logout_agent(
        &self,
        Parameters(LogoutAgentRequest { agent_id }): Parameters<LogoutAgentRequest>,
    ) -> ToolResult {
        self.client
            .logout_agent(agent_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// List registered ACP proxies.
    #[tool(description = "List registered ACP proxies")]
    async fn list_proxies(&self) -> ToolResult {
        structured(self.client.list_proxies().await.map_err(hub_error)?)
    }

    /// Register or replace a stdio ACP proxy endpoint.
    #[tool(description = "Register or replace a stdio ACP proxy endpoint")]
    async fn register_proxy(
        &self,
        Parameters(params): Parameters<RegisterProxyRequest>,
    ) -> ToolResult {
        let config = ProxyEndpointConfig {
            transport: ProxyTransport::Stdio {
                command: params.command,
                args: params.args.unwrap_or_default(),
                env: params.env.unwrap_or_default(),
            },
        };
        self.client
            .register_proxy(params.proxy_id, config)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Remove a registered ACP proxy endpoint.
    #[tool(description = "Remove a registered ACP proxy endpoint")]
    async fn remove_proxy(
        &self,
        Parameters(RemoveProxyRequest { proxy_id }): Parameters<RemoveProxyRequest>,
    ) -> ToolResult {
        self.client
            .remove_proxy(proxy_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// List Hub conversations, optionally filtered by agent id.
    #[tool(description = "List Hub conversations, optionally filtered by agent id")]
    async fn list_conversations(
        &self,
        Parameters(params): Parameters<ListConversationsRequest>,
    ) -> ToolResult {
        structured(
            self.client
                .list_conversations(params.agent_id)
                .await
                .map_err(hub_error)?,
        )
    }

    /// Create a Hub conversation for an ACP agent.
    #[tool(description = "Create a Hub conversation for an ACP agent")]
    async fn create_conversation(
        &self,
        Parameters(params): Parameters<CreateConversationRequest>,
    ) -> ToolResult {
        let created = self
            .client
            .create_conversation(CreateConversationParams {
                agent_id: params.agent_id,
                cwd: params.cwd,
                agent_session_id: params.agent_session_id,
                mcp_servers: Vec::new(),
                additional_directories: params.additional_directories.unwrap_or_default(),
            })
            .await
            .map_err(hub_error)?;
        structured(created)
    }

    /// Delete a Hub conversation projection and optionally the remote ACP session.
    #[tool(
        description = "Delete a Hub conversation projection and optionally the remote ACP session"
    )]
    async fn delete_conversation(
        &self,
        Parameters(params): Parameters<DeleteConversationRequest>,
    ) -> ToolResult {
        self.client
            .delete_conversation(params.conv_id, params.local_only.unwrap_or(false))
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Close a remote ACP session while retaining the Hub projection.
    #[tool(description = "Close a remote ACP session while retaining the Hub projection")]
    async fn close_conversation(
        &self,
        Parameters(CloseConversationRequest { conv_id }): Parameters<CloseConversationRequest>,
    ) -> ToolResult {
        self.client
            .close_conversation(conv_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Search Hub conversation and message projections.
    #[tool(description = "Search Hub conversation and message projections")]
    async fn search(&self, Parameters(params): Parameters<SearchRequest>) -> ToolResult {
        structured(
            self.client
                .search(SearchParams {
                    query: params.query,
                    agent_id: params.agent_id,
                    conv_id: params.conv_id,
                    limit: params.limit.unwrap_or(DEFAULT_SEARCH_LIMIT),
                    offset: 0,
                })
                .await
                .map_err(hub_error)?,
        )
    }

    /// Send a text message to a conversation and return the final result plus stored messages.
    #[tool(
        description = "Send a text message to a conversation and return the final result plus stored messages"
    )]
    async fn send_message(&self, Parameters(params): Parameters<SendMessageRequest>) -> ToolResult {
        let conv_id = params.conv_id;
        let result = self
            .client
            .send_prompt(SendPromptParams {
                conv_id: conv_id.clone(),
                prompt: vec![ContentBlock::from(params.text)],
                params: params
                    .params
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                mode_id: params.mode_id,
            })
            .await
            .map_err(hub_error)?;
        let messages = self
            .client
            .messages(conv_id, false)
            .await
            .map_err(hub_error)?;
        structured(json!({
            "final": result,
            "messages": messages,
        }))
    }

    /// Set one ACP session configuration option for a conversation.
    #[tool(description = "Set one ACP session configuration option for a conversation")]
    async fn set_param(&self, Parameters(params): Parameters<SetParamRequest>) -> ToolResult {
        self.client
            .set_param(params.conv_id, params.config_id, params.value)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Set the ACP session mode for a conversation.
    #[tool(description = "Set the ACP session mode for a conversation")]
    async fn set_mode(&self, Parameters(params): Parameters<SetModeRequest>) -> ToolResult {
        self.client
            .set_mode(params.conv_id, params.mode_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Read the stored config/mode snapshot for a conversation.
    #[tool(description = "Read the stored config/mode snapshot for a conversation")]
    async fn get_config(
        &self,
        Parameters(GetConfigRequest { conv_id }): Parameters<GetConfigRequest>,
    ) -> ToolResult {
        structured(self.client.get_config(conv_id).await.map_err(hub_error)?)
    }

    /// Get stored messages for a conversation.
    #[tool(description = "Get stored messages for a conversation")]
    async fn get_messages(&self, Parameters(params): Parameters<GetMessagesRequest>) -> ToolResult {
        structured(
            self.client
                .messages(params.conv_id, params.include_audit.unwrap_or(false))
                .await
                .map_err(hub_error)?,
        )
    }
}

#[tool_handler]
impl ServerHandler for AcpHubMcp {}

#[derive(Debug, Deserialize, JsonSchema)]
struct RegisterAgentRequest {
    agent_id: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    url: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    transport_type: Option<String>,
    proxy_chain: Option<Vec<String>>,
    terminal: Option<bool>,
    read_text_file: Option<bool>,
    write_text_file: Option<bool>,
}

impl RegisterAgentRequest {
    fn into_config(mut self) -> Result<AgentEndpointConfig, McpError> {
        let proxy_chain = self.proxy_chain.take().unwrap_or_default();
        let terminal = self.terminal.take().unwrap_or(false);
        let read_text_file = self.read_text_file.take().unwrap_or(false);
        let write_text_file = self.write_text_file.take().unwrap_or(false);

        Ok(AgentEndpointConfig {
            transport: self.into_transport()?,
            proxy_chain,
            permission_policy: PermissionPolicy::default(),
            client_capabilities: ClientCapabilityConfig {
                terminal,
                fs: acp_hub::endpoint::FsConfig {
                    read_text_file,
                    write_text_file,
                    allowed_roots: Vec::new(),
                },
            },
        })
    }

    fn into_transport(self) -> Result<AgentTransport, McpError> {
        let kind = self
            .transport_type
            .as_deref()
            .map(normalize_transport_type)
            .transpose()?
            .unwrap_or_else(|| infer_transport_type(self.url.as_deref()));

        match kind {
            AgentTransportKind::Stdio => Ok(AgentTransport::Stdio {
                command: self.command.ok_or_else(|| {
                    invalid_params("register_agent requires command for stdio transport")
                })?,
                args: self.args.unwrap_or_default(),
                env: self.env.unwrap_or_default(),
            }),
            AgentTransportKind::Http => Ok(AgentTransport::Http {
                url: self.url.ok_or_else(|| {
                    invalid_params("register_agent requires url for http transport")
                })?,
                headers: self.headers.unwrap_or_default(),
            }),
            AgentTransportKind::WebSocket => Ok(AgentTransport::WebSocket {
                url: self.url.ok_or_else(|| {
                    invalid_params("register_agent requires url for websocket transport")
                })?,
                headers: self.headers.unwrap_or_default(),
            }),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveAgentRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct InspectAgentRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AuthenticateAgentRequest {
    agent_id: String,
    method_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogoutAgentRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RegisterProxyRequest {
    proxy_id: String,
    command: String,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveProxyRequest {
    proxy_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListConversationsRequest {
    agent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateConversationRequest {
    agent_id: String,
    cwd: Option<PathBuf>,
    agent_session_id: Option<String>,
    additional_directories: Option<Vec<PathBuf>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteConversationRequest {
    conv_id: String,
    local_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CloseConversationRequest {
    conv_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchRequest {
    query: String,
    agent_id: Option<String>,
    conv_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SendMessageRequest {
    conv_id: String,
    text: String,
    params: Option<Vec<McpConfigParam>>,
    mode_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct McpConfigParam {
    config_id: String,
    value: String,
}

impl From<McpConfigParam> for ConfigParam {
    fn from(value: McpConfigParam) -> Self {
        Self {
            config_id: value.config_id,
            value: value.value,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetParamRequest {
    conv_id: String,
    config_id: String,
    value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetModeRequest {
    conv_id: String,
    mode_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetConfigRequest {
    conv_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetMessagesRequest {
    conv_id: String,
    include_audit: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
enum AgentTransportKind {
    Stdio,
    Http,
    WebSocket,
}

fn infer_transport_type(url: Option<&str>) -> AgentTransportKind {
    match url {
        Some(url) if url.starts_with("ws://") || url.starts_with("wss://") => {
            AgentTransportKind::WebSocket
        }
        Some(_) => AgentTransportKind::Http,
        None => AgentTransportKind::Stdio,
    }
}

fn normalize_transport_type(value: &str) -> Result<AgentTransportKind, McpError> {
    match value.to_ascii_lowercase().as_str() {
        "stdio" => Ok(AgentTransportKind::Stdio),
        "http" | "https" => Ok(AgentTransportKind::Http),
        "ws" | "wss" | "websocket" | "web_socket" => Ok(AgentTransportKind::WebSocket),
        other => Err(invalid_params(format!(
            "unknown transport_type {other:?}; expected stdio, http, or websocket"
        ))),
    }
}

fn structured(value: impl Serialize) -> ToolResult {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|err| McpError::internal_error(err.to_string(), None))
}

fn ok() -> ToolResult {
    structured(json!({ "ok": true }))
}

fn invalid_params(message: impl Into<String>) -> McpError {
    McpError::invalid_params(message.into(), None)
}

fn hub_error(err: acp_hub::HubError) -> McpError {
    use acp_hub::HubError;

    match err {
        HubError::AuthRequired {
            endpoint,
            auth_methods,
        } => McpError::new(
            rmcp::model::ErrorCode(-32001),
            format!("authentication required for endpoint {endpoint}"),
            Some(json!({
                "reason": "auth_required",
                "endpoint": endpoint,
                "authMethods": auth_methods,
            })),
        ),
        HubError::NotFound { kind, id } => McpError::resource_not_found(
            format!("{kind} not found: {id}"),
            Some(json!({
                "kind": kind,
                "id": id,
            })),
        ),
        HubError::Conflict(conv_id) => McpError::invalid_params(
            format!("conversation {conv_id} is busy with an in-flight turn"),
            Some(json!({
                "reason": "conversation_busy",
                "convId": conv_id,
            })),
        ),
        HubError::UnsupportedCapability {
            endpoint,
            operation,
            required_capability,
        } => McpError::invalid_params(
            format!("endpoint {endpoint} does not support {operation}"),
            Some(json!({
                "reason": "unsupported_capability",
                "endpoint": endpoint,
                "operation": operation,
                "requiredCapability": required_capability,
            })),
        ),
        HubError::UnsupportedProxyTransport => McpError::invalid_params(
            "unsupported proxy transport (only stdio proxies are available in this build)",
            Some(json!({ "reason": "unsupported_proxy_transport" })),
        ),
        HubError::UnsupportedProtocolVersion => McpError::invalid_params(
            "unsupported protocol version: only ACP v1 is supported",
            Some(json!({ "reason": "unsupported_protocol_version" })),
        ),
        HubError::InvalidRegistry(message) => McpError::invalid_params(
            format!("invalid registry: {message}"),
            Some(json!({ "reason": "invalid_registry" })),
        ),

        other => McpError::internal_error(other.to_string(), None),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListAgentSessionsRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CancelConversationRequest {
    conv_id: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    use acp_hub::endpoint::AgentTransport;
    use schemars::schema_for;
    use serde_json::json;

    #[test]
    fn register_agent_request_preserves_proxy_chain_and_client_capabilities_in_config() {
        let params: RegisterAgentRequest = serde_json::from_value(json!({
            "agent_id": "codex",
            "command": "codex",
            "args": ["acp"],
            "env": { "RUST_LOG": "debug" },
            "proxy_chain": ["audit-proxy", "policy-proxy"],
            "terminal": true,
            "read_text_file": true,
            "write_text_file": true
        }))
        .unwrap();

        let config = params.into_config().unwrap();

        assert_eq!(
            config.proxy_chain,
            vec!["audit-proxy".to_string(), "policy-proxy".to_string()]
        );
        assert!(config.client_capabilities.terminal);
        assert!(config.client_capabilities.fs.read_text_file);
        assert!(config.client_capabilities.fs.write_text_file);
        assert!(config.client_capabilities.fs.allowed_roots.is_empty());
        assert!(matches!(
            config.transport,
            AgentTransport::Stdio { ref command, .. } if command == "codex"
        ));
    }

    #[test]
    fn register_agent_request_preserves_http_and_websocket_headers_in_config() {
        let http_params: RegisterAgentRequest = serde_json::from_value(json!({
            "agent_id": "remote-http",
            "url": "https://example.com/acp",
            "headers": { "Authorization": "Bearer token", "X-Agent": "hub" }
        }))
        .unwrap();
        let ws_params: RegisterAgentRequest = serde_json::from_value(json!({
            "agent_id": "remote-ws",
            "url": "wss://example.com/acp",
            "headers": { "Authorization": "Bearer ws-token" }
        }))
        .unwrap();

        let http_config = http_params.into_config().unwrap();
        let ws_config = ws_params.into_config().unwrap();

        match http_config.transport {
            AgentTransport::Http { headers, .. } => {
                assert_eq!(headers.get("Authorization").unwrap(), "Bearer token");
                assert_eq!(headers.get("X-Agent").unwrap(), "hub");
            }
            other => panic!("expected HTTP transport, got {other:?}"),
        }
        match ws_config.transport {
            AgentTransport::WebSocket { headers, .. } => {
                assert_eq!(headers.get("Authorization").unwrap(), "Bearer ws-token");
            }
            other => panic!("expected WebSocket transport, got {other:?}"),
        }
    }

    #[test]
    fn mcp_request_schemas_include_register_agent_headers() {
        let register_agent_schema =
            serde_json::to_value(schema_for!(RegisterAgentRequest)).unwrap();
        assert!(register_agent_schema.to_string().contains("\"headers\""));
        let _ = schema_for!(ListAgentSessionsRequest);
        let _ = schema_for!(CancelConversationRequest);
    }
}
