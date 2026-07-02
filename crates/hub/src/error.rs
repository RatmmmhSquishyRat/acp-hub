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
    #[error("conversation {0} is busy with an in-flight turn")]
    Conflict(String),

    /// A conversation, agent, or proxy id was not found in the registry/projection.
    #[error("{kind} not found: {id}")]
    NotFound { kind: &'static str, id: String },

    /// Registry/agents.json validation failure.
    #[error("invalid registry: {0}")]
    InvalidRegistry(String),

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
}
impl HubError {
    pub fn into_acp_error(self) -> agent_client_protocol::Error {
        agent_client_protocol::Error::internal_error().data(format!("{self}"))
    }
}
