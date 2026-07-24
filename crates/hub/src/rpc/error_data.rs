use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SafeResumeSourceData {
    NotFound {
        kind: String,
        id: String,
    },
    Conflict {
        #[serde(rename = "convId")]
        conv_id: String,
    },
    UnsupportedCapability {
        endpoint: String,
        operation: String,
        #[serde(rename = "requiredCapability")]
        required_capability: String,
    },
    AuthRequired {
        endpoint: String,
        #[serde(rename = "authMethods")]
        auth_methods: Vec<AuthMethodSummary>,
    },
    UnsupportedProxyTransport {},
    UnsupportedProtocolVersion {},
    InvalidRegistry {},
    DaemonUnavailable {},
    /// Agent-side ACP protocol error during resume/load (payload redacted).
    AgentAcp {},
    /// Local I/O while resume/load was in progress (path text redacted).
    Io {},
    /// Timeout / deadline class (no free-form detail).
    Timeout {},
    /// Catch-all redacted endpoint failure (not a daemon outage).
    Internal {},
}

impl SafeResumeSourceData {
    fn from_hub_error(error: &HubError) -> Self {
        match error {
            HubError::NotFound { kind, id } => Self::NotFound {
                kind: (*kind).to_string(),
                id: id.clone(),
            },
            HubError::Conflict(conv_id) => Self::Conflict {
                conv_id: conv_id.clone(),
            },
            HubError::ConversationBusy { conv_id, .. } => Self::Conflict {
                conv_id: conv_id.clone(),
            },
            HubError::UnsupportedCapability {
                endpoint,
                operation,
                required_capability,
            } => Self::UnsupportedCapability {
                endpoint: endpoint.clone(),
                operation: (*operation).to_string(),
                required_capability: (*required_capability).to_string(),
            },
            HubError::AuthRequired {
                endpoint,
                auth_methods,
            } => Self::AuthRequired {
                endpoint: endpoint.clone(),
                auth_methods: auth_methods.clone(),
            },
            HubError::UnsupportedProxyTransport => Self::UnsupportedProxyTransport {},
            HubError::UnsupportedProtocolVersion => Self::UnsupportedProtocolVersion {},
            HubError::InvalidRegistry(_) => Self::InvalidRegistry {},
            HubError::DaemonUnavailable(_) => Self::DaemonUnavailable {},
            HubError::Acp(_) => Self::AgentAcp {},
            HubError::Io(_) => Self::Io {},
            HubError::Other(message) if looks_like_timeout(message) => Self::Timeout {},
            HubError::ResumeLoadFailed { .. }
            | HubError::ResourceLimit { .. }
            | HubError::InvalidCursor { .. }
            | HubError::StaleCursor { .. }
            | HubError::ReadOnlyConversation { .. }
            | HubError::ConversationClosed { .. }
            | HubError::NotBusy { .. }
            | HubError::PermissionPolicyReject { .. }
            | HubError::Sqlite(_)
            | HubError::Json(_)
            | HubError::Other(_) => Self::Internal {},
        }
    }

    fn into_hub_error(self) -> Option<HubError> {
        match self {
            Self::NotFound { kind, id } => Some(HubError::NotFound {
                kind: known_not_found_kind(&kind)?,
                id,
            }),
            Self::Conflict { conv_id } => Some(HubError::Conflict(conv_id)),
            Self::UnsupportedCapability {
                endpoint,
                operation,
                required_capability,
            } => {
                let (operation, required_capability) =
                    known_capability_pair(&operation, &required_capability)?;
                Some(HubError::UnsupportedCapability {
                    endpoint,
                    operation,
                    required_capability,
                })
            }
            Self::AuthRequired {
                endpoint,
                auth_methods,
            } => Some(HubError::AuthRequired {
                endpoint,
                auth_methods,
            }),
            Self::UnsupportedProxyTransport {} => Some(HubError::UnsupportedProxyTransport),
            Self::UnsupportedProtocolVersion {} => Some(HubError::UnsupportedProtocolVersion),
            Self::InvalidRegistry {} => Some(HubError::InvalidRegistry(
                "registry validation failed".to_string(),
            )),
            // Distinct classes so operators do not misread endpoint failures as
            // a dead daemon (doc/ssot/agent-managed/pillars/Product-UX.md).
            Self::DaemonUnavailable {} => Some(HubError::DaemonUnavailable(
                "daemon unavailable while resume/load was in progress".to_string(),
            )),
            Self::AgentAcp {} => Some(HubError::other(
                "resume/load failed: agent ACP error (details redacted at the RPC boundary)",
            )),
            Self::Io {} => Some(HubError::other(
                "resume/load failed: I/O error (details redacted at the RPC boundary)",
            )),
            Self::Timeout {} => Some(HubError::other("resume/load failed: timeout")),
            Self::Internal {} => Some(HubError::other(
                "resume/load failed at the endpoint (details redacted at the RPC boundary; check daemon/agent logs)",
            )),
        }
    }
}

