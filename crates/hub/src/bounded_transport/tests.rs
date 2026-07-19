use super::*;
use futures::io::{BufReader, Cursor};
use std::time::Duration;
use tokio::io::{AsyncReadExt as TokioAsyncReadExt, AsyncWriteExt as TokioAsyncWriteExt};
use tokio::net::TcpListener;

struct RetainedPostBytes {
    active: Arc<std::sync::atomic::AtomicUsize>,
    bytes: usize,
}

impl RetainedPostBytes {
    fn new(active: Arc<std::sync::atomic::AtomicUsize>, bytes: usize) -> Self {
        active.fetch_add(bytes, std::sync::atomic::Ordering::SeqCst);
        Self { active, bytes }
    }
}

impl Drop for RetainedPostBytes {
    fn drop(&mut self) {
        self.active
            .fetch_sub(self.bytes, std::sync::atomic::Ordering::SeqCst);
    }
}

fn stalled_post(bytes: usize, active: Arc<std::sync::atomic::AtomicUsize>) -> PendingPost {
    let retained = RetainedPostBytes::new(active, bytes);
    PendingPost {
        body_bytes: bytes,
        pending_request: None,
        response: async move {
            let _retained = retained;
            futures::future::pending::<Result<(), String>>().await
        }
        .boxed(),
    }
}
fn post_budget_usage(budget: &SharedPostBudget) -> (usize, usize) {
    let budget = budget.lock();
    (budget.frames, budget.bytes)
}

