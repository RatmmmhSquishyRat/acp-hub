use super::*;
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
async fn one_connection_processes_requests_concurrently() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let activity = Arc::new(ActivityTracker::new());

    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        |line| async move {
            let request: RpcRequest = serde_json::from_str(&line)?;
            if request.method == "slow" {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let id = request.id.expect("test request id");
            encode_response(&RpcResponse::success(
                id,
                json!({"method": request.method}),
            )?)
            .map(|response| Some(EncodedRpcResponse::Reply(retain_test_response(response))))
        },
    ));

    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(1), "slow", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(2), "fast", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    drop(client_writer);

    let mut lines = BufReader::new(client_reader).lines();
    let first: RpcResponse =
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    let second: RpcResponse =
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    drop(lines);
    server.await.unwrap().unwrap();

    assert_eq!(first.id, json!(2));
    assert_eq!(second.id, json!(1));
}
#[tokio::test]
async fn frame_budget_does_not_turn_into_dispatch_concurrency_limit() {
    let (server_io, client_io) = tokio::io::duplex(16 * 1024);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let activity = Arc::new(ActivityTracker::with_limits(
        1,
        64,
        4,
        MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL,
        MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL,
        MAX_RETAINED_RPC_FALLBACK_BYTES_GLOBAL,
    ));
    let gate = Arc::new(Semaphore::new(0));

    let server_gate = Arc::clone(&gate);
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        move |line| {
            let gate = Arc::clone(&server_gate);
            async move {
                let request: RpcRequest = serde_json::from_str(&line)?;
                if request.method == "blocked" {
                    let _permit = gate.acquire().await.unwrap();
                }
                encode_response(&RpcResponse::success(
                    request.id.unwrap(),
                    json!({"method": request.method}),
                )?)
                .map(|response| Some(EncodedRpcResponse::Reply(retain_test_response(response))))
            }
        },
    ));

    for id in 1..=4 {
        client_writer
            .write_all(
                &encode_response(&RpcRequest::new(json!(id), "blocked", Value::Null)).unwrap(),
            )
            .await
            .unwrap();
    }
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(5), "fast", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer.flush().await.unwrap();

    let mut lines = BufReader::new(client_reader).lines();
    let fast: RpcResponse = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("a fifth frame must be admitted while four dispatches are blocked")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(fast.id, json!(5));

    gate.add_permits(4);
    for _ in 0..4 {
        tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    drop(client_writer);
    drop(lines);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelling_connection_drops_partial_frame_reader() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (_client_reader, mut client_writer) = tokio::io::split(client_io);
    let (read_tx, read_rx) = tokio::sync::oneshot::channel();
    let (drop_tx, drop_rx) = tokio::sync::oneshot::channel();
    let observed_reader = DropObservedReader {
        inner: server_reader,
        read: Some(read_tx),
        dropped: Some(drop_tx),
    };
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let server = tokio::spawn(handle_client_io(
        observed_reader,
        server_writer,
        Arc::new(ActivityTracker::new()),
        notification_rx,
        |_line| async move { Ok(None) },
    ));

    client_writer
        .write_all(b"{\"jsonrpc\":\"2.0\"")
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), read_rx)
        .await
        .expect("frame reader should consume the first fragment")
        .unwrap();

    server.abort();
    let _ = server.await;
    tokio::time::timeout(Duration::from_millis(200), drop_rx)
        .await
        .expect("cancelling the connection must cancel and drop its frame reader")
        .unwrap();
}

#[tokio::test]
async fn global_admission_caps_clients_and_buffered_frames() {
    let activity = Arc::new(ActivityTracker::with_limits(
        1,
        1,
        2,
        MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL,
        MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL,
        MAX_RETAINED_RPC_FALLBACK_BYTES_GLOBAL,
    ));
    let first_client = activity
        .try_client_slot()
        .expect("first client should be admitted");
    assert!(
        activity.try_client_slot().is_none(),
        "client admission must be globally bounded"
    );
    drop(first_client);
    assert!(activity.try_client_slot().is_some());

    let (mut writer_one, reader_one) = tokio::io::duplex(128);
    let (mut writer_two, reader_two) = tokio::io::duplex(128);
    let (mut writer_three, reader_three) = tokio::io::duplex(128);
    for writer in [&mut writer_one, &mut writer_two, &mut writer_three] {
        writer.write_all(b"{\"frame\":true}\n").await.unwrap();
    }
    let mut reader_one = BufReader::new(reader_one);
    let mut reader_two = BufReader::new(reader_two);
    let mut reader_three = BufReader::new(reader_three);
    let first = read_bounded_frame(
        &mut reader_one,
        Arc::clone(&activity.frame_slots),
        Arc::clone(&activity.rpc_bytes.requests),
    )
    .await
    .unwrap()
    .unwrap();
    let second = read_bounded_frame(
        &mut reader_two,
        Arc::clone(&activity.frame_slots),
        Arc::clone(&activity.rpc_bytes.requests),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            read_bounded_frame(
                &mut reader_three,
                Arc::clone(&activity.frame_slots),
                Arc::clone(&activity.rpc_bytes.requests),
            ),
        )
        .await
        .is_err(),
        "third connection must backpressure while both frame slots are held"
    );
    drop(first);
    let third = tokio::time::timeout(
        Duration::from_secs(2),
        read_bounded_frame(
            &mut reader_three,
            Arc::clone(&activity.frame_slots),
            Arc::clone(&activity.rpc_bytes.requests),
        ),
    )
    .await
    .expect("releasing one frame slot must unblock another connection")
    .unwrap()
    .unwrap();
    assert_eq!(third.line, "{\"frame\":true}");
    drop((second, third));
}