fn looks_like_timeout(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("timeout") || lower.contains("timed out") || lower.contains("deadline")
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TypedHubErrorData {
    NotFound {
        kind: String,
        id: String,
    },
    Conflict {
        #[serde(rename = "convId")]
        conv_id: String,
    },
    ConversationBusy {
        #[serde(rename = "convId")]
        conv_id: String,
        busy: String,
        reason: String,
    },
    NotBusy {
        #[serde(rename = "convId")]
        conv_id: String,
        reason: String,
    },
    UnsupportedCapability {
        endpoint: String,
        operation: String,
        #[serde(rename = "requiredCapability")]
        required_capability: String,
    },
    ResourceLimit {
        resource: String,
        limit: usize,
    },
    AuthRequired {
        endpoint: String,
        #[serde(rename = "authMethods")]
        auth_methods: Vec<AuthMethodSummary>,
    },
    InvalidRegistry {},
    UnsupportedProtocolVersion {},
    UnsupportedProxyTransport {},
    ResumeLoadFailed {
        #[serde(rename = "attemptedMethod")]
        attempted_method: String,
        endpoint: String,
        #[serde(rename = "convId")]
        conv_id: String,
        #[serde(rename = "agentSessionId")]
        agent_session_id: String,
        source: SafeResumeSourceData,
    },
    InvalidCursor {
        reason: String,
    },
    StaleCursor {
        #[serde(rename = "convId")]
        conv_id: String,
        #[serde(rename = "expectedGeneration")]
        expected_generation: i64,
        #[serde(rename = "currentGeneration")]
        current_generation: i64,
    },
    ReadOnlyConversation {
        #[serde(rename = "convId")]
        conv_id: String,
        origin: String,
        interaction: String,
        /// SC-06/07: must survive daemon wire so CLI/MCP keep IDE wording.
        ide: bool,
        reason: String,
    },
    ConversationClosed {
        #[serde(rename = "convId")]
        conv_id: String,
        reason: String,
    },
    PermissionPolicyReject {
        reason: String,
    },
}

impl TypedHubErrorData {
    fn from_hub_error(error: &HubError) -> Option<Self> {
        match error {
            HubError::NotFound { kind, id } => Some(Self::NotFound {
                kind: (*kind).to_string(),
                id: id.clone(),
            }),
            HubError::Conflict(conv_id) => Some(Self::Conflict {
                conv_id: conv_id.clone(),
            }),
            HubError::ConversationBusy { conv_id, busy } => Some(Self::ConversationBusy {
                conv_id: conv_id.clone(),
                busy: busy.clone(),
                reason: "conversation_busy".into(),
            }),
            HubError::NotBusy { conv_id } => Some(Self::NotBusy {
                conv_id: conv_id.clone(),
                reason: "not_busy".into(),
            }),
            HubError::UnsupportedCapability {
                endpoint,
                operation,
                required_capability,
            } => Some(Self::UnsupportedCapability {
                endpoint: endpoint.clone(),
                operation: (*operation).to_string(),
                required_capability: (*required_capability).to_string(),
            }),
            HubError::ResourceLimit { resource, limit } => Some(Self::ResourceLimit {
                resource: (*resource).to_string(),
                limit: *limit,
            }),
            HubError::AuthRequired {
                endpoint,
                auth_methods,
            } => Some(Self::AuthRequired {
                endpoint: endpoint.clone(),
                auth_methods: auth_methods.clone(),
            }),
            HubError::InvalidRegistry(_) => Some(Self::InvalidRegistry {}),
            HubError::UnsupportedProtocolVersion => Some(Self::UnsupportedProtocolVersion {}),
            HubError::UnsupportedProxyTransport => Some(Self::UnsupportedProxyTransport {}),
            HubError::ResumeLoadFailed {
                attempted_method,
                endpoint,
                conv_id,
                agent_session_id,
                source,
            } => Some(Self::ResumeLoadFailed {
                attempted_method: (*attempted_method).to_string(),
                endpoint: endpoint.clone(),
                conv_id: conv_id.clone(),
                agent_session_id: agent_session_id.clone(),
                source: SafeResumeSourceData::from_hub_error(source),
            }),
            HubError::InvalidCursor { reason } => Some(Self::InvalidCursor {
                reason: reason.clone(),
            }),
            HubError::StaleCursor {
                conv_id,
                expected_generation,
                current_generation,
            } => Some(Self::StaleCursor {
                conv_id: conv_id.clone(),
                expected_generation: *expected_generation,
                current_generation: *current_generation,
            }),
            HubError::ReadOnlyConversation {
                conv_id,
                origin,
                interaction,
                ide,
                ..
            } => Some(Self::ReadOnlyConversation {
                conv_id: conv_id.clone(),
                origin: origin.clone(),
                interaction: interaction.clone(),
                ide: *ide,
                reason: "read_only_conversation".into(),
            }),
            HubError::ConversationClosed { conv_id } => Some(Self::ConversationClosed {
                conv_id: conv_id.clone(),
                reason: "conversation_closed".into(),
            }),
            HubError::PermissionPolicyReject { .. } => Some(Self::PermissionPolicyReject {
                reason: "permission_policy_reject".into(),
            }),
            _ => None,
        }
    }

    pub(super) fn into_hub_error(self, code: i64) -> Option<HubError> {
        match self {
            Self::NotFound { kind, id } if code == NOT_FOUND_ERROR => Some(HubError::NotFound {
                kind: known_not_found_kind(&kind)?,
                id,
            }),
            Self::Conflict { conv_id } if code == CONFLICT_ERROR => {
                Some(HubError::Conflict(conv_id))
            }
            Self::ConversationBusy { conv_id, busy, .. } if code == CONFLICT_ERROR => {
                Some(HubError::ConversationBusy { conv_id, busy })
            }
            Self::NotBusy { conv_id, .. } if code == INVALID_PARAMS => {
                Some(HubError::NotBusy { conv_id })
            }
            Self::UnsupportedCapability {
                endpoint,
                operation,
                required_capability,
            } if code == UNSUPPORTED_CAPABILITY_ERROR => {
                let (operation, required_capability) =
                    known_capability_pair(&operation, &required_capability)?;
                Some(HubError::UnsupportedCapability {
                    endpoint,
                    operation,
                    required_capability,
                })
            }
            Self::ResourceLimit { resource, limit } if code == RESOURCE_LIMIT_ERROR => {
                Some(HubError::ResourceLimit {
                    resource: known_resource_limit(&resource)?,
                    limit,
                })
            }
            Self::AuthRequired {
                endpoint,
                auth_methods,
            } if code == AUTH_REQUIRED_ERROR => Some(HubError::AuthRequired {
                endpoint,
                auth_methods,
            }),
            Self::InvalidRegistry {} if code == INVALID_REGISTRY_ERROR => Some(
                HubError::InvalidRegistry("registry validation failed".to_string()),
            ),
            Self::UnsupportedProtocolVersion {} if code == UNSUPPORTED_PROTOCOL_VERSION_ERROR => {
                Some(HubError::UnsupportedProtocolVersion)
            }
            Self::UnsupportedProxyTransport {} if code == UNSUPPORTED_PROXY_TRANSPORT_ERROR => {
                Some(HubError::UnsupportedProxyTransport)
            }
            Self::ResumeLoadFailed {
                attempted_method,
                endpoint,
                conv_id,
                agent_session_id,
                source,
            } if code == RESUME_LOAD_FAILED_ERROR
                && valid_registry_id(&endpoint)
                && valid_registry_id(&conv_id)
                && valid_opaque_id(&agent_session_id) =>
            {
                Some(HubError::ResumeLoadFailed {
                    attempted_method: known_resume_method(&attempted_method)?,
                    endpoint,
                    conv_id,
                    agent_session_id,
                    source: Box::new(source.into_hub_error()?),
                })
            }
            Self::InvalidCursor { reason }
                if code == INVALID_CURSOR_ERROR && known_cursor_reason(&reason) =>
            {
                Some(HubError::InvalidCursor { reason })
            }
            Self::StaleCursor {
                conv_id,
                expected_generation,
                current_generation,
            } if code == STALE_CURSOR_ERROR
                && valid_registry_id(&conv_id)
                && expected_generation >= 0
                && current_generation >= 0 =>
            {
                Some(HubError::StaleCursor {
                    conv_id,
                    expected_generation,
                    current_generation,
                })
            }
            Self::ReadOnlyConversation {
                conv_id,
                origin,
                interaction,
                ide,
                ..
            } if code == INVALID_PARAMS
                && valid_registry_id(&conv_id)
                && (origin == "hub_created" || origin == "bound" || origin == "imported_list")
                && (interaction == "writable" || interaction == "read_only") =>
            {
                Some(HubError::read_only_conversation(
                    conv_id,
                    origin,
                    interaction,
                    ide,
                ))
            }
            Self::ConversationClosed { conv_id, .. }
                if code == INVALID_PARAMS && valid_registry_id(&conv_id) =>
            {
                Some(HubError::ConversationClosed { conv_id })
            }
            Self::PermissionPolicyReject { .. } if code == INVALID_PARAMS => {
                Some(HubError::PermissionPolicyReject {
                    message:
                        "permission_policy=reject; re-add agent with defaults or edit agents.json"
                            .into(),
                })
            }
            _ => None,
        }
    }
}

pub(crate) fn typed_hub_error_data(error: &HubError) -> Option<Value> {
    TypedHubErrorData::from_hub_error(error).and_then(|data| serde_json::to_value(data).ok())
}

fn known_resume_method(method: &str) -> Option<&'static str> {
    match method {
        "session/load" => Some("session/load"),
        "session/resume" => Some("session/resume"),
        _ => None,
    }
}

