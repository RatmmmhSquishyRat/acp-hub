use super::*;
use crate::error::AuthMethodSummary;
use crate::rpc::INVALID_REQUEST;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, ReadBuf};

struct DropObservedReader<R> {
    inner: R,
    read: Option<tokio::sync::oneshot::Sender<()>>,
    dropped: Option<tokio::sync::oneshot::Sender<()>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for DropObservedReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buffer);
        if matches!(result, std::task::Poll::Ready(Ok(())))
            && buffer.filled().len() > before
            && let Some(read) = self.read.take()
        {
            let _ = read.send(());
        }
        result
    }
}

impl<R> Drop for DropObservedReader<R> {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

#[tokio::test]
async fn invalid_request_shape_and_id_are_fixed_and_data_free() {
    let activity = Arc::new(ActivityTracker::new());
    let home = tempfile::tempdir().unwrap();
    let hub = Arc::new(CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::clone(&activity),
    ));
    let cases = [
        (
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": 7,
                "method": "hub/agent/list",
                "params": null,
                "rpc-secret-sentinel": true
            }),
            json!(7),
        ),
        (
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": true,
                "method": "hub/agent/list",
                "params": null
            }),
            Value::Null,
        ),
        (
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": {"rpc-secret-sentinel": true},
                "method": "hub/agent/list",
                "params": null
            }),
            Value::Null,
        ),
        (
            json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": ["rpc-secret-sentinel"],
                "method": "hub/agent/list",
                "params": null
            }),
            Value::Null,
        ),
    ];

    for (raw, expected_id) in cases {
        let response = handle_rpc_line(
            &serde_json::to_string(&raw).unwrap(),
            Arc::clone(&hub),
            Arc::clone(&activity),
        )
        .await
        .unwrap()
        .unwrap();
        let bytes = match response {
            EncodedRpcResponse::Reply(bytes) => bytes,
            EncodedRpcResponse::Terminal(_) => panic!("small invalid request was terminal"),
        };
        let response: RpcError = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.id, expected_id);
        assert_eq!(response.error.code, INVALID_REQUEST);
        assert_eq!(response.error.message, "invalid JSON-RPC request");
        assert!(response.error.data.is_none());
        assert!(
            !String::from_utf8(bytes.bytes)
                .unwrap()
                .contains("rpc-secret-sentinel")
        );
    }
}

#[tokio::test]
async fn explicit_null_id_is_a_request_but_absent_id_is_a_notification() {
    let activity = Arc::new(ActivityTracker::new());
    let home = tempfile::tempdir().unwrap();
    let hub = Arc::new(CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::clone(&activity),
    ));

    let request = json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": null,
        "method": "hub/agent/list",
        "params": null
    });
    let response = handle_rpc_line(
        &serde_json::to_string(&request).unwrap(),
        Arc::clone(&hub),
        Arc::clone(&activity),
    )
    .await
    .unwrap()
    .expect("explicit null id must receive a response");
    let bytes = match response {
        EncodedRpcResponse::Reply(bytes) => bytes,
        EncodedRpcResponse::Terminal(_) => panic!("small null-id response was terminal"),
    };
    let response: RpcResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response.id, Value::Null);

    let notification = json!({
        "jsonrpc": JSONRPC_VERSION,
        "method": "hub/agent/list",
        "params": null
    });
    assert!(
        handle_rpc_line(
            &serde_json::to_string(&notification).unwrap(),
            hub,
            activity,
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn overload_path_never_echoes_invalid_request_ids() {
    let (client, server) = tokio::io::duplex(4096);
    let mut client = BufReader::new(client);
    let writer = Arc::new(AsyncMutex::new(server));
    for id in [
        json!(true),
        json!({"rpc-secret-sentinel": true}),
        json!(["rpc-secret-sentinel"]),
    ] {
        let line = json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "method": "blocked",
            "params": null
        })
        .to_string();
        assert!(
            !write_overload_response(
                &writer,
                &line,
                "global RPC concurrency limit exceeded",
                ActivityTracker::new().rpc_bytes,
                Duration::from_secs(2),
            )
            .await
            .unwrap()
        );

        let mut response = Vec::new();
        client.read_until(b'\n', &mut response).await.unwrap();
        let response: RpcError = serde_json::from_slice(&response).unwrap();
        assert_eq!(response.id, Value::Null);
        assert_eq!(response.error.code, INVALID_REQUEST);
        assert_eq!(response.error.message, "invalid JSON-RPC request");
        assert!(response.error.data.is_none());
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("rpc-secret-sentinel")
        );
    }
}