#[tokio::test]
async fn retained_byte_admission_covers_request_and_slow_response_lifetimes() {
    let activity = Arc::new(ActivityTracker::new());
    let request = encode_response(&RpcRequest::new(
        json!(1),
        "bounded",
        json!({"body": "request"}),
    ))
    .unwrap();
    let request_charge = request.len();
    let mut reader = BufReader::new(request.as_slice());
    let frame = read_bounded_frame(
        &mut reader,
        Arc::clone(&activity.frame_slots),
        Arc::clone(&activity.rpc_bytes.requests),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        activity.rpc_bytes.requests.available_permits(),
        MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL - request_charge
    );

    let response = encode_retained_response(
        &RpcResponse::success(json!(1), json!({"body": "x".repeat(4096)})).unwrap(),
        Arc::clone(&activity.rpc_bytes.responses),
        MAX_RPC_LINE_BYTES,
    )
    .unwrap();
    let response_charge = response.len();
    assert_eq!(
        activity.rpc_bytes.responses.available_permits(),
        MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL - response_charge
    );

    let (writer, _unread_peer) = tokio::io::duplex(64);
    let writer = Arc::new(AsyncMutex::new(writer));
    let delivery = tokio::spawn(async move {
        deliver_response(&writer, &response, false, Duration::from_millis(50)).await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        activity.rpc_bytes.responses.available_permits(),
        MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL - response_charge,
        "encoded response bytes must remain admitted while the peer is not reading"
    );
    assert!(
        delivery
            .await
            .unwrap()
            .expect_err("the deliberately stalled writer must time out")
            .to_string()
            .contains("delivery timed out")
    );
    assert_eq!(
        activity.rpc_bytes.responses.available_permits(),
        MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL
    );
    drop(frame);
    assert_eq!(
        activity.rpc_bytes.requests.available_permits(),
        MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL
    );
}

#[tokio::test]
async fn partial_request_saturation_cannot_starve_an_admitted_response() {
    let request = encode_response(&RpcRequest::new(
        json!(17),
        "already-dispatched",
        Value::Null,
    ))
    .unwrap();
    let partial_bytes = 32;
    let request_limit = request.len() + partial_bytes;
    let activity = Arc::new(ActivityTracker::with_limits(
        2,
        2,
        2,
        request_limit,
        4096,
        4096,
    ));
    let (dispatch_started_tx, dispatch_started_rx) = tokio::sync::oneshot::channel();
    let dispatch_started_tx = Arc::new(Mutex::new(Some(dispatch_started_tx)));
    let dispatch_release = Arc::new(Semaphore::new(0));
    let response_admission = activity.rpc_bytes.clone();

    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let release = Arc::clone(&dispatch_release);
    let started = Arc::clone(&dispatch_started_tx);
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        Arc::clone(&activity),
        notification_rx,
        move |line| {
            let release = Arc::clone(&release);
            let started = Arc::clone(&started);
            let response_admission = response_admission.clone();
            async move {
                if let Some(started) = started.lock().take() {
                    let _ = started.send(());
                }
                release.acquire().await.unwrap().forget();
                let request: RpcRequest = serde_json::from_str(&line)?;
                let response = RpcResponse::success(request.id.unwrap(), json!({"ok": true}))?;
                encode_response_with_fallback(json!(17), &response, &response_admission).map(Some)
            }
        },
    ));
    client_writer.write_all(&request).await.unwrap();
    client_writer.flush().await.unwrap();
    dispatch_started_rx.await.unwrap();

    let (mut partial_writer, partial_reader) = tokio::io::duplex(128);
    partial_writer
        .write_all(&vec![b'x'; partial_bytes])
        .await
        .unwrap();
    partial_writer.flush().await.unwrap();
    let request_slots = Arc::clone(&activity.rpc_bytes.requests);
    let frame_slots = Arc::clone(&activity.frame_slots);
    let partial = tokio::spawn(async move {
        let mut reader = BufReader::new(partial_reader);
        read_bounded_frame(&mut reader, frame_slots, request_slots).await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while activity.rpc_bytes.requests.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("partial request did not saturate request admission");

    dispatch_release.add_permits(1);
    let mut lines = BufReader::new(client_reader).lines();
    let response: RpcResponse = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("an admitted dispatch must retain response completion capacity")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response.id, json!(17));
    assert_eq!(response.result, Some(json!({"ok": true})));

    partial.abort();
    let _ = partial.await;
    drop(partial_writer);
    drop(client_writer);
    drop(lines);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn clean_eof_drain_aborts_long_dispatch_after_terminal_response() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (_client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        Arc::new(ActivityTracker::new()),
        notification_rx,
        |line| async move {
            let request: RpcRequest = serde_json::from_str(&line)?;
            if request.method == "long" {
                return std::future::pending().await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let terminal = encode_response(&RpcError::new(
                Value::Null,
                INTERNAL_ERROR,
                "connection closing",
                None,
            ))?;
            Ok(Some(EncodedRpcResponse::Terminal(retain_test_response(
                terminal,
            ))))
        },
    ));
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(1), "long", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer
        .write_all(&encode_response(&RpcRequest::new(json!(2), "terminal", Value::Null)).unwrap())
        .await
        .unwrap();
    client_writer.shutdown().await.unwrap();

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("terminal response must abort long dispatch during EOF drain")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn daemon_connection_forwards_streamed_notifications() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let activity = Arc::new(ActivityTracker::new());

    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        |_line| async move { Ok(None) },
    ));
    notifications
        .send(RpcRequest::notification(
            "hub/conv/update",
            json!({"conversationId": "conv-a"}),
        ))
        .unwrap();

    let mut lines = BufReader::new(client_reader).lines();
    let notification: RpcRequest =
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(notification.method, "hub/conv/update");
    assert_eq!(notification.params["conversationId"], "conv-a");

    drop(client_writer);
    drop(lines);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn fragmented_request_survives_notification_select_wakeup() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (client_reader, mut client_writer) = tokio::io::split(client_io);
    let (notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let (seen_tx, mut seen_rx) = tokio::sync::mpsc::channel(1);
    let activity = Arc::new(ActivityTracker::new());

    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        move |line| {
            let seen_tx = seen_tx.clone();
            async move {
                seen_tx.send(line.clone()).await.unwrap();
                let request: RpcRequest = serde_json::from_str(&line)?;
                encode_response(&RpcResponse::success(
                    request.id.unwrap(),
                    json!({"ok": true}),
                )?)
                .map(|response| Some(EncodedRpcResponse::Reply(retain_test_response(response))))
            }
        },
    ));

    let request = RpcRequest::new(json!(41), "fragmented", Value::Null);
    let line = serde_json::to_string(&request).unwrap();
    let split_at = line.len() / 2;
    client_writer
        .write_all(&line.as_bytes()[..split_at])
        .await
        .unwrap();
    client_writer.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    notifications
        .send(RpcRequest::notification(
            "hub/conv/update",
            json!({"betweenFragments": true}),
        ))
        .unwrap();
    let mut lines = BufReader::new(client_reader).lines();
    let notification: RpcRequest = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("notification should become ready between request fragments")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(notification.method, "hub/conv/update");

    client_writer
        .write_all(&line.as_bytes()[split_at..])
        .await
        .unwrap();
    client_writer.write_all(b"\n").await.unwrap();
    client_writer.flush().await.unwrap();

    let observed = tokio::time::timeout(Duration::from_secs(2), seen_rx.recv())
        .await
        .expect("fragmented request should be dispatched")
        .expect("dispatch observer should remain open");
    assert_eq!(observed, line);
    let response: RpcResponse = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("fragmented request should receive one response")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response.id, json!(41));

    drop(client_writer);
    drop(lines);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn request_without_newline_is_rejected_at_eof_without_dispatch() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let (server_reader, server_writer) = tokio::io::split(server_io);
    let (_client_reader, mut client_writer) = tokio::io::split(client_io);
    let (_notifications, notification_rx) = tokio::sync::broadcast::channel(8);
    let (seen_tx, mut seen_rx) = tokio::sync::mpsc::channel(1);
    let activity = Arc::new(ActivityTracker::new());
    let server = tokio::spawn(handle_client_io(
        server_reader,
        server_writer,
        activity,
        notification_rx,
        move |line| {
            let seen_tx = seen_tx.clone();
            async move {
                seen_tx.send(line).await.unwrap();
                Ok(None)
            }
        },
    ));

    let request = RpcRequest::new(json!(52), "partial", Value::Null);
    client_writer
        .write_all(&serde_json::to_vec(&request).unwrap())
        .await
        .unwrap();
    client_writer.shutdown().await.unwrap();

    let error = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("server must reject a partial request at EOF")
        .unwrap()
        .expect_err("partial request must be a framing error");
    assert!(matches!(
        &error,
        HubError::Io(error) if error.kind() == ErrorKind::UnexpectedEof
    ));
    assert!(
        seen_rx.try_recv().is_err(),
        "partial request was dispatched"
    );
}
