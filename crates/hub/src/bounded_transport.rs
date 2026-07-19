//! Resource-bounded ACP transports.
//!
//! The upstream SDK's typed handlers only see a message after transport
//! framing and JSON deserialization. These adapters enforce a byte ceiling
//! before that allocation boundary for stdio lines, HTTP bodies/SSE events,
//! and WebSocket messages.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex as StdMutex};

use agent_client_protocol::schema::v1::{RequestId, Response as RpcResponse};
use agent_client_protocol::{
    AcpAgent, Agent, Channel, Client, ConnectTo, Error as AcpError, Lines, RawJsonRpcMessage, Role,
};
use async_process::Child;
use async_tungstenite::tungstenite::{
    Error as WsError, Message as WsMessage, client::IntoClientRequest, protocol::WebSocketConfig,
};
use futures::channel::mpsc::{self, Sender};
use futures::future::{BoxFuture, FutureExt};
use futures::stream::FuturesUnordered;
use futures::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, SinkExt, Stream,
    StreamExt, pin_mut,
};

use parking_lot::Mutex as ParkingMutex;

use crate::error::HubError;

/// Maximum serialized ACP JSON-RPC message accepted from or sent to an
/// endpoint. This matches the daemon RPC ceiling while remaining above the
/// tighter per-callback and per-page budgets.
pub(crate) const MAX_ACP_FRAME_BYTES: usize = 32 * 1024 * 1024;

const HEADER_CONNECTION_ID: &str = "acp-connection-id";
const HEADER_SESSION_ID: &str = "acp-session-id";
const SSE_QUEUE_DEPTH: usize = 64;
const MAX_SSE_STREAMS: usize = 64;
const MAX_OUTSTANDING_INBOUND_REQUESTS: usize = 8;
const MAX_OUTSTANDING_INBOUND_FRAMES: usize = 4096;
const MAX_OUTSTANDING_INBOUND_BYTES: usize = 32 * 1024 * 1024;
// These ceilings cover queued bodies only. Each ordering lane may additionally
// retain one in-flight body, individually capped by MAX_ACP_FRAME_BYTES.
const MAX_PENDING_HTTP_POST_FRAMES: usize = 64;
const MAX_PENDING_HTTP_POST_BYTES: usize = MAX_ACP_FRAME_BYTES;

type SharedFlowBudget = Arc<StdMutex<FlowBudget>>;

#[derive(Debug, Default)]
struct FlowBudget {
    frames: usize,
    bytes: usize,
    partial_bytes: usize,
    requests: HashMap<RequestId, usize>,
    notifications: VecDeque<(String, usize)>,
    responses: VecDeque<(RequestId, usize)>,
    logical_ack: bool,
}

impl FlowBudget {
    fn track(&mut self, message: &RawJsonRpcMessage, bytes: usize) -> Result<(), String> {
        let next_frames = self.frames.saturating_add(1);
        let next_bytes = self.bytes.saturating_add(bytes);
        if next_frames > MAX_OUTSTANDING_INBOUND_FRAMES
            || next_bytes.saturating_add(self.partial_bytes) > MAX_OUTSTANDING_INBOUND_BYTES
        {
            return Err(format!(
                "inbound ACP flow exceeds {MAX_OUTSTANDING_INBOUND_FRAMES} outstanding \
                 frames or {MAX_OUTSTANDING_INBOUND_BYTES} outstanding bytes"
            ));
        }
        match message {
            RawJsonRpcMessage::Request(request) => {
                if self.requests.len() >= MAX_OUTSTANDING_INBOUND_REQUESTS {
                    return Err(format!(
                        "inbound ACP flow exceeds {MAX_OUTSTANDING_INBOUND_REQUESTS} \
                         outstanding requests"
                    ));
                }
                if self.requests.contains_key(&request.id) {
                    return Err("duplicate outstanding inbound ACP request id".to_string());
                }
                self.requests.insert(request.id.clone(), bytes);
            }
            RawJsonRpcMessage::Notification(notification) => {
                self.notifications
                    .push_back((notification.method.to_string(), bytes));
            }
            RawJsonRpcMessage::Response(response) => {
                let id = match response {
                    RpcResponse::Result { id, .. } | RpcResponse::Error { id, .. } => id,
                };
                if self.responses.iter().any(|(pending, _)| pending == id) {
                    return Err("duplicate outstanding inbound ACP response id".to_string());
                }
                self.responses.push_back((id.clone(), bytes));
            }
        }
        self.frames = next_frames;
        self.bytes = next_bytes;
        Ok(())
    }

    fn acknowledge_notification(&mut self, method: &str) {
        let position = if self.logical_ack {
            (!self.notifications.is_empty()).then_some(0)
        } else {
            self.notifications
                .iter()
                .position(|(pending, _)| pending == method)
        };
        if let Some(bytes) = position
            .and_then(|position| self.notifications.remove(position))
            .map(|(_, bytes)| bytes)
        {
            self.release(bytes);
        }
    }

    fn acknowledge_response(&mut self, id: &RequestId) {
        let position = if self.logical_ack {
            (!self.responses.is_empty()).then_some(0)
        } else {
            self.responses.iter().position(|(pending, _)| pending == id)
        };
        if let Some(bytes) = position
            .and_then(|position| self.responses.remove(position))
            .map(|(_, bytes)| bytes)
        {
            self.release(bytes);
        }
    }

