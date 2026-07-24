use super::*;

#[test]
fn rejects_unbounded_page_limits() {
    assert!(bounded_limit(Some(0), 50).is_err());
    assert!(bounded_limit(Some(MAX_PAGE_LIMIT + 1), 50).is_err());
    assert_eq!(bounded_limit(Some(25), 50).unwrap(), 25);
}

#[test]
fn register_agent_defaults_to_usable_local_trust() {
    let config = RegisterAgentRequest {
        agent_id: "fixture".to_string(),
        transport: RegisterAgentTransport::Stdio {
            command: "fixture-agent".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
        proxy_chain: None,
        permission_policy: None,
        client_capabilities: None,
    }
    .into_config()
    .expect("valid config");
    assert_eq!(config.permission_policy, PermissionPolicy::AutoAllow);
    assert!(config.client_capabilities.terminal);
    assert!(config.client_capabilities.fs.read_text_file);
    assert!(config.client_capabilities.fs.write_text_file);
}

#[test]
fn register_agent_explicit_reject_is_preserved() {
    let config = RegisterAgentRequest {
        agent_id: "fixture".to_string(),
        transport: RegisterAgentTransport::Stdio {
            command: "fixture-agent".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
        proxy_chain: None,
        permission_policy: Some("reject".into()),
        client_capabilities: Some(McpClientCapabilityConfig {
            fs: Some(McpFsConfig {
                read_text_file: Some(false),
                write_text_file: Some(false),
                allowed_roots: None,
            }),
            terminal: Some(false),
        }),
    }
    .into_config()
    .expect("valid config");
    assert_eq!(config.permission_policy, PermissionPolicy::Reject);
    assert!(!config.client_capabilities.terminal);
    assert!(!config.client_capabilities.fs.read_text_file);
    assert!(!config.client_capabilities.fs.write_text_file);
}

#[test]
fn register_agent_transport_rejects_mixed_fields() {
    let request = serde_json::from_value::<RegisterAgentRequest>(json!({
        "agent_id": "fixture",
        "transport": {
            "type": "stdio",
            "command": "fixture-agent",
            "url": "https://contradictory.invalid"
        }

    }));
    assert!(request.is_err());
}

#[test]
fn every_mcp_request_rejects_unknown_fields_and_schemas_are_closed() {
    macro_rules! rejects_unknown {
        ($request:ty, $value:expr) => {
            assert!(
                serde_json::from_value::<$request>($value).is_err(),
                "{} accepted an unknown field",
                stringify!($request)
            );
        };
    }

    rejects_unknown!(EmptyRequest, json!({"unexpected": true}));
    rejects_unknown!(
        RegisterAgentRequest,
        json!({
            "agent_id": "fixture",
            "transport": {"type": "stdio", "command": "fixture"},
            "unexpected": true
        })
    );
    rejects_unknown!(
        RemoveAgentRequest,
        json!({"agent_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        InspectAgentRequest,
        json!({"agent_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        AuthenticateAgentRequest,
        json!({"agent_id": "fixture", "method_id": "browser", "unexpected": true})
    );
    rejects_unknown!(
        LogoutAgentRequest,
        json!({"agent_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        RegisterProxyRequest,
        json!({"proxy_id": "fixture", "command": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        RemoveProxyRequest,
        json!({"proxy_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(ListConversationsRequest, json!({"unexpected": true}));
    rejects_unknown!(
        CreateConversationRequest,
        json!({"agent_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        DeleteConversationRequest,
        json!({"conv_id": "fixture", "local_ony": true})
    );
    rejects_unknown!(
        CloseConversationRequest,
        json!({"conv_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        SearchRequest,
        json!({"query": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        SendMessageRequest,
        json!({"conv_id": "fixture", "text": "hello", "unexpected": true})
    );
    rejects_unknown!(
        McpConfigParam,
        json!({"config_id": "fixture", "value": "on", "unexpected": true})
    );
    rejects_unknown!(
        SetParamRequest,
        json!({
            "conv_id": "fixture",
            "config_id": "setting",
            "value": "on",
            "unexpected": true
        })
    );
    rejects_unknown!(
        SetModeRequest,
        json!({"conv_id": "fixture", "mode_id": "fast", "unexpected": true})
    );
    rejects_unknown!(
        GetConfigRequest,
        json!({"conv_id": "fixture", "unexpected": true})
    );
    rejects_unknown!(
        GetMessagesRequest,
        json!({"conv_id": "fixture", "unexpected": true})
    );

    let delete_schema = serde_json::to_value(schemars::schema_for!(DeleteConversationRequest))
        .expect("serializes delete request schema");
    assert_eq!(
        delete_schema.get("additionalProperties"),
        Some(&json!(false)),
        "delete schema must visibly reject unknown fields: {delete_schema}"
    );
}

#[test]
fn maps_typed_hub_errors_to_structured_mcp_data() {
    use acp_hub::{HubError, error::AuthMethodSummary};

    let cases = [
        (
            HubError::not_found("conversation", "missing-conversation"),
            json!({"kind": "conversation", "id": "missing-conversation"}),
        ),
        (
            HubError::Conflict("busy-conversation".to_string()),
            json!({
                "reason": "conversation_busy",
                "convId": "busy-conversation"
            }),
        ),
        (
            HubError::UnsupportedCapability {
                endpoint: "fixture-agent".to_string(),
                operation: "session/list",
                required_capability: "session_capabilities.list",
            },
            json!({
                "reason": "unsupported_capability",
                "endpoint": "fixture-agent",
                "operation": "session/list",
                "requiredCapability": "session_capabilities.list"
            }),
        ),
        (
            HubError::AuthRequired {
                endpoint: "fixture-agent".to_string(),
                auth_methods: vec![AuthMethodSummary {
                    id: "browser".to_string(),
                    kind: "agent".to_string(),
                    display: Some("Open browser".to_string()),
                }],
            },
            json!({
                "reason": "auth_required",
                "endpoint": "fixture-agent",
                "authMethods": [{
                    "id": "browser",
                    "kind": "agent",
                    "display": "Open browser"
                }]
            }),
        ),
    ];

    for (error, expected_data) in cases {
        assert_eq!(hub_error(error).data, Some(expected_data));
    }
}

#[test]
fn maps_cursor_errors_to_caller_recoverable_invalid_params() {
    use acp_hub::HubError;

    let cases = [
        (
            HubError::InvalidCursor {
                reason: "cursor signature mismatch".to_string(),
            },
            json!({
                "reason": "invalid_cursor",
                "detail": "cursor signature mismatch",
            }),
        ),
        (
            HubError::StaleCursor {
                conv_id: "fixture-conversation".to_string(),
                expected_generation: 3,
                current_generation: 4,
            },
            json!({
                "reason": "stale_cursor",
                "convId": "fixture-conversation",
                "expectedGeneration": 3,
                "currentGeneration": 4,
            }),
        ),
    ];

    for (error, expected_data) in cases {
        let mapped = hub_error(error);
        assert_eq!(mapped.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.data, Some(expected_data));
    }
}

#[test]
fn prompt_messages_page_uses_returned_prompt_and_run_identity() {
    let params = prompt_messages_page_params("fixture-conversation", 73, "fixture-run");
    let serialized = serde_json::to_value(params).expect("serializes message page params");
    assert_eq!(
        serialized.get("convId"),
        Some(&json!("fixture-conversation"))
    );
    assert_eq!(
        serialized.get("afterSeq"),
        Some(&json!(73)),
        "MCP must page after the promptSeq returned by hub/conv/send"
    );
    assert_eq!(
        serialized.get("runId"),
        Some(&json!("fixture-run")),
        "MCP must scope pages to the runId returned by hub/conv/send"
    );
    assert_eq!(serialized.get("cursor"), Some(&Value::Null));
    assert_eq!(serialized.get("offset"), Some(&json!(0)));
}

#[test]
fn get_messages_accepts_exact_run_continuation_identity() {
    let request = serde_json::from_value::<GetMessagesRequest>(json!({
        "conv_id": "fixture-conversation",
        "run_id": "fixture-run",
        "after_seq": 73,
        "cursor": "opaque-fixture-cursor",
        "offset": 0,
        "limit": 200
    }))
    .expect("continuation request parses");
    assert_eq!(request.run_id.as_deref(), Some("fixture-run"));
    assert_eq!(request.after_seq, Some(73));
    assert_eq!(request.cursor.as_deref(), Some("opaque-fixture-cursor"));
    assert_eq!(request.offset, Some(0));
    assert_eq!(request.limit, Some(200));
}

#[test]
fn destructive_and_read_only_tools_are_annotated() {
    let delete = AcpHubMcp::delete_conversation_tool_attr();
    let delete_annotations = delete.annotations.expect("delete annotations");
    assert_eq!(delete_annotations.read_only_hint, Some(false));
    assert_eq!(delete_annotations.destructive_hint, Some(true));

    let search = AcpHubMcp::search_tool_attr();
    let search_annotations = search.annotations.expect("search annotations");
    assert_eq!(search_annotations.read_only_hint, Some(true));
    assert_eq!(search_annotations.open_world_hint, Some(false));

    let sessions = AcpHubMcp::list_agent_sessions_tool_attr();
    let session_annotations = sessions.annotations.expect("session annotations");
    assert_eq!(session_annotations.read_only_hint, Some(false));
    assert_eq!(session_annotations.destructive_hint, Some(false));
    assert_eq!(session_annotations.open_world_hint, Some(true));

    for tool in [
        AcpHubMcp::send_message_tool_attr(),
        AcpHubMcp::cancel_conversation_tool_attr(),
    ] {
        let annotations = tool.annotations.expect("mutating tool annotations");
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
    }
}