#[test]
fn sse_decoder_reassembles_data_lines() {
    let mut decoder = SseDecoder::default();
    assert!(
        decoder
            .push(b"event: message\ndata: {\"a\":")
            .unwrap()
            .is_empty()
    );
    let events = decoder.push(b"1}\n\n").unwrap();
    assert_eq!(events, vec![br#"{"a":1}"#.to_vec()]);
}

#[test]
fn flow_budget_releases_only_the_consumed_or_completed_message() {
    let mut frames = FlowBudget::default();
    let notification =
        RawJsonRpcMessage::notification("session/update".to_string(), serde_json::json!({}))
            .unwrap();
    for _ in 0..MAX_OUTSTANDING_INBOUND_FRAMES {
        frames.track(&notification, 1).unwrap();
    }
    let overflow =
        RawJsonRpcMessage::notification("session/update".to_string(), serde_json::json!({}))
            .unwrap();
    assert!(
        frames
            .track(&overflow, 1)
            .unwrap_err()
            .contains("outstanding frames")
    );
    frames
        .acknowledge_notification("session/update", &serde_json::json!({}))
        .unwrap();
    frames.track(&overflow, 1).unwrap();
    assert_eq!(frames.frames, MAX_OUTSTANDING_INBOUND_FRAMES);

    let mut bytes = FlowBudget::default();
    bytes
        .track(&overflow, MAX_OUTSTANDING_INBOUND_BYTES)
        .unwrap();
    assert!(
        bytes
            .track(&overflow, 1)
            .unwrap_err()
            .contains("outstanding bytes")
    );
    bytes
        .acknowledge_notification("session/update", &serde_json::json!({}))
        .unwrap();
    assert_eq!(bytes.frames, 0);
    assert_eq!(bytes.bytes, 0);
}

#[test]
fn flow_budget_bounds_callback_response_amplification_and_partial_sse_bytes() {
    let mut requests = FlowBudget::default();
    for index in 0..MAX_OUTSTANDING_INBOUND_REQUESTS {
        let message = RawJsonRpcMessage::request(
            "fs/read_text_file".to_string(),
            serde_json::json!({}),
            RequestId::Number(index as i64),
        )
        .unwrap();
        requests.track(&message, 1).unwrap();
    }
    let overflow = RawJsonRpcMessage::request(
        "fs/read_text_file".to_string(),
        serde_json::json!({}),
        RequestId::Number(MAX_OUTSTANDING_INBOUND_REQUESTS as i64),
    )
    .unwrap();
    assert!(
        requests
            .track(&overflow, 1)
            .unwrap_err()
            .contains("outstanding requests")
    );

    let flow = InboundFlowControl::new();
    flow.reserve_partial(MAX_OUTSTANDING_INBOUND_BYTES).unwrap();
    assert!(
        flow.reserve_partial(1)
            .unwrap_err()
            .contains("partial framing")
    );
    flow.release_partial(MAX_OUTSTANDING_INBOUND_BYTES);
    flow.reserve_partial(1).unwrap();
}

#[test]
fn unrelated_outbound_messages_do_not_release_inbound_requests() {
    let flow = InboundFlowControl::new();
    let request = RawJsonRpcMessage::request(
        "terminal/output".to_string(),
        serde_json::json!({}),
        RequestId::Number(7),
    )
    .unwrap();
    flow.track(&request, 128).unwrap();
    let outbound =
        RawJsonRpcMessage::notification("session/cancel".to_string(), serde_json::json!({}))
            .unwrap();
    flow.complete_outbound_response(&outbound).unwrap();
    {
        let budget = flow.inner.lock().unwrap();
        assert_eq!(budget.frames, 1);
        assert_eq!(budget.bytes, 128);
    }

    let response = RawJsonRpcMessage::response(RequestId::Number(7), Ok(serde_json::json!({})));
    flow.complete_outbound_response(&response).unwrap();
    let budget = flow.inner.lock().unwrap();
    assert_eq!(budget.frames, 0);
    assert_eq!(budget.bytes, 0);
}

#[test]
fn uncorrelated_null_id_protocol_error_is_not_a_request_completion() {
    let flow = InboundFlowControl::new();
    let notification =
        RawJsonRpcMessage::notification("session/update".to_string(), serde_json::json!({}))
            .unwrap();
    flow.track(&notification, 128).unwrap();
    let protocol_error = RawJsonRpcMessage::response(
        RequestId::Null,
        Err(AcpError::internal_error().data("notification capture failed")),
    );

    flow.complete_outbound_response(&protocol_error).unwrap();
    {
        let budget = flow.inner.lock().unwrap();
        assert_eq!(budget.frames, 1);
        assert_eq!(budget.bytes, 128);
    }

    let request = RawJsonRpcMessage::request(
        "request/with-null-id".to_string(),
        serde_json::json!({}),
        RequestId::Null,
    )
    .unwrap();
    flow.track(&request, 64).unwrap();
    flow.complete_outbound_response(&protocol_error).unwrap();
    let budget = flow.inner.lock().unwrap();
    assert_eq!(budget.frames, 1);
    assert_eq!(budget.bytes, 128);
    assert!(budget.requests.is_empty());
}

#[test]
fn physical_ack_requires_a_matching_reservation_identity() {
    let mut budget = FlowBudget::default();
    let small = RawJsonRpcMessage::notification(
        "session/update".to_string(),
        serde_json::json!({"body": "small"}),
    )
    .unwrap();
    let large = RawJsonRpcMessage::notification(
        "session/update".to_string(),
        serde_json::json!({"body": "large"}),
    )
    .unwrap();
    let large_token = budget.track(&large, 1024).unwrap();
    let small_token = budget.track(&small, 1).unwrap();

    let error = budget
        .acknowledge_notification("session/update", &serde_json::json!({"body": "missing"}))
        .unwrap_err();
    assert!(error.contains("no physical ACP reservation matches"));
    assert_eq!(budget.frames, 2);
    assert_eq!(budget.bytes, 1025);

    let acknowledged = budget
        .acknowledge_notification("session/update", &serde_json::json!({"body": "small"}))
        .unwrap();
    assert_eq!(acknowledged.token, small_token);
    assert_ne!(acknowledged.token, large_token);
    assert_eq!(budget.frames, 1);
    assert_eq!(budget.bytes, 1024);
    let large_identity =
        notification_identity("session/update", &serde_json::json!({"body": "large"}));
    assert_eq!(
        budget
            .notifications
            .front()
            .map(|frame| frame.identity.as_str()),
        Some(large_identity.as_str())
    );
}

#[test]
fn ambiguous_notification_identity_releases_the_smallest_reservation() {
    let mut budget = FlowBudget::default();
    let first = RawJsonRpcMessage::notification(
        "session/update".to_string(),
        serde_json::json!({"a": 1, "b": 2}),
    )
    .unwrap();
    let reordered = RawJsonRpcMessage::notification(
        "session/update".to_string(),
        serde_json::json!({"b": 2, "a": 1}),
    )
    .unwrap();
    let large_token = budget.track(&first, 1024).unwrap();
    let small_token = budget.track(&reordered, 32).unwrap();

    let acknowledged = budget
        .acknowledge_notification("session/update", &serde_json::json!({"a": 1, "b": 2}))
        .unwrap();
    assert_eq!(acknowledged.token, small_token);
    assert_ne!(acknowledged.token, large_token);
    assert_eq!(acknowledged.bytes, 32);
    assert_eq!(budget.frames, 1);
    assert_eq!(budget.bytes, 1024);
}

#[tokio::test]
async fn bounded_line_reader_rejects_oversized_frames() {
    let bytes = vec![b'x'; MAX_ACP_FRAME_BYTES + 1];
    let mut reader = BufReader::new(Cursor::new(bytes));
    let error = read_bounded_line(&mut reader).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn http_initialize_round_trip_uses_the_bounded_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut post, _) = listener.accept().await.unwrap();
        let mut request = vec![0_u8; 8192];
        let _ = post.read(&mut request).await.unwrap();
        let body =
            br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}"#;
        post.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAcp-Connection-Id: connection-test\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        post.write_all(body).await.unwrap();

        let (mut sse, _) = listener.accept().await.unwrap();
        let _ = sse.read(&mut request).await.unwrap();
        sse.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let client = BoundedHttpAgent::new(&format!("http://{address}"), &BTreeMap::new()).unwrap();
    let (mut caller, transport_side) = Channel::duplex();
    let transport = tokio::spawn(run_http(client, transport_side));
    caller
        .tx
        .unbounded_send(Ok(RawJsonRpcMessage::request(
            "initialize".to_string(),
            serde_json::json!({ "protocolVersion": 1 }),
            RequestId::Number(1),
        )
        .unwrap()))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), caller.rx.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(response, RawJsonRpcMessage::Response(_)));

    drop(caller);
    server.abort();
    transport.abort();
}