    fn complete_request(&mut self, id: &RequestId) {
        if let Some(bytes) = self.requests.remove(id) {
            self.release(bytes);
        }
    }

    fn release(&mut self, bytes: usize) {
        self.frames = self.frames.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    fn reserve_partial(&mut self, bytes: usize) -> Result<(), String> {
        let next = self.partial_bytes.saturating_add(bytes);
        if self.bytes.saturating_add(next) > MAX_OUTSTANDING_INBOUND_BYTES {
            return Err(format!(
                "inbound ACP partial framing exceeds {MAX_OUTSTANDING_INBOUND_BYTES} bytes"
            ));
        }
        self.partial_bytes = next;
        Ok(())
    }

    fn release_partial(&mut self, bytes: usize) {
        self.partial_bytes = self.partial_bytes.saturating_sub(bytes);
    }

    fn allow_logical_ack(&mut self) {
        self.logical_ack = true;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InboundFlowControl {
    inner: SharedFlowBudget,
}

impl InboundFlowControl {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(StdMutex::new(FlowBudget::default())),
        }
    }

    fn track(&self, message: &RawJsonRpcMessage, bytes: usize) -> Result<(), String> {
        self.inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?
            .track(message, bytes)
    }

    pub(crate) fn acknowledge_notification(&self, method: &str) -> Result<(), AcpError> {
        self.inner
            .lock()
            .map_err(|_| AcpError::internal_error().data("ACP flow-budget mutex poisoned"))?
            .acknowledge_notification(method);
        Ok(())
    }

    pub(crate) fn acknowledge_response(&self, id: serde_json::Value) -> Result<(), AcpError> {
        let id = serde_json::from_value(id)
            .map_err(|error| AcpError::internal_error().data(error.to_string()))?;
        self.inner
            .lock()
            .map_err(|_| AcpError::internal_error().data("ACP flow-budget mutex poisoned"))?
            .acknowledge_response(&id);
        Ok(())
    }

    pub(crate) fn allow_logical_ack(&self) {
        if let Ok(mut flow) = self.inner.lock() {
            flow.allow_logical_ack();
        }
    }

    fn complete_outbound_response(&self, message: &RawJsonRpcMessage) -> Result<(), String> {
        let RawJsonRpcMessage::Response(response) = message else {
            return Ok(());
        };
        let id = match response {
            RpcResponse::Result { id, .. } | RpcResponse::Error { id, .. } => id,
        };
        self.inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?
            .complete_request(id);
        Ok(())
    }

    fn complete_request_id(&self, id: &RequestId) -> Result<(), String> {
        self.inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?
            .complete_request(id);
        Ok(())
    }

    fn reserve_partial(&self, bytes: usize) -> Result<(), String> {
        self.inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?
            .reserve_partial(bytes)
    }

    fn release_partial(&self, bytes: usize) {
        if let Ok(mut flow) = self.inner.lock() {
            flow.release_partial(bytes);
        }
    }

    fn track_from_partial(&self, message: &RawJsonRpcMessage, bytes: usize) -> Result<(), String> {
        let mut flow = self
            .inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?;
        flow.release_partial(bytes);
        flow.track(message, bytes)
    }
}

fn charge_flow(
    flow: &InboundFlowControl,
    message: &RawJsonRpcMessage,
    bytes: usize,
) -> Result<(), String> {
    flow.track(message, bytes)
}

fn complete_outbound_response(
    flow: &InboundFlowControl,
    message: &RawJsonRpcMessage,
) -> Result<(), String> {
    flow.complete_outbound_response(message)
}

#[derive(Debug)]
pub(crate) struct BoundedStdioAgent {
    inner: AcpAgent,
    flow: InboundFlowControl,
}

impl BoundedStdioAgent {
    pub(crate) fn with_flow(inner: AcpAgent, flow: InboundFlowControl) -> Self {
        Self { inner, flow }
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.0.status().await
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        drop(self.0.kill());
    }
}

impl<Counterpart: Role> ConnectTo<Counterpart> for BoundedStdioAgent {
    async fn connect_to(
        self,
        client: impl ConnectTo<Counterpart::Counterpart>,
    ) -> Result<(), AcpError> {
        let (child_stdin, child_stdout, child_stderr, child) = self.inner.spawn_process()?;

        let flow = self.flow;
        let incoming = bounded_line_stream(child_stdout, flow.clone());
        let outgoing = futures::sink::unfold(
            (child_stdin, flow),
            async move |(mut writer, flow), line: String| {
                if line.len().saturating_add(1) > MAX_ACP_FRAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("outbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes"),
                    ));
                }
                let message: RawJsonRpcMessage = serde_json::from_str(&line).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("malformed outbound ACP JSON-RPC frame: {error}"),
                    )
                })?;
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                complete_outbound_response(&flow, &message).map_err(io::Error::other)?;
                Ok::<_, io::Error>((writer, flow))
            },
        );

        let protocol = ConnectTo::<Counterpart>::connect_to(Lines::new(outgoing, incoming), client);
        let monitor = async move {
            let mut guard = ChildGuard(child);
            let status = guard.wait().await.map_err(|error| {
                AcpError::internal_error().data(format!("failed to wait for ACP process: {error}"))
            })?;
            if status.success() {
                Ok(())
            } else {
                Err(AcpError::internal_error().data(format!("ACP process exited with {status}")))
            }
        };
        let stderr = drain_stderr(child_stderr);

        let protocol = pin!(protocol);
        let monitor = pin!(monitor);
        let main_race = async {
            match futures::future::select(protocol, monitor).await {
                futures::future::Either::Left((result, _))
                | futures::future::Either::Right((result, _)) => result,
            }
        };
        let main_race = pin!(main_race);
        let stderr = pin!(stderr);
        match futures::future::select(main_race, stderr).await {
            futures::future::Either::Left((result, _)) => result,
            futures::future::Either::Right((result, main)) => {
                result.map_err(|error| {
                    AcpError::internal_error().data(format!("failed to drain ACP stderr: {error}"))
                })?;
                main.await
            }
        }
    }
}

