//! On-demand singleton daemon discovery and server loop.
//!
//! The daemon is protected by an advisory file lock in the Hub home directory.
//! Clients discover it through `daemon.json`, then speak newline-delimited
//! JSON-RPC 2.0 over an interprocess local socket.

use std::{
    fs::{self, File, OpenOptions},
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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    task::JoinSet,
};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    endpoint::Registry,
    error::HubError,
    hub::CoreHub,
    rpc::{
        INTERNAL_ERROR, INVALID_REQUEST, JSONRPC_VERSION, METHOD_NOT_FOUND, RpcError, RpcRequest,
        RpcResponse,
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
    last_activity: Mutex<Instant>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self {
            active_clients: AtomicUsize::new(0),
            active_rpcs: AtomicUsize::new(0),
            active_runs: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
        }
    }

    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    pub fn client_lease(self: &Arc<Self>) -> ActivityLease {
        self.lease(ActivityKind::Client)
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

    fs::write(home.join(ID_FILE), &daemon_id)?;
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
    let mut clients = JoinSet::new();

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let stream = accepted?;
                let hub = Arc::clone(&hub);
                let activity = Arc::clone(&activity);
                let lease = activity.client_lease();
                clients.spawn(async move {
                    let _lease = lease;
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
            shutdown = tokio::signal::ctrl_c() => {
                if let Err(err) = shutdown {
                    warn!(error = %err, "could not listen for Ctrl-C");
                }
                break;
            }
        }
    }

    while let Some(joined) = clients.join_next().await {
        if let Err(err) = joined {
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
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        activity.touch();
        let Some(response) =
            handle_rpc_line(&line, Arc::clone(&hub), Arc::clone(&activity)).await?
        else {
            continue;
        };
        writer.write_all(&response).await?;
        writer.flush().await?;
    }
    Ok(())
}

async fn handle_rpc_line(
    line: &str,
    hub: Arc<CoreHub>,
    activity: Arc<ActivityTracker>,
) -> Result<Option<Vec<u8>>, HubError> {
    if line.trim().is_empty() {
        return Ok(None);
    }

    let raw: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => return encode_response(&RpcError::parse_error(err.to_string())).map(Some),
    };
    let id = raw.get("id").cloned().unwrap_or(Value::Null);
    let request: RpcRequest = match serde_json::from_value(raw) {
        Ok(request) => request,
        Err(err) => {
            return encode_response(&RpcError::invalid_request(id, err.to_string())).map(Some);
        }
    };

    if request.jsonrpc != JSONRPC_VERSION || request.method.is_empty() {
        let error = RpcError::invalid_request(
            request.id.clone().unwrap_or(Value::Null),
            "expected JSON-RPC 2.0 request with a non-empty method",
        );
        return encode_response(&error).map(Some);
    }

    let Some(id) = request.id else {
        let _rpc = activity.rpc_lease();
        if let Err(err) = hub.handle_rpc(&request.method, request.params).await {
            warn!(method = %request.method, error = %err, "JSON-RPC notification failed");
        }
        return Ok(None);
    };

    let _rpc = activity.rpc_lease();
    let response = match hub.handle_rpc(&request.method, request.params).await {
        Ok(result) => encode_response(&RpcResponse::success(id, result)?)?,
        Err(err) => encode_response(&hub_error_to_rpc(id, err))?,
    };
    Ok(Some(response))
}

fn hub_error_to_rpc(id: Value, error: HubError) -> RpcError {
    let code = match &error {
        HubError::Other(message) if message.starts_with("unknown RPC method ") => METHOD_NOT_FOUND,
        HubError::NotFound { .. } => -32_004,
        HubError::Conflict(_) => -32_009,
        HubError::Json(_) => INVALID_REQUEST,
        _ => INTERNAL_ERROR,
    };
    RpcError::new(id, code, error.to_string(), None)
}

fn encode_response<T: Serialize>(message: &T) -> Result<Vec<u8>, HubError> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    Ok(line)
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
    fs::create_dir_all(home)?;
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
        let _ = daemon_id;
        home.join(SOCKET_FILE).to_string_lossy().into_owned()
    }
}

fn bind_listener(endpoint: &str) -> Result<LocalSocketListener, HubError> {
    let name = Path::new(endpoint).to_fs_name::<GenericFilePath>()?;
    Ok(ListenerOptions::new().name(name).create_tokio()?)
}

fn open_daemon_lock(home: &Path) -> Result<FdRwLock<File>, HubError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join(LOCK_FILE))?;
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
    if target.exists() {
        fs::remove_file(&target)?;
    }
    fs::rename(tmp, target)?;
    Ok(())
}

fn remove_stale_daemon_state(home: &Path) -> Result<(), HubError> {
    remove_file_if_exists(home.join(METADATA_FILE))?;
    remove_file_if_exists(home.join(ID_FILE))?;
    #[cfg(unix)]
    remove_file_if_exists(home.join(SOCKET_FILE))?;
    Ok(())
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
                unsafe {
                    setsid();
                }
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
