//! Client-side callback handlers and session/update capture.
//!
//! The Hub is the ACP *Client*. It answers agent-to-client requests
//! (`session/request_permission`, `fs/*`, `terminal/*`) and captures every
//! `session/update` notification into the projection store. All handlers share
//! `Arc<HubCtx>`. Session-scoped state is keyed by both endpoint id and the
//! endpoint-local `session_id`; ACP does not require session ids to be globally
//! unique across agents.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

#[allow(unused_imports)]
use agent_client_protocol::schema::v1::{
    AvailableCommandsUpdate, ConfigOptionUpdate, CreateTerminalRequest, CreateTerminalResponse,
    CurrentModeUpdate, EnvVariable, KillTerminalRequest, KillTerminalResponse,
    PermissionOptionKind, Plan, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionInfoUpdate,
    SessionNotification, SessionUpdate, TerminalExitStatus, TerminalId, TerminalOutputRequest,
    TerminalOutputResponse, UsageUpdate, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use parking_lot::{Mutex, RwLock};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::daemon::{ActivityLease, ActivityTracker};
use crate::endpoint::{AgentEndpointConfig, FsConfig, PermissionPolicy};
use crate::error::HubError;
use crate::rpc::RpcRequest;
use crate::store::{MessageSource, NewMessage, Store, search_body};

#[derive(Clone)]
pub struct SessionBinding {
    pub conv_id: String,
    pub agent_id: String,
    pub permission_policy: PermissionPolicy,
    pub fs: FsConfig,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionKey {
    agent_id: String,
    session_id: String,
}

impl SessionKey {
    fn new(agent_id: &str, session_id: &str) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
        }
    }
}

#[derive(Default)]
struct TerminalOutput {
    text: String,
    truncated: bool,
}

#[cfg(unix)]
struct ProcessTree {
    process_group: i32,
}

#[cfg(unix)]
impl ProcessTree {
    fn attach(child: &Child) -> Result<Self, HubError> {
        Ok(Self {
            process_group: i32::try_from(child.id())
                .map_err(|_| HubError::other("terminal process id is too large"))?,
        })
    }

    fn terminate(&self) -> Result<(), HubError> {
        let result = unsafe { libc::kill(-self.process_group, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error.into())
        }
    }
}

#[cfg(windows)]
mod windows_process_tree {
    use super::{Child, HubError};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    pub(super) struct ProcessTree {
        job: isize,
    }

    impl ProcessTree {
        pub(super) fn attach(child: &Child) -> Result<Self, HubError> {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(limits).cast(),
                    u32::try_from(std::mem::size_of_val(&limits))
                        .expect("job limit structure fits in u32"),
                )
            };
            let assigned = configured != 0
                && unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) } != 0;
            if !assigned {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(job);
                }
                return Err(error.into());
            }
            Ok(Self { job: job as isize })
        }

        pub(super) fn resume_initial_thread(child: &Child) -> Result<(), HubError> {
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut entry = THREADENTRY32 {
                dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>())
                    .expect("thread entry structure fits in u32"),
                ..THREADENTRY32::default()
            };
            let mut found = None;
            let first_entry = unsafe { Thread32First(snapshot, &mut entry) };
            if first_entry == 0 {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(snapshot);
                }
                return Err(error.into());
            }
            let mut has_entry = true;
            while has_entry {
                if entry.th32OwnerProcessID == child.id() {
                    found = Some(entry.th32ThreadID);
                    break;
                }
                has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
            }
            unsafe {
                CloseHandle(snapshot);
            }
            let thread_id = found.ok_or_else(|| {
                HubError::other(format!(
                    "cannot find the initial thread for suspended terminal process {}",
                    child.id()
                ))
            })?;
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
            if thread.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let previous_suspend_count = unsafe { ResumeThread(thread) };
            let resume_result = if previous_suspend_count == u32::MAX {
                Err(std::io::Error::last_os_error().into())
            } else if previous_suspend_count == 1 {
                Ok(())
            } else {
                Err(HubError::other(format!(
                    "unexpected terminal initial-thread suspend count {previous_suspend_count}"
                )))
            };
            unsafe {
                CloseHandle(thread);
            }
            resume_result
        }

        pub(super) fn terminate(&self) -> Result<(), HubError> {
            if unsafe { TerminateJobObject(self.job as HANDLE, 1) } == 0 {
                Err(std::io::Error::last_os_error().into())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.job as HANDLE);
            }
        }
    }
}

#[cfg(windows)]
use windows_process_tree::ProcessTree;