fn bounded_line_stream<R>(
    reader: R,
    flow: InboundFlowControl,
) -> Pin<Box<dyn Stream<Item = io::Result<String>> + Send + 'static>>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    let reader = futures::io::BufReader::new(reader);
    Box::pin(futures::stream::try_unfold(
        (reader, flow),
        |(mut reader, flow)| async move {
            match read_bounded_line(&mut reader).await? {
                Some(line) => {
                    let message: RawJsonRpcMessage =
                        serde_json::from_str(&line).map_err(|error| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("malformed inbound ACP JSON-RPC frame: {error}"),
                            )
                        })?;
                    charge_flow(&flow, &message, line.len().saturating_add(1))
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    Ok(Some((line, (reader, flow))))
                }
                None => Ok(None),
            }
        },
    ))
}

async fn read_bounded_line<R>(reader: &mut R) -> io::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            if bytes.len().saturating_add(newline) > MAX_ACP_FRAME_BYTES {
                return Err(frame_too_large());
            }
            bytes.extend_from_slice(&buffer[..newline]);
            reader.consume_unpin(newline + 1);
            break;
        }
        if bytes.len().saturating_add(buffer.len()) > MAX_ACP_FRAME_BYTES {
            return Err(frame_too_large());
        }
        let consumed = buffer.len();
        bytes.extend_from_slice(buffer);
        reader.consume_unpin(consumed);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn frame_too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("inbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes"),
    )
}

async fn drain_stderr<R>(mut reader: R) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
    }
}

#[derive(Clone, Debug)]
struct HttpConnection {
    endpoint: reqwest::Url,
    http: reqwest::Client,
    connection_id: Arc<StdMutex<Option<String>>>,
    flow: InboundFlowControl,
}

impl HttpConnection {
    fn new(endpoint: reqwest::Url, http: reqwest::Client, flow: InboundFlowControl) -> Self {
        Self {
            endpoint,
            http,
            connection_id: Arc::new(StdMutex::new(None)),
            flow,
        }
    }

    fn connection_id(&self) -> Option<String> {
        self.connection_id.lock().expect("mutex poisoned").clone()
    }

    fn set_connection_id(&self, connection_id: String) {
        *self.connection_id.lock().expect("mutex poisoned") = Some(connection_id);
    }

    fn take_connection_id(&self) -> Option<String> {
        self.connection_id.lock().expect("mutex poisoned").take()
    }

    fn charge_inbound(&self, message: &RawJsonRpcMessage, bytes: usize) -> Result<(), String> {
        charge_flow(&self.flow, message, bytes)
    }

    fn complete_request_id(&self, id: &RequestId) -> Result<(), String> {
        self.flow.complete_request_id(id)
    }

    async fn close(&self) {
        let Some(connection_id) = self.take_connection_id() else {
            return;
        };
        let _ = self
            .http
            .delete(self.endpoint.clone())
            .header(HEADER_CONNECTION_ID, connection_id)
            .send()
            .await;
    }
}

/// HTTP/WebSocket ACP component with a pre-deserialization frame ceiling.
#[derive(Debug)]
pub(crate) struct BoundedHttpAgent {
    endpoint: reqwest::Url,
    http: reqwest::Client,
    headers: BTreeMap<String, String>,
    flow: InboundFlowControl,
}

impl BoundedHttpAgent {
    #[cfg(test)]
    pub(crate) fn new(url: &str, headers: &BTreeMap<String, String>) -> Result<Self, HubError> {
        Self::with_flow(url, headers, InboundFlowControl::new())
    }

    pub(crate) fn with_flow(
        url: &str,
        headers: &BTreeMap<String, String>,
        flow: InboundFlowControl,
    ) -> Result<Self, HubError> {
        let mut endpoint = reqwest::Url::parse(url)
            .map_err(|error| HubError::other(format!("invalid ACP endpoint URL: {error}")))?;
        let path = endpoint.path().trim_end_matches('/').to_string();
        let path = if path.is_empty() {
            "/acp".to_string()
        } else if path.ends_with("/acp") {
            path
        } else {
            format!("{path}/acp")
        };
        endpoint.set_path(&path);

        let mut header_map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| HubError::other(format!("invalid header name: {error}")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|error| HubError::other(format!("invalid header value: {error}")))?;
            header_map.append(name, value);
        }
        let http = reqwest::Client::builder()
            .default_headers(header_map)
            .build()
            .map_err(|error| HubError::other(format!("reqwest build: {error}")))?;
        Ok(Self {
            endpoint,
            http,
            headers: headers.clone(),
            flow,
        })
    }

    fn is_websocket(&self) -> bool {
        matches!(self.endpoint.scheme(), "ws" | "wss")
    }
}

