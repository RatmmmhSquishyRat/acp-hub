use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, FsConfig, PermissionPolicy,
    ProxyEndpointConfig, ProxyTransport,
};
use acp_hub::hub::{
    ConfigParam, CreateConversationParams, HubClient, MessagesPageParams, SearchParams,
    SendPromptParams,
};
use agent_client_protocol::schema::v1::{ContentBlock, McpServer};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{
    ErrorData as McpError, Json, ServerHandler, tool, tool_handler, tool_router, transport,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};

const DEFAULT_SEARCH_LIMIT: usize = 50;
const DEFAULT_MESSAGE_LIMIT: usize = 100;
const MAX_PAGE_LIMIT: usize = 200;
const MAX_PROMPT_BYTES: usize = 16 * 1024 * 1024;

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
    #[tool(
        description = "List registered ACP agents",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_agents(&self, Parameters(_params): Parameters<EmptyRequest>) -> ToolResult {
        structured(self.client.list_agents().await.map_err(hub_error)?)
    }

    /// Register or replace an ACP agent endpoint.
    #[tool(
        description = "Register or replace an ACP agent endpoint",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
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
    #[tool(
        description = "Remove a registered ACP agent endpoint",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
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
    #[tool(
        description = "Inspect a registered ACP agent endpoint config and cached capabilities",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
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

    /// Authenticate an ACP agent using an advertised method id.
    #[tool(
        description = "Authenticate an ACP agent using an advertised method id",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
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
    #[tool(
        description = "Logout an ACP agent",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
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
    #[tool(
        description = "List registered ACP proxies",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_proxies(&self, Parameters(_params): Parameters<EmptyRequest>) -> ToolResult {
        structured(self.client.list_proxies().await.map_err(hub_error)?)
    }

    /// Register or replace a stdio ACP proxy endpoint.
    #[tool(
        description = "Register or replace a stdio ACP proxy endpoint",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
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
    #[tool(
        description = "Remove a registered ACP proxy endpoint",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
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
    #[tool(
        description = "List Hub conversations, optionally filtered by agent id",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
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

    /// List sessions advertised by one ACP agent.
    #[tool(
        description = "Discover sessions advertised by one ACP agent and refresh their Hub projections",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn list_agent_sessions(
        &self,
        Parameters(InspectAgentRequest { agent_id }): Parameters<InspectAgentRequest>,
    ) -> ToolResult {
        structured(
            self.client
                .list_agent_sessions(agent_id)
                .await
                .map_err(hub_error)?,
        )
    }

    /// Create a Hub conversation for an ACP agent.
    #[tool(
        description = "Create a Hub conversation for an ACP agent",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn create_conversation(
        &self,
        Parameters(params): Parameters<CreateConversationRequest>,
    ) -> ToolResult {
        let cwd = resolve_cwd(params.cwd)?;
        let mcp_servers = parse_mcp_servers(params.mcp_servers)?;
        let additional_directories =
            resolve_directories(params.additional_directories.unwrap_or_default())?;
        let created = self
            .client
            .create_conversation(CreateConversationParams {
                agent_id: params.agent_id,
                cwd: Some(cwd),
                agent_session_id: params.agent_session_id,
                mcp_servers,
                additional_directories,
            })
            .await
            .map_err(hub_error)?;
        structured(created)
    }

    /// Delete a Hub conversation projection and optionally the remote ACP session.
    #[tool(
        description = "Delete a Hub conversation projection and optionally the remote ACP session",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
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
    #[tool(
        description = "Close a remote ACP session while retaining the Hub projection",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
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
    #[tool(
        description = "Search Hub conversation and message projections",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search(&self, Parameters(params): Parameters<SearchRequest>) -> ToolResult {
        structured(
            self.client
                .search(SearchParams {
                    query: params.query,
                    agent_id: params.agent_id,
                    conv_id: params.conv_id,
                    limit: bounded_limit(params.limit, DEFAULT_SEARCH_LIMIT)?,
                    offset: params.offset.unwrap_or_default(),
                })
                .await
                .map_err(hub_error)?,
        )
    }

    /// Send a text message to a conversation and return the final result plus stored messages.
    #[tool(
        description = "Send a text message to a conversation and return the final result plus stored messages",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    async fn send_message(&self, Parameters(params): Parameters<SendMessageRequest>) -> ToolResult {
        if params.text.len() > MAX_PROMPT_BYTES {
            return Err(invalid_params(format!(
                "text exceeds {MAX_PROMPT_BYTES} bytes"
            )));
        }
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
        let page = self
            .client
            .messages_page(prompt_messages_page_params(
                &conv_id,
                result.prompt_seq,
                &result.run_id,
            ))
            .await
            .map_err(hub_error)?;
        structured(json!({
            "final": result,
            "messages": page["items"],
            "nextCursor": page["nextCursor"],
        }))
    }

    /// Cancel the active run for a conversation.
    #[tool(
        description = "Cancel the active run for a conversation",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    async fn cancel_conversation(
        &self,
        Parameters(CloseConversationRequest { conv_id }): Parameters<CloseConversationRequest>,
    ) -> ToolResult {
        structured(self.client.cancel(conv_id).await.map_err(hub_error)?)
    }

    /// Set one ACP session configuration option for a conversation.
    #[tool(
        description = "Set one ACP session configuration option for a conversation",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn set_param(&self, Parameters(params): Parameters<SetParamRequest>) -> ToolResult {
        self.client
            .set_param(params.conv_id, params.config_id, params.value)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Set the ACP session mode for a conversation.
    #[tool(
        description = "Set the ACP session mode for a conversation",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn set_mode(&self, Parameters(params): Parameters<SetModeRequest>) -> ToolResult {
        self.client
            .set_mode(params.conv_id, params.mode_id)
            .await
            .map_err(hub_error)?;
        ok()
    }

    /// Read the stored config/mode snapshot for a conversation.
    #[tool(
        description = "Read the stored config/mode snapshot for a conversation",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_config(
        &self,
        Parameters(GetConfigRequest { conv_id }): Parameters<GetConfigRequest>,
    ) -> ToolResult {
        structured(self.client.get_config(conv_id).await.map_err(hub_error)?)
    }

    /// Get stored messages for a conversation.
    #[tool(
        description = "Get stored messages for a conversation",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_messages(&self, Parameters(params): Parameters<GetMessagesRequest>) -> ToolResult {
        let limit = bounded_limit(params.limit, DEFAULT_MESSAGE_LIMIT)?;
        structured(
            self.client
                .messages_page(MessagesPageParams {
                    conv_id: params.conv_id,
                    include_audit: params.include_audit.unwrap_or(false),
                    after_seq: params.after_seq,
                    run_id: params.run_id,
                    cursor: params.cursor,
                    limit,
                    offset: params.offset.unwrap_or_default(),
                })
                .await
                .map_err(hub_error)?,
        )
    }
}

