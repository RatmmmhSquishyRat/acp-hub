use super::*;

pub(super) async fn handle_client(
    stream: LocalSocketStream,
    hub: Arc<CoreHub>,
    activity: Arc<ActivityTracker>,
) -> Result<(), HubError> {
    let (reader, writer) = tokio::io::split(stream);
    let notifications = hub.ctx().subscribe_notifications();
    let dispatch_activity = Arc::clone(&activity);
    handle_client_io(reader, writer, activity, notifications, move |line| {
        let hub = Arc::clone(&hub);
        let activity = Arc::clone(&dispatch_activity);
        async move { handle_rpc_line(&line, hub, activity).await }
    })
    .await
}

struct RetainedRpcBytes {
    bytes: Vec<u8>,
    _reservation: OwnedSemaphorePermit,
}

impl std::ops::Deref for RetainedRpcBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

#[cfg(test)]
fn retain_test_response(bytes: Vec<u8>) -> RetainedRpcBytes {
    let byte_slots = Arc::new(Semaphore::new(bytes.len()));
    let reservation = Arc::clone(&byte_slots)
        .try_acquire_many_owned(bytes.len() as u32)
        .expect("test response reservation");
    RetainedRpcBytes {
        bytes,
        _reservation: reservation,
    }
}

enum EncodedRpcResponse {
    Reply(RetainedRpcBytes),
    Terminal(RetainedRpcBytes),
}

enum RpcTaskCompletion {
    Complete,
    CloseConnection,
}

struct AbortOnDropTask<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortOnDropTask<T> {
    fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn abort_and_join(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

struct BufferedRpcFrame {
    line: String,
    _slot: OwnedSemaphorePermit,
    _request_bytes: OwnedSemaphorePermit,
}

const MAX_RPC_FALLBACK_FRAME_BYTES: usize = 4096;

async fn handle_client_io<R, W, F, Fut>(
    reader: R,
    writer: W,
    activity: Arc<ActivityTracker>,
    mut notifications: tokio::sync::broadcast::Receiver<crate::rpc::RpcRequest>,
    dispatch: F,
) -> Result<(), HubError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    F: Fn(String) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<EncodedRpcResponse>, HubError>> + Send + 'static,
{
    let (frames_tx, mut frames_rx) = mpsc::channel(1);
    let frame_slots = Arc::clone(&activity.frame_slots);
    let request_byte_slots = Arc::clone(&activity.rpc_bytes.requests);
    let write_timeout = activity.rpc_write_timeout;
    let frame_reader = AbortOnDropTask::new(tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        loop {
            let frame = read_bounded_frame(
                &mut reader,
                Arc::clone(&frame_slots),
                Arc::clone(&request_byte_slots),
            )
            .await;
            let terminal = !matches!(&frame, Ok(Some(_)));
            if frames_tx.send(frame).await.is_err() || terminal {
                break;
            }
        }
    }));
    let writer = Arc::new(AsyncMutex::new(writer));
    let mut requests = JoinSet::new();
    let client_slots = Arc::new(Semaphore::new(MAX_INFLIGHT_RPC_PER_CLIENT));
    let mut connection_error = None;
    let mut abort_requests = false;

