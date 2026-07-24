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
pub const RESOURCE_LIMIT_ERROR: i64 = -32_015;
pub const INVALID_CURSOR_ERROR: i64 = -32_016;
pub const STALE_CURSOR_ERROR: i64 = -32_017;
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

mod error_data;

use error_data::TypedHubErrorData;
pub(crate) use error_data::typed_hub_error_data;

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
        let (notifications, _) = broadcast::channel(2048);
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
#[path = "rpc/tests.rs"]
mod tests;
