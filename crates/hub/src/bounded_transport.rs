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
#[cfg(any(test, feature = "test-flow-ledger"))]
use std::sync::{
    LazyLock,
    atomic::{AtomicU64, Ordering},
};

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

mod flow;

pub(crate) use flow::{BoundedStdioAgent, InboundFlowControl};
#[cfg(test)]
use flow::{FlowBudget, notification_identity, read_bounded_line};
#[cfg(feature = "test-flow-ledger")]
pub use flow::{
    TestFlowLedgerEvent, pause_test_flow_acknowledgements, reset_test_flow_ledger,
    test_flow_ledger_snapshot,
};
use flow::{charge_flow, complete_outbound_response};

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

mod wire;

#[cfg(test)]
use wire::SseDecoder;
use wire::{
    bounded_response_bytes, read_sse, sanitized_reqwest_error, sanitized_websocket_error,
    serialize_bounded,
};

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
#[path = "bounded_transport/tests.rs"]
mod tests;