#[test]
fn stalled_post_backlog_does_not_exceed_the_declared_frame_budget() {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut queue = PostQueue::default();
    for _ in 0..=MAX_PENDING_HTTP_POST_FRAMES {
        queue.push(stalled_post(1, active.clone())).unwrap();
    }
    let error = queue.push(stalled_post(1, active.clone())).unwrap_err();

    assert_eq!(error, post_queue_full());
    assert_eq!(queue.queued.len(), MAX_PENDING_HTTP_POST_FRAMES);
    assert_eq!(
        post_budget_usage(&queue.budget),
        (MAX_PENDING_HTTP_POST_FRAMES, MAX_PENDING_HTTP_POST_FRAMES)
    );
}

#[test]
fn stalled_post_backlog_does_not_exceed_the_declared_byte_budget() {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut queue = PostQueue::default();
    let bytes = MAX_PENDING_HTTP_POST_BYTES / 2 + 1;
    queue.push(stalled_post(bytes, active.clone())).unwrap();
    queue.push(stalled_post(bytes, active.clone())).unwrap();
    let error = queue.push(stalled_post(bytes, active.clone())).unwrap_err();

    assert_eq!(error, post_queue_full());
    assert_eq!(queue.queued.len(), 1);
    assert_eq!(post_budget_usage(&queue.budget), (1, bytes));
}

