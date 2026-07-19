//! JSON-RPC 2.0 framing and client transport for the Hub daemon.
//!
//! The daemon protocol is newline-delimited JSON-RPC over an interprocess
//! local socket (Unix domain socket / Windows named pipe). `RpcClient` keeps
//! one reader task per connection so long-running calls, out-of-order
//! responses, and daemon-pushed notifications do not block unrelated requests.

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use interprocess::local_socket::{GenericFilePath, tokio::prelude::*};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Number, Value};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};

use crate::error::{AuthMethodSummary, HubError};

pub const JSONRPC_VERSION: &str = "2.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const AUTH_REQUIRED_ERROR: i64 = -32_001;
pub const NOT_FOUND_ERROR: i64 = -32_004;
pub const CONFLICT_ERROR: i64 = -32_009;
pub const UNSUPPORTED_CAPABILITY_ERROR: i64 = -32_010;
pub const INVALID_REGISTRY_ERROR: i64 = -32_011;
pub const UNSUPPORTED_PROTOCOL_VERSION_ERROR: i64 = -32_012;
pub const RESUME_LOAD_FAILED_ERROR: i64 = -32_013;
pub const UNSUPPORTED_PROXY_TRANSPORT_ERROR: i64 = -32_014;
pub const MAX_RPC_LINE_BYTES: usize = 32 * 1024 * 1024;

/// JSON-RPC request or notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default = "null_value")]
    pub params: Value,
}

/// JSON-RPC success response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

/// JSON-RPC error response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub jsonrpc: String,
    pub id: Value,
    pub error: RpcErrorObject,
}

/// JSON-RPC error payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum SafeResumeSourceData {
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
            HubError::ResumeLoadFailed { .. }
            | HubError::Acp(_)
            | HubError::Io(_)
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
            Self::DaemonUnavailable {} | Self::Internal {} => Some(HubError::DaemonUnavailable(
                "resume/load operation failed".to_string(),
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TypedHubErrorData {
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
            HubError::UnsupportedCapability {
                endpoint,
                operation,
                required_capability,
            } => Some(Self::UnsupportedCapability {
                endpoint: endpoint.clone(),
                operation: (*operation).to_string(),
                required_capability: (*required_capability).to_string(),
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
            _ => None,
        }
    }

    fn into_hub_error(self, code: i64) -> Option<HubError> {
        match self {
            Self::NotFound { kind, id } if code == NOT_FOUND_ERROR => Some(HubError::NotFound {
                kind: known_not_found_kind(&kind)?,
                id,
            }),
            Self::Conflict { conv_id } if code == CONFLICT_ERROR => {
                Some(HubError::Conflict(conv_id))
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
        _ => None,
    }
}

impl RpcRequest {
    pub fn new(id: Value, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(id),
            method: method.into(),
            params,
        }
    }

    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: method.into(),
            params,
        }
    }
}

impl RpcResponse {
    pub fn success(id: Value, result: impl Serialize) -> Result<Self, HubError> {
        Ok(Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(serde_json::to_value(result)?),
        })
    }
}

impl RpcError {
    pub fn new(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            error: RpcErrorObject {
                code,
                message: message.into(),
                data,
            },
        }
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(Value::Null, PARSE_ERROR, message, None)
    }

    pub fn invalid_request(id: Value, message: impl Into<String>) -> Self {
        Self::new(id, INVALID_REQUEST, message, None)
    }
}

const OUTBOUND_QUEUE_CAPACITY: usize = 8;
const WRITER_FAILED_MESSAGE: &str = "daemon RPC writer failed";

struct OutboundLine {
    line: Vec<u8>,
    result: oneshot::Sender<Result<(), HubError>>,
}

type Pending = HashMap<String, oneshot::Sender<Result<Value, HubError>>>;

struct RpcClientInner {
    outbound: mpsc::Sender<OutboundLine>,
    pending: Mutex<Pending>,
    notifications: broadcast::Sender<RpcRequest>,
    next_id: AtomicU64,
    closed: AtomicBool,
}

struct PendingRegistration {
    inner: Arc<RpcClientInner>,
    key: Option<String>,
}

impl PendingRegistration {
    fn new(inner: Arc<RpcClientInner>, key: String) -> Self {
        Self {
            inner,
            key: Some(key),
        }
    }
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.inner.pending.lock().remove(&key);
        }
    }
}

/// Client for newline-delimited JSON-RPC over the Hub daemon transport.
pub struct RpcClient {
    inner: Arc<RpcClientInner>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
}

impl RpcClient {
    /// Connect to a daemon endpoint from `daemon.json`.
    ///
    /// On Windows this is a named pipe path such as
    /// `\\.\pipe\acp-hub-{daemon_id}`. On Unix it is the filesystem path to the
    /// local socket.
    pub async fn connect(endpoint: &str) -> Result<Self, HubError> {
        let name = Path::new(endpoint).to_fs_name::<GenericFilePath>()?;
        let stream = LocalSocketStream::connect(name).await.map_err(|e| {
            HubError::DaemonUnavailable(format!("could not connect to {endpoint}: {e}"))
        })?;
        let (reader, writer) = stream.split();
        Ok(Self::from_reader_writer(reader, writer))
    }

