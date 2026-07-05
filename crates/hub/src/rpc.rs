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
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Number, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
};

use crate::error::HubError;

pub const JSONRPC_VERSION: &str = "2.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

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

type Pending = HashMap<String, oneshot::Sender<Result<Value, HubError>>>;

struct RpcClientInner {
    writer: Mutex<Box<dyn AsyncWrite + Unpin + Send>>,
    pending: Mutex<Pending>,
    notifications: broadcast::Sender<RpcRequest>,
    next_id: AtomicU64,
    closed: AtomicBool,
}

/// Client for newline-delimited JSON-RPC over the Hub daemon transport.
pub struct RpcClient {
    inner: Arc<RpcClientInner>,
    reader_task: JoinHandle<()>,
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
        let inner = Arc::new(RpcClientInner {
            writer: Mutex::new(Box::new(writer)),
            pending: Mutex::new(HashMap::new()),
            notifications,
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });
        let reader_task = tokio::spawn(reader_loop(reader, Arc::clone(&inner)));
        Self { inner, reader_task }
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
            let mut pending = self.inner.pending.lock().await;
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(HubError::DaemonUnavailable("connection is closed".into()));
            }
            pending.insert(key.clone(), tx);
        }
        if let Err(error) = self.write_line(&line).await {
            self.inner.pending.lock().await.remove(&key);
            return Err(error);
        }

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
        self.write_line(&line).await
    }

    async fn write_line(&self, line: &[u8]) -> Result<(), HubError> {
        let mut writer = self.inner.writer.lock().await;
        writer.write_all(line).await?;
        writer.flush().await?;
        Ok(())
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

async fn reader_loop<R>(reader: R, inner: Arc<RpcClientInner>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Err(error) = handle_line(&line, &inner).await {
                    close_pending(&inner, error).await;
                    break;
                }
            }
            Ok(None) => {
                close_pending(
                    &inner,
                    HubError::DaemonUnavailable("daemon closed the connection".into()),
                )
                .await;
                break;
            }
            Err(error) => {
                close_pending(&inner, HubError::Io(error)).await;
                break;
            }
        }
    }
}

async fn handle_line(line: &str, inner: &RpcClientInner) -> Result<(), HubError> {
    let value: Value = serde_json::from_str(line)?;
    ensure_jsonrpc_version(&value)?;
    if value.get("method").is_some() {
        let notification: RpcRequest = serde_json::from_value(value)?;
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
        if let Some(tx) = inner.pending.lock().await.remove(&key) {
            let _ = tx.send(Err(rpc_error_to_hub_error(error.error)));
        }
        return Ok(());
    }

    if has_result {
        let response: RpcResponse = serde_json::from_value(value)?;
        if let Some(tx) = inner.pending.lock().await.remove(&key) {
            let _ = tx.send(Ok(response.result.unwrap_or(Value::Null)));
        }
        return Ok(());
    }

    Err(HubError::DaemonUnavailable(
        "rpc response had neither result nor error".into(),
    ))
}

async fn close_pending(inner: &RpcClientInner, error: HubError) {
    inner.closed.store(true, Ordering::SeqCst);
    let mut pending = inner.pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(HubError::DaemonUnavailable(error.to_string())));
    }
}

fn encode_line<T: Serialize>(message: &T) -> Result<Vec<u8>, HubError> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    Ok(line)
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
    if let Some(data) = &error.data {
        if let Some(error_type) = data.get("type").and_then(Value::as_str) {
            return match error_type {
                "notFound" => HubError::NotFound {
                    kind: data
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("resource")
                        .to_string(),
                    id: data
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(&error.message)
                        .to_string(),
                },
                "conflict" => HubError::Conflict(
                    data.get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or(&error.message)
                        .to_string(),
                ),
                "unsupportedCapability" => HubError::UnsupportedCapability {
                    endpoint: data
                        .get("endpoint")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    operation: data
                        .get("operation")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    required_capability: data
                        .get("requiredCapability")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                },
                "authRequired" => HubError::AuthRequired {
                    endpoint: data
                        .get("endpoint")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    auth_methods: data
                        .get("authMethods")
                        .cloned()
                        .and_then(|v| serde_json::from_value(v).ok())
                        .unwrap_or_default(),
                },
                "invalidRegistry" => HubError::InvalidRegistry(
                    data.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or(&error.message)
                        .to_string(),
                ),
                "unsupportedProtocolVersion" => HubError::UnsupportedProtocolVersion,
                "unsupportedProxyTransport" => HubError::UnsupportedProxyTransport,
                _ => HubError::other(format_rpc_error(error)),
            };
        }
    }
    HubError::other(format_rpc_error(error))
}

fn format_rpc_error(error: RpcErrorObject) -> String {
    let mut message = format!("rpc error {}: {}", error.code, error.message);
    if let Some(data) = error.data {
        message.push_str("; data=");
        message.push_str(&data.to_string());
    }
    message
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
    async fn maps_rpc_error_responses_to_hub_errors() {
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
                        METHOD_NOT_FOUND,
                        "missing method",
                        Some(json!({"method": "missing"})),
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

        assert!(
            error
                .to_string()
                .contains("rpc error -32601: missing method")
        );
        assert!(error.to_string().contains("\"method\":\"missing\""));
    }

    #[tokio::test]
    async fn preserves_unsupported_capability_rpc_error_fields() {
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
                        -32_010,
                        "endpoint `testy` does not support `terminal/create`; requires client_capabilities.terminal",
                        Some(json!({
                            "type": "unsupportedCapability",
                            "endpoint": "testy",
                            "operation": "terminal/create",
                            "requiredCapability": "client_capabilities.terminal"
                        })),
                    ))
                    .unwrap(),
                )
                .await
                .unwrap();
        });

        let error = client
            .request_value("hub/agent/logout", Value::Null)
            .await
            .expect_err("typed unsupported capability error must fail the request");
        server.await.unwrap();

        match error {
            HubError::UnsupportedCapability {
                endpoint,
                operation,
                required_capability,
            } => {
                assert_eq!(endpoint, "testy");
                assert_eq!(operation, "terminal/create");
                assert_eq!(required_capability, "client_capabilities.terminal");
            }
            other => panic!("expected UnsupportedCapability, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn preserves_not_found_rpc_error_kind() {
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
                        "conversation missing",
                        Some(json!({
                            "type": "notFound",
                            "kind": "conversation",
                            "id": "conv-404"
                        })),
                    ))
                    .unwrap(),
                )
                .await
                .unwrap();
        });

        let error = client
            .request_value("hub/conv/messages", Value::Null)
            .await
            .expect_err("typed notFound error must fail the request");
        server.await.unwrap();

        match error {
            HubError::NotFound { kind, id } => {
                assert_eq!(kind, "conversation");
                assert_eq!(id, "conv-404");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
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
}