impl ConnectTo<Client> for BoundedHttpAgent {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> Result<(), AcpError> {
        let (channel, transport) = ConnectTo::<Client>::into_channel_and_future(self);
        match futures::future::select(pin!(client.connect_to(channel)), pin!(transport)).await {
            futures::future::Either::Left((result, _))
            | futures::future::Either::Right((result, _)) => result,
        }
    }

    fn into_channel_and_future(self) -> (Channel, BoxFuture<'static, Result<(), AcpError>>) {
        let (caller, transport) = Channel::duplex();
        let future = if self.is_websocket() {
            run_websocket(self, transport).boxed()
        } else {
            run_http(self, transport).boxed()
        };
        (caller, future)
    }
}

struct ClientState {
    connection: HttpConnection,
    open_session_streams: HashSet<String>,
    pending_requests: HashMap<RequestId, String>,
    incoming: futures::channel::mpsc::UnboundedSender<Result<RawJsonRpcMessage, AcpError>>,
}

impl ClientState {
    async fn initialize(&self, message: RawJsonRpcMessage) -> Result<bool, String> {
        let body = serialize_bounded(&message)?;
        let response = self
            .connection
            .http
            .post(self.connection.endpoint.clone())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body)
            .send()
            .await
            .map_err(sanitized_reqwest_error)?;
        let status = response.status();
        let connection_id = response
            .headers()
            .get(HEADER_CONNECTION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = bounded_response_bytes(response).await?;
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        let message: RawJsonRpcMessage = serde_json::from_slice(&body)
            .map_err(|error| format!("malformed initialize response: {error}"))?;
        self.connection.charge_inbound(&message, body.len())?;
        let rejected = matches!(
            message,
            RawJsonRpcMessage::Response(RpcResponse::Error { .. })
        );
        self.deliver(message);
        if rejected {
            self.connection.close().await;
            return Ok(false);
        }
        let connection_id =
            connection_id.ok_or_else(|| format!("missing {HEADER_CONNECTION_ID} header"))?;
        self.connection.set_connection_id(connection_id);
        Ok(true)
    }

    fn prepare_post(
        &mut self,
        message: RawJsonRpcMessage,
        session_id: Option<&str>,
    ) -> Result<PendingPost, String> {
        if let Some(method) = method_for_message(&message)
            && method_requires_session_header(method)
            && session_id.is_none()
        {
            return Err(format!("method {method:?} requires sessionId"));
        }
        let connection_id = self
            .connection
            .connection_id()
            .ok_or_else(|| "POST attempted before initialize".to_string())?;
        let body = serialize_bounded(&message)?;
        let body_bytes = body.len();
        let mut request = self
            .connection
            .http
            .post(self.connection.endpoint.clone())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header(HEADER_CONNECTION_ID, connection_id)
            .body(body);
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id);
        }

        let pending_request = pending_request_for_message(&message);
        if let Some((id, method)) = &pending_request {
            self.pending_requests.insert(id.clone(), method.clone());
        }
        let completed_request = message.response_id().cloned();
        let connection = self.connection.clone();
        let response = async move {
            let response = request.send().await.map_err(sanitized_reqwest_error)?;
            if let Some(id) = &completed_request {
                connection.complete_request_id(id)?;
            }
            let status = response.status();
            if status.as_u16() != 202 && !status.is_success() {
                let _ = bounded_response_bytes(response).await?;
                return Err(format!("HTTP {status}"));
            }
            Ok(())
        };
        Ok(PendingPost {
            body_bytes,
            pending_request,
            response: response.boxed(),
        })
    }

    fn remove_pending(&mut self, pending: Option<&(RequestId, String)>) {
        if let Some((id, _)) = pending {
            self.pending_requests.remove(id);
        }
    }

    fn session_to_open_for_response(&mut self, message: &RawJsonRpcMessage) -> Option<String> {
        let id = message.response_id().and_then(pending_request_key)?;
        let method = self.pending_requests.remove(&id)?;
        let RawJsonRpcMessage::Response(RpcResponse::Result { result, .. }) = message else {
            return None;
        };
        if !matches!(method.as_str(), "session/new" | "session/fork") {
            return None;
        }
        let session_id = result.get("sessionId")?.as_str()?.to_owned();
        self.open_session_streams
            .insert(session_id.clone())
            .then_some(session_id)
    }

    fn deliver(&self, message: RawJsonRpcMessage) {
        let _ = self.incoming.unbounded_send(Ok(message));
    }
}

struct PendingPost {
    body_bytes: usize,
    pending_request: Option<(RequestId, String)>,
    response: BoxFuture<'static, Result<(), String>>,
}

