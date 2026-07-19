//! On-demand singleton daemon discovery and server loop.
//!
//! The daemon is protected by an advisory file lock in the Hub home directory.
//! Clients discover it through `daemon.json`, then speak newline-delimited
//! JSON-RPC 2.0 over an interprocess local socket.

use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use fd_lock::RwLock as FdRwLock;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, tokio::prelude::*};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore, mpsc},
    task::{JoinHandle, JoinSet},
};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    endpoint::{Registry, harden_home, harden_sensitive_file},
    error::HubError,
    hub::CoreHub,
    rpc::{
        AUTH_REQUIRED_ERROR, CONFLICT_ERROR, INTERNAL_ERROR, INVALID_PARAMS,
        INVALID_REGISTRY_ERROR, JSONRPC_VERSION, MAX_RPC_LINE_BYTES, METHOD_NOT_FOUND,
        NOT_FOUND_ERROR, RESUME_LOAD_FAILED_ERROR, RpcError, RpcRequest, RpcResponse,
        UNSUPPORTED_CAPABILITY_ERROR, UNSUPPORTED_PROTOCOL_VERSION_ERROR,
        UNSUPPORTED_PROXY_TRANSPORT_ERROR, typed_hub_error_data,
    },
    store::Store,
};

const LOCK_FILE: &str = "daemon.lock";
const ID_FILE: &str = "daemon.id";
const METADATA_FILE: &str = "daemon.json";
#[cfg(unix)]
const SOCKET_FILE: &str = "daemon.sock";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const SERVE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(1_800);
const MAX_INFLIGHT_RPC_PER_CLIENT: usize = 8;
const MAX_INFLIGHT_RPC_GLOBAL: usize = 64;
const MAX_DAEMON_CLIENTS: usize = 64;
const MAX_BUFFERED_RPC_FRAMES_GLOBAL: usize = 4;
const RPC_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_FRAME_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Contents of `${ACP_HUB_HOME}/daemon.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonMetadata {
    pub pid: u32,
    pub endpoint: String,
    pub daemon_id: String,
    pub started_at: DateTime<Utc>,
}

/// Shared daemon liveness counters used by the idle-exit gate.
#[derive(Debug)]
pub struct ActivityTracker {
    active_clients: AtomicUsize,
    active_rpcs: AtomicUsize,
    active_runs: AtomicUsize,
    rpc_slots: Arc<Semaphore>,
    client_slots: Arc<Semaphore>,
    frame_slots: Arc<Semaphore>,
    rpc_write_timeout: Duration,
    last_activity: Mutex<Instant>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self::with_limits(
            MAX_DAEMON_CLIENTS,
            MAX_INFLIGHT_RPC_GLOBAL,
            MAX_BUFFERED_RPC_FRAMES_GLOBAL,
        )
    }

    fn with_limits(client_limit: usize, rpc_limit: usize, frame_slots: usize) -> Self {
        Self::with_limits_and_timeout(client_limit, rpc_limit, frame_slots, RPC_WRITE_TIMEOUT)
    }

    fn with_limits_and_timeout(
        client_limit: usize,
        rpc_limit: usize,
        frame_slots: usize,
        rpc_write_timeout: Duration,
    ) -> Self {
        Self {
            active_clients: AtomicUsize::new(0),
            active_rpcs: AtomicUsize::new(0),
            active_runs: AtomicUsize::new(0),
            rpc_slots: Arc::new(Semaphore::new(rpc_limit)),
            client_slots: Arc::new(Semaphore::new(client_limit)),
            frame_slots: Arc::new(Semaphore::new(frame_slots)),
            rpc_write_timeout,
            last_activity: Mutex::new(Instant::now()),
        }
    }

    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    pub fn client_lease(self: &Arc<Self>) -> ActivityLease {
        self.lease(ActivityKind::Client)
    }

    fn try_client_slot(self: &Arc<Self>) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.client_slots).try_acquire_owned().ok()
    }

    pub fn rpc_lease(self: &Arc<Self>) -> ActivityLease {
        self.lease(ActivityKind::Rpc)
    }

    pub fn run_lease(self: &Arc<Self>) -> ActivityLease {
        self.lease(ActivityKind::Run)
    }

    pub fn active_client_count(&self) -> usize {
        self.active_clients.load(Ordering::SeqCst)
    }

    pub fn active_rpc_count(&self) -> usize {
        self.active_rpcs.load(Ordering::SeqCst)
    }

    pub fn active_run_count(&self) -> usize {
        self.active_runs.load(Ordering::SeqCst)
    }

    fn lease(self: &Arc<Self>, kind: ActivityKind) -> ActivityLease {
        match kind {
            ActivityKind::Client => self.active_clients.fetch_add(1, Ordering::SeqCst),
            ActivityKind::Rpc => self.active_rpcs.fetch_add(1, Ordering::SeqCst),
            ActivityKind::Run => self.active_runs.fetch_add(1, Ordering::SeqCst),
        };
        self.touch();
        ActivityLease {
            tracker: Arc::clone(self),
            kind,
        }
    }

    fn is_quiescent(&self) -> bool {
        self.active_client_count() == 0
            && self.active_rpc_count() == 0
            && self.active_run_count() == 0
    }

    fn idle_for(&self) -> Duration {
        Instant::now().saturating_duration_since(*self.last_activity.lock())
    }
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
enum ActivityKind {
    Client,
    Rpc,
    Run,
}

/// RAII guard for one active daemon activity counter.
#[derive(Debug)]
pub struct ActivityLease {
    tracker: Arc<ActivityTracker>,
    kind: ActivityKind,
}

impl Drop for ActivityLease {
    fn drop(&mut self) {
        match self.kind {
            ActivityKind::Client => self.tracker.active_clients.fetch_sub(1, Ordering::SeqCst),
            ActivityKind::Rpc => self.tracker.active_rpcs.fetch_sub(1, Ordering::SeqCst),
            ActivityKind::Run => self.tracker.active_runs.fetch_sub(1, Ordering::SeqCst),
        };
        self.tracker.touch();
    }
}

/// Run the singleton daemon rooted at `home`.
pub async fn serve(home: impl AsRef<Path>) -> Result<(), HubError> {
    let home = canonical_home(home.as_ref())?;
    // The home path is serialized into the daemon endpoint and persisted in
    // metadata, so it must be valid UTF-8 — otherwise it would be silently
    // corrupted downstream by lossy string conversion.
    if home.to_str().is_none() {
        return Err(HubError::other(format!(
            "home path is not valid UTF-8: {}",
            home.display()
        )));
    }
    let mut lock = open_daemon_lock(&home)?;
    let lock_started = Instant::now();
    let _guard = loop {
        match lock.try_write() {
            Ok(guard) => break guard,
            Err(err)
                if err.kind() == ErrorKind::WouldBlock
                    && lock_started.elapsed() < SERVE_LOCK_TIMEOUT =>
            {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                return Err(HubError::DaemonUnavailable(
                    "another ACP Hub daemon already holds daemon.lock".into(),
                ));
            }
            Err(err) => return Err(HubError::Io(err)),
        }
    };

    remove_stale_daemon_state(&home)?;
    let daemon_id = Uuid::new_v4().to_string();
    let endpoint = daemon_endpoint(&home, &daemon_id);
    let listener = bind_listener(&endpoint)?;

    let registry = Registry::load(&home)?;
    let store = Store::open(&home)?;
    let activity = Arc::new(ActivityTracker::new());
    let hub = Arc::new(CoreHub::new(
        home.clone(),
        registry,
        store,
        Arc::clone(&activity),
    ));

    let id_path = home.join(ID_FILE);
    fs::write(&id_path, &daemon_id)?;
    harden_sensitive_file(&id_path)?;
    let metadata = DaemonMetadata {
        pid: std::process::id(),
        endpoint: endpoint.clone(),
        daemon_id,
        started_at: Utc::now(),
    };
    write_metadata(&home, &metadata)?;
    let idle_timeout = idle_timeout();

    debug!(endpoint = %endpoint, "ACP Hub daemon listening");
    let result = run_server(listener, hub, activity, idle_timeout).await;
    cleanup_daemon_state(&home);
    result
}

/// Discover an existing daemon or spawn one, then connect a JSON-RPC client.
pub async fn ensure_daemon(home: impl AsRef<Path>) -> Result<crate::rpc::RpcClient, HubError> {
    let home = canonical_home(home.as_ref())?;
    if let Some(client) = try_connect_metadata(&home).await {
        return Ok(client);
    }

    let mut lock = open_daemon_lock(&home)?;
    match lock.try_write() {
        Ok(guard) => {
            if let Some(client) = try_connect_metadata(&home).await {
                drop(guard);
                return Ok(client);
            }
            remove_stale_daemon_state(&home)?;
            spawn_daemon(&home)?;
            drop(guard);
            poll_daemon(&home, STARTUP_TIMEOUT).await
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            poll_daemon(&home, STARTUP_TIMEOUT).await
        }
        Err(err) => Err(HubError::Io(err)),
    }
}

async fn run_server(
    listener: LocalSocketListener,
    hub: Arc<CoreHub>,
    activity: Arc<ActivityTracker>,
    idle_timeout: Duration,
) -> Result<(), HubError> {
    run_server_until(listener, hub, activity, idle_timeout, async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(error = %err, "could not listen for Ctrl-C");
        }
    })
    .await
}

async fn run_server_until<S>(
    listener: LocalSocketListener,
    hub: Arc<CoreHub>,
    activity: Arc<ActivityTracker>,
    idle_timeout: Duration,
    shutdown: S,
) -> Result<(), HubError>
where
    S: Future<Output = ()>,
{
    let mut clients = JoinSet::new();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let stream = accepted?;
                let Some(client_slot) = activity.try_client_slot() else {
                    warn!("rejecting daemon client because the global client limit is full");
                    drop(stream);
                    continue;
                };
                let hub = Arc::clone(&hub);
                let activity = Arc::clone(&activity);
                let lease = activity.client_lease();
                clients.spawn(async move {
                    let _admission = (client_slot, lease);
                    if let Err(err) = handle_client(stream, hub, activity).await {
                        warn!(error = %err, "daemon client connection ended with error");
                    }
                });
            }
            Some(joined) = clients.join_next(), if !clients.is_empty() => {
                if let Err(err) = joined {
                    warn!(error = %err, "daemon client task panicked");
                }
            }
            _ = idle_wait(Arc::clone(&activity), idle_timeout) => {
                debug!("ACP Hub daemon idle timeout elapsed");
                break;
            }
            _ = &mut shutdown => break,
        }
    }

    clients.abort_all();
    while let Some(joined) = clients.join_next().await {
        if let Err(err) = joined
            && !err.is_cancelled()
        {
            warn!(error = %err, "daemon client task panicked during shutdown");
        }
    }
    Ok(())
}

async fn handle_client(
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

enum EncodedRpcResponse {
    Reply(Vec<u8>),
    Terminal(Vec<u8>),
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
}

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
    let write_timeout = activity.rpc_write_timeout;
    let frame_reader = AbortOnDropTask::new(tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        loop {
            let frame = read_bounded_frame(&mut reader, Arc::clone(&frame_slots)).await;
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
                let BufferedRpcFrame { line, _slot: frame_slot } = frame;
                let client_permit = match Arc::clone(&client_slots).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        match write_overload_response(
                            &writer,
                            &line,
                            "client RPC concurrency limit exceeded",
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
                    let _permits = (client_permit, global_permit);
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
                        let line = match encode_response(&notification) {
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
                        warn!(skipped, "daemon client lagged behind streamed notifications");
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
    let response = encode_response_with_fallback(id, &response)?;
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
) -> Result<Option<BufferedRpcFrame>, HubError>
where
    R: AsyncBufRead + Unpin,
{
    {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(None);
        }
    }
    let slot = frame_slots
        .acquire_owned()
        .await
        .map_err(|_| HubError::other("daemon RPC frame admission closed"))?;
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
            bytes.extend_from_slice(&available[..take]);
            reader.consume(take);
            if newline.is_some() {
                return Ok(bytes);
            }
        }
    })
    .await
    .map_err(|_| HubError::DaemonUnavailable("daemon RPC frame read timed out".to_string()))??;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(|line| Some(BufferedRpcFrame { line, _slot: slot }))
        .map_err(|error| HubError::other(format!("daemon RPC frame is not UTF-8: {error}")))
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
            return encode_response_with_fallback(Value::Null, &response).map(Some);
        }
    };
    let id = match request_id_presence(&raw) {
        Ok(Some(id)) => Some(id),
        Ok(None) => None,
        Err(()) => {
            let response = RpcError::invalid_request(Value::Null, "invalid JSON-RPC request");
            return encode_response_with_fallback(Value::Null, &response).map(Some);
        }
    };
    let request: RpcRequest = match serde_json::from_value(raw) {
        Ok(request) => request,
        Err(_) => {
            let response = RpcError::invalid_request(
                id.clone().unwrap_or(Value::Null),
                "invalid JSON-RPC request",
            );
            return encode_response_with_fallback(id.unwrap_or(Value::Null), &response).map(Some);
        }
    };

    if request.jsonrpc != JSONRPC_VERSION || request.method.is_empty() {
        let id = id.clone().unwrap_or(Value::Null);
        let error = RpcError::invalid_request(
            id.clone(),
            "expected JSON-RPC 2.0 request with a non-empty method",
        );
        return encode_response_with_fallback(id, &error).map(Some);
    }

    let Some(id) = id else {
        let _rpc = activity.rpc_lease();
        if let Err(err) = hub.handle_rpc(&request.method, request.params).await {
            warn!(method = %request.method, error = %err, "JSON-RPC notification failed");
        }
        return Ok(None);
    };

    let _rpc = activity.rpc_lease();
    let response = match hub.handle_rpc(&request.method, request.params).await {
        Ok(result) => {
            let success = RpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: id.clone(),
                result: Some(result),
            };
            encode_response_with_fallback(id, &success)?
        }
        Err(err) => {
            let error = hub_error_to_rpc(id.clone(), err);
            encode_response_with_fallback(id, &error)?
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
        HubError::Conflict(_) => (CONFLICT_ERROR, "resource conflict"),
        HubError::UnsupportedCapability { .. } => {
            (UNSUPPORTED_CAPABILITY_ERROR, "unsupported capability")
        }
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

struct CappedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl CappedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(4096)),
            limit,
            exceeded: false,
        }
    }
}

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

fn encode_response_with_fallback<T: Serialize>(
    id: Value,
    message: &T,
) -> Result<EncodedRpcResponse, HubError> {
    if let Ok(response) = encode_response(message) {
        return Ok(EncodedRpcResponse::Reply(response));
    }

    let fallback = RpcError::new(id, INTERNAL_ERROR, "RPC response too large", None);
    if let Ok(response) = encode_response(&fallback) {
        return Ok(EncodedRpcResponse::Reply(response));
    }

    let terminal = RpcError::new(
        Value::Null,
        INTERNAL_ERROR,
        "RPC response too large; connection closing",
        None,
    );
    encode_response(&terminal).map(EncodedRpcResponse::Terminal)
}

async fn idle_wait(activity: Arc<ActivityTracker>, idle_timeout: Duration) {
    loop {
        if activity.is_quiescent() && activity.idle_for() > idle_timeout {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn try_connect_metadata(home: &Path) -> Option<crate::rpc::RpcClient> {
    let metadata = read_metadata(home).ok().flatten()?;
    crate::rpc::RpcClient::connect(&metadata.endpoint)
        .await
        .ok()
}

async fn poll_daemon(home: &Path, timeout: Duration) -> Result<crate::rpc::RpcClient, HubError> {
    let started = Instant::now();
    while started.elapsed() <= timeout {
        if let Some(client) = try_connect_metadata(home).await {
            return Ok(client);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(HubError::DaemonUnavailable(format!(
        "daemon did not become ready within {}s",
        timeout.as_secs()
    )))
}

fn canonical_home(home: &Path) -> Result<PathBuf, HubError> {
    harden_home(home)?;
    // dunce::canonicalize strips the `\\?\` verbatim prefix on Windows when safe,
    // so the home path can be forwarded to the child daemon process and compared
    // without mangling. It is a no-op on Unix.
    Ok(dunce::canonicalize(home)?)
}

fn daemon_endpoint(home: &Path, daemon_id: &str) -> String {
    #[cfg(windows)]
    {
        let _ = home;
        format!(r"\\.\pipe\acp-hub-{daemon_id}")
    }
    #[cfg(unix)]
    {
        // Prefer `$home/daemon.sock`. On macOS `sockaddr_un.sun_path` is only ~104
        // bytes; deep temp/home paths overflow and fail bind/connect. Fall back to
        // a short socket under the process temp dir, keyed by daemon_id.
        unix_daemon_endpoint(home, daemon_id)
    }
}

/// Maximum bytes we accept for a filesystem Unix-domain socket path.
/// Keep under both Linux (108) and macOS (104) `sun_path` limits.
#[cfg(unix)]
const UNIX_SOCK_PATH_MAX: usize = 100;

#[cfg(unix)]
fn unix_daemon_endpoint(home: &Path, daemon_id: &str) -> String {
    let preferred = home.join(SOCKET_FILE);
    let preferred_s = preferred.to_string_lossy();
    if preferred_s.len() < UNIX_SOCK_PATH_MAX {
        return preferred_s.into_owned();
    }
    let short: String = daemon_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    let fallback = std::env::temp_dir()
        .join(format!("ah-{short}"))
        .join(SOCKET_FILE);
    fallback.to_string_lossy().into_owned()
}

fn bind_listener(endpoint: &str) -> Result<LocalSocketListener, HubError> {
    let name = Path::new(endpoint).to_fs_name::<GenericFilePath>()?;
    let options = ListenerOptions::new().name(name);
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        use std::os::unix::fs::PermissionsExt;

        if let Some(parent) = Path::new(endpoint).parent() {
            fs::create_dir_all(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        Ok(options.mode(0o600).create_tokio()?)
    }
    #[cfg(windows)]
    {
        use interprocess::os::windows::{
            local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
        };
        use widestring::U16CString;

        let sddl =
            U16CString::from_str("D:P(A;;GA;;;OW)(A;;GA;;;SY)(A;;GA;;;BA)").map_err(|error| {
                HubError::other(format!("invalid pipe security descriptor: {error}"))
            })?;
        let descriptor = SecurityDescriptor::deserialize(sddl.as_ucstr())?;
        Ok(options.security_descriptor(descriptor).create_tokio()?)
    }
}

fn open_daemon_lock(home: &Path) -> Result<FdRwLock<File>, HubError> {
    let path = home.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    harden_sensitive_file(&path)?;
    Ok(FdRwLock::new(file))
}

fn read_metadata(home: &Path) -> Result<Option<DaemonMetadata>, HubError> {
    match fs::read_to_string(home.join(METADATA_FILE)) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(HubError::Io(err)),
    }
}

fn write_metadata(home: &Path, metadata: &DaemonMetadata) -> Result<(), HubError> {
    let tmp = home.join(format!("{METADATA_FILE}.tmp"));
    let target = home.join(METADATA_FILE);
    fs::write(&tmp, serde_json::to_vec_pretty(metadata)?)?;
    harden_sensitive_file(&tmp)?;
    if target.exists() {
        fs::remove_file(&target)?;
    }
    fs::rename(tmp, &target)?;
    harden_sensitive_file(&target)?;
    Ok(())
}

fn remove_stale_daemon_state(home: &Path) -> Result<(), HubError> {
    // Drop the previous endpoint first (may live outside `home` on Unix when the
    // preferred `$home/daemon.sock` path would exceed `sun_path`).
    if let Ok(Some(meta)) = read_metadata(home) {
        #[cfg(unix)]
        {
            let expected = daemon_endpoint(home, &meta.daemon_id);
            if meta.endpoint == expected {
                let endpoint = PathBuf::from(expected);
                remove_file_if_exists(endpoint.clone())?;
                remove_private_socket_parent(&endpoint);
            } else {
                warn!(
                    recorded = %meta.endpoint,
                    expected,
                    "ignoring daemon metadata endpoint that does not match its home and daemon id"
                );
            }
        }
        #[cfg(windows)]
        {
            let _ = meta;
        }
    }
    remove_file_if_exists(home.join(METADATA_FILE))?;
    remove_file_if_exists(home.join(ID_FILE))?;
    #[cfg(unix)]
    remove_file_if_exists(home.join(SOCKET_FILE))?;
    Ok(())
}

#[cfg(unix)]
fn remove_private_socket_parent(endpoint: &Path) {
    let Some(parent) = endpoint.parent() else {
        return;
    };
    let temp = std::env::temp_dir();
    let is_private_fallback = parent.parent() == Some(temp.as_path())
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("ah-"));
    if is_private_fallback {
        let _ = fs::remove_dir(parent);
    }
}

fn cleanup_daemon_state(home: &Path) {
    if let Err(err) = remove_stale_daemon_state(home) {
        warn!(error = %err, "failed to remove daemon metadata during shutdown");
    }
}

fn remove_file_if_exists(path: PathBuf) -> Result<(), HubError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(HubError::Io(err)),
    }
}

fn idle_timeout() -> Duration {
    std::env::var("ACP_HUB_IDLE_TIMEOUT")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT)
}

fn spawn_daemon(home: &Path) -> Result<(), HubError> {
    let mut command = Command::new(daemon_program());
    command
        .arg("serve")
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        // Detach into a new session so the daemon survives the parent process /
        // controlling terminal exiting (a bare process_group does not create a
        // new session and leaves the child vulnerable to SIGHUP).
        unsafe {
            command.pre_exec(|| {
                setsid();
                Ok(())
            });
        }
    }

    command.spawn()?;
    Ok(())
}

fn daemon_program() -> PathBuf {
    if let Some(path) = std::env::var_os("ACP_HUB_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        let is_hub = exe
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case("acp-hub"));
        if is_hub {
            return exe;
        }
    }
    PathBuf::from("acp-hub")
}

#[cfg(test)]
mod connection_tests {
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
                .map(|response| Some(EncodedRpcResponse::Reply(response)))
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
        let activity = Arc::new(ActivityTracker::with_limits(1, 64, 4));
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
                    .map(|response| Some(EncodedRpcResponse::Reply(response)))
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
        let activity = Arc::new(ActivityTracker::with_limits(1, 1, 2));
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
        let first = read_bounded_frame(&mut reader_one, Arc::clone(&activity.frame_slots))
            .await
            .unwrap()
            .unwrap();
        let second = read_bounded_frame(&mut reader_two, Arc::clone(&activity.frame_slots))
            .await
            .unwrap()
            .unwrap();

        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                read_bounded_frame(&mut reader_three, Arc::clone(&activity.frame_slots),),
            )
            .await
            .is_err(),
            "third connection must backpressure while both frame slots are held"
        );
        drop(first);
        let third = tokio::time::timeout(
            Duration::from_secs(2),
            read_bounded_frame(&mut reader_three, Arc::clone(&activity.frame_slots)),
        )
        .await
        .expect("releasing one frame slot must unblock another connection")
        .unwrap()
        .unwrap();
        assert_eq!(third.line, "{\"frame\":true}");
        drop((second, third));
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
                Ok(Some(EncodedRpcResponse::Terminal(terminal)))
            },
        ));
        client_writer
            .write_all(&encode_response(&RpcRequest::new(json!(1), "long", Value::Null)).unwrap())
            .await
            .unwrap();
        client_writer
            .write_all(
                &encode_response(&RpcRequest::new(json!(2), "terminal", Value::Null)).unwrap(),
            )
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
                    .map(|response| Some(EncodedRpcResponse::Reply(response)))
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
                !String::from_utf8(bytes)
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
        let error = match read_bounded_frame(&mut reader, Arc::new(Semaphore::new(1))).await {
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
                Ok(Some(EncodedRpcResponse::Reply(encode_response(&response)?)))
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
                Ok(Some(EncodedRpcResponse::Reply(encode_response(&response)?)))
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
        let padding_len =
            MAX_RPC_LINE_BYTES - encode_response(&empty).unwrap().len() - secret.len();
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

        let internal =
            hub_error_to_rpc(json!(8), HubError::Other("rpc-secret-sentinel".to_string()));
        assert_eq!(internal.error.code, INTERNAL_ERROR);
        assert!(internal.error.data.is_none());
        assert!(!internal.error.message.contains("rpc-secret-sentinel"));
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn daemon_endpoint_uses_named_pipe_on_windows() {
        #[cfg(windows)]
        {
            let ep = daemon_endpoint(Path::new(r"C:\Users\me\.acp-hub"), "deadbeef-1234");
            assert_eq!(ep, r"\\.\pipe\acp-hub-deadbeef-1234");
        }
    }

    #[test]
    fn unix_short_home_keeps_socket_under_home() {
        #[cfg(unix)]
        {
            let home = PathBuf::from("/tmp/ah");
            let ep = daemon_endpoint(&home, "abc123def456");
            assert_eq!(ep, "/tmp/ah/daemon.sock");
            assert!(ep.len() < UNIX_SOCK_PATH_MAX);
        }
    }

    #[test]
    fn unix_deep_home_falls_back_to_short_temp_socket() {
        #[cfg(unix)]
        {
            // Build a home path long enough that `$home/daemon.sock` exceeds the cap.
            let deep = format!("/{}", "x".repeat(UNIX_SOCK_PATH_MAX));
            let home = PathBuf::from(&deep);
            let ep = daemon_endpoint(&home, "deadbeef-cafe-babe");
            assert!(
                !ep.starts_with(&deep),
                "expected fallback away from deep home, got {ep}"
            );
            assert!(
                ep.contains("ah-deadbeefcafe") || ep.contains("ah-deadbeef"),
                "fallback should key on daemon_id, got {ep}"
            );
            assert!(
                ep.len() < UNIX_SOCK_PATH_MAX,
                "fallback path must fit sun_path, len={} path={ep}",
                ep.len()
            );
        }
    }
}