#[tokio::test]
async fn newline_inclusive_frame_limit_rejects_exactly_one_byte_over() {
    let mut inbound = vec![b'x'; MAX_RPC_LINE_BYTES];
    inbound.push(b'\n');
    let mut reader = BufReader::new(inbound.as_slice());
    let error = match read_bounded_frame(
        &mut reader,
        Arc::new(Semaphore::new(1)),
        Arc::new(Semaphore::new(MAX_RETAINED_RPC_BYTES_GLOBAL)),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("MAX + 1 inbound frame must be rejected"),
    };
    assert!(error.to_string().contains("exceeds"));

    let empty = RpcRequest::new(json!(""), "future/missing", Value::Null);
    let extra = MAX_RPC_LINE_BYTES + 1 - encode_response(&empty).unwrap().len();
    let oversized = RpcRequest::new(json!("x".repeat(extra)), "future/missing", Value::Null);
    let error = encode_response(&oversized).expect_err("MAX + 1 response must be rejected");
    assert!(error.to_string().contains("exceeds"));
}

#[tokio::test]
async fn stalled_response_delivery_times_out_and_releases_global_rpc_slot() {
    let activity = Arc::new(ActivityTracker::with_limits_and_timeout(
        2,
        1,
        2,
        MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL,
        MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL,
        MAX_RETAINED_RPC_FALLBACK_BYTES_GLOBAL,
        Duration::from_millis(50),
    ));
    let (server_io, client_io) = tokio::io::duplex(64);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (_client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let first = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        Arc::clone(&activity),
        notification_rx,
        |line| async move {
            let request: RpcRequest = serde_json::from_str(&line)?;
            let response =
                RpcResponse::success(request.id.unwrap(), json!({"body": "x".repeat(1024)}))?;
            Ok(Some(EncodedRpcResponse::Reply(retain_test_response(
                encode_response(&response)?,
            ))))
        },
    ));
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(1), "large", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("stalled response delivery must terminate")
        .unwrap()
        .expect_err("stalled response delivery must fail");
    assert!(error.to_string().contains("delivery timed out"));
    assert_eq!(activity.rpc_slots.available_permits(), 1);

    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let second = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        Arc::clone(&activity),
        notification_rx,
        |line| async move {
            let request: RpcRequest = serde_json::from_str(&line)?;
            let response = RpcResponse::success(request.id.unwrap(), json!({"ok": true}))?;
            Ok(Some(EncodedRpcResponse::Reply(retain_test_response(
                encode_response(&response)?,
            ))))
        },
    ));
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(2), "small", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    let mut lines = BufReader::new(client_reader).lines();
    let response: RpcResponse = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("unrelated request should receive a response")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response.id, json!(2));
    drop(client_writer);
    drop(lines);
    second.await.unwrap().unwrap();
}