#[test]
fn one_oversized_post_is_rejected_without_retaining_its_body() {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut queue = PostQueue::default();
    let error = queue
        .push(stalled_post(
            MAX_PENDING_HTTP_POST_BYTES + 1,
            active.clone(),
        ))
        .unwrap_err();

    assert_eq!(
        error,
        format!("outbound ACP POST frame exceeds {MAX_PENDING_HTTP_POST_BYTES} bytes")
    );
    assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn post_queue_releases_reservations_on_pop_failure_and_close() {
    let budget = Arc::new(ParkingMutex::new(PostBudget::default()));
    let mut queue = PostQueue::with_budget(budget.clone());
    queue
        .push(PendingPost {
            body_bytes: 7,
            pending_request: None,
            response: futures::future::ready(Ok(())).boxed(),
        })
        .unwrap();
    assert_eq!(post_budget_usage(&budget), (0, 0));
    let popped_active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    queue.push(stalled_post(19, popped_active.clone())).unwrap();
    assert_eq!(post_budget_usage(&budget), (1, 19));
    assert!(queue.next_completion().await.result.is_ok());
    assert_eq!(post_budget_usage(&budget), (1, 19));
    queue.start_next();
    assert_eq!(post_budget_usage(&budget), (0, 0));
    assert!(queue.in_flight.is_some());
    queue.close();
    assert_eq!(popped_active.load(std::sync::atomic::Ordering::SeqCst), 0);

    queue
        .push(PendingPost {
            body_bytes: 11,
            pending_request: None,
            response: futures::future::ready(Err("POST failed".to_string())).boxed(),
        })
        .unwrap();
    assert!(queue.next_completion().await.result.is_err());
    assert_eq!(post_budget_usage(&budget), (0, 0));

    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    queue.push(stalled_post(13, active.clone())).unwrap();
    assert_eq!(post_budget_usage(&budget), (0, 0));
    queue.push(stalled_post(17, active.clone())).unwrap();
    assert_eq!(post_budget_usage(&budget), (1, 17));
    queue.close();
    assert_eq!(post_budget_usage(&budget), (0, 0));
    assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn pending_post_budget_is_shared_across_both_http_ordering_lanes() {
    let budget = Arc::new(ParkingMutex::new(PostBudget::default()));
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ordered = PostQueue::with_budget(budget.clone());
    let mut responses = PostQueue::with_budget(budget.clone());
    for index in 0..MAX_PENDING_HTTP_POST_FRAMES + 2 {
        let queue = if index % 2 == 0 {
            &mut ordered
        } else {
            &mut responses
        };
        queue.push(stalled_post(1, active.clone())).unwrap();
    }

    let error = responses.push(stalled_post(1, active.clone())).unwrap_err();
    assert_eq!(error, post_queue_full());
    assert_eq!(
        post_budget_usage(&budget),
        (MAX_PENDING_HTTP_POST_FRAMES, MAX_PENDING_HTTP_POST_FRAMES)
    );

    ordered.close();
    responses.close();
    assert_eq!(post_budget_usage(&budget), (0, 0));
}

#[test]
fn a_full_lane_does_not_block_an_idle_lane_from_starting() {
    let budget = Arc::new(ParkingMutex::new(PostBudget::default()));
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ordered = PostQueue::with_budget(budget.clone());
    let mut responses = PostQueue::with_budget(budget.clone());
    ordered.push(stalled_post(1, active.clone())).unwrap();
    for _ in 0..MAX_PENDING_HTTP_POST_FRAMES {
        ordered.push(stalled_post(1, active.clone())).unwrap();
    }
    assert_eq!(
        post_budget_usage(&budget),
        (MAX_PENDING_HTTP_POST_FRAMES, MAX_PENDING_HTTP_POST_FRAMES)
    );

    responses
        .push(stalled_post(1, active.clone()))
        .expect("an idle lane must start without consuming queued-backlog capacity");
    assert!(responses.in_flight.is_some());
    assert_eq!(
        responses.push(stalled_post(1, active.clone())).unwrap_err(),
        post_queue_full()
    );

    ordered.close();
    responses.close();
    assert_eq!(post_budget_usage(&budget), (0, 0));
}

#[tokio::test]
async fn http_connect_errors_do_not_expose_endpoint_markers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);
    });
    let markers = [
        "http_user_marker_7d4e",
        "http_password_marker_8c5f",
        "http_path_marker_9b6a",
        "http_query_marker_0a7d",
    ];
    let endpoint = format!(
        "http://{}:{}@{address}/{}?access_token={}",
        markers[0], markers[1], markers[2], markers[3]
    );
    let client = BoundedHttpAgent::new(&endpoint, &BTreeMap::new()).unwrap();
    let (caller, transport_side) = Channel::duplex();
    caller
        .tx
        .unbounded_send(Ok(RawJsonRpcMessage::request(
            "initialize".to_string(),
            serde_json::json!({ "protocolVersion": 1 }),
            RequestId::Number(1),
        )
        .unwrap()))
        .unwrap();

    let error = tokio::time::timeout(Duration::from_secs(2), run_http(client, transport_side))
        .await
        .unwrap()
        .unwrap_err()
        .to_string();
    server.await.unwrap();
    assert!(error.contains("error sending request"));

    for marker in markers {
        assert!(
            !error.contains(marker),
            "HTTP transport error exposed endpoint marker {marker:?}: {error}"
        );
    }
}