struct CompletedPost {
    pending_request: Option<(RequestId, String)>,
    result: Result<(), String>,
}

type SharedPostBudget = Arc<ParkingMutex<PostBudget>>;

#[derive(Debug, Default)]
struct PostBudget {
    frames: usize,
    bytes: usize,
}

impl PostBudget {
    fn reserve(shared: &SharedPostBudget, bytes: usize) -> Result<PostReservation, String> {
        validate_post_frame(bytes)?;
        let mut budget = shared.lock();
        let next_frames = budget.frames.checked_add(1).ok_or_else(post_queue_full)?;
        let next_bytes = budget
            .bytes
            .checked_add(bytes)
            .ok_or_else(post_queue_full)?;
        if next_frames > MAX_PENDING_HTTP_POST_FRAMES || next_bytes > MAX_PENDING_HTTP_POST_BYTES {
            return Err(post_queue_full());
        }
        budget.frames = next_frames;
        budget.bytes = next_bytes;
        drop(budget);
        Ok(PostReservation {
            budget: shared.clone(),
            bytes,
        })
    }
}
fn validate_post_frame(bytes: usize) -> Result<(), String> {
    if bytes > MAX_PENDING_HTTP_POST_BYTES {
        return Err(format!(
            "outbound ACP POST frame exceeds {MAX_PENDING_HTTP_POST_BYTES} bytes"
        ));
    }
    Ok(())
}

fn post_queue_full() -> String {
    format!(
        "HTTP ACP pending POST queue exceeds {MAX_PENDING_HTTP_POST_FRAMES} frames or \
         {MAX_PENDING_HTTP_POST_BYTES} bytes"
    )
}

struct PostReservation {
    budget: SharedPostBudget,
    bytes: usize,
}

impl Drop for PostReservation {
    fn drop(&mut self) {
        let mut budget = self.budget.lock();
        budget.frames = budget.frames.saturating_sub(1);
        budget.bytes = budget.bytes.saturating_sub(self.bytes);
    }
}

struct ReservedPost {
    post: PendingPost,
    reservation: PostReservation,
}

struct PostQueue {
    budget: SharedPostBudget,
    queued: VecDeque<ReservedPost>,
    in_flight: Option<BoxFuture<'static, CompletedPost>>,
}

impl Default for PostQueue {
    fn default() -> Self {
        Self::with_budget(Arc::new(ParkingMutex::new(PostBudget::default())))
    }
}

impl PostQueue {
    fn with_budget(budget: SharedPostBudget) -> Self {
        Self {
            budget,
            queued: VecDeque::new(),
            in_flight: None,
        }
    }

    fn push(&mut self, post: PendingPost) -> Result<(), String> {
        validate_post_frame(post.body_bytes)?;
        if self.in_flight.is_none() {
            self.start_in_flight(post);
            return Ok(());
        }
        let reservation = PostBudget::reserve(&self.budget, post.body_bytes)?;
        self.queued.push_back(ReservedPost { post, reservation });
        Ok(())
    }

    fn start_next(&mut self) {
        if self.in_flight.is_none()
            && let Some(ReservedPost { post, reservation }) = self.queued.pop_front()
        {
            drop(reservation);
            self.start_in_flight(post);
        }
    }

    fn start_in_flight(&mut self, post: PendingPost) {
        debug_assert!(self.in_flight.is_none());
        self.in_flight = Some(
            async move {
                CompletedPost {
                    pending_request: post.pending_request,
                    result: post.response.await,
                }
            }
            .boxed(),
        );
    }

    async fn next_completion(&mut self) -> CompletedPost {
        loop {
            self.start_next();
            if let Some(future) = self.in_flight.as_mut() {
                let completed = future.await;
                self.in_flight = None;
                return completed;
            }
            futures::future::pending::<()>().await;
        }
    }

    fn close(&mut self) {
        self.queued.clear();
        self.in_flight = None;
    }
}

struct SseMessage {
    message: RawJsonRpcMessage,
}

struct SseFailure(String);

enum HttpLoopEvent {
    Outgoing(Option<Result<RawJsonRpcMessage, AcpError>>),
    Sse(Option<SseMessage>),
    SseFailure(SseFailure),
    OrderedPost(CompletedPost),
    ResponsePost(CompletedPost),
}

#[derive(Default)]
struct SseTasks {
    tasks: FuturesUnordered<BoxFuture<'static, SseFailure>>,
}

impl SseTasks {
    fn start(
        &mut self,
        connection: HttpConnection,
        session_id: Option<String>,
        sender: Sender<SseMessage>,
    ) -> Result<(), String> {
        if self.tasks.len() >= MAX_SSE_STREAMS {
            return Err(format!(
                "HTTP ACP connection exceeds {MAX_SSE_STREAMS} concurrent SSE streams"
            ));
        }
        self.tasks.push(
            async move {
                let error = read_sse(connection, session_id, sender)
                    .await
                    .err()
                    .unwrap_or_else(|| "SSE stream closed".to_string());
                SseFailure(error)
            }
            .boxed(),
        );
        Ok(())
    }

    async fn next_failure(&mut self) -> SseFailure {
        loop {
            if let Some(failure) = self.tasks.next().await {
                return failure;
            }
            futures::future::pending::<()>().await;
        }
    }
}

