//! Hub-wide error type.

/// All Hub operations return [`Result<T, HubError>`].
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// A requested operation requires an endpoint capability the agent did not
    /// advertise (e.g. `session/load` without `loadSession`).
    #[error("endpoint {endpoint} does not support {operation} (requires {required_capability})")]
    UnsupportedCapability {
        endpoint: String,
        operation: &'static str,
        required_capability: &'static str,
    },

    /// A bounded protocol or daemon resource exceeded its documented ceiling.
    #[error("resource limit exceeded for {resource} (limit {limit})")]
    ResourceLimit {
        resource: &'static str,
        limit: usize,
    },

    /// A proxy endpoint used a transport this Hub build does not support.
    /// Proxy components are stdio-only for this SDK revision.
    #[error("unsupported proxy transport (only stdio proxies are available in this build)")]
    UnsupportedProxyTransport,

    /// The peer negotiated an ACP major version this Hub does not serve.
    #[error("unsupported protocol version: only ACP v1 is supported")]
    UnsupportedProtocolVersion,

    /// An agent returned `auth_required`; the caller must authenticate first.
    #[error("authentication required for endpoint {endpoint}")]
    AuthRequired {
        endpoint: String,
        auth_methods: Vec<AuthMethodSummary>,
    },

    /// Resuming or loading an existing conversation failed and the projection
    /// was left untouched (never silently creates a new empty session).
    #[error("could not {attempted_method} conversation on endpoint {endpoint}")]
    ResumeLoadFailed {
        attempted_method: &'static str,
        endpoint: String,
        conv_id: String,
        agent_session_id: String,
        #[source]
        source: Box<HubError>,
    },

    /// A mutating operation collided with an in-flight turn on the same
    /// conversation (`hub/conv/send` is single-flight per conversation).
    /// Prefer [`HubError::ConversationBusy`] for operator-facing busy gates.
    #[error("conversation {0} is busy with an in-flight turn")]
    Conflict(String),

    /// Phase-1: conversation has an in-flight run (code `conversation_busy`).
    #[error("conversation {conv_id} has an in-flight run")]
    ConversationBusy { conv_id: String, busy: String },

    /// Phase-1: cancel when not busy (code `not_busy`).
    #[error("conversation {conv_id} is not busy")]
    NotBusy { conv_id: String },

    /// Write gate: conversation is read-only (Option A / IDE).
    #[error("{message}")]
    ReadOnlyConversation {
        conv_id: String,
        origin: String,
        interaction: String,
        /// When true, message must state bind cannot make IDE sessions writable.
        ide: bool,
        message: String,
    },

    /// Conversation is closed; send/param-set denied.
    #[error("conversation {conv_id} is closed")]
    ConversationClosed { conv_id: String },

    /// Endpoint permission policy is reject (operator must re-add/edit).
    #[error("{message}")]
    PermissionPolicyReject { message: String },

    /// A conversation, agent, or proxy id was not found in the registry/projection.
    #[error("{kind} not found: {id}")]
    NotFound { kind: &'static str, id: String },

    /// Registry/agents.json validation failure.
    #[error("invalid registry: {0}")]
    InvalidRegistry(String),

    /// An opaque message-page cursor was malformed, tampered with, or reused
    /// with a different query.
    #[error("invalid message cursor: {reason}")]
    InvalidCursor { reason: String },

    /// The conversation projection changed after a message-page cursor was
    /// issued, so continuing it would mix two projection generations.
    #[error(
        "stale message cursor for conversation {conv_id}: expected projection generation \
         {expected_generation}, current generation is {current_generation}"
    )]
    StaleCursor {
        conv_id: String,
        expected_generation: i64,
        current_generation: i64,
    },

    /// The on-demand daemon could not be reached or spawned.
    #[error("daemon unavailable: {0}")]
    DaemonUnavailable(String),

    /// An underlying ACP protocol error from the SDK.
    #[error("acp error: {0}")]
    Acp(#[from] agent_client_protocol::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

/// Minimal description of an advertised auth method (mirrors ACP `AuthMethod`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthMethodSummary {
    pub id: String,
    pub kind: String,
    pub display: Option<String>,
}

impl HubError {
    /// Wrap an arbitrary message as [`HubError::Other`].
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }

    /// Construct a typed not-found error.
    pub fn not_found(kind: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            kind,
            id: id.into(),
        }
    }

    /// Phase-1 read-only gate error (display equals interaction gate).
    pub fn read_only_conversation(
        conv_id: impl Into<String>,
        origin: impl Into<String>,
        interaction: impl Into<String>,
        ide: bool,
    ) -> Self {
        let conv_id = conv_id.into();
        let origin = origin.into();
        let interaction = interaction.into();
        let message = if ide {
            format!(
                "conversation is read-only; bind cannot make IDE sessions writable — create a new conversation to send (conv_id={conv_id}, origin={origin})"
            )
        } else {
            format!(
                "conversation is read-only (origin={origin}). Use conv create for a writable session, or bind only if space allows write."
            )
        };
        Self::ReadOnlyConversation {
            conv_id,
            origin,
            interaction,
            ide,
            message,
        }
    }

    pub fn conversation_busy(conv_id: impl Into<String>, busy: impl Into<String>) -> Self {
        Self::ConversationBusy {
            conv_id: conv_id.into(),
            busy: busy.into(),
        }
    }

    pub fn not_busy(conv_id: impl Into<String>) -> Self {
        Self::NotBusy {
            conv_id: conv_id.into(),
        }
    }

    /// PHASE1-CONTRACT §5 operator code, when this error maps to a stable code.
    pub fn phase1_code(&self) -> Option<&'static str> {
        match self {
            Self::ReadOnlyConversation { .. } => Some("read_only_conversation"),
            Self::ConversationClosed { .. } => Some("conversation_closed"),
            Self::ConversationBusy { .. } | Self::Conflict(_) => Some("conversation_busy"),
            Self::NotBusy { .. } => Some("not_busy"),
            Self::NotFound { kind, .. } if *kind == "conversation" => {
                Some("conversation_not_found")
            }
            Self::NotFound { kind, .. } if *kind == "agent" => Some("agent_not_found"),
            Self::DaemonUnavailable(_) => Some("daemon_unavailable"),
            Self::ResumeLoadFailed { .. } => Some("resume_load_failed"),
            Self::PermissionPolicyReject { .. } => Some("permission_policy_reject"),
            Self::UnsupportedCapability { .. } => Some("unsupported_capability"),
            _ => None,
        }
    }

    /// CLI stderr form: `error: <code>: <message>` (PHASE1-CONTRACT §5.1).
    pub fn phase1_cli_line(&self) -> String {
        match self.phase1_code() {
            Some(code) => format!("error: {code}: {self}"),
            None => format!("error: {self}"),
        }
    }

    /// Construct an explicit invalid-cursor error without leaking cursor data.
    pub fn invalid_cursor(reason: impl Into<String>) -> Self {
        Self::InvalidCursor {
            reason: reason.into(),
        }
    }

    pub fn into_acp_error(self) -> agent_client_protocol::Error {
        agent_client_protocol::Error::internal_error().data(format!("{self}"))
    }
}