#[tokio::test]
async fn valid_method_with_wrong_params_returns_fixed_invalid_params_error() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let activity = Arc::new(ActivityTracker::new());
    let home = tempfile::tempdir().unwrap();
    let hub = Arc::new(CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::clone(&activity),
    ));
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let dispatch_hub = Arc::clone(&hub);
    let dispatch_activity = Arc::clone(&activity);
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        move |line| {
            let hub = Arc::clone(&dispatch_hub);
            let activity = Arc::clone(&dispatch_activity);
            async move { handle_rpc_line(&line, hub, activity).await }
        },
    ));

    let request = RpcRequest::new(
        json!(71),
        "hub/conv/message_cursor",
        json!({"unexpected": "rpc-secret-sentinel"}),
    );
    client_writer
        .write_all(&encode_response(&request).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    let mut lines = BufReader::new(client_reader).lines();
    let response: RpcError = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("wrong params must receive a response")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response.id, json!(71));
    assert_eq!(response.error.code, crate::rpc::INVALID_PARAMS);
    assert_eq!(response.error.message, "invalid request parameters");
    assert!(response.error.data.is_none());
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("rpc-secret-sentinel")
    );

    drop(client_writer);
    drop(lines);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn clean_eof_returns_outstanding_dispatch_failure() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (_client_reader, mut client_writer) = tokio::io::split(client_io);
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let observed_reader = DropObservedReader {
        inner: server_reader,
        read: None,
        dropped: Some(dropped_tx),
    };
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let dispatch_started = Arc::clone(&started);
    let dispatch_release = Arc::clone(&release);
    let server = tokio::spawn(handle_client_io(
        observed_reader,
        server_writer,
        Arc::new(ActivityTracker::new()),
        notification_rx,
        move |_line| {
            let started = Arc::clone(&dispatch_started);
            let release = Arc::clone(&dispatch_release);
            async move {
                started.notify_one();
                release.notified().await;
                Err(HubError::other("expected dispatch failure"))
            }
        },
    ));

    let started_wait = started.notified();
    tokio::pin!(started_wait);
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(72), "fail", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    started_wait.await;
    client_writer.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), dropped_rx)
        .await
        .expect("reader task must observe and process clean EOF")
        .unwrap();
    release.notify_one();

    let error = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("outstanding failure must complete connection cleanup")
        .unwrap()
        .expect_err("clean EOF must not hide an outstanding dispatch failure");
    assert_eq!(error.to_string(), "expected dispatch failure");
}