struct TerminalHandle {
    owner: SessionKey,
    child: Option<Child>,
    process_tree: Option<ProcessTree>,
    readers: Vec<thread::JoinHandle<()>>,
    output: Arc<Mutex<TerminalOutput>>,
    exit_status: Option<TerminalExitStatus>,
    _activity: Option<ActivityLease>,
    #[cfg(test)]
    reaped: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl TerminalHandle {
    fn cleanup(&mut self) -> Result<(), HubError> {
        let has_process_tree = self.process_tree.is_some();
        if let Some(process_tree) = &self.process_tree {
            process_tree.terminate()?;
        }
        if let Some(child) = self.child.as_mut() {
            let status = match child.try_wait()? {
                Some(status) => status,
                None => {
                    if !has_process_tree {
                        child.kill()?;
                    }
                    child.wait()?
                }
            };
            self.child = None;
            if let Some(exit) = make_exit(status) {
                self.exit_status = Some(exit);
            }
        }
        let mut reader_panicked = false;
        for reader in std::mem::take(&mut self.readers) {
            reader_panicked |= reader.join().is_err();
        }
        self.process_tree = None;
        if reader_panicked {
            return Err(HubError::other("terminal output reader panicked"));
        }
        Ok(())
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        let _cleaned = self.cleanup().is_ok();
        #[cfg(test)]
        if _cleaned && let Some(reaped) = &self.reaped {
            reaped.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

struct PendingNotification {
    connection_id: String,
    notification: SessionNotification,
    bytes: usize,
}

#[derive(Default)]
struct PendingNotificationState {
    sessions: HashMap<SessionKey, VecDeque<PendingNotification>>,
    draining: HashSet<SessionKey>,
    bytes: usize,
    count: usize,
}

#[derive(Clone, Default)]
struct CaptureBudget {
    updates: usize,
    bytes: usize,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CaptureFailureKey {
    session: SessionKey,
    connection_id: String,
}

struct CaptureFailure {
    operation: &'static str,
    run_id: Option<String>,
    first_error: Option<String>,
}

#[cfg(test)]
#[derive(Clone)]
struct CallbackTestGate {
    reached: Arc<std::sync::Barrier>,
    resume: Arc<std::sync::Barrier>,
}

#[cfg(test)]
impl CallbackTestGate {
    fn new() -> Self {
        Self {
            reached: Arc::new(std::sync::Barrier::new(2)),
            resume: Arc::new(std::sync::Barrier::new(2)),
        }
    }

    fn wait(self) {
        self.reached.wait();
        self.resume.wait();
    }
}

#[cfg(test)]
#[derive(Clone)]
struct TerminalSpawnTestGate {
    callback: CallbackTestGate,
    reaped: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
impl TerminalSpawnTestGate {
    fn new() -> Self {
        Self {
            callback: CallbackTestGate::new(),
            reaped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

const MAX_PENDING_SESSIONS: usize = 64;
const MAX_PENDING_NOTIFICATIONS: usize = 1_024;
const MAX_PENDING_NOTIFICATION_BYTES: usize = 4 * 1024 * 1024;
const MAX_PENDING_SINGLE_NOTIFICATION_BYTES: usize = 256 * 1024;
const MAX_CAPTURE_UPDATES_PER_TURN: usize = 4_096;
const MAX_CAPTURE_BYTES_PER_TURN: usize = 16 * 1024 * 1024;
const MAX_READ_TEXT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_WRITE_TEXT_FILE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_TERMINAL_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_TERMINAL_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TERMINALS_GLOBAL: usize = 64;
const MAX_TERMINALS_PER_SESSION: usize = 8;

#[derive(Clone)]
struct AgentConnection {
    connection_id: String,
    config: AgentEndpointConfig,
}

#[derive(Default)]
struct GenerationGate {
    commands: Arc<tokio::sync::RwLock<()>>,
    callbacks: Arc<tokio::sync::RwLock<()>>,
    waiting_writers: AtomicUsize,
    active_commands: AtomicUsize,
}

impl GenerationGate {
    fn writer_intent(self: &Arc<Self>) -> GenerationWriteIntent {
        self.waiting_writers.fetch_add(1, Ordering::SeqCst);
        GenerationWriteIntent {
            gate: Arc::clone(self),
        }
    }
}

struct GenerationWriteIntent {
    gate: Arc<GenerationGate>,
}

impl Drop for GenerationWriteIntent {
    fn drop(&mut self) {
        self.gate.waiting_writers.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) struct AgentConnectionLease {
    _guard: tokio::sync::OwnedRwLockReadGuard<()>,
    gate: Arc<GenerationGate>,
}

impl Drop for AgentConnectionLease {
    fn drop(&mut self) {
        self.gate.active_commands.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) type InboundConnectionLease = tokio::sync::OwnedRwLockReadGuard<()>;

pub(crate) struct AgentGenerationWriter {
    _intent: GenerationWriteIntent,
    _commands: tokio::sync::OwnedRwLockWriteGuard<()>,
    _callbacks: tokio::sync::OwnedRwLockWriteGuard<()>,
}

pub struct HubCtx {
    store: Store,
    sessions: RwLock<HashMap<SessionKey, SessionBinding>>,
    current_run: RwLock<HashMap<SessionKey, String>>,
    loading_sessions: RwLock<HashSet<SessionKey>>,
    pending_notifications: Mutex<PendingNotificationState>,
    capture_budgets: Mutex<HashMap<SessionKey, CaptureBudget>>,
    capture_failures: Mutex<HashMap<CaptureFailureKey, CaptureFailure>>,
    agent_connections: RwLock<HashMap<String, AgentConnection>>,
    generation_gates: Mutex<HashMap<String, Arc<GenerationGate>>>,
    activity: RwLock<Option<Arc<ActivityTracker>>>,
    notifications: broadcast::Sender<RpcRequest>,
    terminals: Mutex<HashMap<String, TerminalHandle>>,
    #[cfg(test)]
    bind_drain_gate: Mutex<Option<CallbackTestGate>>,
    #[cfg(test)]
    terminal_spawn_gate: Mutex<Option<TerminalSpawnTestGate>>,
    #[cfg(all(test, windows))]
    terminal_job_assignment_gate: Mutex<Option<CallbackTestGate>>,
    #[cfg(test)]
    terminal_kill_error_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    capture_after_connection_gate: Mutex<Option<CallbackTestGate>>,
}

impl HubCtx {
    pub fn new(store: Store) -> Arc<Self> {
        let (notifications, _) = broadcast::channel(1024);
        Arc::new(Self {
            store,
            sessions: RwLock::default(),
            current_run: RwLock::default(),
            loading_sessions: RwLock::default(),
            pending_notifications: Mutex::default(),
            capture_budgets: Mutex::default(),
            capture_failures: Mutex::default(),
            agent_connections: RwLock::default(),
            generation_gates: Mutex::default(),
            activity: RwLock::default(),
            notifications,
            terminals: Mutex::default(),
            #[cfg(test)]
            bind_drain_gate: Mutex::default(),
            #[cfg(test)]
            terminal_spawn_gate: Mutex::default(),
            #[cfg(all(test, windows))]
            terminal_job_assignment_gate: Mutex::default(),
            #[cfg(test)]
            terminal_kill_error_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            capture_after_connection_gate: Mutex::default(),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn set_activity_tracker(&self, activity: Arc<ActivityTracker>) {
        *self.activity.write() = Some(activity);
    }

    fn generation_gate(&self, agent_id: &str) -> Arc<GenerationGate> {
        Arc::clone(
            self.generation_gates
                .lock()
                .entry(agent_id.into())
                .or_default(),
        )
    }

    pub(crate) async fn acquire_connection_lease(
        &self,
        agent_id: &str,
        connection_id: &str,
    ) -> Result<AgentConnectionLease, HubError> {
        let gate = self.generation_gate(agent_id);
        let guard = Arc::clone(&gate.commands).read_owned().await;
        self.connection(agent_id, connection_id)?;
        gate.active_commands.fetch_add(1, Ordering::SeqCst);
        Ok(AgentConnectionLease {
            _guard: guard,
            gate,
        })
    }

    pub(crate) fn try_acquire_connection_lease(
        &self,
        agent_id: &str,
        connection_id: &str,
    ) -> Result<InboundConnectionLease, HubError> {
        let gate = self.generation_gate(agent_id);
        if gate.waiting_writers.load(Ordering::SeqCst) > 0
            && gate.active_commands.load(Ordering::SeqCst) == 0
        {
            return Err(HubError::other(format!(
                "agent {agent_id:?} connection replacement is pending"
            )));
        }
        let lease = Arc::clone(&gate.callbacks).try_read_owned().map_err(|_| {
            HubError::other(format!(
                "agent {agent_id:?} connection replacement is active"
            ))
        })?;
        self.connection(agent_id, connection_id)?;
        Ok(lease)
    }

    pub(crate) async fn agent_generation_writer(&self, agent_id: &str) -> AgentGenerationWriter {
        let gate = self.generation_gate(agent_id);
        let intent = gate.writer_intent();
        let commands = Arc::clone(&gate.commands).write_owned().await;
        let callbacks = Arc::clone(&gate.callbacks).write_owned().await;
        AgentGenerationWriter {
            _intent: intent,
            _commands: commands,
            _callbacks: callbacks,
        }
    }

    fn try_agent_generation_writer(
        &self,
        agent_id: &str,
    ) -> Result<AgentGenerationWriter, HubError> {
        let gate = self.generation_gate(agent_id);
        let intent = gate.writer_intent();
        let commands = Arc::clone(&gate.commands)
            .try_write_owned()
            .map_err(|_| HubError::other(format!("agent {agent_id:?} connection is busy")))?;
        let callbacks = Arc::clone(&gate.callbacks)
            .try_write_owned()
            .map_err(|_| HubError::other(format!("agent {agent_id:?} callbacks are busy")))?;
        Ok(AgentGenerationWriter {
            _intent: intent,
            _commands: commands,
            _callbacks: callbacks,
        })
    }

    pub fn configure_agent(
        &self,
        agent_id: &str,
        connection_id: &str,
        config: AgentEndpointConfig,
    ) -> Result<(), HubError> {
        let _generation = self.try_agent_generation_writer(agent_id)?;
        self.configure_agent_locked(agent_id, connection_id, config);
        Ok(())
    }

    pub(crate) async fn configure_agent_async(
        &self,
        agent_id: &str,
        connection_id: &str,
        config: AgentEndpointConfig,
    ) {
        let _generation = self.agent_generation_writer(agent_id).await;
        self.configure_agent_locked(agent_id, connection_id, config);
    }

    fn configure_agent_locked(
        &self,
        agent_id: &str,
        connection_id: &str,
        config: AgentEndpointConfig,
    ) {
        let mut connections = self.agent_connections.write();
        let replaced = connections
            .get(agent_id)
            .is_some_and(|connection| connection.connection_id != connection_id);
        if replaced {
            self.remove_agent_state(agent_id);
        }
        connections.insert(
            agent_id.into(),
            AgentConnection {
                connection_id: connection_id.into(),
                config,
            },
        );
    }

    pub fn revoke_agent(&self, agent_id: &str) -> Result<(), HubError> {
        let _generation = self.try_agent_generation_writer(agent_id)?;
        self.revoke_agent_locked(agent_id);
        Ok(())
    }

    pub(crate) fn revoke_agent_locked(&self, agent_id: &str) {
        self.agent_connections.write().remove(agent_id);
        self.remove_agent_state(agent_id);
    }

    pub(crate) async fn revoke_connection(&self, agent_id: &str, connection_id: &str) {
        if self
            .agent_connections
            .read()
            .get(agent_id)
            .is_none_or(|connection| connection.connection_id != connection_id)
        {
            return;
        }
        let _generation = self.agent_generation_writer(agent_id).await;
        let removed = {
            let mut connections = self.agent_connections.write();
            if connections
                .get(agent_id)
                .is_some_and(|connection| connection.connection_id == connection_id)
            {
                connections.remove(agent_id);
                true
            } else {
                false
            }
        };
        if removed {
            self.remove_agent_state(agent_id);
        }
    }

    fn connection(&self, agent_id: &str, connection_id: &str) -> Result<AgentConnection, HubError> {
        self.agent_connections
            .read()
            .get(agent_id)
            .filter(|connection| connection.connection_id == connection_id)
            .cloned()
            .ok_or_else(|| HubError::other(format!("stale connection for agent {agent_id:?}")))
    }

    fn remove_agent_state(&self, agent_id: &str) {
        self.sessions
            .write()
            .retain(|key, _| key.agent_id != agent_id);
        self.current_run
            .write()
            .retain(|key, _| key.agent_id != agent_id);
        self.loading_sessions
            .write()
            .retain(|key| key.agent_id != agent_id);
        self.capture_budgets
            .lock()
            .retain(|key, _| key.agent_id != agent_id);
        self.capture_failures
            .lock()
            .retain(|key, _| key.session.agent_id != agent_id);
        {
            let mut pending = self.pending_notifications.lock();
            let keys = pending
                .sessions
                .keys()
                .filter(|key| key.agent_id == agent_id)
                .cloned()
                .collect::<Vec<_>>();
            for key in keys {
                if let Some(queue) = pending.sessions.remove(&key) {
                    pending.count = pending.count.saturating_sub(queue.len());
                    pending.bytes = pending
                        .bytes
                        .saturating_sub(queue.iter().map(|entry| entry.bytes).sum());
                }
            }
            pending.draining.retain(|key| key.agent_id != agent_id);
        }
        let mut terminals = self.terminals.lock();
        let ids = terminals
            .iter()
            .filter_map(|(id, handle)| (handle.owner.agent_id == agent_id).then_some(id.clone()))
            .collect::<Vec<_>>();
        for id in ids {
            let cleaned = terminals
                .get_mut(&id)
                .is_some_and(|handle| handle.cleanup().is_ok());
            if cleaned {
                terminals.remove(&id);
            }
        }
    }

    pub fn subscribe_notifications(&self) -> broadcast::Receiver<RpcRequest> {
        self.notifications.subscribe()
    }

    /// Return whether this endpoint-local session currently has a callback binding.
    pub fn is_session_bound(&self, agent_id: &str, session_id: &str) -> bool {
        self.sessions
            .read()
            .contains_key(&SessionKey::new(agent_id, session_id))
    }

    pub fn bind_session(&self, session_id: &str, binding: SessionBinding) -> Result<(), HubError> {
        let key = SessionKey::new(&binding.agent_id, session_id);
        {
            let mut sessions = self.sessions.write();
            let mut state = self.pending_notifications.lock();
            if !state.draining.insert(key.clone()) {
                return Err(HubError::other(format!(
                    "session {session_id:?} is already being bound"
                )));
            }
            sessions.remove(&key);
            state.sessions.entry(key.clone()).or_default();
        }
        self.capture_failures
            .lock()
            .retain(|failure_key, _| failure_key.session != key);
        self.capture_budgets.lock().entry(key.clone()).or_default();

        #[cfg(test)]
        {
            let gate = self.bind_drain_gate.lock().take();
            if let Some(gate) = gate {
                gate.wait();
            }
        }

        loop {
            let entry = {
                let mut state = self.pending_notifications.lock();
                let entry = state.sessions.get_mut(&key).and_then(VecDeque::pop_front);
                if let Some(entry) = &entry {
                    state.count = state.count.saturating_sub(1);
                    state.bytes = state.bytes.saturating_sub(entry.bytes);
                }
                entry
            };

            let Some(entry) = entry else {
                let mut sessions = self.sessions.write();
                let mut state = self.pending_notifications.lock();
                if !state.draining.contains(&key) {
                    return Err(HubError::other(format!(
                        "session {session_id:?} was revoked while binding"
                    )));
                }
                if state
                    .sessions
                    .get(&key)
                    .is_some_and(|queue| !queue.is_empty())
                {
                    continue;
                }
                state.sessions.remove(&key);
                state.draining.remove(&key);
                sessions.insert(key.clone(), binding);
                return Ok(());
            };

            let connections = self.agent_connections.read();
            if connections
                .get(&key.agent_id)
                .is_none_or(|connection| connection.connection_id != entry.connection_id)
            {
                continue;
            }

            let budget_before = self
                .capture_budgets
                .lock()
                .get(&key)
                .cloned()
                .unwrap_or_default();
            let capture_result = self.capture_bound_notification(
                &key.agent_id,
                &key,
                session_id,
                &binding,
                entry.notification.clone(),
                entry.bytes,
            );
            drop(connections);
            if let Err(error) = capture_result {
                self.record_capture_failure(&key, &entry.connection_id, &error);
                let restored = {
                    let mut state = self.pending_notifications.lock();
                    if state.draining.remove(&key) {
                        state
                            .sessions
                            .entry(key.clone())
                            .or_default()
                            .push_front(entry);
                        state.count = state.count.saturating_add(1);
                        state.bytes = state.bytes.saturating_add(
                            state
                                .sessions
                                .get(&key)
                                .and_then(VecDeque::front)
                                .map_or(0, |entry| entry.bytes),
                        );
                        true
                    } else {
                        false
                    }
                };
                if restored {
                    self.capture_budgets
                        .lock()
                        .insert(key.clone(), budget_before);
                }
                return Err(error);
            }
        }
    }

    pub fn unbind_session(&self, agent_id: &str, session_id: &str) {
        let key = SessionKey::new(agent_id, session_id);
        self.sessions.write().remove(&key);
        self.current_run.write().remove(&key);
        self.loading_sessions.write().remove(&key);
        self.capture_budgets.lock().remove(&key);
        self.capture_failures
            .lock()
            .retain(|failure_key, _| failure_key.session != key);
        {
            let mut state = self.pending_notifications.lock();
            if let Some(pending) = state.sessions.remove(&key) {
                state.count = state.count.saturating_sub(pending.len());
                state.bytes = state
                    .bytes
                    .saturating_sub(pending.iter().map(|entry| entry.bytes).sum());
            }
            state.draining.remove(&key);
        }

        let mut terminals = self.terminals.lock();
        let ids = terminals
            .iter()
            .filter_map(|(id, handle)| (handle.owner == key).then_some(id.clone()))
            .collect::<Vec<_>>();
        for id in ids {
            let cleaned = terminals
                .get_mut(&id)
                .is_some_and(|handle| handle.cleanup().is_ok());
            if cleaned {
                terminals.remove(&id);
            }
        }
    }

    pub fn set_current_run(&self, agent_id: &str, session_id: &str, run_id: &str) {
        let key = SessionKey::new(agent_id, session_id);
        self.current_run.write().insert(key.clone(), run_id.into());
        self.capture_budgets
            .lock()
            .insert(key, CaptureBudget::default());
    }

    pub fn clear_current_run(&self, agent_id: &str, session_id: &str) {
        self.current_run
            .write()
            .remove(&SessionKey::new(agent_id, session_id));
    }

    pub(crate) fn begin_capture_operation(
        &self,
        agent_id: &str,
        connection_id: &str,
        session_id: &str,
        operation: &'static str,
    ) -> Result<(), HubError> {
        let connections = self.agent_connections.read();
        if connections
            .get(agent_id)
            .is_none_or(|connection| connection.connection_id != connection_id)
        {
            return Err(HubError::other(format!(
                "stale connection for agent {agent_id:?}"
            )));
        }
        let session = SessionKey::new(agent_id, session_id);
        let run_id = self.current_run.read().get(&session).cloned();
        self.capture_failures.lock().insert(
            CaptureFailureKey {
                session,
                connection_id: connection_id.into(),
            },
            CaptureFailure {
                operation,
                run_id,
                first_error: None,
            },
        );
        Ok(())
    }

    pub(crate) fn take_capture_failure(
        &self,
        agent_id: &str,
        connection_id: &str,
        session_id: &str,
    ) -> Option<HubError> {
        let failure = self.capture_failures.lock().remove(&CaptureFailureKey {
            session: SessionKey::new(agent_id, session_id),
            connection_id: connection_id.into(),
        })?;
        let source = failure.first_error?;
        let correlation = failure
            .run_id
            .map(|run_id| format!(" for run {run_id}"))
            .unwrap_or_default();
        Some(HubError::other(format!(
            "{}{} failed because a session update could not be captured: {source}",
            failure.operation, correlation
        )))
    }

    fn record_capture_failure(&self, key: &SessionKey, connection_id: &str, error: &HubError) {
        let mut failures = self.capture_failures.lock();
        let failure_key = CaptureFailureKey {
            session: key.clone(),
            connection_id: connection_id.into(),
        };
        if let Some(failure) = failures.get_mut(&failure_key)
            && failure.first_error.is_none()
        {
            failure.first_error = Some(error.to_string());
        }
    }

    fn binding(&self, agent_id: &str, session_id: &str) -> Result<SessionBinding, HubError> {
        self.sessions
            .read()
            .get(&SessionKey::new(agent_id, session_id))
            .cloned()
            .ok_or_else(|| {
                HubError::other(format!(
                    "unknown session {session_id:?} for agent {agent_id:?}"
                ))
            })
    }

    fn run_for_session(&self, agent_id: &str, session_id: &str) -> Option<String> {
        self.current_run
            .read()
            .get(&SessionKey::new(agent_id, session_id))
            .cloned()
    }

    /// Mark a session as currently in load-replay mode (Layer 1).
    pub fn set_loading(&self, agent_id: &str, session_id: &str, loading: bool) {
        let key = SessionKey::new(agent_id, session_id);
        if loading {
            self.loading_sessions.write().insert(key.clone());
            self.capture_budgets
                .lock()
                .insert(key, CaptureBudget::default());
        } else {
            self.loading_sessions.write().remove(&key);
        }
    }

    /// Check if a session is in load-replay mode.
    fn is_loading(&self, agent_id: &str, session_id: &str) -> bool {
        self.loading_sessions
            .read()
            .contains(&SessionKey::new(agent_id, session_id))
    }

    // ---- notification capture ----------------------------------------------

    pub fn handle_notification(
        &self,
        agent_id: &str,
        connection_id: &str,
        notif: SessionNotification,
    ) -> Result<(), HubError> {
        let key = SessionKey::new(agent_id, notif.session_id.to_string().as_str());
        let result = self.capture_notification(agent_id, connection_id, notif);
        if let Err(error) = &result {
            self.record_capture_failure(&key, connection_id, error);
        }
        result
    }

    fn capture_notification(
        &self,
        agent_id: &str,
        connection_id: &str,
        notif: SessionNotification,
    ) -> Result<(), HubError> {
        let connections = self.agent_connections.read();
        if connections
            .get(agent_id)
            .is_none_or(|connection| connection.connection_id != connection_id)
        {
            return Err(HubError::other(format!(
                "stale connection for agent {agent_id:?}"
            )));
        }
        #[cfg(test)]
        {
            let gate = self.capture_after_connection_gate.lock().take();
            if let Some(gate) = gate {
                gate.wait();
            }
        }
        let sid = notif.session_id.to_string();
        let key = SessionKey::new(agent_id, &sid);
        let bytes = serde_json::to_vec(&notif)?.len();
        if bytes > MAX_PENDING_SINGLE_NOTIFICATION_BYTES {
            return Err(HubError::other(format!(
                "session update exceeds the {MAX_PENDING_SINGLE_NOTIFICATION_BYTES}-byte limit"
            )));
        }
        let binding = {
            let sessions = self.sessions.read();
            match sessions.get(&key) {
                Some(binding) => binding.clone(),
                None => {
                    const MAX_PENDING_PER_SESSION: usize = 256;
                    let mut pending = self.pending_notifications.lock();
                    let is_new_session = !pending.sessions.contains_key(&key);
                    if is_new_session && pending.sessions.len() >= MAX_PENDING_SESSIONS {
                        return Err(HubError::other(
                            "too many unbound sessions have pending updates",
                        ));
                    }
                    if pending.count >= MAX_PENDING_NOTIFICATIONS
                        || pending.bytes.saturating_add(bytes) > MAX_PENDING_NOTIFICATION_BYTES
                    {
                        return Err(HubError::other("pending pre-bind update quota exceeded"));
                    }
                    let queue = pending.sessions.entry(key).or_default();
                    if queue.len() >= MAX_PENDING_PER_SESSION {
                        return Err(HubError::other(format!(
                            "too many updates arrived before session {sid:?} was bound"
                        )));
                    }
                    queue.push_back(PendingNotification {
                        connection_id: connection_id.into(),
                        notification: notif,
                        bytes,
                    });
                    pending.count += 1;
                    pending.bytes += bytes;
                    return Ok(());
                }
            }
        };
        let result = self.capture_bound_notification(agent_id, &key, &sid, &binding, notif, bytes);
        drop(connections);
        result
    }

    fn capture_bound_notification(
        &self,
        agent_id: &str,
        key: &SessionKey,
        sid: &str,
        binding: &SessionBinding,
        notif: SessionNotification,
        bytes: usize,
    ) -> Result<(), HubError> {
        {
            let mut budgets = self.capture_budgets.lock();
            let budget = budgets.entry(key.clone()).or_default();
            if budget.updates >= MAX_CAPTURE_UPDATES_PER_TURN
                || budget.bytes.saturating_add(bytes) > MAX_CAPTURE_BYTES_PER_TURN
            {
                return Err(HubError::other(format!(
                    "session update capture budget exceeded ({MAX_CAPTURE_UPDATES_PER_TURN} updates or {MAX_CAPTURE_BYTES_PER_TURN} bytes)"
                )));
            }
            budget.updates += 1;
            budget.bytes += bytes;
        }
        let conv_id = binding.conv_id.clone();
        let run_id = self.run_for_session(agent_id, sid);
        let source = if self.is_loading(agent_id, sid) {
            MessageSource::LoadReplay
        } else {
            MessageSource::LocalTurn
        };
        let update_json = serde_json::to_value(&notif.update)?;
        match notif.update {
            SessionUpdate::AgentMessageChunk(c) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                None,
                &c,
            ),
            SessionUpdate::UserMessageChunk(c) => {
                cap(&self.store, &conv_id, &run_id, source, "user", None, &c)
            }
            SessionUpdate::AgentThoughtChunk(c) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("thought"),
                &c,
            ),
            SessionUpdate::ToolCall(t) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("tool_call"),
                &t,
            ),
            SessionUpdate::ToolCallUpdate(u) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("tool_call_update"),
                &u,
            ),
            SessionUpdate::Plan(p) => self
                .store
                .set_plan_snapshot(&conv_id, &serde_json::to_value(&p)?),
            SessionUpdate::AvailableCommandsUpdate(cmds) => self
                .store
                .set_available_commands_snapshot(&conv_id, &serde_json::to_value(&cmds)?),
            SessionUpdate::CurrentModeUpdate(m) => {
                let mut patch = serde_json::Map::new();
                patch.insert("currentMode".into(), serde_json::to_value(&m)?);
                self.store
                    .apply_session_info(&conv_id, None, None, Some(&patch))?;
                Ok(())
            }
            SessionUpdate::ConfigOptionUpdate(c) => {
                let v = serde_json::to_value(&c.config_options)?;
                self.store.set_config_snapshot(&conv_id, &v, None)?;
                Ok(())
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                let title: Option<String> = info.title.value().map(|s| s.to_string());
                let updated: Option<String> = info.updated_at.value().map(|s| s.to_string());
                let t = title.as_deref();
                let u = updated.as_deref();
                let meta: Option<&serde_json::Map<String, serde_json::Value>> = info.meta.as_ref();
                self.store.apply_session_info(&conv_id, t, u, meta)?;
                Ok(())
            }
            SessionUpdate::UsageUpdate(u) => {
                let cost = serde_json::to_value(&u.cost)?;
                self.store.upsert_usage_snapshot(
                    &conv_id,
                    u.used as i64,
                    u.size as i64,
                    Some(&cost),
                )?;
                Ok(())
            }
            other => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("update"),
                &other,
            ),
        }?;
        let _ = self.notifications.send(RpcRequest::notification(
            "hub/conv/update",
            serde_json::json!({
                "agentId": agent_id,
                "sessionId": sid,
                "conversationId": conv_id,
                "runId": run_id,
                "source": source,
                "update": update_json,
            }),
        ));
        Ok(())
    }

    // ---- permission --------------------------------------------------------

    pub fn handle_permission(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, HubError> {
        self.connection(agent_id, connection_id)?;
        let policy = self
            .binding(agent_id, req.session_id.to_string().as_str())?
            .permission_policy;
        let outcome = match policy {
            PermissionPolicy::AutoAllow => first_option(req, true),
            PermissionPolicy::AutoCancel => RequestPermissionOutcome::Cancelled,
            PermissionPolicy::Reject => first_option(req, false),
        };
        Ok(RequestPermissionResponse::new(outcome))
    }

    // ---- fs ----------------------------------------------------------------

    pub fn handle_read_text_file(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &ReadTextFileRequest,
    ) -> Result<ReadTextFileResponse, HubError> {
        let binding = self.binding(agent_id, req.session_id.to_string().as_str())?;
        let advertised = self
            .connection(agent_id, connection_id)?
            .config
            .client_capabilities
            .fs
            .read_text_file;
        if !advertised || !binding.fs.read_text_file {
            return Err(HubError::other("fs/read_text_file not enabled"));
        }
        let path = resolve(&req.path, &binding.fs.allowed_roots, &binding.cwd)?;
        let mut file = fs::File::open(&path)
            .map_err(|e| HubError::other(format!("open {}: {e}", path.display())))?;
        let metadata = file.metadata()?;
        if metadata.len() > MAX_READ_TEXT_FILE_BYTES {
            return Err(HubError::other(format!(
                "file exceeds the {MAX_READ_TEXT_FILE_BYTES}-byte callback read limit"
            )));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_READ_TEXT_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| HubError::other(format!("read {}: {e}", path.display())))?;
        if bytes.len() as u64 > MAX_READ_TEXT_FILE_BYTES {
            return Err(HubError::other(format!(
                "file exceeds the {MAX_READ_TEXT_FILE_BYTES}-byte callback read limit"
            )));
        }
        let text = String::from_utf8(bytes)
            .map_err(|e| HubError::other(format!("read {}: {e}", path.display())))?;
        Ok(ReadTextFileResponse::new(slice_lines(
            &text, req.line, req.limit,
        )))
    }

    pub fn handle_write_text_file(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &WriteTextFileRequest,
    ) -> Result<WriteTextFileResponse, HubError> {
        let binding = self.binding(agent_id, req.session_id.to_string().as_str())?;
        let advertised = self
            .connection(agent_id, connection_id)?
            .config
            .client_capabilities
            .fs
            .write_text_file;
        if !advertised || !binding.fs.write_text_file {
            return Err(HubError::other("fs/write_text_file not enabled"));
        }
        if req.content.len() > MAX_WRITE_TEXT_FILE_BYTES {
            return Err(HubError::other(format!(
                "write content exceeds the {MAX_WRITE_TEXT_FILE_BYTES}-byte callback limit"
            )));
        }
        let path = resolve(&req.path, &binding.fs.allowed_roots, &binding.cwd)?;
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        write_text_no_follow(&path, req.content.as_bytes())?;
        Ok(WriteTextFileResponse::new())
    }

    // ---- terminal ----------------------------------------------------------

    pub fn handle_terminal_create(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, HubError> {
        let session_id = req.session_id.to_string();
        let owner = SessionKey::new(agent_id, &session_id);
        let connections = self.agent_connections.read();
        let connection = connections
            .get(agent_id)
            .filter(|connection| connection.connection_id == connection_id)
            .ok_or_else(|| HubError::other(format!("stale connection for agent {agent_id:?}")))?;
        let sessions = self.sessions.read();
        let binding = sessions.get(&owner).cloned().ok_or_else(|| {
            HubError::other(format!(
                "unknown session {session_id:?} for agent {agent_id:?}"
            ))
        })?;
        if !connection.config.client_capabilities.terminal {
            return Err(HubError::other("terminal capability not enabled"));
        }
        let cwd = req.cwd.clone().unwrap_or_else(|| binding.cwd.clone());
        let cwd = resolve(&cwd, &binding.fs.allowed_roots, &binding.cwd)?;
        let limit = match req.output_byte_limit {
            Some(limit) => {
                let limit = usize::try_from(limit)
                    .map_err(|_| HubError::other("terminal output limit is too large"))?;
                if limit > MAX_TERMINAL_OUTPUT_BYTES {
                    return Err(HubError::other(format!(
                        "terminal output limit exceeds {MAX_TERMINAL_OUTPUT_BYTES} bytes"
                    )));
                }
                limit
            }
            None => DEFAULT_TERMINAL_OUTPUT_BYTES,
        };
        {
            let terminals = self.terminals.lock();
            if terminals.len() >= MAX_TERMINALS_GLOBAL {
                return Err(HubError::other("global terminal quota exceeded"));
            }
            if terminals
                .values()
                .filter(|terminal| terminal.owner == owner)
                .count()
                >= MAX_TERMINALS_PER_SESSION
            {
                return Err(HubError::other("session terminal quota exceeded"));
            }
        }
        let mut cmd = Command::new(&req.command);
        cmd.args(&req.args);
        for ev in &req.env {
            cmd.env(&ev.name, &ev.value);
        }
        cmd.current_dir(&cwd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_SUSPENDED,
            );
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| HubError::other(format!("spawn: {e}")))?;
        #[cfg(all(test, windows))]
        if let Some(gate) = self.terminal_job_assignment_gate.lock().take() {
            gate.wait();
        }
        let process_tree = match ProcessTree::attach(&child) {
            Ok(process_tree) => process_tree,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        #[cfg(windows)]
        if let Err(error) = ProcessTree::resume_initial_thread(&child) {
            let _ = process_tree.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let id = format!("term-{}", Uuid::new_v4().simple());
        let output = Arc::new(Mutex::new(TerminalOutput::default()));
        let mut readers = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_output_reader(stdout, Arc::clone(&output), limit));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_output_reader(stderr, Arc::clone(&output), limit));
        }
        let activity = self
            .activity
            .read()
            .as_ref()
            .map(|tracker| tracker.run_lease());
        #[cfg(test)]
        let reaped = {
            let gate = self.terminal_spawn_gate.lock().take();
            gate.map(|gate| {
                let reaped = Arc::clone(&gate.reaped);
                gate.callback.wait();
                reaped
            })
        };
        let mut terminal = TerminalHandle {
            owner: owner.clone(),
            child: Some(child),
            process_tree: Some(process_tree),
            readers,
            output,
            exit_status: None,
            _activity: activity,
            #[cfg(test)]
            reaped,
        };
        let mut terminals = self.terminals.lock();
        if terminals.len() >= MAX_TERMINALS_GLOBAL
            || terminals
                .values()
                .filter(|terminal| terminal.owner == owner)
                .count()
                >= MAX_TERMINALS_PER_SESSION
        {
            drop(terminals);
            terminal.cleanup()?;
            return Err(HubError::other("terminal quota exceeded during creation"));
        }
        terminals.insert(id.clone(), terminal);
        drop(terminals);
        drop(sessions);
        drop(connections);
        Ok(CreateTerminalResponse::new(TerminalId::new(id)))
    }

    pub fn handle_terminal_output(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &TerminalOutputRequest,
    ) -> Result<TerminalOutputResponse, HubError> {
        let tid = req.terminal_id.to_string();
        let session_id = req.session_id.to_string();
        self.verify_terminal_owner(agent_id, connection_id, &session_id, &tid)?;
        let mut terms = self.terminals.lock();
        let h = terms
            .get_mut(&tid)
            .ok_or_else(|| HubError::other("unknown terminal"))?;
        if let Some(child) = h.child.as_mut()
            && let Some(status) = child.try_wait()?
        {
            h.exit_status = make_exit(status);
            h.child = None;
        }
        let output = h.output.lock();
        let mut resp = TerminalOutputResponse::new(output.text.clone(), output.truncated);
        resp.exit_status = h.exit_status.clone();
        Ok(resp)
    }

    pub async fn handle_terminal_wait(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse, HubError> {
        let tid = req.terminal_id.to_string();
        let session_id = req.session_id.to_string();
        loop {
            let generation = self.try_acquire_connection_lease(agent_id, connection_id)?;
            self.verify_terminal_owner(agent_id, connection_id, &session_id, &tid)?;
            let exit = {
                let mut terms = self.terminals.lock();
                let h = terms
                    .get_mut(&tid)
                    .ok_or_else(|| HubError::other("unknown terminal"))?;
                if let Some(exit) = &h.exit_status {
                    return Ok(WaitForTerminalExitResponse::new(exit.clone()));
                }
                let child = h
                    .child
                    .as_mut()
                    .ok_or_else(|| HubError::other("terminal has no process or exit status"))?;
                child.try_wait()?.and_then(make_exit)
            };
            if let Some(exit) = exit {
                let mut terms = self.terminals.lock();
                let h = terms
                    .get_mut(&tid)
                    .ok_or_else(|| HubError::other("terminal released while waiting"))?;
                h.child = None;
                h.exit_status = Some(exit.clone());
                return Ok(WaitForTerminalExitResponse::new(exit));
            }
            drop(generation);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    pub fn handle_terminal_kill(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &KillTerminalRequest,
    ) -> Result<KillTerminalResponse, HubError> {
        let tid = req.terminal_id.to_string();
        let session_id = req.session_id.to_string();
        self.verify_terminal_owner(agent_id, connection_id, &session_id, &tid)?;
        let mut terminals = self.terminals.lock();
        let handle = terminals
            .get_mut(&tid)
            .ok_or_else(|| HubError::other("unknown terminal"))?;
        #[cfg(test)]
        if self
            .terminal_kill_error_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(HubError::other("forced terminal kill failure"));
        }
        handle.cleanup()?;
        Ok(KillTerminalResponse::new())
    }

    pub fn handle_terminal_release(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse, HubError> {
        let tid = req.terminal_id.to_string();
        let session_id = req.session_id.to_string();
        self.verify_terminal_owner(agent_id, connection_id, &session_id, &tid)?;
        let mut terminals = self.terminals.lock();
        let handle = terminals
            .get_mut(&tid)
            .ok_or_else(|| HubError::other("unknown terminal"))?;
        handle.cleanup()?;
        terminals.remove(&tid);
        Ok(ReleaseTerminalResponse::new())
    }

    fn verify_terminal_owner(
        &self,
        agent_id: &str,
        connection_id: &str,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<(), HubError> {
        self.connection(agent_id, connection_id)?;
        let owner = self
            .terminals
            .lock()
            .get(terminal_id)
            .map(|h| h.owner.clone())
            .ok_or_else(|| HubError::other("unknown terminal"))?;
        let caller = SessionKey::new(agent_id, session_id);
        if owner != caller {
            let scope = if owner.agent_id != caller.agent_id {
                "agent"
            } else {
                "session"
            };
            return Err(HubError::other(format!(
                "terminal belongs to another {scope}"
            )));
        }
        if !self.sessions.read().contains_key(&owner) {
            return Err(HubError::other("terminal session is no longer bound"));
        }
        Ok(())
    }
}

// ---- helpers --------------------------------------------------------------

fn cap(
    store: &Store,
    conv: &str,
    run: &Option<String>,
    source: MessageSource,
    role: &str,
    kind: Option<&str>,
    p: &impl serde::Serialize,
) -> Result<(), HubError> {
    let val = serde_json::to_value(p)?;
    let id = format!("msg-{}", Uuid::new_v4().simple());
    let body = search_body(&val);
    store
        .append_message(&NewMessage {
            id,
            conv_id: conv.into(),
            run_id: run.clone(),
            source,
            role: role.into(),
            kind: kind.map(str::to_string),
            content_json: val,
            body_text: body,
        })
        .map(|_| ())
}

fn first_option(req: &RequestPermissionRequest, allow: bool) -> RequestPermissionOutcome {
    let desired: &[PermissionOptionKind] = if allow {
        &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ]
    } else {
        &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]
    };
    for opt in &req.options {
        if desired.contains(&opt.kind) {
            return RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                opt.option_id.clone(),
            ));
        }
    }
    RequestPermissionOutcome::Cancelled
}

fn resolve(path: &Path, roots: &[PathBuf], cwd: &Path) -> Result<PathBuf, HubError> {
    let r = if path.is_absolute() {
        path.into()
    } else {
        cwd.join(path)
    };
    let c = match r.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            // Target doesn't exist yet (e.g. writing a new file): canonicalize the
            // existing parent and re-attach the leaf component, so the allowed-roots
            // check below still confines the write.
            //
            // If the leaf already exists but cannot be canonicalized, it may be a
            // dangling symlink. Treat it as an invalid target instead of re-attaching
            // it: a later write would follow that symlink outside the allowed root.
            if std::fs::symlink_metadata(&r).is_ok() {
                return Err(HubError::other(format!(
                    "resolve {}: existing target could not be canonicalized",
                    r.display()
                )));
            }
            let parent = r.parent().unwrap_or_else(|| Path::new(""));
            let leaf = r
                .file_name()
                .ok_or_else(|| HubError::other(format!("invalid path: {}", r.display())))?;
            let pc = parent
                .canonicalize()
                .map_err(|e| HubError::other(format!("resolve {}: {e}", r.display())))?;
            pc.join(leaf)
        }
    };
    let allowed: Vec<PathBuf> = if roots.is_empty() {
        vec![cwd.canonicalize().unwrap_or_else(|_| cwd.into())]
    } else {
        roots
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect()
    };
    for root in &allowed {
        if c.starts_with(root) {
            return Ok(c);
        }
    }
    Err(HubError::other(format!(
        "{} outside allowed roots",
        c.display()
    )))
}

fn write_text_no_follow(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself instead of following a final-component
        // symlink/junction that appeared between resolve() and open().
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    if file.metadata()?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to write through a symlink",
        ));
    }
    file.write_all(content)
}

fn slice_lines(text: &str, line: Option<u32>, limit: Option<u32>) -> String {
    match (line, limit) {
        (None, None) | (Some(0), _) => text.into(),
        _ => {
            let s = line.unwrap_or(1) as usize;
            let n = limit.map(|l| l as usize).unwrap_or(usize::MAX);
            text.lines()
                .skip(s.saturating_sub(1))
                .take(n)
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

fn spawn_output_reader(
    mut reader: impl Read + Send + 'static,
    output: Arc<Mutex<TerminalOutput>>,
    limit: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut state = output.lock();
            state.text.push_str(&String::from_utf8_lossy(&buf[..n]));
            truncate_from_start(&mut state, limit);
        }
    })
}

fn truncate_from_start(state: &mut TerminalOutput, limit: usize) {
    if state.text.len() <= limit {
        return;
    }
    state.truncated = true;
    if limit == 0 {
        state.text.clear();
        return;
    }
    let mut start = state.text.len().saturating_sub(limit);
    while start < state.text.len() && !state.text.is_char_boundary(start) {
        start += 1;
    }
    state.text.drain(..start);
}

fn exit_code(s: &std::process::ExitStatus) -> Option<u32> {
    if let Some(c) = s.code() {
        return Some(c as u32);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // Encode signal terminations as 128 + signum (shell convention); on Unix
        // ExitStatus::code() returns None when the process was killed by a signal.
        s.signal().map(|sig| 128u32 + sig as u32)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn make_exit(s: std::process::ExitStatus) -> Option<TerminalExitStatus> {
    Some(TerminalExitStatus::new().exit_code(exit_code(&s)))
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::endpoint::{AgentTransport, ClientCapabilityConfig};
    use crate::store::NewConversation;
    use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};
    use std::collections::BTreeMap;

    fn context() -> (Arc<HubCtx>, PathBuf) {
        let home = std::env::temp_dir().join(format!("acp-hub-callbacks-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create test home");
        let store = Store::open(&home).expect("open test store");
        (HubCtx::new(store), home)
    }

    fn binding(agent_id: &str, conv_id: &str, cwd: &Path) -> SessionBinding {
        SessionBinding {
            conv_id: conv_id.into(),
            agent_id: agent_id.into(),
            permission_policy: PermissionPolicy::Reject,
            fs: FsConfig {
                read_text_file: true,
                write_text_file: true,
                allowed_roots: vec![cwd.into()],
            },
            cwd: cwd.into(),
        }
    }

    fn config(read: bool, terminal: bool) -> AgentEndpointConfig {
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "unused".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: PermissionPolicy::Reject,
            client_capabilities: ClientCapabilityConfig {
                fs: FsConfig {
                    read_text_file: read,
                    write_text_file: read,
                    allowed_roots: Vec::new(),
                },
                terminal,
            },
        }
    }

    #[test]
    fn same_session_id_is_isolated_by_agent() {
        let (ctx, home) = context();
        ctx.bind_session("shared", binding("agent-a", "conv-a", &home))
            .unwrap();
        ctx.bind_session("shared", binding("agent-b", "conv-b", &home))
            .unwrap();

        ctx.set_current_run("agent-a", "shared", "run-a");
        ctx.set_loading("agent-b", "shared", true);

        assert_eq!(ctx.binding("agent-a", "shared").unwrap().conv_id, "conv-a");
        assert_eq!(ctx.binding("agent-b", "shared").unwrap().conv_id, "conv-b");
        assert_eq!(
            ctx.run_for_session("agent-a", "shared").as_deref(),
            Some("run-a")
        );
        assert_eq!(ctx.run_for_session("agent-b", "shared"), None);
        assert!(!ctx.is_loading("agent-a", "shared"));
        assert!(ctx.is_loading("agent-b", "shared"));

        ctx.unbind_session("agent-a", "shared");
        assert!(ctx.binding("agent-a", "shared").is_err());
        assert!(ctx.binding("agent-b", "shared").is_ok());

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn fs_capability_is_scoped_to_the_calling_agent() {
        let (ctx, home) = context();
        let file = home.join("visible.txt");
        fs::write(&file, "scoped").expect("write fixture");
        ctx.configure_agent("agent-a", "connection-a", config(true, false))
            .unwrap();
        ctx.configure_agent("agent-b", "connection-b", config(false, false))
            .unwrap();
        ctx.bind_session("shared", binding("agent-a", "conv-a", &home))
            .unwrap();
        ctx.bind_session("shared", binding("agent-b", "conv-b", &home))
            .unwrap();
        let request = ReadTextFileRequest::new(SessionId::new("shared"), &file);

        assert_eq!(
            ctx.handle_read_text_file("agent-a", "connection-a", &request)
                .unwrap()
                .content,
            "scoped"
        );
        assert!(
            ctx.handle_read_text_file("agent-b", "connection-b", &request)
                .unwrap_err()
                .to_string()
                .contains("not enabled")
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn filesystem_callbacks_write_and_read_inside_the_bound_root() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, false))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let file = home.join("callback.txt");

        ctx.handle_write_text_file(
            "agent-a",
            "connection-a",
            &WriteTextFileRequest::new(SessionId::new("session"), &file, "callback body"),
        )
        .unwrap();
        let response = ctx
            .handle_read_text_file(
                "agent-a",
                "connection-a",
                &ReadTextFileRequest::new(SessionId::new("session"), &file),
            )
            .unwrap();
        assert_eq!(response.content, "callback body");

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn replaced_connection_cannot_inherit_bound_session_permissions() {
        let (ctx, home) = context();
        let file = home.join("visible.txt");
        fs::write(&file, "scoped").unwrap();
        ctx.configure_agent("agent-a", "old-connection", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        ctx.configure_agent("agent-a", "new-connection", config(false, false))
            .unwrap();
        let request = ReadTextFileRequest::new(SessionId::new("session"), &file);

        for connection_id in ["old-connection", "new-connection"] {
            let error = ctx
                .handle_read_text_file("agent-a", connection_id, &request)
                .expect_err("replacement must revoke the old bound session");
            assert!(
                error.to_string().contains("unknown session"),
                "unexpected replacement error for {connection_id}: {error}"
            );
        }
        assert!(!ctx.is_session_bound("agent-a", "session"));

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn terminal_requires_advertised_agent_capability() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, false))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let request = CreateTerminalRequest::new(SessionId::new("session"), "unused");

        let error = ctx
            .handle_terminal_create("agent-a", "connection-a", &request)
            .expect_err("disabled terminal must be rejected");
        assert!(error.to_string().contains("not enabled"));

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn terminal_ids_are_scoped_to_the_bound_agent_and_session() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.configure_agent("agent-b", "connection-b", config(true, true))
            .unwrap();
        ctx.bind_session("shared", binding("agent-a", "conv-a", &home))
            .unwrap();
        ctx.bind_session("shared", binding("agent-b", "conv-b", &home))
            .unwrap();
        ctx.terminals.lock().insert(
            "terminal-a".into(),
            TerminalHandle {
                owner: SessionKey::new("agent-a", "shared"),
                child: None,
                process_tree: None,
                readers: Vec::new(),
                output: Arc::new(Mutex::new(TerminalOutput::default())),
                exit_status: None,
                _activity: None,
                reaped: None,
            },
        );

        ctx.verify_terminal_owner("agent-a", "connection-a", "shared", "terminal-a")
            .expect("owner can access terminal");
        assert!(
            ctx.verify_terminal_owner("agent-b", "connection-b", "shared", "terminal-a")
                .unwrap_err()
                .to_string()
                .contains("another agent")
        );

        ctx.unbind_session("agent-a", "shared");
        assert!(
            ctx.verify_terminal_owner("agent-a", "connection-a", "shared", "terminal-a")
                .unwrap_err()
                .to_string()
                .contains("unknown terminal")
        );
        assert!(ctx.binding("agent-b", "shared").is_ok());

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[ignore = "spawned as a descendant that keeps terminal pipes open"]
    #[test]
    fn terminal_descendant_holds_pipe_fixture() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[ignore = "spawned as a terminal parent fixture"]
    #[test]
    // Intentionally orphan the descendant: the regression requires the terminal parent to exit
    // while its descendant remains alive with inherited output pipes.
    #[allow(clippy::zombie_processes)]
    fn terminal_parent_spawns_pipe_holding_descendant_fixture() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "callbacks::state_tests::terminal_descendant_holds_pipe_fixture",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn pipe-holding descendant");
        println!("acp-hub-descendant-pid={}", child.id());
    }

    #[test]
    fn terminal_child_fixture() {
        println!("acp-hub-terminal-fixture");
    }

    #[ignore = "spawned as a long-lived terminal child fixture"]
    #[test]
    fn terminal_long_lived_child_fixture() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[ignore = "spawned as a short-lived pre-assignment descendant fixture"]
    #[test]
    fn terminal_short_lived_descendant_fixture() {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    #[ignore = "spawned as an immediate descendant parent fixture"]
    #[test]
    // Intentionally keep the child handle unreaped: the terminal cleanup under test must kill
    // this parent and its descendant as one Job/process tree.
    #[allow(clippy::zombie_processes)]
    fn terminal_immediate_descendant_parent_fixture() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "callbacks::state_tests::terminal_short_lived_descendant_fixture",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn immediate descendant");
        let marker =
            std::env::var_os("ACP_HUB_TERMINAL_ASSIGNMENT_MARKER").expect("assignment marker path");
        fs::write(marker, child.id().to_string()).expect("write descendant marker");
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_callbacks_capture_output_wait_and_release() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let executable = std::env::current_exe().unwrap();
        let request = CreateTerminalRequest::new(
            SessionId::new("session"),
            executable.to_string_lossy().into_owned(),
        )
        .args(vec![
            "--exact".to_string(),
            "callbacks::state_tests::terminal_child_fixture".to_string(),
            "--nocapture".to_string(),
        ])
        .cwd(home.clone())
        .output_byte_limit(64 * 1024);
        let created = ctx
            .handle_terminal_create("agent-a", "connection-a", &request)
            .unwrap();
        let terminal_id = created.terminal_id;

        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            ctx.handle_terminal_wait(
                "agent-a",
                "connection-a",
                &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
            ),
        )
        .await
        .unwrap()
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let output = ctx
                .handle_terminal_output(
                    "agent-a",
                    "connection-a",
                    &TerminalOutputRequest::new(SessionId::new("session"), terminal_id.clone()),
                )
                .unwrap();
            if output.output.contains("acp-hub-terminal-fixture") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "terminal output readers did not publish the fixture output"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        ctx.handle_terminal_release(
            "agent-a",
            "connection-a",
            &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
        )
        .unwrap();

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_terminal_kill_preserves_reaped_exit_status() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let executable = std::env::current_exe().unwrap();
        let request = CreateTerminalRequest::new(
            SessionId::new("session"),
            executable.to_string_lossy().into_owned(),
        )
        .args(vec![
            "--exact".to_string(),
            "callbacks::state_tests::terminal_child_fixture".to_string(),
            "--nocapture".to_string(),
        ])
        .cwd(home.clone());
        let terminal_id = ctx
            .handle_terminal_create("agent-a", "connection-a", &request)
            .unwrap()
            .terminal_id;
        let original = ctx
            .handle_terminal_wait(
                "agent-a",
                "connection-a",
                &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
            )
            .await
            .unwrap()
            .exit_status;

        let kill = KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone());
        ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
            .unwrap();
        ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
            .unwrap();

        let after_repeated_kill = ctx
            .handle_terminal_wait(
                "agent-a",
                "connection-a",
                &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
            )
            .await
            .expect("cached exit status must survive repeated kill calls")
            .exit_status;
        assert_eq!(after_repeated_kill.exit_code, original.exit_code);
        ctx.handle_terminal_release(
            "agent-a",
            "connection-a",
            &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
        )
        .unwrap();

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_kill_error_retains_child_for_retry() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let executable = std::env::current_exe().unwrap();
        let request = CreateTerminalRequest::new(
            SessionId::new("session"),
            executable.to_string_lossy().into_owned(),
        )
        .args(vec![
            "--ignored".to_string(),
            "--exact".to_string(),
            "callbacks::state_tests::terminal_long_lived_child_fixture".to_string(),
            "--nocapture".to_string(),
        ])
        .cwd(home.clone());
        let terminal_id = ctx
            .handle_terminal_create("agent-a", "connection-a", &request)
            .unwrap()
            .terminal_id;
        let terminal_key = terminal_id.to_string();
        let kill = KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone());
        ctx.terminal_kill_error_once
            .store(true, std::sync::atomic::Ordering::SeqCst);

        ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
            .expect_err("forced kill error");
        assert!(
            ctx.terminals
                .lock()
                .get(&terminal_key)
                .and_then(|handle| handle.child.as_ref())
                .is_some(),
            "a fallible kill must retain the child handle for retry"
        );

        ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
            .expect("retry terminal kill");
        ctx.handle_terminal_release(
            "agent-a",
            "connection-a",
            &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
        )
        .unwrap();

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_kill_reaps_descendants_before_joining_output_readers() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let request = CreateTerminalRequest::new(
            SessionId::new("session"),
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )
        .args(vec![
            "--ignored".to_string(),
            "--exact".to_string(),
            "callbacks::state_tests::terminal_parent_spawns_pipe_holding_descendant_fixture"
                .to_string(),
            "--nocapture".to_string(),
        ])
        .cwd(home.clone());
        let terminal_id = ctx
            .handle_terminal_create("agent-a", "connection-a", &request)
            .unwrap()
            .terminal_id;
        ctx.handle_terminal_wait(
            "agent-a",
            "connection-a",
            &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
        )
        .await
        .expect("terminal parent exits while its descendant keeps the pipes open");

        let terminal_key = terminal_id.to_string();
        let kill = KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone());
        let kill_ctx = Arc::clone(&ctx);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::task::spawn_blocking(move || {
                kill_ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
            }),
        )
        .await
        .expect("process-tree cleanup must not wait for the descendant's sleep")
        .expect("kill task")
        .expect("kill terminal process tree");

        {
            let terminals = ctx.terminals.lock();
            let handle = terminals
                .get(&terminal_key)
                .expect("terminal remains cached");
            assert!(handle.child.is_none(), "terminal parent must be reaped");
            assert!(
                handle.process_tree.is_none(),
                "the process tree guard must be closed after termination"
            );
            assert!(
                handle.readers.is_empty(),
                "all terminal output readers must be joined before kill returns"
            );
        }
        ctx.handle_terminal_release(
            "agent-a",
            "connection-a",
            &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
        )
        .unwrap();

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_is_suspended_until_job_assignment() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let marker = home.join("descendant-started");
        let gate = CallbackTestGate::new();
        *ctx.terminal_job_assignment_gate.lock() = Some(gate.clone());
        let request = CreateTerminalRequest::new(
            SessionId::new("session"),
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )
        .args(vec![
            "--ignored".to_string(),
            "--exact".to_string(),
            "callbacks::state_tests::terminal_immediate_descendant_parent_fixture".to_string(),
            "--nocapture".to_string(),
        ])
        .env(vec![EnvVariable::new(
            "ACP_HUB_TERMINAL_ASSIGNMENT_MARKER",
            marker.to_string_lossy().into_owned(),
        )])
        .cwd(home.clone());
        let create_ctx = Arc::clone(&ctx);
        let create = thread::spawn(move || {
            create_ctx.handle_terminal_create("agent-a", "connection-a", &request)
        });

        gate.reached.wait();
        let premature_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !marker.exists() && std::time::Instant::now() < premature_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ran_before_assignment = marker.exists();
        gate.resume.wait();
        let terminal_id = create
            .join()
            .expect("terminal create thread")
            .expect("create terminal after job assignment")
            .terminal_id;

        let started_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.exists() && std::time::Instant::now() < started_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            marker.exists(),
            "terminal must run after assignment and resume"
        );
        let terminal_key = terminal_id.to_string();
        ctx.handle_terminal_kill(
            "agent-a",
            "connection-a",
            &KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone()),
        )
        .expect("kill assigned terminal tree");
        {
            let terminals = ctx.terminals.lock();
            let handle = terminals.get(&terminal_key).expect("cached terminal");
            assert!(handle.readers.is_empty(), "kill must join terminal readers");
            assert!(handle.process_tree.is_none(), "kill must close the Job");
        }
        ctx.handle_terminal_release(
            "agent-a",
            "connection-a",
            &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
        )
        .unwrap();
        assert!(
            !ran_before_assignment,
            "terminal spawned its descendant before Job assignment"
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn terminal_spawn_racing_unbind_is_reaped_without_consuming_quota() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(true, true))
            .unwrap();
        ctx.bind_session("session", binding("agent-a", "conv-a", &home))
            .unwrap();
        let gate = TerminalSpawnTestGate::new();
        *ctx.terminal_spawn_gate.lock() = Some(gate.clone());
        let executable = std::env::current_exe().unwrap();
        let request = CreateTerminalRequest::new(
            SessionId::new("session"),
            executable.to_string_lossy().into_owned(),
        )
        .args(vec![
            "--ignored".to_string(),
            "--exact".to_string(),
            "callbacks::state_tests::terminal_long_lived_child_fixture".to_string(),
            "--nocapture".to_string(),
        ])
        .cwd(home.clone());
        let create_ctx = Arc::clone(&ctx);
        let create = thread::spawn(move || {
            create_ctx.handle_terminal_create("agent-a", "connection-a", &request)
        });

        gate.callback.reached.wait();
        let teardown_started = Arc::new(std::sync::Barrier::new(2));
        let teardown_ctx = Arc::clone(&ctx);
        let teardown_marker = Arc::clone(&teardown_started);
        let teardown = thread::spawn(move || {
            teardown_marker.wait();
            teardown_ctx.unbind_session("agent-a", "session");
        });
        teardown_started.wait();
        gate.callback.resume.wait();

        let _ = create.join().expect("terminal create thread");
        teardown.join().expect("session teardown thread");
        assert!(
            gate.reaped.load(std::sync::atomic::Ordering::SeqCst),
            "teardown must kill and reap the spawned child"
        );
        assert!(
            ctx.terminals.lock().is_empty(),
            "teardown must remove the terminal and release its quota slot"
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn terminal_output_limit_keeps_utf8_tail() {
        let mut state = TerminalOutput {
            text: "prefix-你好-tail".into(),
            truncated: false,
        };

        truncate_from_start(&mut state, 8);

        assert!(state.truncated);
        assert!(state.text.is_char_boundary(0));
        assert!(state.text.len() <= 8);
        assert!(state.text.ends_with("-tail"));
    }

    #[test]
    fn update_before_new_session_response_flushes_after_parent_is_created() {
        let (ctx, home) = context();
        let mut notifications = ctx.subscribe_notifications();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        let update = SessionNotification::new(
            SessionId::new("new-session"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("early update"),
            ))),
        );

        ctx.handle_notification("agent-a", "connection-a", update)
            .expect("queue pre-bind update");
        assert!(matches!(
            notifications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-a".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "new-session".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .expect("create parent conversation");
        ctx.bind_session("new-session", binding("agent-a", "conv-a", &home))
            .expect("bind and flush update");

        let event = notifications.try_recv().expect("streamed notification");
        assert_eq!(event.method, "hub/conv/update");
        assert_eq!(event.params["conversationId"], "conv-a");
        let messages = ctx.store().messages("conv-a", false).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].body_text.contains("early update"));

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn notifications_arriving_during_bind_drain_preserve_protocol_order() {
        fn message(session_id: &str, text: &str) -> SessionNotification {
            SessionNotification::new(
                SessionId::new(session_id),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text),
                ))),
            )
        }

        let (ctx, home) = context();
        let mut notifications = ctx.subscribe_notifications();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-order".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-order".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.handle_notification("agent-a", "connection-a", message("session-order", "older"))
            .expect("queue older pre-bind update");
        let gate = CallbackTestGate::new();
        *ctx.bind_drain_gate.lock() = Some(gate.clone());
        let bind_ctx = Arc::clone(&ctx);
        let bind_home = home.clone();
        let bind = thread::spawn(move || {
            bind_ctx.bind_session(
                "session-order",
                binding("agent-a", "conv-order", &bind_home),
            )
        });

        gate.reached.wait();
        ctx.handle_notification("agent-a", "connection-a", message("session-order", "newer"))
            .expect("queue newer update behind the drain");
        gate.resume.wait();
        bind.join().expect("binding thread").expect("bind session");

        let messages = ctx.store().messages("conv-order", false).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[0].body_text.contains("older"));
        assert!(messages[1].body_text.contains("newer"));
        let first = notifications.try_recv().expect("older broadcast");
        let second = notifications.try_recv().expect("newer broadcast");
        assert_eq!(first.params["update"]["content"]["text"], "older");
        assert_eq!(second.params["update"]["content"]["text"], "newer");

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn bind_drain_failure_restores_pending_state_and_keeps_successful_budget() {
        fn message(text: &str) -> SessionNotification {
            SessionNotification::new(
                SessionId::new("session-drain-failure"),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text),
                ))),
            )
        }

        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-drain-failure".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-drain-failure".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.handle_notification("agent-a", "connection-a", message("persisted first"))
            .unwrap();
        ctx.handle_notification("agent-a", "connection-a", message("still pending"))
            .unwrap();
        let key = SessionKey::new("agent-a", "session-drain-failure");
        ctx.capture_budgets.lock().insert(
            key.clone(),
            CaptureBudget {
                updates: MAX_CAPTURE_UPDATES_PER_TURN - 1,
                bytes: 0,
            },
        );

        let error = ctx
            .bind_session(
                "session-drain-failure",
                binding("agent-a", "conv-drain-failure", &home),
            )
            .expect_err("second drained update must exceed the capture budget");
        assert!(error.to_string().contains("capture budget exceeded"));
        assert!(!ctx.is_session_bound("agent-a", "session-drain-failure"));
        let messages = ctx.store().messages("conv-drain-failure", false).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].body_text.contains("persisted first"));
        let pending = ctx.pending_notifications.lock();
        let queue = pending.sessions.get(&key).expect("failed update requeued");
        assert_eq!(queue.len(), 1);
        assert_eq!(pending.count, 1);
        assert_eq!(pending.bytes, queue[0].bytes);
        assert!(!pending.draining.contains(&key));
        drop(pending);
        assert_eq!(
            ctx.capture_budgets
                .lock()
                .get(&key)
                .expect("successful drain budget retained")
                .updates,
            MAX_CAPTURE_UPDATES_PER_TURN
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn stale_generation_pending_update_is_discarded_before_new_bind() {
        let (ctx, home) = context();
        let mut notifications = ctx.subscribe_notifications();
        ctx.configure_agent("agent-a", "connection-old", config(false, false))
            .unwrap();
        ctx.handle_notification(
            "agent-a",
            "connection-old",
            SessionNotification::new(
                SessionId::new("session-generation"),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("stale"),
                ))),
            ),
        )
        .expect("queue old-generation update");
        ctx.configure_agent("agent-a", "connection-new", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-generation".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-generation".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();

        ctx.bind_session(
            "session-generation",
            binding("agent-a", "conv-generation", &home),
        )
        .expect("stale queued update must not poison the new generation");
        assert!(
            ctx.store()
                .messages("conv-generation", false)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            notifications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        let pending = ctx.pending_notifications.lock();
        assert_eq!(pending.count, 0);
        assert_eq!(pending.bytes, 0);
        assert!(
            !pending
                .sessions
                .contains_key(&SessionKey::new("agent-a", "session-generation"))
        );
        drop(pending);

        ctx.handle_notification(
            "agent-a",
            "connection-new",
            SessionNotification::new(
                SessionId::new("session-generation"),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("current"),
                ))),
            ),
        )
        .expect("current generation remains capturable");
        let messages = ctx.store().messages("conv-generation", false).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].body_text.contains("current"));

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn replacing_connection_purges_all_old_generation_pending_quota() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-old", config(false, false))
            .unwrap();
        for index in 0..MAX_PENDING_SESSIONS {
            ctx.handle_notification(
                "agent-a",
                "connection-old",
                SessionNotification::new(
                    SessionId::new(format!("old-session-{index}")),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new("old generation"),
                    ))),
                ),
            )
            .expect("fill old-generation pending session quota");
        }
        let old_count = ctx.pending_notifications.lock().count;
        ctx.configure_agent("agent-a", "connection-old", config(false, false))
            .unwrap();
        assert_eq!(
            ctx.pending_notifications.lock().count,
            old_count,
            "reconfiguring the current connection must preserve its pending updates"
        );

        ctx.configure_agent("agent-a", "connection-new", config(false, false))
            .unwrap();
        {
            let pending = ctx.pending_notifications.lock();
            assert!(pending.sessions.is_empty());
            assert_eq!(pending.count, 0);
            assert_eq!(pending.bytes, 0);
        }
        ctx.handle_notification(
            "agent-a",
            "connection-new",
            SessionNotification::new(
                SessionId::new("new-session"),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("new generation"),
                ))),
            ),
        )
        .expect("purged quota must admit a new-generation pre-bind update");

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn replacing_connection_revokes_all_bound_state_and_reaps_terminal() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-old", config(false, true))
            .unwrap();
        ctx.bind_session("bound-session", binding("agent-a", "conv-a", &home))
            .unwrap();
        ctx.set_current_run("agent-a", "bound-session", "run-old");
        ctx.set_loading("agent-a", "bound-session", true);
        ctx.begin_capture_operation(
            "agent-a",
            "connection-old",
            "bound-session",
            "session/prompt",
        )
        .unwrap();
        ctx.handle_notification(
            "agent-a",
            "connection-old",
            SessionNotification::new(
                SessionId::new("unbound-session"),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("pending old generation"),
                ))),
            ),
        )
        .unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "callbacks::state_tests::terminal_long_lived_child_fixture",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn old-generation terminal child");
        let reaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        ctx.terminals.lock().insert(
            "old-terminal".into(),
            TerminalHandle {
                owner: SessionKey::new("agent-a", "bound-session"),
                child: Some(child),
                process_tree: None,
                readers: Vec::new(),
                output: Arc::new(Mutex::new(TerminalOutput::default())),
                exit_status: None,
                _activity: None,
                reaped: Some(Arc::clone(&reaped)),
            },
        );

        ctx.configure_agent("agent-a", "connection-old", config(false, true))
            .unwrap();
        assert!(ctx.is_session_bound("agent-a", "bound-session"));
        assert!(ctx.terminals.lock().contains_key("old-terminal"));
        assert!(!reaped.load(std::sync::atomic::Ordering::SeqCst));

        ctx.configure_agent("agent-a", "connection-new", config(false, true))
            .unwrap();

        let bound_key = SessionKey::new("agent-a", "bound-session");
        assert!(!ctx.is_session_bound("agent-a", "bound-session"));
        assert!(!ctx.current_run.read().contains_key(&bound_key));
        assert!(!ctx.loading_sessions.read().contains(&bound_key));
        assert!(!ctx.capture_budgets.lock().contains_key(&bound_key));
        assert!(
            !ctx.capture_failures
                .lock()
                .keys()
                .any(|key| key.session.agent_id == "agent-a")
        );
        assert!(
            !ctx.pending_notifications
                .lock()
                .sessions
                .keys()
                .any(|key| key.agent_id == "agent-a")
        );
        assert!(ctx.terminals.lock().is_empty());
        assert!(
            reaped.load(std::sync::atomic::Ordering::SeqCst),
            "replacement must kill and reap old-generation terminal children"
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn connection_replacement_cannot_race_after_capture_validation() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-old", config(false, false))
            .unwrap();
        let gate = CallbackTestGate::new();
        *ctx.capture_after_connection_gate.lock() = Some(gate.clone());
        let capture_ctx = Arc::clone(&ctx);
        let capture = thread::spawn(move || {
            capture_ctx.handle_notification(
                "agent-a",
                "connection-old",
                SessionNotification::new(
                    SessionId::new("racing-session"),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new("old generation"),
                    ))),
                ),
            )
        });

        gate.reached.wait();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let configure_ctx = Arc::clone(&ctx);
        let configure = thread::spawn(move || {
            started_tx.send(()).unwrap();
            configure_ctx
                .configure_agent("agent-a", "connection-new", config(false, false))
                .unwrap();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        let completed_before_capture = done_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_ok();
        gate.resume.wait();
        capture
            .join()
            .expect("capture thread")
            .expect("old capture completes before replacement");
        if !completed_before_capture {
            done_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("replacement completes after capture");
        }
        configure.join().expect("configure thread");

        assert!(
            !completed_before_capture,
            "replacement must wait for the validated capture generation lease"
        );
        let pending = ctx.pending_notifications.lock();
        assert_eq!(pending.count, 0);
        assert_eq!(pending.bytes, 0);
        assert!(pending.sessions.is_empty());
        drop(pending);

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn unbound_notification_sessions_have_a_global_quota() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        for index in 0..MAX_PENDING_SESSIONS {
            let update = SessionNotification::new(
                SessionId::new(format!("session-{index}")),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("queued"),
                ))),
            );
            ctx.handle_notification("agent-a", "connection-a", update)
                .unwrap();
        }
        let overflow = SessionNotification::new(
            SessionId::new("overflow"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("queued"),
            ))),
        );
        assert!(
            ctx.handle_notification("agent-a", "connection-a", overflow)
                .unwrap_err()
                .to_string()
                .contains("too many unbound sessions")
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn bound_session_updates_enforce_single_and_turn_budgets() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-budget".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-budget".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.bind_session("session-budget", binding("agent-a", "conv-budget", &home))
            .unwrap();

        let oversized = SessionNotification::new(
            SessionId::new("session-budget"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES)),
            ))),
        );
        assert!(
            ctx.handle_notification("agent-a", "connection-a", oversized)
                .unwrap_err()
                .to_string()
                .contains("session update exceeds")
        );

        ctx.capture_budgets.lock().insert(
            SessionKey::new("agent-a", "session-budget"),
            CaptureBudget {
                updates: MAX_CAPTURE_UPDATES_PER_TURN,
                bytes: 0,
            },
        );
        let small = SessionNotification::new(
            SessionId::new("session-budget"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("small"),
            ))),
        );
        assert!(
            ctx.handle_notification("agent-a", "connection-a", small)
                .unwrap_err()
                .to_string()
                .contains("capture budget exceeded")
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn capture_failure_ledger_keeps_first_error_and_clears_at_session_boundaries() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-ledger".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-ledger".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        let session_binding = binding("agent-a", "conv-ledger", &home);
        ctx.bind_session("session-ledger", session_binding.clone())
            .unwrap();
        ctx.set_loading("agent-a", "session-ledger", true);
        ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
            .unwrap();

        let oversized = SessionNotification::new(
            SessionId::new("session-ledger"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES)),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", oversized)
            .unwrap_err();
        ctx.capture_budgets.lock().insert(
            SessionKey::new("agent-a", "session-ledger"),
            CaptureBudget {
                updates: MAX_CAPTURE_UPDATES_PER_TURN,
                bytes: 0,
            },
        );
        let budget_overflow = SessionNotification::new(
            SessionId::new("session-ledger"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("small"),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", budget_overflow)
            .unwrap_err();

        let first = ctx
            .take_capture_failure("agent-a", "connection-a", "session-ledger")
            .expect("active operation must retain its capture failure")
            .to_string();
        assert!(first.contains("session update exceeds"));
        assert!(!first.contains("capture budget exceeded"));
        assert!(
            ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
                .is_none()
        );

        ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
            .unwrap();
        let stale = SessionNotification::new(
            SessionId::new("session-ledger"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES)),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", stale)
            .unwrap_err();
        ctx.bind_session("session-ledger", session_binding.clone())
            .unwrap();
        assert!(
            ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
                .is_none(),
            "binding must clear stale capture correlation"
        );

        ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
            .unwrap();
        ctx.unbind_session("agent-a", "session-ledger");
        assert!(
            ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
                .is_none(),
            "unbinding must clear capture correlation"
        );

        ctx.bind_session("session-ledger", session_binding).unwrap();
        ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
            .unwrap();
        ctx.revoke_agent("agent-a").unwrap();
        assert!(
            ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
                .is_none(),
            "revoking an endpoint must clear capture correlation"
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn stale_begin_after_current_begin_is_rejected_without_poisoning_current_ledger() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-old", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-generation-ledger".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-generation-ledger".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.bind_session(
            "session-generation-ledger",
            binding("agent-a", "conv-generation-ledger", &home),
        )
        .unwrap();
        ctx.configure_agent("agent-a", "connection-new", config(false, false))
            .unwrap();
        let begun = Arc::new(std::sync::Barrier::new(2));
        let begin_ctx = Arc::clone(&ctx);
        let begin_barrier = Arc::clone(&begun);
        let current_begin = thread::spawn(move || {
            begin_ctx
                .begin_capture_operation(
                    "agent-a",
                    "connection-new",
                    "session-generation-ledger",
                    "session/prompt",
                )
                .unwrap();
            begin_barrier.wait();
        });
        begun.wait();
        let stale_begin_error = ctx
            .begin_capture_operation(
                "agent-a",
                "connection-old",
                "session-generation-ledger",
                "session/prompt",
            )
            .expect_err("stale command loop must not overwrite the current ledger");
        assert!(stale_begin_error.to_string().contains("stale connection"));
        current_begin.join().expect("current begin thread");
        let stale = SessionNotification::new(
            SessionId::new("session-generation-ledger"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("stale"),
            ))),
        );
        let stale_error = ctx
            .handle_notification("agent-a", "connection-old", stale)
            .expect_err("old generation must be rejected");
        assert!(stale_error.to_string().contains("stale connection"));
        assert!(
            ctx.take_capture_failure("agent-a", "connection-old", "session-generation-ledger")
                .is_none(),
            "old generation must not consume the current operation ledger"
        );

        let oversized = SessionNotification::new(
            SessionId::new("session-generation-ledger"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES)),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-new", oversized)
            .expect_err("current generation oversized update");
        let current = ctx
            .take_capture_failure("agent-a", "connection-new", "session-generation-ledger")
            .expect("current generation retains its capture failure")
            .to_string();
        assert!(current.contains("session update exceeds"));
        assert!(!current.contains("stale connection"));

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_prompt_captures_failure_before_queued_replacement_swaps_generation() {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-old", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-active-replacement".into(),
                agent_id: "agent-a".into(),
                agent_session_id: "session-active-replacement".into(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.bind_session(
            "session-active-replacement",
            binding("agent-a", "conv-active-replacement", &home),
        )
        .unwrap();
        let command = ctx
            .acquire_connection_lease("agent-a", "connection-old")
            .await
            .unwrap();
        ctx.begin_capture_operation(
            "agent-a",
            "connection-old",
            "session-active-replacement",
            "session/prompt",
        )
        .unwrap();

        let replacement_ctx = Arc::clone(&ctx);
        let replacement = tokio::spawn(async move {
            replacement_ctx
                .configure_agent_async("agent-a", "connection-new", config(false, false))
                .await;
        });
        while ctx
            .generation_gate("agent-a")
            .waiting_writers
            .load(Ordering::SeqCst)
            == 0
        {
            tokio::task::yield_now().await;
        }

        let callback = ctx
            .try_acquire_connection_lease("agent-a", "connection-old")
            .expect("active command admits its nested current-generation callback");
        let oversized = SessionNotification::new(
            SessionId::new("session-active-replacement"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES)),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-old", oversized)
            .expect_err("oversized active-generation update must fail capture");
        drop(callback);
        assert!(
            !replacement.is_finished(),
            "replacement must wait for the active command to finish"
        );
        let prompt_error = ctx
            .take_capture_failure("agent-a", "connection-old", "session-active-replacement")
            .expect("prompt response must observe the queued update failure");
        assert!(prompt_error.to_string().contains("session/prompt"));

        drop(command);
        replacement.await.expect("replacement task");
        let stale_command = match ctx
            .acquire_connection_lease("agent-a", "connection-old")
            .await
        {
            Ok(_) => panic!("queued old-generation command must be rejected after replacement"),
            Err(error) => error,
        };
        assert!(stale_command.to_string().contains("stale connection"));
        ctx.connection("agent-a", "connection-new")
            .expect("replacement generation installed");

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }
}

#[cfg(all(test, unix))]
mod resolve_tests {
    use super::{resolve, write_text_no_follow};
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn rejects_dangling_symlink_leaf() {
        let base = std::env::temp_dir().join(format!("acp-hub-resolve-{}", uuid::Uuid::new_v4()));
        let root = base.join("root");
        let outside = base.join("outside.txt");
        fs::create_dir_all(&root).expect("create test root");
        symlink(&outside, root.join("link.txt")).expect("create dangling symlink");

        let result = resolve(&root.join("link.txt"), std::slice::from_ref(&root), &root);

        assert!(
            result.is_err(),
            "dangling symlink must not pass root confinement"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn no_follow_open_rejects_leaf_swapped_after_resolve() {
        let base = std::env::temp_dir().join(format!("acp-hub-write-{}", uuid::Uuid::new_v4()));
        let root = base.join("root");
        let outside = base.join("outside.txt");
        fs::create_dir_all(&root).expect("create test root");
        let requested = root.join("new.txt");
        let resolved =
            resolve(&requested, std::slice::from_ref(&root), &root).expect("resolve new leaf");
        symlink(&outside, &requested).expect("swap leaf for dangling symlink");

        assert!(write_text_no_follow(&resolved, b"blocked").is_err());
        assert!(!outside.exists(), "outside target must not be created");
        let _ = fs::remove_dir_all(&base);
    }
}