    loop {
        tokio::select! {
            frame = frames_rx.recv() => {
                let frame = match frame {
                    Some(Ok(Some(frame))) => frame,
                    Some(Ok(None)) | None => break,
                    Some(Err(error)) => {
                        connection_error = Some(error);
                        abort_requests = true;
                        break;
                    }
                };
                let BufferedRpcFrame {
                    line,
                    _slot: frame_slot,
                    _request_bytes: request_bytes,
                } = frame;
                let client_permit = match Arc::clone(&client_slots).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        match write_overload_response(
                            &writer,
                            &line,
                            "client RPC concurrency limit exceeded",
                            activity.rpc_bytes.clone(),
                            write_timeout,
                        ).await {
                            Ok(false) => continue,
                            Ok(true) => {
                                abort_requests = true;
                                break;
                            }
                            Err(error) => {
                                connection_error = Some(error);
                                abort_requests = true;
                                break;
                            }
                        }
                    }
                };
                let global_permit = match Arc::clone(&activity.rpc_slots).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        match write_overload_response(
                            &writer,
                            &line,
                            "global RPC concurrency limit exceeded",
                            activity.rpc_bytes.clone(),
                            write_timeout,
                        ).await {
                            Ok(false) => continue,
                            Ok(true) => {
                                abort_requests = true;
                                break;
                            }
                            Err(error) => {
                                connection_error = Some(error);
                                abort_requests = true;
                                break;
                            }
                        }
                    }
                };
                drop(frame_slot);
                activity.touch();
                let dispatch = dispatch.clone();
                let writer = Arc::clone(&writer);
                requests.spawn(async move {
                    let _permits = (client_permit, global_permit, request_bytes);
                    let Some(response) = dispatch(line).await? else {
                        return Ok(RpcTaskCompletion::Complete);
                    };
                    let (response, close_connection) = match response {
                        EncodedRpcResponse::Reply(response) => (response, false),
                        EncodedRpcResponse::Terminal(response) => (response, true),
                    };
                    deliver_response(
                        &writer,
                        &response,
                        close_connection,
                        write_timeout,
                    )
                    .await?;
                    if close_connection {
                        Ok(RpcTaskCompletion::CloseConnection)
                    } else {
                        Ok(RpcTaskCompletion::Complete)
                    }
                });
            }
            Some(joined) = requests.join_next(), if !requests.is_empty() => {
                match joined {
                    Ok(Ok(RpcTaskCompletion::Complete)) => {}
                    Ok(Ok(RpcTaskCompletion::CloseConnection)) => {
                        abort_requests = true;
                        break;
                    }
                    Ok(Err(error)) => {
                        warn!(error = %error, "daemon RPC response could not be delivered");
                        connection_error = Some(error);
                        abort_requests = true;
                        break;
                    }
                    Err(error) => {
                        warn!(error = %error, "daemon RPC task failed");
                        connection_error =
                            Some(HubError::other(format!("daemon RPC task failed: {error}")));
                        abort_requests = true;
                        break;
                    }
                }
            }
            notification = notifications.recv() => {
                match notification {
                    Ok(notification) => {
                        let line = match encode_retained_response(
                            &notification,
                            Arc::clone(&activity.rpc_bytes.responses),
                            MAX_RPC_LINE_BYTES,
                        ) {
                            Ok(line) => line,
                            Err(err) => {
                                warn!(error = %err, "dropping oversized daemon notification");
                                continue;
                            }
                        };
                        if let Err(error) =
                            deliver_response(&writer, &line, false, write_timeout).await
                        {
                            warn!(error = %error, "daemon notification delivery failed");
                            connection_error = Some(error);
                            abort_requests = true;
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        // Store-first Product-UX: live hub/conv/update fan-out may
                        // drop frames; durable conversation is already in the Hub
                        // Store (capture writes Store before broadcast). Never kill
                        // the client or in-flight RPCs solely for live lag, and do
                        // not treat this as incomplete Store projection / resync.
                        // See doc/ssot/agent-managed/pillars/Product-UX.md §5.
                        warn!(
                            skipped,
                            "daemon client lagged behind live notification stream; continuing (Store remains durable truth; live fan-out only)"
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    frame_reader.abort_and_join().await;
    if abort_requests {
        requests.abort_all();
    }
    while let Some(joined) = requests.join_next().await {
        match joined {
            Ok(Ok(RpcTaskCompletion::Complete)) => {}
            Ok(Ok(RpcTaskCompletion::CloseConnection)) => {
                abort_requests = true;
                requests.abort_all();
            }
            Ok(Err(error)) => {
                warn!(error = %error, "daemon RPC response could not be delivered");
                if connection_error.is_none() {
                    connection_error = Some(error);
                }
                abort_requests = true;
                requests.abort_all();
            }
            Err(error) if abort_requests && error.is_cancelled() => {}
            Err(error) => {
                warn!(error = %error, "daemon RPC task failed");
                if connection_error.is_none() {
                    connection_error = Some(HubError::other(
                        "daemon RPC task failed during connection cleanup",
                    ));
                }
                abort_requests = true;
                requests.abort_all();
            }
        }
    }
    match connection_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn write_overload_response<W>(
    writer: &Arc<AsyncMutex<W>>,
    line: &str,
    message: &str,
    byte_admission: RpcByteAdmission,
    write_timeout: Duration,
) -> Result<bool, HubError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let raw = serde_json::from_str::<Value>(line).ok();
    let id = raw.as_ref().map(request_id_presence);
    let (id, response) = match id {
        Some(Ok(Some(id))) => {
            let response = RpcError::new(id.clone(), -32_008, message, None);
            (id, response)
        }
        Some(Ok(None)) => {
            warn!(
                reason = message,
                "dropping overloaded JSON-RPC notification"
            );
            return Ok(false);
        }
        Some(Err(())) | None => {
            let response = RpcError::invalid_request(Value::Null, "invalid JSON-RPC request");
            (Value::Null, response)
        }
    };
    let response = encode_response_with_fallback(id, &response, &byte_admission)?;
    let (response, terminal) = match response {
        EncodedRpcResponse::Reply(response) => (response, false),
        EncodedRpcResponse::Terminal(response) => (response, true),
    };
    deliver_response(writer, &response, terminal, write_timeout).await?;
    Ok(terminal)
}

async fn deliver_response<W>(
    writer: &Arc<AsyncMutex<W>>,
    response: &[u8],
    close_connection: bool,
    write_timeout: Duration,
) -> Result<(), HubError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(write_timeout, async {
        let mut writer = writer.lock().await;
        writer.write_all(response).await?;
        writer.flush().await?;
        if close_connection {
            writer.shutdown().await?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| HubError::DaemonUnavailable("daemon RPC delivery timed out".to_string()))??;
    Ok(())
}

async fn read_bounded_frame<R>(
    reader: &mut R,
    frame_slots: Arc<Semaphore>,
    request_byte_slots: Arc<Semaphore>,
) -> Result<Option<BufferedRpcFrame>, HubError>
where
    R: AsyncBufRead + Unpin,
{
    let slot = frame_slots
        .acquire_owned()
        .await
        .map_err(|_| HubError::other("daemon RPC frame admission closed"))?;
    let has_data = tokio::time::timeout(RPC_FRAME_READ_TIMEOUT, async {
        Ok::<_, HubError>(!reader.fill_buf().await?.is_empty())
    })
    .await
    .map_err(|_| HubError::DaemonUnavailable("daemon RPC frame read timed out".to_string()))??;
    if !has_data {
        return Ok(None);
    }
    let mut request_bytes = None;
    let mut bytes = tokio::time::timeout(RPC_FRAME_READ_TIMEOUT, async {
        let mut bytes = Vec::new();
        loop {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Err(HubError::Io(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "daemon RPC frame ended before newline",
                )));
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(available.len(), |index| index + 1);
            if bytes.len().saturating_add(take) > MAX_RPC_LINE_BYTES {
                return Err(HubError::other(format!(
                    "daemon RPC frame exceeds {MAX_RPC_LINE_BYTES} bytes"
                )));
            }
            reserve_request_bytes(&request_byte_slots, &mut request_bytes, take)?;
            bytes.extend_from_slice(&available[..take]);
            reader.consume(take);
            if newline.is_some() {
                return Ok(bytes);
            }
        }
    })
    .await
    .map_err(|_| HubError::DaemonUnavailable("daemon RPC frame read timed out".to_string()))??;
    let request_bytes =
        request_bytes.expect("a non-empty complete RPC frame must retain admitted bytes");
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(|line| {
            Some(BufferedRpcFrame {
                line,
                _slot: slot,
                _request_bytes: request_bytes,
            })
        })
        .map_err(|error| HubError::other(format!("daemon RPC frame is not UTF-8: {error}")))
}

fn reserve_request_bytes(
    byte_slots: &Arc<Semaphore>,
    reservation: &mut Option<OwnedSemaphorePermit>,
    bytes: usize,
) -> Result<(), HubError> {
    if bytes == 0 {
        return Ok(());
    }
    let permits = u32::try_from(bytes).map_err(|_| HubError::ResourceLimit {
        resource: "daemon_retained_rpc_request_bytes",
        limit: MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL,
    })?;
    let permit = Arc::clone(byte_slots)
        .try_acquire_many_owned(permits)
        .map_err(|_| HubError::ResourceLimit {
            resource: "daemon_retained_rpc_request_bytes",
            limit: MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL,
        })?;
    if let Some(reservation) = reservation {
        reservation.merge(permit);
    } else {
        *reservation = Some(permit);
    }
    Ok(())
}

fn request_id_presence(value: &Value) -> Result<Option<Value>, ()> {
    let object = value.as_object().ok_or(())?;
    match object.get("id") {
        None => Ok(None),
        Some(id @ (Value::Null | Value::String(_) | Value::Number(_))) => Ok(Some(id.clone())),
        Some(_) => Err(()),
    }
}

async fn handle_rpc_line(
    line: &str,
    hub: Arc<CoreHub>,
    activity: Arc<ActivityTracker>,
) -> Result<Option<EncodedRpcResponse>, HubError> {
    if line.trim().is_empty() {
        return Ok(None);
    }

    let raw: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            let response = RpcError::parse_error("invalid JSON");
            return encode_response_with_fallback(Value::Null, &response, &activity.rpc_bytes)
                .map(Some);
        }
    };
    let id = match request_id_presence(&raw) {
        Ok(Some(id)) => Some(id),
        Ok(None) => None,
        Err(()) => {
            let response = RpcError::invalid_request(Value::Null, "invalid JSON-RPC request");
            return encode_response_with_fallback(Value::Null, &response, &activity.rpc_bytes)
                .map(Some);
        }
    };
    let request: RpcRequest = match serde_json::from_value(raw) {
        Ok(request) => request,
        Err(_) => {
            let response = RpcError::invalid_request(
                id.clone().unwrap_or(Value::Null),
                "invalid JSON-RPC request",
            );
            return encode_response_with_fallback(
                id.unwrap_or(Value::Null),
                &response,
                &activity.rpc_bytes,
            )
            .map(Some);
        }
    };

    if request.jsonrpc != JSONRPC_VERSION || request.method.is_empty() {
        let id = id.clone().unwrap_or(Value::Null);
        let error = RpcError::invalid_request(
            id.clone(),
            "expected JSON-RPC 2.0 request with a non-empty method",
        );
        return encode_response_with_fallback(id, &error, &activity.rpc_bytes).map(Some);
    }

    let Some(id) = id else {
        let _rpc = activity.rpc_lease();
        if let Err(err) = hub.handle_rpc(&request.method, request.params).await {
            warn!(method = %request.method, error = %err, "JSON-RPC notification failed");
        }
        return Ok(None);
    };

    let _rpc = activity.rpc_lease();
    let result = if request.method == DAEMON_HANDSHAKE_METHOD {
        let handshake: DaemonHandshakeRequest = serde_json::from_value(request.params)?;
        serde_json::to_value(DaemonHandshakeResponse {
            protocol_version: DAEMON_RPC_PROTOCOL_VERSION,
            compatible: handshake.protocol_version == DAEMON_RPC_PROTOCOL_VERSION,
            package_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .map_err(HubError::from)
    } else {
        hub.handle_rpc(&request.method, request.params).await
    };
    let response = match result {
        Ok(result) => {
            let success = RpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: id.clone(),
                result: Some(result),
            };
            encode_response_with_fallback(id, &success, &activity.rpc_bytes)?
        }
        Err(err) => {
            let error = hub_error_to_rpc(id.clone(), err);
            encode_response_with_fallback(id, &error, &activity.rpc_bytes)?
        }
    };
    Ok(Some(response))
}

fn hub_error_to_rpc(id: Value, error: HubError) -> RpcError {
    let (code, message) = match &error {
        HubError::Other(message) if message.starts_with("unknown RPC method ") => {
            (METHOD_NOT_FOUND, "method not found")
        }
        HubError::NotFound { .. } => (NOT_FOUND_ERROR, "resource not found"),
        HubError::Conflict(_) => (CONFLICT_ERROR, "conversation_busy"),
        HubError::ConversationBusy { .. } => (CONFLICT_ERROR, "conversation_busy"),
        HubError::NotBusy { .. } => (INVALID_PARAMS, "not_busy"),
        HubError::ReadOnlyConversation { .. } => (INVALID_PARAMS, "read_only_conversation"),
        HubError::ConversationClosed { .. } => (INVALID_PARAMS, "conversation_closed"),
        HubError::PermissionPolicyReject { .. } => (INVALID_PARAMS, "permission_policy_reject"),
        HubError::UnsupportedCapability { .. } => {
            (UNSUPPORTED_CAPABILITY_ERROR, "unsupported capability")
        }
        HubError::ResourceLimit { .. } => (RESOURCE_LIMIT_ERROR, "resource limit exceeded"),
        HubError::InvalidCursor { .. } => (INVALID_CURSOR_ERROR, "invalid message cursor"),
        HubError::StaleCursor { .. } => (STALE_CURSOR_ERROR, "stale message cursor"),
        HubError::AuthRequired { .. } => (AUTH_REQUIRED_ERROR, "authentication required"),
        HubError::InvalidRegistry(_) => (INVALID_REGISTRY_ERROR, "invalid registry"),
        HubError::UnsupportedProtocolVersion => (
            UNSUPPORTED_PROTOCOL_VERSION_ERROR,
            "unsupported protocol version",
        ),
        HubError::UnsupportedProxyTransport => (
            UNSUPPORTED_PROXY_TRANSPORT_ERROR,
            "unsupported proxy transport",
        ),
        HubError::ResumeLoadFailed { .. } => (RESUME_LOAD_FAILED_ERROR, "resume or load failed"),
        HubError::Json(_) => (INVALID_PARAMS, "invalid request parameters"),
        _ => (INTERNAL_ERROR, "internal daemon error"),
    };
    let data = typed_hub_error_data(&error);
    RpcError::new(id, code, message, data)
}

#[cfg(test)]
struct CappedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

#[cfg(test)]
impl CappedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(4096)),
            limit,
            exceeded: false,
        }
    }
}

