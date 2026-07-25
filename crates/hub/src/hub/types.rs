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
    /// When true (default), block until run finalize. When false, return after
    /// accepted enqueue (UX-CORE `--no-wait`).
    #[serde(default = "default_true")]
    pub wait: bool,
}

impl Default for SendPromptParams {
    fn default() -> Self {
        Self {
            conv_id: String::new(),
            prompt: Vec::new(),
            params: Vec::new(),
            mode_id: None,
            wait: true,
        }
    }
}

/// Result for `hub/conv/send`.
///
/// - Default `wait=true`: `stop_reason` set; `busy` absent.
/// - `wait=false` (accepted): `busy=running`; `stop_reason` empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub conv_id: String,
    pub run_id: String,
    /// Exact sequence allocated to this run's persisted user prompt.
    pub prompt_seq: i64,
    #[serde(default)]
    pub stop_reason: String,
    /// Present on accepted (`wait=false`) responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<String>,
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
    /// When true, connect agent to refresh capability cache (Phase 3).
    #[serde(default)]
    pub probe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShowConversationParams {
    pub conv_id: String,
    /// When true, return unmerged Store rows in transcript items.
    #[serde(default)]
    pub raw: bool,
    /// Pre-merge filter: only messages with this run_id.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Closed interval lower bound (inclusive). Requires `to_seq` when set.
    #[serde(default)]
    pub from_seq: Option<i64>,
    /// Closed interval upper bound (inclusive). Requires `from_seq` when set.
    #[serde(default)]
    pub to_seq: Option<i64>,
    /// Keep only the last N view items after merge (or raw rows if raw).
    #[serde(default)]
    pub tail: Option<usize>,
    /// Keep only the first N view items after merge.
    #[serde(default)]
    pub head: Option<usize>,
    /// Filter tokens: user|assistant|thought|tool (+ aliases). Empty = all.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Shortcut: exclude tool kinds.
    #[serde(default)]
    pub no_tools: bool,
    /// Truncate each body to at most N chars (human path; 0 = no limit).
    #[serde(default)]
    pub max_chars: Option<usize>,
}

impl ShowConversationParams {
    pub fn new(conv_id: impl Into<String>) -> Self {
        Self {
            conv_id: conv_id.into(),
            ..Default::default()
        }
    }

    pub fn with_raw(mut self, raw: bool) -> Self {
        self.raw = raw;
        self
    }
}

/// Parameters for `hub/conv/run` (UX-CORE wait resolve / stopReason SSOT).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRunParams {
    pub conv_id: String,
    /// When omitted, resolve the active in-flight run (or not_busy).
    #[serde(default)]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInspection {
    pub agent_id: String,
    pub config: PublicEndpointConfig,
    pub agent_info: Option<Value>,
    pub capabilities: Option<Value>,
    pub cache_populated: bool,
    /// `skipped` | `cached` | `ok` | `failed` (Phase 3).
    pub probe_status: String,
    pub auth_methods: Option<Value>,
    pub permission_policy: String,
    /// Operator next-step text (probe skipped, reject policy, failures).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Fixed SC-12 / Phase-3 reject substring (must appear in inspect/doctor).
pub const PERMISSION_POLICY_REJECT_HINT: &str =
    "permission_policy=reject; re-add agent with defaults or edit agents.json";

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