    /// Build a client around an existing owned reader/writer pair.
    ///
    /// This supports stdio bridges or child daemon processes whose stdout is
    /// the client's read side and stdin is the client's write side.
    pub fn from_reader_writer<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (notifications, _) = broadcast::channel(256);
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
        let inner = Arc::new(RpcClientInner {
            outbound,
            pending: Mutex::new(HashMap::new()),
            notifications,
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });
        let reader_task = tokio::spawn(reader_loop(reader, Arc::clone(&inner)));
        let writer_task = tokio::spawn(writer_loop(writer, outbound_rx, Arc::clone(&inner)));
        Self {
            inner,
            reader_task,
            writer_task,
        }
    }

    /// Alias for stdio-style transports: `stdout` is the read side and `stdin`
    /// is the write side from the client's perspective.
    pub fn from_stdio<R, W>(stdout: R, stdin: W) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::from_reader_writer(stdout, stdin)
    }

    /// Subscribe to id-less daemon notifications (for example
    /// `hub/conv/update`).
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<RpcRequest> {
        self.inner.notifications.subscribe()
    }

    /// Send a typed request and deserialize the response result.
    pub async fn request<P, T>(&self, method: &str, params: P) -> Result<T, HubError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        let value = self
            .request_value(method, serde_json::to_value(params)?)
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Compatibility alias for callers that already serialized params.
    pub async fn call<T>(&self, method: &str, params: Value) -> Result<T, HubError>
    where
        T: DeserializeOwned,
    {
        let value = self.request_value(method, params).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Send a request and return the raw JSON result.
    pub async fn request_value(&self, method: &str, params: Value) -> Result<Value, HubError> {
        let id_num = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let id = Value::Number(Number::from(id_num));
        let key = id_key(&id)?;
        let request = RpcRequest::new(id, method, params);
        let line = encode_line(&request)?;
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.inner.pending.lock();
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(HubError::DaemonUnavailable("connection is closed".into()));
            }
            pending.insert(key.clone(), tx);
        }
        let _registration = PendingRegistration::new(Arc::clone(&self.inner), key);
        self.write_line(line).await?;

        rx.await
            .map_err(|_| HubError::DaemonUnavailable("connection reader stopped".into()))?
    }

    /// Send an id-less notification.
    pub async fn notify<P>(&self, method: &str, params: P) -> Result<(), HubError>
    where
        P: Serialize,
    {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(HubError::DaemonUnavailable("connection is closed".into()));
        }
        let request = RpcRequest::notification(method, serde_json::to_value(params)?);
        let line = encode_line(&request)?;
        self.write_line(line).await
    }

    async fn write_line(&self, line: Vec<u8>) -> Result<(), HubError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(HubError::DaemonUnavailable("connection is closed".into()));
        }
        let (result, written) = oneshot::channel();
        if self
            .inner
            .outbound
            .send(OutboundLine { line, result })
            .await
            .is_err()
        {
            close_pending(&self.inner, WRITER_FAILED_MESSAGE);
            return Err(HubError::DaemonUnavailable(
                WRITER_FAILED_MESSAGE.to_string(),
            ));
        }
        written.await.unwrap_or_else(|_| {
            Err(HubError::DaemonUnavailable(
                WRITER_FAILED_MESSAGE.to_string(),
            ))
        })
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

async fn writer_loop<W>(
    mut writer: W,
    mut outbound: mpsc::Receiver<OutboundLine>,
    inner: Arc<RpcClientInner>,
) where
    W: AsyncWrite + Unpin,
{
    while let Some(message) = outbound.recv().await {
        if inner.closed.load(Ordering::SeqCst) {
            let _ = message.result.send(Err(HubError::DaemonUnavailable(
                "connection is closed".to_string(),
            )));
            continue;
        }
        let delivered = async {
            writer.write_all(&message.line).await?;
            writer.flush().await
        }
        .await;
        match delivered {
            Ok(()) => {
                let _ = message.result.send(Ok(()));
            }
            Err(_) => {
                close_pending(&inner, WRITER_FAILED_MESSAGE);
                let _ = message.result.send(Err(HubError::DaemonUnavailable(
                    WRITER_FAILED_MESSAGE.to_string(),
                )));
                while let Ok(queued) = outbound.try_recv() {
                    let _ = queued.result.send(Err(HubError::DaemonUnavailable(
                        WRITER_FAILED_MESSAGE.to_string(),
                    )));
                }
                break;
            }
        }
    }
}

