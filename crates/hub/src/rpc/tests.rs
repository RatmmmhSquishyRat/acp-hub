use super::*;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn demuxes_out_of_order_responses_by_id() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (server_reader, mut server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let first: RpcRequest =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let second: RpcRequest =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();

        server_writer
            .write_all(
                &encode_line(
                    &RpcResponse::success(second.id.clone().unwrap(), json!({"value": 2})).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        server_writer
            .write_all(
                &encode_line(
                    &RpcResponse::success(first.id.clone().unwrap(), json!({"value": 1})).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    });

    let (first, second) = tokio::join!(
        client.request_value("first", json!({"input": 1})),
        client.request_value("second", json!({"input": 2}))
    );
    server.await.unwrap();

    assert_eq!(first.unwrap(), json!({"value": 1}));
    assert_eq!(second.unwrap(), json!({"value": 2}));
}

#[tokio::test]
async fn delivers_idless_notifications_to_subscribers() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (_server_reader, mut server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);
    let mut notifications = client.subscribe_notifications();

    server_writer
        .write_all(
            &encode_line(&RpcRequest::notification(
                "hub/conv/update",
                json!({"seq": 7}),
            ))
            .unwrap(),
        )
        .await
        .unwrap();

    let notification = notifications.recv().await.unwrap();
    assert_eq!(notification.method, "hub/conv/update");
    assert_eq!(notification.id, None);
    assert_eq!(notification.params, json!({"seq": 7}));
}

#[tokio::test]
async fn maps_typed_rpc_error_responses_to_exact_hub_errors() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (server_reader, mut server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let request: RpcRequest =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        server_writer
            .write_all(
                &encode_line(&RpcError::new(
                    request.id.unwrap(),
                    -32_004,
                    "resource not found",
                    Some(json!({
                        "type": "not_found",
                        "kind": "conversation",
                        "id": "missing-conversation"
                    })),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
    });

    let error = client
        .request_value("missing", Value::Null)
        .await
        .expect_err("rpc error response must fail the request");
    server.await.unwrap();

    assert!(matches!(
        &error,
        HubError::NotFound {
            kind: "conversation",
            id
        } if id == "missing-conversation"
    ));

    let conflict = rpc_error_to_hub_error(RpcErrorObject {
        code: -32_009,
        message: "resource conflict".to_string(),
        data: Some(json!({"type": "conflict", "convId": "conv-busy"})),
    });
    assert!(matches!(&conflict, HubError::Conflict(id) if id == "conv-busy"));

    let unsupported = rpc_error_to_hub_error(RpcErrorObject {
        code: -32_010,
        message: "unsupported capability".to_string(),
        data: Some(json!({
            "type": "unsupported_capability",
            "endpoint": "fixture-agent",
            "operation": "session/list",
            "requiredCapability": "session_capabilities.list"
        })),
    });
    assert!(matches!(
        &unsupported,
        HubError::UnsupportedCapability {
            endpoint,
            operation: "session/list",
            required_capability: "session_capabilities.list"
        } if endpoint == "fixture-agent"
    ));

    let prompt_capability = rpc_error_to_hub_error(RpcErrorObject {
        code: UNSUPPORTED_CAPABILITY_ERROR,
        message: "unsupported capability".to_string(),
        data: Some(json!({
            "type": "unsupported_capability",
            "endpoint": "fixture-agent",
            "operation": "session/prompt",
            "requiredCapability": "prompt_capabilities.image"
        })),
    });
    assert!(matches!(
        &prompt_capability,
        HubError::UnsupportedCapability {
            endpoint,
            operation: "session/prompt",
            required_capability: "prompt_capabilities.image"
        } if endpoint == "fixture-agent"
    ));

    let resource_limit = rpc_error_to_hub_error(RpcErrorObject {
        code: RESOURCE_LIMIT_ERROR,
        message: "resource limit exceeded".to_string(),
        data: Some(json!({
            "type": "resource_limit",
            "resource": "session_list_pages",
            "limit": 256
        })),
    });
    assert!(matches!(
        resource_limit,
        HubError::ResourceLimit {
            resource: "session_list_pages",
            limit: 256,
        }
    ));

    let auth = rpc_error_to_hub_error(RpcErrorObject {
        code: -32_001,
        message: "authentication required".to_string(),
        data: Some(json!({
            "type": "auth_required",
            "endpoint": "fixture-agent",
            "authMethods": [{
                "id": "browser",
                "kind": "agent",
                "display": "Open browser"
            }]
        })),
    });
    match auth {
        HubError::AuthRequired {
            endpoint,
            auth_methods,
        } => {
            assert_eq!(endpoint, "fixture-agent");
            assert_eq!(auth_methods.len(), 1);
            assert_eq!(auth_methods[0].id, "browser");
            assert_eq!(auth_methods[0].kind, "agent");
            assert_eq!(auth_methods[0].display.as_deref(), Some("Open browser"));
        }
        other => panic!("expected typed auth error, got {other}"),
    }
    let invalid_registry = rpc_error_to_hub_error(RpcErrorObject {
        code: INVALID_REGISTRY_ERROR,
        message: "rpc-secret-sentinel".to_string(),
        data: Some(json!({"type": "invalid_registry"})),
    });
    assert!(matches!(
        invalid_registry,
        HubError::InvalidRegistry(reason) if reason == "registry validation failed"
    ));

    let protocol = rpc_error_to_hub_error(RpcErrorObject {
        code: UNSUPPORTED_PROTOCOL_VERSION_ERROR,
        message: "rpc-secret-sentinel".to_string(),
        data: Some(json!({"type": "unsupported_protocol_version"})),
    });
    assert!(matches!(protocol, HubError::UnsupportedProtocolVersion));

    let proxy_transport = rpc_error_to_hub_error(RpcErrorObject {
        code: UNSUPPORTED_PROXY_TRANSPORT_ERROR,
        message: "rpc-secret-sentinel".to_string(),
        data: Some(json!({"type": "unsupported_proxy_transport"})),
    });
    assert!(matches!(
        proxy_transport,
        HubError::UnsupportedProxyTransport
    ));

    let replay = rpc_error_to_hub_error(RpcErrorObject {
        code: RESUME_LOAD_FAILED_ERROR,
        message: "rpc-secret-sentinel".to_string(),
        data: Some(json!({
            "type": "resume_load_failed",
            "attemptedMethod": "session/load",
            "endpoint": "fixture-agent",
            "convId": "conv-1",
            "agentSessionId": "opaque/session/1",
            "source": {"type": "internal"}
        })),
    });
    match replay {
        HubError::ResumeLoadFailed {
            attempted_method,
            endpoint,
            conv_id,
            agent_session_id,
            source,
        } => {
            assert_eq!(attempted_method, "session/load");
            assert_eq!(endpoint, "fixture-agent");
            assert_eq!(conv_id, "conv-1");
            assert_eq!(agent_session_id, "opaque/session/1");
            assert!(matches!(
                *source,
                HubError::Other(ref message)
                    if message.contains("resume/load failed at the endpoint")
            ));
            let text = source.to_string();
            assert!(
                !text.contains("daemon unavailable: resume/load operation failed"),
                "internal source must not reuse the old mislabeled daemon string: {text}"
            );
        }
        other => panic!("expected typed resume/load error, got {other}"),
    }
}

#[test]
fn resume_load_source_classes_rehydrate_without_daemon_mislabel() {
    // Wire-level reconstruction is the shipped CLI path for ResumeLoadFailed.
    let cases = [
        ("internal", "resume/load failed at the endpoint", false),
        ("agent_acp", "agent ACP", false),
        ("io", "I/O error", false),
        ("timeout", "timeout", false),
        (
            "daemon_unavailable",
            "daemon unavailable while resume/load",
            true,
        ),
    ];
    for (source_type, needle, expect_daemon) in cases {
        let replay = rpc_error_to_hub_error(RpcErrorObject {
            code: RESUME_LOAD_FAILED_ERROR,
            message: "rpc-secret-sentinel".to_string(),
            data: Some(json!({
                "type": "resume_load_failed",
                "attemptedMethod": "session/load",
                "endpoint": "fixture-agent",
                "convId": "conv-1",
                "agentSessionId": "opaque/session/1",
                "source": {"type": source_type}
            })),
        });
        match replay {
            HubError::ResumeLoadFailed { source, .. } => {
                let text = source.to_string();
                assert!(
                    text.contains(needle),
                    "source type {source_type} missing {needle:?}: {text}"
                );
                if expect_daemon {
                    assert!(
                        matches!(*source, HubError::DaemonUnavailable(_)),
                        "source type {source_type} should be DaemonUnavailable, got {source}"
                    );
                } else {
                    assert!(
                        !text.contains("daemon unavailable: resume/load operation failed"),
                        "source type {source_type} reused old mislabel: {text}"
                    );
                    assert!(
                        !matches!(*source, HubError::DaemonUnavailable(_)),
                        "source type {source_type} must not be DaemonUnavailable: {source}"
                    );
                }
            }
            other => panic!("expected ResumeLoadFailed for {source_type}, got {other}"),
        }
    }
}

#[test]
fn resume_load_encode_maps_acp_source_to_agent_acp_wire_tag() {
    let error = HubError::ResumeLoadFailed {
        attempted_method: "session/load",
        endpoint: "fixture-agent".into(),
        conv_id: "conv-1".into(),
        agent_session_id: "opaque/session/1".into(),
        source: Box::new(HubError::Acp(agent_client_protocol::Error::internal_error())),
    };
    let data = typed_hub_error_data(&error).expect("typed data for ResumeLoadFailed");
    assert_eq!(data["type"], "resume_load_failed");
    assert_eq!(data["source"]["type"], "agent_acp");
    let roundtrip = rpc_error_to_hub_error(RpcErrorObject {
        code: RESUME_LOAD_FAILED_ERROR,
        message: "resume or load failed".into(),
        data: Some(data),
    });
    match roundtrip {
        HubError::ResumeLoadFailed { source, .. } => {
            let text = source.to_string();
            assert!(
                text.contains("agent ACP"),
                "encoded Acp source must rehydrate as agent ACP class, got {text}"
            );
        }
        other => panic!("expected ResumeLoadFailed roundtrip, got {other}"),
    }
}

#[test]
fn malformed_or_unknown_rpc_error_data_is_sanitized() {
    for data in [
        json!({"type": "future_error", "secret": "rpc-secret-sentinel"}),
        json!({"type": "not_found", "kind": "conversation"}),
    ] {
        let error = rpc_error_to_hub_error(RpcErrorObject {
            code: INTERNAL_ERROR,
            message: "rpc-secret-sentinel".to_string(),
            data: Some(data),
        });
        assert!(matches!(
            &error,
            HubError::Other(_) | HubError::DaemonUnavailable(_)
        ));
        assert!(!error.to_string().contains("rpc-secret-sentinel"));
    }
}

#[test]
fn typed_rpc_error_tags_require_their_exact_error_codes() {
    let cases = [
        (
            NOT_FOUND_ERROR,
            json!({
                "type": "not_found",
                "kind": "conversation",
                "id": "rpc-secret-sentinel"
            }),
        ),
        (
            CONFLICT_ERROR,
            json!({
                "type": "conflict",
                "convId": "rpc-secret-sentinel"
            }),
        ),
        (
            UNSUPPORTED_CAPABILITY_ERROR,
            json!({
                "type": "unsupported_capability",
                "endpoint": "rpc-secret-sentinel",
                "operation": "session/list",
                "requiredCapability": "session_capabilities.list"
            }),
        ),
        (
            RESOURCE_LIMIT_ERROR,
            json!({
                "type": "resource_limit",
                "resource": "session_list_pages",
                "limit": 256
            }),
        ),
        (
            AUTH_REQUIRED_ERROR,
            json!({
                "type": "auth_required",
                "endpoint": "rpc-secret-sentinel",
                "authMethods": []
            }),
        ),
        (INVALID_REGISTRY_ERROR, json!({"type": "invalid_registry"})),
        (
            UNSUPPORTED_PROTOCOL_VERSION_ERROR,
            json!({"type": "unsupported_protocol_version"}),
        ),
        (
            UNSUPPORTED_PROXY_TRANSPORT_ERROR,
            json!({"type": "unsupported_proxy_transport"}),
        ),
        (
            RESUME_LOAD_FAILED_ERROR,
            json!({
                "type": "resume_load_failed",
                "attemptedMethod": "session/resume",
                "endpoint": "fixture-agent",
                "convId": "rpc-secret-sentinel",
                "agentSessionId": "opaque/session/1",
                "source": {"type": "internal"}
            }),
        ),
    ];

    for (expected_code, data) in &cases {
        for (actual_code, _) in &cases {
            if actual_code == expected_code {
                continue;
            }
            let error = rpc_error_to_hub_error(RpcErrorObject {
                code: *actual_code,
                message: "rpc-secret-sentinel".to_string(),
                data: Some(data.clone()),
            });
            assert!(
                matches!(
                    &error,
                    HubError::DaemonUnavailable(message)
                        if message == "daemon returned malformed error data"
                ),
                "code {actual_code} unexpectedly accepted tag for {expected_code}: {error}"
            );
            assert!(!error.to_string().contains("rpc-secret-sentinel"));
        }
    }
    for (code, data) in &cases {
        let mut with_unknown_field = data.clone();
        with_unknown_field
            .as_object_mut()
            .unwrap()
            .insert("rpc-secret-sentinel".to_string(), Value::Bool(true));
        let error = rpc_error_to_hub_error(RpcErrorObject {
            code: *code,
            message: "rpc-secret-sentinel".to_string(),
            data: Some(with_unknown_field),
        });
        assert!(matches!(
            error,
            HubError::DaemonUnavailable(message)
                if message == "daemon returned malformed error data"
        ));
    }

    for (code, data) in [
        (
            NOT_FOUND_ERROR,
            json!({
                "type": "not_found",
                "kind": "future-kind",
                "id": "rpc-secret-sentinel"
            }),
        ),
        (
            UNSUPPORTED_CAPABILITY_ERROR,
            json!({
                "type": "unsupported_capability",
                "endpoint": "fixture-agent",
                "operation": "session/list",
                "requiredCapability": "plausible-but-wrong"
            }),
        ),
        (
            RESOURCE_LIMIT_ERROR,
            json!({
                "type": "resource_limit",
                "resource": "plausible-but-wrong",
                "limit": 256
            }),
        ),
        (
            RESUME_LOAD_FAILED_ERROR,
            json!({
                "type": "resume_load_failed",
                "attemptedMethod": "future/method",
                "endpoint": "fixture-agent",
                "convId": "conv-1",
                "agentSessionId": "session-1",
                "source": {"type": "internal"}
            }),
        ),
    ] {
        let error = rpc_error_to_hub_error(RpcErrorObject {
            code,
            message: "rpc-secret-sentinel".to_string(),
            data: Some(data),
        });
        assert!(matches!(
            &error,
            HubError::DaemonUnavailable(message)
                if message == "daemon returned malformed error data"
        ));
        assert!(!error.to_string().contains("rpc-secret-sentinel"));
    }
}

#[tokio::test]
async fn malformed_response_shape_fails_pending_request_without_echoing_wire_data() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (server_reader, mut server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let request: RpcRequest =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let malformed = json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": request.id.unwrap(),
            "result": null,
            "rpc-secret-sentinel": true
        });
        server_writer
            .write_all(format!("{malformed}\n").as_bytes())
            .await
            .unwrap();
    });

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.request_value("malformed", Value::Null),
    )
    .await
    .expect("malformed response must close pending requests")
    .expect_err("malformed response must fail");
    server.await.unwrap();

    assert!(matches!(
        &error,
        HubError::DaemonUnavailable(message)
            if message == "daemon returned an invalid RPC response"
    ));
    assert!(!error.to_string().contains("rpc-secret-sentinel"));
}

