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

mod capture;
mod connection;
mod permission_filesystem;
mod terminal;

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

#[cfg(test)]
use capture::{
    MAX_CAPTURE_UPDATES_PER_TURN, MAX_PENDING_SESSIONS, MAX_PENDING_SINGLE_NOTIFICATION_BYTES,
};
use connection::{AgentConnection, GenerationGate};
#[allow(unused_imports)]
pub(crate) use connection::{AgentConnectionLease, AgentGenerationWriter, InboundConnectionLease};
use permission_filesystem::resolve;
#[cfg(all(test, unix))]
use permission_filesystem::write_text_no_follow;
use terminal::TerminalHandle;
#[cfg(test)]
use terminal::{TerminalOutput, truncate_from_start};

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

#[cfg(test)]
#[path = "callbacks/tests.rs"]
mod state_tests;

#[cfg(all(test, unix))]
#[path = "callbacks/resolve_tests.rs"]
mod resolve_tests;