async fn run_http(client: BoundedHttpAgent, channel: Channel) -> Result<(), AcpError> {
    let BoundedHttpAgent {
        endpoint,
        http,
        flow,
        ..
    } = client;
    let Channel {
        rx: mut outgoing,
        tx: incoming,
    } = channel;
    let connection = HttpConnection::new(endpoint, http, flow);
    let mut state = ClientState {
        connection: connection.clone(),
        open_session_streams: HashSet::new(),
        pending_requests: HashMap::new(),
        incoming,
    };
    let (event_tx, mut event_rx) = mpsc::channel(SSE_QUEUE_DEPTH);
    let mut sse = SseTasks::default();
    let post_budget = Arc::new(ParkingMutex::new(PostBudget::default()));
    let mut ordered_posts = PostQueue::with_budget(post_budget.clone());
    let mut response_posts = PostQueue::with_budget(post_budget);

    let result = loop {
        let event = {
            let outgoing_next = outgoing.next().fuse();
            let event_next = event_rx.next().fuse();
            let sse_failure_next = sse.next_failure().fuse();
            let ordered_next = ordered_posts.next_completion().fuse();
            let response_next = response_posts.next_completion().fuse();
            pin_mut!(
                outgoing_next,
                event_next,
                sse_failure_next,
                ordered_next,
                response_next
            );
            futures::select! {
                outbound = outgoing_next => HttpLoopEvent::Outgoing(outbound),
                event = event_next => HttpLoopEvent::Sse(event),
                failure = sse_failure_next => HttpLoopEvent::SseFailure(failure),
                completed = ordered_next => HttpLoopEvent::OrderedPost(completed),
                completed = response_next => HttpLoopEvent::ResponsePost(completed),
            }
        };

        match event {
            HttpLoopEvent::Outgoing(outbound) => {
                let Some(outbound) = outbound else {
                    break Ok(());
                };
                let message = outbound?;
                if state.connection.connection_id().is_none() {
                    if !is_initialize_request(&message) {
                        break Err(AcpError::invalid_request()
                            .data("first HTTP ACP message must be initialize"));
                    }
                    match state.initialize(message).await {
                        Ok(true) => {
                            if let Err(error) =
                                sse.start(connection.clone(), None, event_tx.clone())
                            {
                                break Err(AcpError::internal_error().data(error));
                            }
                        }
                        Ok(false) => {}
                        Err(error) => break Err(AcpError::internal_error().data(error)),
                    }
                    continue;
                }
                let session_id = session_id_from_message(&message);
                if let Some(session_id) = &session_id
                    && state.open_session_streams.insert(session_id.clone())
                    && let Err(error) = sse.start(
                        connection.clone(),
                        Some(session_id.clone()),
                        event_tx.clone(),
                    )
                {
                    break Err(AcpError::internal_error().data(error));
                }
                let is_response = matches!(message, RawJsonRpcMessage::Response(_));
                let queued = match state.prepare_post(message, session_id.as_deref()) {
                    Ok(post) if is_response => response_posts.push(post),
                    Ok(post) => ordered_posts.push(post),
                    Err(error) => Err(error),
                };
                if let Err(error) = queued {
                    break Err(AcpError::internal_error().data(error));
                }
            }
            HttpLoopEvent::Sse(event) => {
                let Some(event) = event else {
                    continue;
                };
                let session_id = state.session_to_open_for_response(&event.message);
                state.deliver(event.message);
                if let Some(session_id) = session_id
                    && let Err(error) =
                        sse.start(connection.clone(), Some(session_id), event_tx.clone())
                {
                    break Err(AcpError::internal_error().data(error));
                }
            }
            HttpLoopEvent::SseFailure(failure) => {
                break Err(AcpError::internal_error().data(failure.0));
            }
            HttpLoopEvent::OrderedPost(completed) | HttpLoopEvent::ResponsePost(completed) => {
                if let Err(error) = completed.result {
                    state.remove_pending(completed.pending_request.as_ref());
                    break Err(AcpError::internal_error().data(error));
                }
            }
        }
    };

    ordered_posts.close();
    response_posts.close();
    connection.close().await;
    result
}