#[tokio::test]
async fn response_without_newline_is_rejected_at_eof() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (server_reader, mut server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let request: RpcRequest =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let response =
            RpcResponse::success(request.id.unwrap(), json!({"accepted": false})).unwrap();
        server_writer
            .write_all(&serde_json::to_vec(&response).unwrap())
            .await
            .unwrap();
    });

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.request_value("partial-response", Value::Null),
    )
    .await
    .expect("partial response EOF must complete the pending request")
    .expect_err("response without newline must not decode");
    server.await.unwrap();

    assert!(matches!(
        error,
        HubError::DaemonUnavailable(message)
            if message == "daemon RPC frame ended before newline"
    ));
}

#[tokio::test]
async fn pending_requests_fail_when_reader_reaches_eof() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let _: RpcRequest =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        drop(server_writer);
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.request_value("wait", Value::Null),
    )
    .await
    .expect("pending request should not hang after EOF");
    server.await.unwrap();

    assert!(matches!(result, Err(HubError::DaemonUnavailable(_))));
}
#[tokio::test]
async fn cancelled_request_removes_its_pending_registration() {
    let (client_reader, _server_writer) = tokio::io::duplex(64);
    let (client_writer, _server_reader) = tokio::io::duplex(1);
    let client = Arc::new(RpcClient::from_reader_writer(client_reader, client_writer));
    let request_client = Arc::clone(&client);
    let request = tokio::spawn(async move {
        request_client
            .request_value("blocked-write", json!({"padding": "x".repeat(1024)}))
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while client.inner.pending.lock().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request must register before its write completes");
    request.abort();
    let _ = request.await;
    assert!(
        client.inner.pending.lock().is_empty(),
        "cancelling a caller must remove its pending response sender"
    );
}

#[tokio::test]
async fn writer_failure_fails_every_pending_request_while_reader_stays_open() {
    let (client_reader, _server_writer) = tokio::io::duplex(64);
    let (client_writer, server_reader) = tokio::io::duplex(64);
    let client = Arc::new(RpcClient::from_reader_writer(client_reader, client_writer));
    let first_client = Arc::clone(&client);
    let first = tokio::spawn(async move {
        first_client
            .request_value("first", json!({"padding": "x".repeat(4096)}))
            .await
    });
    let second_client = Arc::clone(&client);
    let second = tokio::spawn(async move {
        second_client
            .request_value("second", json!({"padding": "y".repeat(4096)}))
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while client.inner.pending.lock().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both requests must be pending before the write side fails");
    drop(server_reader);

    for request in [first, second] {
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), request)
            .await
            .expect("writer failure must resolve all pending calls")
            .unwrap()
            .expect_err("writer failure must fail the request");
        assert!(matches!(
            error,
            HubError::DaemonUnavailable(message) if message == WRITER_FAILED_MESSAGE
        ));
    }
    assert!(client.inner.pending.lock().is_empty());
}

#[tokio::test]
async fn method_frame_is_notification_only_when_id_is_absent() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (_server_reader, mut server_writer) = tokio::io::split(server_io);
    let client = RpcClient::from_reader_writer(client_reader, client_writer);
    let mut notifications = client.subscribe_notifications();

    server_writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"event\",\"params\":{\"ok\":true}}\n")
        .await
        .unwrap();
    let notification = notifications.recv().await.unwrap();
    assert_eq!(notification.method, "event");

    server_writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"request\",\"params\":null}\n")
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !client.inner.closed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("method frame with an explicit null id must close the connection");
    assert!(notifications.try_recv().is_err());
}