fn known_cursor_reason(reason: &str) -> bool {
    matches!(
        reason,
        "malformed cursor"
            | "malformed cursor payload"
            | "cursor authentication failed"
            | "unsupported cursor version or filter"
            | "cursor does not belong to this message query"
            | "cursor sort key is outside the message query"
            | "cursor contains an invalid projection position"
            | "offset cannot be combined with a continuation cursor"
    )
}

fn valid_registry_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn valid_opaque_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 4096 && !id.chars().any(char::is_control)
}

fn known_not_found_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "agent" => Some("agent"),
        "proxy" => Some("proxy"),
        "auth method" => Some("auth method"),
        "conversation" => Some("conversation"),
        _ => None,
    }
}

fn known_capability_pair(
    operation: &str,
    required_capability: &str,
) -> Option<(&'static str, &'static str)> {
    match (operation, required_capability) {
        ("close", "session_capabilities.close") => Some(("close", "session_capabilities.close")),
        ("delete", "session_capabilities.delete") => {
            Some(("delete", "session_capabilities.delete"))
        }
        ("session/load", "load_session") => Some(("session/load", "load_session")),
        ("session/resume", "session_capabilities.resume") => {
            Some(("session/resume", "session_capabilities.resume"))
        }
        ("session/list", "session_capabilities.list") => {
            Some(("session/list", "session_capabilities.list"))
        }
        ("session/prompt", "prompt_capabilities.image") => {
            Some(("session/prompt", "prompt_capabilities.image"))
        }
        ("session/prompt", "prompt_capabilities.audio") => {
            Some(("session/prompt", "prompt_capabilities.audio"))
        }
        ("session/prompt", "prompt_capabilities.embedded_context") => {
            Some(("session/prompt", "prompt_capabilities.embedded_context"))
        }
        ("session/prompt", "prompt_capabilities.unknown_content") => {
            Some(("session/prompt", "prompt_capabilities.unknown_content"))
        }
        _ => None,
    }
}

fn known_resource_limit(resource: &str) -> Option<&'static str> {
    match resource {
        "daemon_retained_rpc_bytes" => Some("daemon_retained_rpc_bytes"),
        "session_list_pages" => Some("session_list_pages"),
        "session_list_sessions" => Some("session_list_sessions"),
        "session_list_cursor_bytes" => Some("session_list_cursor_bytes"),
        "session_list_serialized_bytes" => Some("session_list_serialized_bytes"),
        "materialized_message_rows" => Some("materialized_message_rows"),
        "materialized_message_bytes" => Some("materialized_message_bytes"),
        _ => None,
    }
}