#[cfg(test)]
impl std::io::Write for CappedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("JSON-RPC frame limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BudgetedCappedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
    budget_exceeded: bool,
    byte_slots: Arc<Semaphore>,
    reservation: Option<OwnedSemaphorePermit>,
}

impl BudgetedCappedJsonWriter {
    fn new(limit: usize, byte_slots: Arc<Semaphore>) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(4096)),
            limit,
            exceeded: false,
            budget_exceeded: false,
            byte_slots,
            reservation: None,
        }
    }

    fn reserve(&mut self, bytes: usize) -> std::io::Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        let permits = u32::try_from(bytes)
            .map_err(|_| std::io::Error::other("JSON-RPC byte reservation overflow"))?;
        let permit = Arc::clone(&self.byte_slots)
            .try_acquire_many_owned(permits)
            .map_err(|_| {
                self.budget_exceeded = true;
                std::io::Error::other("daemon retained JSON-RPC byte limit exceeded")
            })?;
        if let Some(reservation) = &mut self.reservation {
            reservation.merge(permit);
        } else {
            self.reservation = Some(permit);
        }
        Ok(())
    }
}

impl std::io::Write for BudgetedCappedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("JSON-RPC frame limit exceeded"));
        }
        self.reserve(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
fn encode_response<T: Serialize>(message: &T) -> Result<Vec<u8>, HubError> {
    let mut writer = CappedJsonWriter::new(MAX_RPC_LINE_BYTES - 1);
    if let Err(error) = serde_json::to_writer(&mut writer, message) {
        if writer.exceeded {
            return Err(HubError::other(format!(
                "daemon RPC frame exceeds {MAX_RPC_LINE_BYTES} bytes"
            )));
        }
        return Err(error.into());
    }
    writer.bytes.push(b'\n');
    Ok(writer.bytes)
}

fn encode_retained_response<T: Serialize>(
    message: &T,
    byte_slots: Arc<Semaphore>,
    frame_limit: usize,
) -> Result<RetainedRpcBytes, HubError> {
    let mut writer = BudgetedCappedJsonWriter::new(frame_limit - 1, byte_slots);
    if let Err(error) = serde_json::to_writer(&mut writer, message) {
        if writer.budget_exceeded {
            return Err(HubError::ResourceLimit {
                resource: "daemon_retained_rpc_bytes",
                limit: MAX_RETAINED_RPC_BYTES_GLOBAL,
            });
        }
        if writer.exceeded {
            return Err(HubError::other(format!(
                "daemon RPC frame exceeds {MAX_RPC_LINE_BYTES} bytes"
            )));
        }
        return Err(error.into());
    }
    writer.reserve(1).map_err(|_| HubError::ResourceLimit {
        resource: "daemon_retained_rpc_bytes",
        limit: MAX_RETAINED_RPC_BYTES_GLOBAL,
    })?;
    writer.bytes.push(b'\n');
    Ok(RetainedRpcBytes {
        bytes: writer.bytes,
        _reservation: writer
            .reservation
            .expect("non-empty JSON-RPC response must reserve retained bytes"),
    })
}

fn encode_response_with_fallback<T: Serialize>(
    id: Value,
    message: &T,
    byte_admission: &RpcByteAdmission,
) -> Result<EncodedRpcResponse, HubError> {
    match encode_retained_response(
        message,
        Arc::clone(&byte_admission.responses),
        MAX_RPC_LINE_BYTES,
    ) {
        Ok(response) => return Ok(EncodedRpcResponse::Reply(response)),
        Err(error @ HubError::ResourceLimit { .. }) => {
            let fallback = RpcError::new(
                id.clone(),
                RESOURCE_LIMIT_ERROR,
                "resource limit exceeded",
                typed_hub_error_data(&error),
            );
            if let Ok(response) = encode_retained_response(
                &fallback,
                Arc::clone(&byte_admission.fallbacks),
                MAX_RPC_FALLBACK_FRAME_BYTES,
            ) {
                return Ok(EncodedRpcResponse::Reply(response));
            }
        }
        Err(_) => {}
    }

    let fallback = RpcError::new(id, INTERNAL_ERROR, "RPC response too large", None);
    if let Ok(response) = encode_retained_response(
        &fallback,
        Arc::clone(&byte_admission.fallbacks),
        MAX_RPC_FALLBACK_FRAME_BYTES,
    ) {
        return Ok(EncodedRpcResponse::Reply(response));
    }

    let terminal = RpcError::new(
        Value::Null,
        INTERNAL_ERROR,
        "RPC response too large; connection closing",
        None,
    );
    encode_retained_response(
        &terminal,
        Arc::clone(&byte_admission.fallbacks),
        MAX_RPC_FALLBACK_FRAME_BYTES,
    )
    .map(EncodedRpcResponse::Terminal)
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod protocol_tests;
