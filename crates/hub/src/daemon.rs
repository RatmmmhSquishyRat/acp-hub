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
        AUTH_REQUIRED_ERROR, CONFLICT_ERROR, INTERNAL_ERROR, INVALID_CURSOR_ERROR, INVALID_PARAMS,
        INVALID_REGISTRY_ERROR, JSONRPC_VERSION, MAX_RPC_LINE_BYTES, METHOD_NOT_FOUND,
        NOT_FOUND_ERROR, RESOURCE_LIMIT_ERROR, RESUME_LOAD_FAILED_ERROR, RpcError, RpcRequest,
        RpcResponse, STALE_CURSOR_ERROR, UNSUPPORTED_CAPABILITY_ERROR,
        UNSUPPORTED_PROTOCOL_VERSION_ERROR, UNSUPPORTED_PROXY_TRANSPORT_ERROR,
        typed_hub_error_data,
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
pub(crate) const MAX_RETAINED_RPC_BYTES_GLOBAL: usize = 128 * 1024 * 1024;
const MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL: usize = 87 * 1024 * 1024;
const MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL: usize = 40 * 1024 * 1024;
const MAX_RETAINED_RPC_FALLBACK_BYTES_GLOBAL: usize = MAX_RETAINED_RPC_BYTES_GLOBAL
    - MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL
    - MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL;
const RPC_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_FRAME_READ_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const DAEMON_HANDSHAKE_METHOD: &str = "hub/daemon/handshake";
/// Version of the daemon JSON-RPC contract, independent of crate SemVer.
/// Increment only when a client could misinterpret a successful response.
pub(crate) const DAEMON_RPC_PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonHandshakeRequest {
    pub(crate) protocol_version: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonHandshakeResponse {
    pub(crate) protocol_version: u32,
    pub(crate) compatible: bool,
    pub(crate) package_version: String,
}

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
    rpc_bytes: RpcByteAdmission,
    rpc_write_timeout: Duration,
    last_activity: Mutex<Instant>,
}

#[derive(Debug, Clone)]
struct RpcByteAdmission {
    requests: Arc<Semaphore>,
    responses: Arc<Semaphore>,
    fallbacks: Arc<Semaphore>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self::with_limits(
            MAX_DAEMON_CLIENTS,
            MAX_INFLIGHT_RPC_GLOBAL,
            MAX_BUFFERED_RPC_FRAMES_GLOBAL,
            MAX_RETAINED_RPC_REQUEST_BYTES_GLOBAL,
            MAX_RETAINED_RPC_RESPONSE_BYTES_GLOBAL,
            MAX_RETAINED_RPC_FALLBACK_BYTES_GLOBAL,
        )
    }

    fn with_limits(
        client_limit: usize,
        rpc_limit: usize,
        frame_slots: usize,
        request_byte_limit: usize,
        response_byte_limit: usize,
        fallback_byte_limit: usize,
    ) -> Self {
        Self::with_limits_and_timeout(
            client_limit,
            rpc_limit,
            frame_slots,
            request_byte_limit,
            response_byte_limit,
            fallback_byte_limit,
            RPC_WRITE_TIMEOUT,
        )
    }

    fn with_limits_and_timeout(
        client_limit: usize,
        rpc_limit: usize,
        frame_slots: usize,
        request_byte_limit: usize,
        response_byte_limit: usize,
        fallback_byte_limit: usize,
        rpc_write_timeout: Duration,
    ) -> Self {
        Self {
            active_clients: AtomicUsize::new(0),
            active_rpcs: AtomicUsize::new(0),
            active_runs: AtomicUsize::new(0),
            rpc_slots: Arc::new(Semaphore::new(rpc_limit)),
            client_slots: Arc::new(Semaphore::new(client_limit)),
            frame_slots: Arc::new(Semaphore::new(frame_slots)),
            rpc_bytes: RpcByteAdmission {
                requests: Arc::new(Semaphore::new(request_byte_limit)),
                responses: Arc::new(Semaphore::new(response_byte_limit)),
                fallbacks: Arc::new(Semaphore::new(fallback_byte_limit)),
            },
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
    // Only the process holding the singleton daemon lock can prove that no
    // live owner from this home still exists. Secondary CoreHub/Store readers
    // must never terminalize another process's active run.
    store.recover_interrupted_load_replays()?;
    store.recover_interrupted_runs()?;
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
            return poll_daemon(&home, STARTUP_TIMEOUT).await;
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => {}
        Err(err) => return Err(HubError::Io(err)),
    }
    poll_daemon_or_recover(&home, &mut lock, STARTUP_TIMEOUT).await
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

mod rpc_io;

use rpc_io::handle_client;

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

async fn poll_daemon_or_recover(
    home: &Path,
    lock: &mut FdRwLock<File>,
    timeout: Duration,
) -> Result<crate::rpc::RpcClient, HubError> {
    let started = Instant::now();
    while started.elapsed() <= timeout {
        if let Some(client) = try_connect_metadata(home).await {
            return Ok(client);
        }

        match lock.try_write() {
            Ok(guard) => {
                if let Some(client) = try_connect_metadata(home).await {
                    drop(guard);
                    return Ok(client);
                }
                // The daemon that held the lock exited between discovery and
                // connection. Become the new singleton owner instead of
                // polling stale metadata for the rest of the startup timeout.
                remove_stale_daemon_state(home)?;
                spawn_daemon(home)?;
                drop(guard);
                let remaining = timeout.saturating_sub(started.elapsed());
                return poll_daemon(home, remaining).await;
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(err) => return Err(HubError::Io(err)),
        }
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
        let listener = match options.mode(0o600).create_tokio() {
            Ok(listener) => listener,
            Err(error) if error.kind() == ErrorKind::Unsupported => {
                // macOS does not support setting a Unix-socket mode atomically
                // through Interprocess. The containing directory is already
                // owner-only, so create there and tighten the socket
                // immediately without exposing it to another user.
                let name = Path::new(endpoint).to_fs_name::<GenericFilePath>()?;
                ListenerOptions::new().name(name).create_tokio()?
            }
            Err(error) => return Err(HubError::Io(error)),
        };
        if let Err(error) = fs::set_permissions(endpoint, fs::Permissions::from_mode(0o600)) {
            drop(listener);
            let _ = fs::remove_file(endpoint);
            return Err(HubError::Io(error));
        }
        Ok(listener)
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
    let stderr = if std::env::var("ACP_HUB_DAEMON_STDERR").as_deref() == Ok("inherit") {
        Stdio::inherit()
    } else {
        Stdio::null()
    };
    command
        .arg("serve")
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);

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