fn sanitized_reqwest_error(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

async fn bounded_response_bytes(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ACP_FRAME_BYTES as u64)
    {
        return Err(format!("HTTP ACP body exceeds {MAX_ACP_FRAME_BYTES} bytes"));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(sanitized_reqwest_error)?;
        if body.len().saturating_add(chunk.len()) > MAX_ACP_FRAME_BYTES {
            return Err(format!("HTTP ACP body exceeds {MAX_ACP_FRAME_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn serialize_bounded(message: &RawJsonRpcMessage) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_ACP_FRAME_BYTES {
        return Err(format!(
            "outbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
}

impl SseDecoder {
    fn buffered_len(&self) -> usize {
        self.line.len().saturating_add(self.data.len())
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let mut events = Vec::new();
        for byte in chunk {
            if *byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.consume_line(&line, &mut events)?;
            } else {
                if self.line.len() >= MAX_ACP_FRAME_BYTES {
                    return Err(format!("SSE line exceeds {MAX_ACP_FRAME_BYTES} bytes"));
                }
                self.line.push(*byte);
            }
        }
        Ok(events)
    }

    fn consume_line(&mut self, line: &[u8], events: &mut Vec<Vec<u8>>) -> Result<(), String> {
        if line.is_empty() {
            if !self.data.is_empty() {
                if self.data.last() == Some(&b'\n') {
                    self.data.pop();
                }
                events.push(std::mem::take(&mut self.data));
            }
            return Ok(());
        }
        let Some(value) = line.strip_prefix(b"data:") else {
            return Ok(());
        };
        let value = value.strip_prefix(b" ").unwrap_or(value);
        if self
            .data
            .len()
            .saturating_add(value.len())
            .saturating_add(1)
            > MAX_ACP_FRAME_BYTES
        {
            return Err(format!("SSE event exceeds {MAX_ACP_FRAME_BYTES} bytes"));
        }
        self.data.extend_from_slice(value);
        self.data.push(b'\n');
        Ok(())
    }
}

struct PartialReservation {
    flow: InboundFlowControl,
    bytes: usize,
}

impl PartialReservation {
    fn new(flow: InboundFlowControl) -> Self {
        Self { flow, bytes: 0 }
    }

    fn add(&mut self, bytes: usize) -> Result<(), String> {
        self.flow.reserve_partial(bytes)?;
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }

    fn retain(&mut self, bytes: usize) {
        if bytes < self.bytes {
            self.flow.release_partial(self.bytes - bytes);
        }
        self.bytes = bytes;
    }

    fn transfer_message(
        &mut self,
        message: &RawJsonRpcMessage,
        bytes: usize,
    ) -> Result<(), String> {
        self.bytes = self.bytes.saturating_sub(bytes);
        self.flow.track_from_partial(message, bytes)
    }
}

impl Drop for PartialReservation {
    fn drop(&mut self) {
        self.flow.release_partial(self.bytes);
    }
}

async fn read_sse(
    connection: HttpConnection,
    session_id: Option<String>,
    mut sender: Sender<SseMessage>,
) -> Result<(), String> {
    let connection_id = connection
        .connection_id()
        .ok_or_else(|| "SSE attempted before initialize".to_string())?;
    let mut request = connection
        .http
        .get(connection.endpoint.clone())
        .header("Accept", "text/event-stream")
        .header(HEADER_CONNECTION_ID, connection_id);
    if let Some(session_id) = &session_id {
        request = request.header(HEADER_SESSION_ID, session_id);
    }
    let response = request.send().await.map_err(sanitized_reqwest_error)?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut partial = PartialReservation::new(connection.flow.clone());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(sanitized_reqwest_error)?;
        partial.add(chunk.len())?;
        let payloads = decoder.push(&chunk)?;
        let retained = decoder.buffered_len().saturating_add(
            payloads
                .iter()
                .map(Vec::len)
                .fold(0_usize, usize::saturating_add),
        );
        partial.retain(retained);
        for payload in payloads {
            if payload.is_empty() {
                continue;
            }
            let message = serde_json::from_slice(&payload)
                .map_err(|error| format!("malformed SSE JSON-RPC payload: {error}"))?;
            partial.transfer_message(&message, payload.len())?;
            sender
                .send(SseMessage { message })
                .await
                .map_err(|_| "upstream channel closed".to_string())?;
        }
    }
    Ok(())
}

fn sanitized_websocket_error(context: &'static str, error: &WsError) -> AcpError {
    let cause = match error {
        WsError::ConnectionClosed => "connection closed".to_string(),
        WsError::AlreadyClosed => "connection already closed".to_string(),
        WsError::Io(error) => format!("I/O failure ({:?})", error.kind()),
        WsError::Tls(_) => "TLS failure".to_string(),
        WsError::Capacity(_) => "capacity limit exceeded".to_string(),
        WsError::Protocol(_) => "protocol violation".to_string(),
        WsError::WriteBufferFull(_) => "write buffer full".to_string(),
        WsError::Utf8(_) => "UTF-8 failure".to_string(),
        WsError::AttackAttempt => "attack attempt detected".to_string(),
        WsError::Url(_) => "invalid URL".to_string(),
        WsError::Http(response) => {
            format!("HTTP handshake rejected ({})", response.status())
        }
        WsError::HttpFormat(_) => "invalid HTTP handshake".to_string(),
    };
    AcpError::internal_error().data(format!("WebSocket {context} failed: {cause}"))
}

async fn run_websocket(client: BoundedHttpAgent, channel: Channel) -> Result<(), AcpError> {
    let Channel {
        rx: mut outgoing,
        tx: incoming,
    } = channel;
    let flow = client.flow.clone();
    let mut request = client
        .endpoint
        .as_str()
        .into_client_request()
        .map_err(|error| sanitized_websocket_error("request construction", &error))?;
    for (name, value) in &client.headers {
        let name = async_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| AcpError::internal_error().data("invalid WebSocket header name"))?;
        let value = async_tungstenite::tungstenite::http::HeaderValue::from_str(value)
            .map_err(|_| AcpError::internal_error().data("invalid WebSocket header value"))?;
        request.headers_mut().append(name, value);
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_ACP_FRAME_BYTES))
        .max_frame_size(Some(MAX_ACP_FRAME_BYTES));
    let (stream, _) = async_tungstenite::tokio::connect_async_with_config(request, Some(config))
        .await
        .map_err(|error| sanitized_websocket_error("connect", &error))?;
    let (mut writer, mut reader) = stream.split();

    loop {
        let outbound = outgoing.next().fuse();
        let inbound = reader.next().fuse();
        pin_mut!(outbound, inbound);
        futures::select! {
            outbound = outbound => match outbound {
                Some(Ok(message)) => {
                    let text = serde_json::to_string(&message)
                        .map_err(|error| AcpError::internal_error().data(error.to_string()))?;
                    if text.len() > MAX_ACP_FRAME_BYTES {
                        return Err(AcpError::internal_error()
                            .data(format!("outbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes")));
                    }
                    writer.send(WsMessage::Text(text.into())).await
                        .map_err(|error| sanitized_websocket_error("send", &error))?;
                    complete_outbound_response(&flow, &message)
                        .map_err(|error| AcpError::internal_error().data(error))?;
                }
                Some(Err(error)) => return Err(error),
                None => break,
            },
            inbound = inbound => match inbound {
                Some(Ok(WsMessage::Text(text))) => {
                    let message = serde_json::from_str(text.as_str())
                        .map_err(|error| AcpError::parse_error().data(error.to_string()))?;
                    charge_flow(&flow, &message, text.len())
                        .map_err(|error| AcpError::invalid_request().data(error))?;
                    if incoming.unbounded_send(Ok(message)).is_err() {
                        break;
                    }
                }
                Some(Ok(WsMessage::Binary(_))) => {
                    return Err(AcpError::invalid_request()
                        .data("ACP WebSocket transport requires text frames"));
                }
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                Some(Ok(WsMessage::Close(frame))) => {
                    let cause = frame
                        .as_ref()
                        .map(|frame| format!(" ({:?})", frame.code))
                        .unwrap_or_default();
                    return Err(AcpError::internal_error()
                        .data(format!("WebSocket closed by peer{cause}")));
                }
                Some(Err(error)) => {
                    return Err(sanitized_websocket_error("receive", &error));
                }
                None => return Err(AcpError::internal_error().data("WebSocket stream ended")),
            },
        }
    }
    drop(writer.send(WsMessage::Close(None)).await);
    Ok(())
}

fn is_initialize_request(message: &RawJsonRpcMessage) -> bool {
    matches!(
        message,
        RawJsonRpcMessage::Request(request) if request.method.as_ref() == "initialize"
    )
}

fn method_for_message(message: &RawJsonRpcMessage) -> Option<&str> {
    match message {
        RawJsonRpcMessage::Request(request) => Some(request.method.as_ref()),
        RawJsonRpcMessage::Notification(notification) => Some(notification.method.as_ref()),
        RawJsonRpcMessage::Response(_) => None,
    }
}

fn method_requires_session_header(method: &str) -> bool {
    matches!(
        method,
        "session/prompt"
            | "session/cancel"
            | "session/close"
            | "session/delete"
            | "session/fork"
            | "session/load"
            | "session/resume"
            | "session/set_config_option"
            | "session/set_mode"
            | "session/set_model"
    )
}

fn session_id_from_message(message: &RawJsonRpcMessage) -> Option<String> {
    let params = match message {
        RawJsonRpcMessage::Request(request) => request.params.as_ref(),
        RawJsonRpcMessage::Notification(notification) => notification.params.as_ref(),
        RawJsonRpcMessage::Response(_) => None,
    }?;
    serde_json::to_value(params)
        .ok()?
        .get("sessionId")?
        .as_str()
        .map(str::to_owned)
}

fn pending_request_for_message(message: &RawJsonRpcMessage) -> Option<(RequestId, String)> {
    let RawJsonRpcMessage::Request(request) = message else {
        return None;
    };
    pending_request_key(&request.id).map(|id| (id, request.method.to_string()))
}

fn pending_request_key(id: &RequestId) -> Option<RequestId> {
    match id {
        RequestId::Null => None,
        RequestId::Number(_) | RequestId::Str(_) => Some(id.clone()),
    }
}

#[cfg(test)]
mod tests {
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
        frames.acknowledge_notification("session/update");
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
        bytes.acknowledge_notification("session/update");
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
    fn logical_proxy_ack_is_strict_fifo_even_when_later_identity_matches() {
        let mut budget = FlowBudget::default();
        budget.allow_logical_ack();
        let small =
            RawJsonRpcMessage::notification("proxy/original".to_string(), serde_json::json!({}))
                .unwrap();
        let large =
            RawJsonRpcMessage::notification("session/update".to_string(), serde_json::json!({}))
                .unwrap();
        budget.track(&small, 1).unwrap();
        budget.track(&large, 1024).unwrap();

        budget.acknowledge_notification("session/update");
        assert_eq!(budget.frames, 1);
        assert_eq!(budget.bytes, 1024);
        assert_eq!(
            budget
                .notifications
                .front()
                .map(|(method, _)| method.as_str()),
            Some("session/update")
        );
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
            let body = br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}"#;
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
                        code: async_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
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
}
