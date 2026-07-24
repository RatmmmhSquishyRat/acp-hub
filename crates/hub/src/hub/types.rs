use std::path::PathBuf;

use crate::endpoint::{AgentEndpointConfig, ProxyEndpointConfig, PublicEndpointConfig};
use crate::store::RunStatus;
use agent_client_protocol::schema::v1::{ContentBlock, McpServer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub interaction: String,
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
    /// Exact sequence allocated to this run's persisted user prompt.
    pub prompt_seq: i64,
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
    /// Default true: workbench only (PHASE1).
    #[serde(default = "default_true")]
    pub workbench: bool,
    /// Museum: all open origins.
    #[serde(default)]
    pub include_imported: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub interaction: Option<String>,
    #[serde(default = "default_list_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_true() -> bool {
    true
}

fn default_list_limit() -> usize {
    100
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
pub struct MessagesPageParams {
    pub conv_id: String,
    #[serde(default)]
    pub include_audit: bool,
    /// Restrict the page to messages owned by one exact run.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Opaque continuation returned as `nextCursor` by the preceding page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Initial sequence filter. This must remain identical on every page.
    #[serde(default)]
    pub after_seq: Option<i64>,
    pub limit: usize,
    /// Legacy pagination input. New callers must use `cursor`.
    #[serde(default)]
    pub offset: usize,
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
    pub config: PublicEndpointConfig,
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
    pub owner_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeRunParams {
    pub conv_id: String,
    pub run_id: String,
    pub owner_token: String,
    pub status: RunStatus,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

fn default_search_limit() -> usize {
    50
}