#[tokio::test]
async fn websocket_connect_errors_do_not_expose_endpoint_markers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let markers = [
        "ws_user_marker_1b8e",
        "ws_password_marker_2c9f",
        "ws_path_marker_3d0a",
        "ws_query_marker_4e1b",
        "ws_header_marker_5f2c",
    ];
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let close_reason = Arc::new(ParkingMutex::new(String::new()));
        let observed = close_reason.clone();
        let mut stream = async_tungstenite::tokio::accept_hdr_async(
            socket,
            // Tungstenite's callback contract fixes the large ErrorResponse type.
            #[allow(clippy::result_large_err)]
            move |request: &async_tungstenite::tungstenite::handshake::server::Request,
                  response: async_tungstenite::tungstenite::handshake::server::Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                *observed.lock() = format!("{}|{authorization}", request.uri());
                Ok(response)
            },
        )
        .await
        .unwrap();
        let reason = close_reason.lock().clone();
        stream
            .send(WsMessage::Close(Some(
                async_tungstenite::tungstenite::protocol::CloseFrame {
                    code:
                        async_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
                    reason: reason.into(),
                },
            )))
            .await
            .unwrap();
    });
    let endpoint = format!(
        "ws://{}:{}@{address}/{}?access_token={}",
        markers[0], markers[1], markers[2], markers[3]
    );
    let mut headers = BTreeMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer {}", markers[4]),
    );
    let client = BoundedHttpAgent::new(&endpoint, &headers).unwrap();
    let (_caller, transport_side) = Channel::duplex();

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        run_websocket(client, transport_side),
    )
    .await
    .unwrap()
    .unwrap_err()
    .to_string();
    server.await.unwrap();
    assert!(error.contains("Policy"));

    for marker in markers {
        assert!(
            !error.contains(marker),
            "WebSocket transport error exposed endpoint marker {marker:?}: {error}"
        );
    }
}
#[test]
fn websocket_error_sanitizer_drops_inner_error_and_handshake_content() {
    let io_marker = "ws_io_cause_marker_6a3d";
    let io_error = WsError::Io(io::Error::other(io_marker));
    let rendered = sanitized_websocket_error("connect", &io_error).to_string();
    assert!(rendered.contains("I/O failure"));
    assert!(!rendered.contains(io_marker));

    let handshake_marker = "ws_handshake_marker_7b4e";
    let response = async_tungstenite::tungstenite::http::Response::builder()
        .status(401)
        .header("x-secret", handshake_marker)
        .body(Some(handshake_marker.as_bytes().to_vec()))
        .unwrap();
    let handshake_error = WsError::Http(Box::new(response));
    let rendered = sanitized_websocket_error("connect", &handshake_error).to_string();
    assert!(rendered.contains("401 Unauthorized"));
    assert!(!rendered.contains(handshake_marker));
}

#[tokio::test]
async fn http_content_length_is_rejected_before_body_allocation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    MAX_ACP_FRAME_BYTES + 1
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let response = reqwest::get(format!("http://{address}")).await.unwrap();
    let error = bounded_response_bytes(response).await.unwrap_err();
    assert!(error.contains("exceeds"));
    server.await.unwrap();
}

#[test]
fn websocket_and_http_session_ids_are_read_from_object_params() {
    let message = RawJsonRpcMessage::notification(
        "session/cancel".to_string(),
        serde_json::json!({
            "sessionId": agent_client_protocol::schema::v1::SessionId::new("session-a")
        }),
    )
    .unwrap();
    assert_eq!(
        session_id_from_message(&message).as_deref(),
        Some("session-a")
    );
}
#[test]
fn prepare_post_uses_the_preextracted_session_id() {
    let client = BoundedHttpAgent::new("http://127.0.0.1:1", &BTreeMap::new()).unwrap();
    let BoundedHttpAgent {
        endpoint,
        http,
        flow,
        ..
    } = client;
    let connection = HttpConnection::new(endpoint, http, flow);
    connection.set_connection_id("connection-test".to_string());
    let (incoming, _outgoing) = mpsc::unbounded();
    let mut state = ClientState {
        connection,
        open_session_streams: HashSet::new(),
        pending_requests: HashMap::new(),
        incoming,
    };
    let message = RawJsonRpcMessage::notification(
        "session/cancel".to_string(),
        serde_json::json!({ "sessionId": "embedded-session" }),
    )
    .unwrap();

    let error = match state.prepare_post(message, None) {
        Ok(_) => panic!("prepare_post must not re-extract sessionId from message params"),
        Err(error) => error,
    };
    assert_eq!(error, "method \"session/cancel\" requires sessionId");
}
#[test]
fn failed_responses_release_pending_request_entries_without_opening_streams() {
    let client = BoundedHttpAgent::new("http://127.0.0.1:1", &BTreeMap::new()).unwrap();
    let BoundedHttpAgent {
        endpoint,
        http,
        flow,
        ..
    } = client;
    let connection = HttpConnection::new(endpoint, http, flow);
    let (incoming, _outgoing) = mpsc::unbounded();
    let mut state = ClientState {
        connection,
        open_session_streams: HashSet::new(),
        pending_requests: HashMap::new(),
        incoming,
    };

    for index in 0..MAX_PENDING_HTTP_POST_FRAMES * 4 {
        let id = RequestId::Number(index as i64);
        state
            .pending_requests
            .insert(id.clone(), "session/new".to_string());
        let response = RawJsonRpcMessage::response(
            id,
            Err::<serde_json::Value, _>(
                AcpError::internal_error().data(format!("failed response marker {index}")),
            ),
        );
        assert!(state.session_to_open_for_response(&response).is_none());
    }

    assert!(state.pending_requests.is_empty());
    assert!(state.open_session_streams.is_empty());
}