#[tool_handler]
impl ServerHandler for AcpHubMcp {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegisterAgentRequest {
    agent_id: String,
    transport: RegisterAgentTransport,
    proxy_chain: Option<Vec<String>>,
    permission_policy: Option<String>,
    client_capabilities: Option<McpClientCapabilityConfig>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RegisterAgentTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    Websocket {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl RegisterAgentTransport {
    fn into_config(self) -> AgentTransport {
        match self {
            Self::Stdio { command, args, env } => AgentTransport::Stdio { command, args, env },
            Self::Http { url, headers } => AgentTransport::Http { url, headers },
            Self::Websocket { url, headers } => AgentTransport::WebSocket { url, headers },
        }
    }
}

impl RegisterAgentRequest {
    fn into_config(self) -> Result<AgentEndpointConfig, McpError> {
        let transport = self.transport.into_config();
        let permission_policy =
            normalize_permission_policy(self.permission_policy.as_deref().unwrap_or("auto-allow"))?;
        let client_capabilities = self.client_capabilities.unwrap_or_default().into_config()?;
        Ok(AgentEndpointConfig {
            transport,
            proxy_chain: self.proxy_chain.unwrap_or_default(),
            permission_policy,
            client_capabilities,
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct McpClientCapabilityConfig {
    fs: Option<McpFsConfig>,
    terminal: Option<bool>,
}

impl McpClientCapabilityConfig {
    fn into_config(self) -> Result<ClientCapabilityConfig, McpError> {
        Ok(ClientCapabilityConfig {
            fs: self.fs.unwrap_or_default().into_config()?,
            // Match CLI / endpoint defaults: local trusted use enables terminal.
            terminal: self.terminal.unwrap_or(true),
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct McpFsConfig {
    read_text_file: Option<bool>,
    write_text_file: Option<bool>,
    allowed_roots: Option<Vec<PathBuf>>,
}

impl McpFsConfig {
    fn into_config(self) -> Result<FsConfig, McpError> {
        let allowed_roots = resolve_directories(self.allowed_roots.unwrap_or_default())?;
        Ok(FsConfig {
            read_text_file: self.read_text_file.unwrap_or(true),
            write_text_file: self.write_text_file.unwrap_or(true),
            allowed_roots,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveAgentRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InspectAgentRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AuthenticateAgentRequest {
    agent_id: String,
    method_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogoutAgentRequest {
    agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegisterProxyRequest {
    proxy_id: String,
    command: String,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveProxyRequest {
    proxy_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListConversationsRequest {
    agent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateConversationRequest {
    agent_id: String,
    cwd: Option<PathBuf>,
    agent_session_id: Option<String>,
    mcp_servers: Option<Vec<Value>>,
    additional_directories: Option<Vec<PathBuf>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteConversationRequest {
    conv_id: String,
    local_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseConversationRequest {
    conv_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchRequest {
    query: String,
    agent_id: Option<String>,
    conv_id: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendMessageRequest {
    conv_id: String,
    text: String,
    params: Option<Vec<McpConfigParam>>,
    mode_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct SetParamRequest {
    conv_id: String,
    config_id: String,
    value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetModeRequest {
    conv_id: String,
    mode_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetConfigRequest {
    conv_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetMessagesRequest {
    conv_id: String,
    include_audit: Option<bool>,
    run_id: Option<String>,
    after_seq: Option<i64>,
    cursor: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

fn normalize_permission_policy(value: &str) -> Result<PermissionPolicy, McpError> {
    match value.to_ascii_lowercase().as_str() {
        "reject" => Ok(PermissionPolicy::Reject),
        "auto-cancel" | "auto_cancel" => Ok(PermissionPolicy::AutoCancel),
        "auto-allow" | "auto_allow" => Ok(PermissionPolicy::AutoAllow),
        other => Err(invalid_params(format!(
            "unknown permission_policy {other:?}; expected reject, auto-cancel, or auto-allow"
        ))),
    }
}

fn resolve_cwd(cwd: Option<PathBuf>) -> Result<PathBuf, McpError> {
    let cwd = cwd
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(|err| invalid_params(format!("cannot resolve caller cwd: {err}")))?;
    let cwd = dunce::canonicalize(&cwd)
        .map_err(|err| invalid_params(format!("invalid cwd {}: {err}", cwd.display())))?;
    if !cwd.is_dir() {
        return Err(invalid_params(format!(
            "cwd is not a directory: {}",
            cwd.display()
        )));
    }
    Ok(cwd)
}

fn resolve_directories(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, McpError> {
    paths
        .into_iter()
        .map(|path| {
            let resolved = dunce::canonicalize(&path).map_err(|err| {
                invalid_params(format!("invalid directory {}: {err}", path.display()))
            })?;
            if !resolved.is_dir() {
                return Err(invalid_params(format!(
                    "path is not a directory: {}",
                    resolved.display()
                )));
            }
            Ok(resolved)
        })
        .collect()
}

fn parse_mcp_servers(values: Option<Vec<Value>>) -> Result<Vec<McpServer>, McpError> {
    values
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            serde_json::from_value(value)
                .map_err(|err| invalid_params(format!("invalid ACP MCP server config: {err}")))
        })
        .collect()
}

fn prompt_messages_page_params(conv_id: &str, prompt_seq: i64, run_id: &str) -> MessagesPageParams {
    MessagesPageParams {
        conv_id: conv_id.to_string(),
        include_audit: false,
        after_seq: Some(prompt_seq),
        run_id: Some(run_id.to_string()),
        cursor: None,
        limit: MAX_PAGE_LIMIT,
        offset: 0,
    }
}

fn bounded_limit(limit: Option<usize>, default: usize) -> Result<usize, McpError> {
    let limit = limit.unwrap_or(default);
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(invalid_params(format!(
            "limit must be between 1 and {MAX_PAGE_LIMIT}"
        )));
    }
    Ok(limit)
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
        HubError::InvalidCursor { reason } => McpError::invalid_params(
            format!("invalid message cursor: {reason}"),
            Some(json!({
                "reason": "invalid_cursor",
                "detail": reason,
            })),
        ),
        HubError::StaleCursor {
            conv_id,
            expected_generation,
            current_generation,
        } => McpError::invalid_params(
            format!("stale message cursor for conversation {conv_id}; restart pagination"),
            Some(json!({
                "reason": "stale_cursor",
                "convId": conv_id,
                "expectedGeneration": expected_generation,
                "currentGeneration": current_generation,
            })),
        ),
        HubError::InvalidRegistry(message) => McpError::invalid_params(
            format!("invalid registry: {message}"),
            Some(json!({ "reason": "invalid_registry" })),
        ),
        other => McpError::internal_error(other.to_string(), None),
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