#[tokio::test]
async fn near_limit_typed_error_completes_client_request_without_echoing_data() {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let activity = Arc::new(ActivityTracker::new());
    let home = tempfile::tempdir().unwrap();
    let hub = Arc::new(CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::clone(&activity),
    ));
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let dispatch_hub = Arc::clone(&hub);
    let dispatch_activity = Arc::clone(&activity);
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        move |line| {
            let hub = Arc::clone(&dispatch_hub);
            let activity = Arc::clone(&dispatch_activity);
            async move { handle_rpc_line(&line, hub, activity).await }
        },
    ));

    let empty = RpcRequest::new(json!(1), "hub/conv/message_cursor", json!({"convId": ""}));
    let missing_len = MAX_RPC_LINE_BYTES - encode_response(&empty).unwrap().len();
    let missing = "m".repeat(missing_len);
    let request = RpcRequest::new(
        json!(1),
        "hub/conv/message_cursor",
        json!({"convId": missing}),
    );
    let request_line = encode_response(&request).unwrap();
    assert_eq!(request_line.len(), MAX_RPC_LINE_BYTES);
    let typed_error = hub_error_to_rpc(
        json!(1),
        HubError::not_found(
            "conversation",
            request.params["convId"].as_str().unwrap().to_string(),
        ),
    );
    assert!(encode_response(&typed_error).is_err());

    client_writer.write_all(&request_line).await.unwrap();
    client_writer.flush().await.unwrap();
    let mut lines = BufReader::new(client_reader).lines();
    let response_line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("oversized typed error fallback must arrive within the bound")
        .unwrap()
        .unwrap();
    let response: RpcError = serde_json::from_str(&response_line).unwrap();
    assert_eq!(response.id, json!(1));
    assert_eq!(response.error.code, INTERNAL_ERROR);
    assert_eq!(response.error.message, "RPC response too large");
    assert!(response.error.data.is_none());
    assert!(!response_line.contains("mmmmmmmm"));

    let follow_up = RpcRequest::new(json!(2), "hub/agent/list", Value::Null);
    client_writer
        .write_all(&encode_response(&follow_up).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    let follow_up: RpcResponse = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("connection must remain open after a normal-id fallback")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(follow_up.id, json!(2));
    assert!(follow_up.result.is_some());

    drop(client_writer);
    drop(lines);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn enormous_request_id_gets_terminal_error_and_closed_connection() {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let activity = Arc::new(ActivityTracker::new());
    let home = tempfile::tempdir().unwrap();
    let hub = Arc::new(CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::clone(&activity),
    ));
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let dispatch_hub = Arc::clone(&hub);
    let dispatch_activity = Arc::clone(&activity);
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        move |line| {
            let hub = Arc::clone(&dispatch_hub);
            let activity = Arc::clone(&dispatch_activity);
            async move { handle_rpc_line(&line, hub, activity).await }
        },
    ));

    let empty = RpcRequest::new(json!(""), "future/missing", Value::Null);
    let secret = "rpc-secret-sentinel";
    let padding_len = MAX_RPC_LINE_BYTES - encode_response(&empty).unwrap().len() - secret.len();
    let enormous_id = format!("{secret}{}", "x".repeat(padding_len));
    let request = RpcRequest::new(json!(enormous_id), "future/missing", Value::Null);
    let request_line = encode_response(&request).unwrap();
    assert_eq!(request_line.len(), MAX_RPC_LINE_BYTES);
    client_writer.write_all(&request_line).await.unwrap();
    client_writer.flush().await.unwrap();

    let mut lines = BufReader::new(client_reader).lines();
    let terminal_line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("terminal response must arrive within the bound")
        .unwrap()
        .expect("terminal response must precede connection close");
    assert!(terminal_line.len() < 256);
    assert!(!terminal_line.contains(secret));
    let terminal: RpcError = serde_json::from_str(&terminal_line).unwrap();
    assert_eq!(terminal.id, Value::Null);
    assert_eq!(terminal.error.code, INTERNAL_ERROR);
    assert!(terminal.error.data.is_none());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("server must close after a terminal response")
            .unwrap()
            .is_none()
    );

    drop(client_writer);
    server.await.unwrap().unwrap();
}

#[test]
fn daemon_encodes_only_safe_typed_hub_error_data() {
    let cases = [
        (
            HubError::not_found("conversation", "missing-conversation"),
            json!({
                "type": "not_found",
                "kind": "conversation",
                "id": "missing-conversation"
            }),
        ),
        (
            HubError::Conflict("conv-busy".to_string()),
            json!({"type": "conflict", "convId": "conv-busy"}),
        ),
        (
            HubError::UnsupportedCapability {
                endpoint: "fixture-agent".to_string(),
                operation: "session/list",
                required_capability: "session_capabilities.list",
            },
            json!({
                "type": "unsupported_capability",
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
                "type": "auth_required",
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
        let rpc = hub_error_to_rpc(json!(7), error);
        assert_eq!(rpc.error.data, Some(expected_data));
    }

    let resource_limit = hub_error_to_rpc(
        json!(8),
        HubError::ResourceLimit {
            resource: "daemon_retained_rpc_bytes",
            limit: MAX_RETAINED_RPC_BYTES_GLOBAL,
        },
    );
    assert_eq!(resource_limit.error.code, RESOURCE_LIMIT_ERROR);
    assert_eq!(
        resource_limit.error.data,
        Some(json!({
            "type": "resource_limit",
            "resource": "daemon_retained_rpc_bytes",
            "limit": MAX_RETAINED_RPC_BYTES_GLOBAL
        }))
    );

    let internal = hub_error_to_rpc(json!(9), HubError::Other("rpc-secret-sentinel".to_string()));
    assert_eq!(internal.error.code, INTERNAL_ERROR);
    assert!(internal.error.data.is_none());
    assert!(!internal.error.message.contains("rpc-secret-sentinel"));
}