async fn reader_loop<R>(reader: R, inner: Arc<RpcClientInner>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    loop {
        match read_bounded_line(&mut reader).await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if handle_line(&line, &inner).await.is_err() {
                    close_pending(&inner, "daemon returned an invalid RPC response");
                    break;
                }
            }
            Ok(None) => {
                close_pending(&inner, "daemon closed the connection");
                break;
            }
            Err(error) => {
                let message = if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    "daemon RPC frame ended before newline"
                } else {
                    "daemon RPC framing error"
                };
                close_pending(&inner, message);
                break;
            }
        }
    }
}

async fn handle_line(line: &str, inner: &RpcClientInner) -> Result<(), HubError> {
    let value: Value = serde_json::from_str(line)?;
    ensure_jsonrpc_version(&value)?;
    if value.get("method").is_some() {
        let has_id = value
            .as_object()
            .is_some_and(|object| object.contains_key("id"));
        let notification: RpcRequest = serde_json::from_value(value)?;
        if has_id {
            return Err(HubError::DaemonUnavailable(
                "server requests are not supported".to_string(),
            ));
        }
        let _ = inner.notifications.send(notification);
        return Ok(());
    }

    let id = value
        .get("id")
        .ok_or_else(|| HubError::DaemonUnavailable("rpc response missing id".into()))?;
    let key = id_key(id)?;

    let has_error = value.get("error").is_some();
    let has_result = value.get("result").is_some();
    if has_error && has_result {
        return Err(HubError::DaemonUnavailable(
            "rpc response had both result and error".into(),
        ));
    }

    if has_error {
        let error: RpcError = serde_json::from_value(value)?;
        if let Some(tx) = inner.pending.lock().remove(&key) {
            let _ = tx.send(Err(rpc_error_to_hub_error(error.error)));
        }
        return Ok(());
    }

    if has_result {
        let response: RpcResponse = serde_json::from_value(value)?;
        if let Some(tx) = inner.pending.lock().remove(&key) {
            let _ = tx.send(Ok(response.result.unwrap_or(Value::Null)));
        }
        return Ok(());
    }

    Err(HubError::DaemonUnavailable(
        "rpc response had neither result nor error".into(),
    ))
}

fn close_pending(inner: &RpcClientInner, message: &'static str) {
    inner.closed.store(true, Ordering::SeqCst);
    let mut pending = inner.pending.lock();
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(HubError::DaemonUnavailable(message.to_string())));
    }
}

fn encode_line<T: Serialize>(message: &T) -> Result<Vec<u8>, HubError> {
    let mut line = serde_json::to_vec(message)?;
    if line.len().saturating_add(1) > MAX_RPC_LINE_BYTES {
        return Err(HubError::other(format!(
            "RPC frame exceeds {MAX_RPC_LINE_BYTES} bytes"
        )));
    }
    line.push(b'\n');
    Ok(line)
}

async fn read_bounded_line<R>(reader: &mut R) -> Result<Option<String>, std::io::Error>
where
    R: AsyncBufRead + Unpin,
{
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "RPC frame ended before newline",
            ));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(take) > MAX_RPC_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("RPC frame exceeds {MAX_RPC_LINE_BYTES} bytes"),
            ));
        }
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes).map(Some).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("RPC frame is not UTF-8: {error}"),
        )
    })
}

fn ensure_jsonrpc_version(value: &Value) -> Result<(), HubError> {
    match value.get("jsonrpc").and_then(Value::as_str) {
        Some(JSONRPC_VERSION) => Ok(()),
        _ => Err(HubError::DaemonUnavailable(
            "invalid json-rpc version".into(),
        )),
    }
}

fn id_key(id: &Value) -> Result<String, HubError> {
    match id {
        Value::Null | Value::String(_) | Value::Number(_) => Ok(serde_json::to_string(id)?),
        _ => Err(HubError::other(
            "json-rpc id must be a string, number, or null",
        )),
    }
}

fn rpc_error_to_hub_error(error: RpcErrorObject) -> HubError {
    if let Some(data) = error.data {
        let code = error.code;
        return serde_json::from_value::<TypedHubErrorData>(data)
            .ok()
            .and_then(|data| data.into_hub_error(code))
            .unwrap_or_else(|| {
                HubError::DaemonUnavailable("daemon returned malformed error data".to_string())
            });
    }

    match error.code {
        METHOD_NOT_FOUND => HubError::other("daemon method not found"),
        INVALID_REQUEST | INVALID_PARAMS => HubError::other("daemon rejected invalid request"),
        _ => HubError::DaemonUnavailable("daemon request failed".to_string()),
    }
}

fn null_value() -> Value {
    Value::Null
}

#[cfg(test)]
mod tests {
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
                        &RpcResponse::success(second.id.clone().unwrap(), json!({"value": 2}))
                            .unwrap(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            server_writer
                .write_all(
                    &encode_line(
                        &RpcResponse::success(first.id.clone().unwrap(), json!({"value": 1}))
                            .unwrap(),
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
                    HubError::DaemonUnavailable(message)
                        if message == "resume/load operation failed"
                ));
            }
            other => panic!("expected typed resume/load error, got {other}"),
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
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"request\",\"params\":null}\n",
            )
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
}
