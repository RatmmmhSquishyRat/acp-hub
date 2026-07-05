//! Client-side callback handlers and session/update capture.
//!
//! The Hub is the ACP *Client*. It answers agent-to-client requests
//! (`session/request_permission`, `fs/*`, `terminal/*`) and captures every
//! `session/update` notification into the projection store. All handlers share
//! `Arc<HubCtx>`, keyed by the agent's `session_id`.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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
use uuid::Uuid;

use crate::endpoint::{FsConfig, PermissionPolicy};
use crate::error::HubError;
use crate::store::{MessageSource, NewMessage, Store, search_body};

#[derive(Clone)]
pub struct SessionBinding {
    pub conv_id: String,
    pub agent_id: String,
    pub permission_policy: PermissionPolicy,
    pub fs: FsConfig,
    pub cwd: PathBuf,
    pub terminal_enabled: bool,
}

struct TerminalHandle {
    child: Option<Child>,
    output: String,
    truncated: bool,
    byte_limit: usize,
    exit_status: Option<TerminalExitStatus>,
    kill_requested: bool,
}

pub struct HubCtx {
    store: Store,
    sessions: RwLock<HashMap<String, SessionBinding>>,
    current_run: RwLock<HashMap<String, String>>,
    loading_sessions: RwLock<std::collections::HashSet<String>>,
    terminals: Mutex<HashMap<String, TerminalHandle>>,
}

impl HubCtx {
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Self {
            store,
            sessions: RwLock::default(),
            current_run: RwLock::default(),
            loading_sessions: RwLock::default(),
            terminals: Mutex::default(),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn bind_session(&self, session_id: &str, binding: SessionBinding) {
        self.sessions.write().insert(session_id.into(), binding);
    }

    pub fn unbind_session(&self, session_id: &str) {
        self.sessions.write().remove(session_id);
        self.current_run.write().remove(session_id);
    }

    pub fn set_current_run(&self, session_id: &str, run_id: &str) {
        self.current_run
            .write()
            .insert(session_id.into(), run_id.into());
    }

    pub fn clear_current_run(&self, session_id: &str) {
        self.current_run.write().remove(session_id);
    }

    fn conv_for_session(&self, sid: &str) -> Option<(String, String)> {
        let g = self.sessions.read();
        let b = g.get(sid)?;
        Some((b.conv_id.clone(), b.agent_id.clone()))
    }

    fn run_for_session(&self, sid: &str) -> Option<String> {
        self.current_run.read().get(sid).cloned()
    }

    /// Mark a session as currently in load-replay mode (Layer 1).
    pub fn set_loading(&self, session_id: &str, loading: bool) {
        if loading {
            self.loading_sessions.write().insert(session_id.into());
        } else {
            self.loading_sessions.write().remove(session_id);
        }
    }

    /// Check if a session is in load-replay mode.
    /// Kept for potential future use; source is now derived from run_id.is_none().
    #[allow(dead_code)]
    fn is_loading(&self, session_id: &str) -> bool {
        self.loading_sessions.read().contains(session_id)
    }

    // ---- notification capture ----------------------------------------------

    pub fn handle_notification(&self, notif: SessionNotification) -> Result<(), HubError> {
        let sid = notif.session_id.to_string();
        let Some((conv_id, _)) = self.conv_for_session(&sid) else {
            return Ok(());
        };
        let run_id = self.run_for_session(&sid);
        let source = if run_id.is_none() {
            MessageSource::LoadReplay
        } else {
            MessageSource::LocalTurn
        };
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
            SessionUpdate::Plan(p) => {
                if let Err(e) = self
                    .store
                    .set_plan_snapshot(&conv_id, &serde_json::to_value(&p).unwrap_or_default())
                {
                    tracing::warn!(%conv_id, error = %e, "plan snapshot capture failed");
                }
            }
            SessionUpdate::AvailableCommandsUpdate(cmds) => {
                if let Err(e) = self.store.set_available_commands_snapshot(
                    &conv_id,
                    &serde_json::to_value(&cmds).unwrap_or_default(),
                ) {
                    tracing::warn!(%conv_id, error = %e, "available commands snapshot failed");
                }
            }
            SessionUpdate::CurrentModeUpdate(m) => {
                let mut patch = serde_json::Map::new();
                patch.insert(
                    "currentMode".into(),
                    serde_json::to_value(&m).unwrap_or_default(),
                );
                if let Err(e) = self
                    .store
                    .apply_session_info(&conv_id, None, None, Some(&patch))
                {
                    tracing::warn!(%conv_id, error = %e, "current mode snapshot failed");
                }
            }
            SessionUpdate::ConfigOptionUpdate(c) => {
                let v = serde_json::to_value(&c.config_options).unwrap_or_default();
                if let Err(e) = self.store.set_config_snapshot(&conv_id, &v, None) {
                    tracing::warn!(%conv_id, error = %e, "config snapshot failed");
                }
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                let title: Option<String> = info.title.value().map(|s| s.to_string());
                let updated: Option<String> = info.updated_at.value().map(|s| s.to_string());
                let t = title.as_deref();
                let u = updated.as_deref();
                let meta: Option<&serde_json::Map<String, serde_json::Value>> = info.meta.as_ref();
                if let Err(e) = self.store.apply_session_info(&conv_id, t, u, meta) {
                    tracing::warn!(%conv_id, error = %e, "session info snapshot failed");
                }
            }
            SessionUpdate::UsageUpdate(u) => {
                let cost = serde_json::to_value(&u.cost).ok();
                if let Err(e) = self.store.upsert_usage_snapshot(
                    &conv_id,
                    u.used as i64,
                    u.size as i64,
                    cost.as_ref(),
                ) {
                    tracing::warn!(%conv_id, error = %e, "usage snapshot failed");
                }
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
        }
        Ok(())
    }

    // ---- permission --------------------------------------------------------

    pub fn handle_permission(&self, req: &RequestPermissionRequest) -> RequestPermissionResponse {
        let policy = self
            .sessions
            .read()
            .get(req.session_id.to_string().as_str())
            .map(|b| b.permission_policy)
            .unwrap_or_default();
        let outcome = match policy {
            PermissionPolicy::AutoAllow => first_option(req, true),
            PermissionPolicy::AutoCancel => RequestPermissionOutcome::Cancelled,
            PermissionPolicy::Reject => first_option(req, false),
        };
        RequestPermissionResponse::new(outcome)
    }

    // ---- fs ----------------------------------------------------------------

    pub fn handle_read_text_file(
        &self,
        req: &ReadTextFileRequest,
    ) -> Result<ReadTextFileResponse, HubError> {
        let binding = self
            .sessions
            .read()
            .get(req.session_id.to_string().as_str())
            .cloned()
            .ok_or_else(|| HubError::other("unknown session"))?;
        if !binding.fs.read_text_file {
            return Err(HubError::other("fs/read_text_file not enabled"));
        }
        let path = resolve(&req.path, &binding.fs.allowed_roots, &binding.cwd)?;
        let text = fs::read_to_string(&path)
            .map_err(|e| HubError::other(format!("read {}: {e}", path.display())))?;
        Ok(ReadTextFileResponse::new(slice_lines(
            &text, req.line, req.limit,
        )))
    }

    pub fn handle_write_text_file(
        &self,
        req: &WriteTextFileRequest,
    ) -> Result<WriteTextFileResponse, HubError> {
        let binding = self
            .sessions
            .read()
            .get(req.session_id.to_string().as_str())
            .cloned()
            .ok_or_else(|| HubError::other("unknown session"))?;
        if !binding.fs.write_text_file {
            return Err(HubError::other("fs/write_text_file not enabled"));
        }
        let path = resolve_for_write(&req.path, &binding.fs.allowed_roots, &binding.cwd)?;
        fs::write(&path, &req.content)?;
        Ok(WriteTextFileResponse::new())
    }

    // ---- terminal ----------------------------------------------------------

    pub fn handle_terminal_create(
        &self,
        req: &CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, HubError> {
        // H1: Reject terminal creation unless the request is tied to a known
        // Hub session whose endpoint explicitly advertises terminal support.
        let sid = req.session_id.to_string();
        let binding = self
            .sessions
            .read()
            .get(&sid)
            .cloned()
            .ok_or_else(|| HubError::other("unknown session"))?;
        if !binding.terminal_enabled {
            return Err(HubError::UnsupportedCapability {
                endpoint: binding.agent_id.clone(),
                operation: "terminal/create".into(),
                required_capability: "client_capabilities.terminal".into(),
            });
        }
        let cwd = req.cwd.clone().unwrap_or_else(|| binding.cwd.clone());
        let mut cmd = Command::new(&req.command);
        cmd.args(&req.args);
        for ev in &req.env {
            cmd.env(&ev.name, &ev.value);
        }
        cmd.current_dir(&cwd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let child = cmd
            .spawn()
            .map_err(|e| HubError::other(format!("spawn: {e}")))?;
        let limit = req
            .output_byte_limit
            .map(|l| l as usize)
            .unwrap_or(usize::MAX);
        let id = format!("term-{}", Uuid::new_v4().simple());
        self.terminals.lock().insert(
            id.clone(),
            TerminalHandle {
                child: Some(child),
                output: String::new(),
                truncated: false,
                byte_limit: limit,
                exit_status: None,
                kill_requested: false,
            },
        );
        Ok(CreateTerminalResponse::new(TerminalId::new(id)))
    }

    pub fn handle_terminal_output(&self, req: &TerminalOutputRequest) -> TerminalOutputResponse {
        let mut terms = self.terminals.lock();
        let Some(h) = terms.get_mut(req.terminal_id.to_string().as_str()) else {
            return TerminalOutputResponse::new(String::new(), false);
        };
        drain(h);
        let mut resp = TerminalOutputResponse::new(h.output.clone(), h.truncated);
        resp.exit_status = h.exit_status.clone();
        resp
    }

    pub fn handle_terminal_wait(
        &self,
        req: &WaitForTerminalExitRequest,
    ) -> WaitForTerminalExitResponse {
        let tid = req.terminal_id.to_string();
        let started = Instant::now();

        loop {
            let child = {
                let mut terms = self.terminals.lock();
                let Some(h) = terms.get_mut(&tid) else {
                    return terminal_wait_error("acp-hub-terminal-not-found");
                };
                if let Some(exit) = h.exit_status.clone() {
                    return WaitForTerminalExitResponse::new(exit);
                }
                h.child.take()
            };

            let (mut exit, mut still_running) = wait_child(child);
            let response = {
                let mut terms = self.terminals.lock();
                let Some(h) = terms.get_mut(&tid) else {
                    if let Some(mut child) = still_running {
                        let _ = child.kill();
                        let _ = child.try_wait();
                    }
                    return terminal_wait_error("acp-hub-terminal-released");
                };

                if h.kill_requested {
                    if let Some(child) = still_running.as_mut() {
                        let _ = child.kill();
                        if let Ok(Some(status)) = child.try_wait() {
                            exit = make_exit(status);
                        }
                    }
                }
                if let Some(child) = still_running {
                    h.child = Some(child);
                }
                // M8: Drain remaining buffered output after each bounded wait slice without
                // turning terminal/output into a blocking pipe read.
                drain(h);
                let exit = exit.or_else(|| h.exit_status.clone());
                if let Some(exit) = exit.clone() {
                    h.kill_requested = false;
                    h.exit_status = Some(exit);
                }
                exit
            };

            if let Some(exit) = response {
                return WaitForTerminalExitResponse::new(exit);
            }
            if started.elapsed() >= TERMINAL_WAIT_TIMEOUT {
                return terminal_wait_error("acp-hub-terminal-wait-timeout");
            }
            thread::sleep(TERMINAL_WAIT_RETRY_INTERVAL);
        }
    }

    pub fn handle_terminal_kill(&self, req: &KillTerminalRequest) -> KillTerminalResponse {
        let mut terms = self.terminals.lock();
        if let Some(h) = terms.get_mut(req.terminal_id.to_string().as_str()) {
            if let Some(c) = &mut h.child {
                let _ = c.kill();
                if let Ok(Some(status)) = c.try_wait() {
                    h.exit_status = make_exit(status);
                    h.kill_requested = false;
                }
            } else if h.exit_status.is_none() {
                h.kill_requested = true;
            }
        }
        KillTerminalResponse::new()
    }

    pub fn handle_terminal_release(&self, req: &ReleaseTerminalRequest) -> ReleaseTerminalResponse {
        if let Some(h) = self
            .terminals
            .lock()
            .remove(req.terminal_id.to_string().as_str())
        {
            if let Some(mut c) = h.child {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        ReleaseTerminalResponse::new()
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
) {
    let val = serde_json::to_value(p).unwrap_or_default();
    let id = format!("msg-{}", Uuid::new_v4().simple());
    let body = search_body(&val);
    if let Err(e) = store.append_message(&NewMessage {
        id,
        conv_id: conv.into(),
        run_id: run.clone(),
        source,
        role: role.into(),
        kind: kind.map(str::to_string),
        content_json: val,
        body_text: body,
    }) {
        tracing::warn!(error=%e, "store");
    }
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
    let c = r
        .canonicalize()
        .map_err(|e| HubError::other(format!("resolve {}: {e}", r.display())))?;
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
/// Resolve a write target whose leaf file may not exist yet.
///
/// Unlike [`resolve`], which `canonicalize`s the full path (and therefore
/// requires it to already exist), this canonicalizes the *parent* directory,
/// validates that it stays inside an allowed root, then re-joins the file
/// name. This lets `write_text_file` create brand-new files. Parent
/// directories are created first so `canonicalize` resolves a new subtree.
fn resolve_for_write(path: &Path, roots: &[PathBuf], cwd: &Path) -> Result<PathBuf, HubError> {
    let r = if path.is_absolute() {
        path.into()
    } else {
        cwd.join(path)
    };
    let parent = match r.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let allowed: Vec<PathBuf> = if roots.is_empty() {
        vec![cwd.canonicalize().unwrap_or_else(|_| cwd.into())]
    } else {
        roots
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect()
    };
    // H2: Authorize BEFORE creating directories. Walk up to nearest
    // existing ancestor and verify it is inside an allowed root.
    let mut ancestor = parent;
    while !ancestor.exists() {
        match ancestor.parent() {
            Some(p) if !p.as_os_str().is_empty() => ancestor = p,
            _ => break,
        }
    }
    let canon_ancestor = ancestor
        .canonicalize()
        .map_err(|e| HubError::other(format!("resolve {}: {e}", r.display())))?;
    if !allowed.iter().any(|root| canon_ancestor.starts_with(root)) {
        return Err(HubError::other(format!(
            "{} outside allowed roots",
            r.display()
        )));
    }
    // Authorized — safe to create the directory subtree.
    fs::create_dir_all(parent)?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|e| HubError::other(format!("resolve {}: {e}", r.display())))?;
    let file_name = r
        .file_name()
        .ok_or_else(|| HubError::other(format!("invalid file path: {}", r.display())))?;
    let final_path = canon_parent.join(file_name);
    if fs::symlink_metadata(&final_path).is_ok() {
        let canon_leaf = final_path
            .canonicalize()
            .map_err(|e| HubError::other(format!("resolve {}: {e}", final_path.display())))?;
        if !allowed.iter().any(|root| canon_leaf.starts_with(root)) {
            return Err(HubError::other(format!(
                "{} outside allowed roots",
                final_path.display()
            )));
        }
        return Ok(canon_leaf);
    }
    Ok(final_path)
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

fn drain(h: &mut TerminalHandle) {
    if let Some(c) = h.child.as_mut() {
        read_to(&mut c.stdout, &mut h.output, h.byte_limit, &mut h.truncated);
        read_to(&mut c.stderr, &mut h.output, h.byte_limit, &mut h.truncated);
        if let Ok(Some(s)) = c.try_wait() {
            h.exit_status = make_exit(s);
        }
    }
}

#[cfg(windows)]
trait PipeRead: Read + std::os::windows::io::AsRawHandle {}

#[cfg(unix)]
trait PipeRead: Read + std::os::fd::AsRawFd {}

#[cfg(not(any(windows, unix)))]
trait PipeRead: Read {}

impl PipeRead for ChildStdout {}
impl PipeRead for ChildStderr {}

const TERMINAL_READ_MAX_CHUNKS: usize = 100;
const TERMINAL_WAIT_MAX_POLLS: usize = 10;
const TERMINAL_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const TERMINAL_WAIT_RETRY_INTERVAL: Duration = Duration::from_millis(5);
const TERMINAL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

fn read_to<R: PipeRead>(src: &mut Option<R>, dst: &mut String, limit: usize, truncated: &mut bool) {
    let Some(r) = src else { return };
    if set_pipe_nonblocking(r).is_err() {
        return;
    }
    let mut buf = [0u8; 8192];
    for _ in 0..TERMINAL_READ_MAX_CHUNKS {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let rem = limit.saturating_sub(dst.len());
                if rem == 0 {
                    *truncated = true;
                    break;
                }
                let take = n.min(rem);
                dst.push_str(&String::from_utf8_lossy(&buf[..take]));
                if n > rem {
                    *truncated = true;
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[cfg(windows)]
fn set_pipe_nonblocking<R: PipeRead>(pipe: &R) -> std::io::Result<()> {
    use windows_sys::Win32::System::Pipes::{PIPE_NOWAIT, SetNamedPipeHandleState};

    let mode = PIPE_NOWAIT;
    let ok = unsafe {
        SetNamedPipeHandleState(
            pipe.as_raw_handle(),
            &mode,
            core::ptr::null(),
            core::ptr::null(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn set_pipe_nonblocking<R: PipeRead>(pipe: &R) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if flags & libc::O_NONBLOCK != 0 {
        return Ok(());
    }
    let status = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if status == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn make_exit(s: std::process::ExitStatus) -> Option<TerminalExitStatus> {
    Some(TerminalExitStatus::new().exit_code(s.code().map(|c| c as u32)))
}

fn terminal_wait_error(signal: &str) -> WaitForTerminalExitResponse {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "acpHubError".into(),
        serde_json::Value::String(signal.into()),
    );
    WaitForTerminalExitResponse::new(TerminalExitStatus::new().signal(Some(signal.into())))
        .meta(meta)
}

fn wait_child(child: Option<Child>) -> (Option<TerminalExitStatus>, Option<Child>) {
    let Some(mut c) = child else {
        return (None, None);
    };
    for _ in 0..TERMINAL_WAIT_MAX_POLLS {
        match c.try_wait() {
            Ok(Some(status)) => return (make_exit(status), Some(c)),
            Ok(None) => thread::sleep(TERMINAL_WAIT_POLL_INTERVAL),
            Err(_) => return (None, Some(c)),
        }
    }
    match c.try_wait() {
        Ok(Some(status)) => (make_exit(status), Some(c)),
        Ok(None) => (None, Some(c)),
        Err(_) => (None, Some(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::{HubCtx, SessionBinding, read_to, wait_child};
    use crate::endpoint::{FsConfig, PermissionPolicy};
    use crate::error::HubError;
    use crate::store::Store;
    use agent_client_protocol::schema::v1::{
        CreateTerminalRequest, KillTerminalRequest, ReleaseTerminalRequest,
        WaitForTerminalExitRequest, WriteTextFileRequest,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    #[test]
    fn terminal_create_rejects_unknown_session_before_spawning() {
        let ctx = HubCtx::new(Store::open_memory().expect("memory store"));

        let error = ctx
            .handle_terminal_create(&CreateTerminalRequest::new(
                "missing-session",
                "command-that-must-not-be-spawned",
            ))
            .expect_err("unknown session must reject terminal/create");

        assert!(matches!(error, HubError::Other(message) if message == "unknown session"));
    }

    #[test]
    fn write_text_file_rejects_existing_leaf_symlink_escape() {
        let base = temp_test_dir("symlink-escape");
        let allowed = base.join("allowed");
        let outside = base.join("outside");
        fs::create_dir_all(&allowed).expect("create allowed root");
        fs::create_dir_all(&outside).expect("create outside dir");
        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "outside-original").expect("write outside target");
        let link = allowed.join("escape.txt");

        match create_file_symlink(&outside_file, &link) {
            Ok(()) => {}
            Err(error) if symlink_creation_denied(&error) => {
                let _ = fs::remove_dir_all(&base);
                return;
            }
            Err(error) => panic!(
                "create symlink {} -> {}: {error}",
                link.display(),
                outside_file.display()
            ),
        }

        let ctx = HubCtx::new(Store::open_memory().expect("memory store"));
        ctx.bind_session(
            "session-with-write",
            SessionBinding {
                conv_id: "conv".into(),
                agent_id: "agent".into(),
                permission_policy: PermissionPolicy::Reject,
                fs: FsConfig {
                    read_text_file: false,
                    write_text_file: true,
                    allowed_roots: vec![allowed.clone()],
                },
                cwd: allowed,
                terminal_enabled: false,
            },
        );

        let error = ctx
            .handle_write_text_file(&WriteTextFileRequest::new(
                "session-with-write",
                &link,
                "escaped-write",
            ))
            .expect_err("symlink leaf escaping allowed roots must be rejected");

        assert!(
            error.to_string().contains("outside allowed roots"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read_to_string(&outside_file).expect("read outside target"),
            "outside-original",
            "rejected write must not modify the symlink target"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn read_to_returns_promptly_when_child_pipe_has_no_output() {
        let mut child = idle_child_with_piped_stdout();
        let stdout = child.stdout.take().expect("stdout should be piped");
        let (tx, rx) = mpsc::channel();

        let reader = std::thread::spawn(move || {
            let mut pipe = Some(stdout);
            let mut output = String::new();
            let mut truncated = false;
            read_to(&mut pipe, &mut output, usize::MAX, &mut truncated);
            tx.send((output, truncated)).expect("send read result");
        });

        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok((output, truncated)) => {
                child.kill().expect("kill idle child");
                let _ = child.wait();
                reader.join().expect("reader thread should finish");
                assert!(output.is_empty());
                assert!(!truncated);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                child.kill().expect("kill idle child");
                let _ = child.wait();
                reader
                    .join()
                    .expect("reader thread should unblock after child exit");
                panic!("read_to blocked on an idle child stdout pipe");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                child.kill().expect("kill idle child");
                let _ = child.wait();
                reader.join().expect("reader thread should finish");
                panic!("reader thread exited without sending a result");
            }
        }
    }

    #[test]
    fn wait_child_returns_promptly_when_process_is_still_running() {
        let child = idle_child_with_piped_stdout();
        let started = Instant::now();
        let (exit, still_running) = wait_child(Some(child));

        assert!(
            started.elapsed() < Duration::from_millis(250),
            "wait_child blocked while waiting for a still-running child"
        );
        assert!(exit.is_none());
        let mut child = still_running.expect("child should be returned on wait timeout");
        assert!(
            child.try_wait().expect("poll child").is_none(),
            "child should still be running after wait timeout"
        );
        child.kill().expect("kill idle child");
        let _ = child.wait();
    }

    #[test]
    fn terminal_wait_waits_for_child_past_short_poll_window() {
        let ctx = HubCtx::new(Store::open_memory().expect("memory store"));
        let cwd = std::env::current_dir().expect("current dir");
        ctx.bind_session(
            "session-with-terminal",
            SessionBinding {
                conv_id: "conv".into(),
                agent_id: "agent".into(),
                permission_policy: PermissionPolicy::Reject,
                fs: FsConfig {
                    read_text_file: false,
                    write_text_file: false,
                    allowed_roots: vec![cwd.clone()],
                },
                cwd: cwd.clone(),
                terminal_enabled: true,
            },
        );
        let (command, args) = delayed_exit_command(7);
        let terminal = ctx
            .handle_terminal_create(
                &CreateTerminalRequest::new("session-with-terminal", command)
                    .args(args)
                    .cwd(cwd),
            )
            .expect("create terminal");

        let started = Instant::now();
        let response = ctx.handle_terminal_wait(&WaitForTerminalExitRequest::new(
            "session-with-terminal",
            terminal.terminal_id.clone(),
        ));
        if response.exit_status.exit_code != Some(7) {
            let _ = ctx.handle_terminal_kill(&KillTerminalRequest::new(
                "session-with-terminal",
                terminal.terminal_id,
            ));
        }

        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "wait_for_exit returned before the delayed child had time to exit"
        );
        assert_eq!(response.exit_status.exit_code, Some(7));
        assert!(response.exit_status.signal.is_none());
    }

    #[test]
    fn terminal_wait_reports_release_without_waiting_for_long_child_exit() {
        let ctx = HubCtx::new(Store::open_memory().expect("memory store"));
        let cwd = std::env::current_dir().expect("current dir");
        ctx.bind_session(
            "session-with-released-terminal",
            SessionBinding {
                conv_id: "conv".into(),
                agent_id: "agent".into(),
                permission_policy: PermissionPolicy::Reject,
                fs: FsConfig {
                    read_text_file: false,
                    write_text_file: false,
                    allowed_roots: vec![cwd.clone()],
                },
                cwd: cwd.clone(),
                terminal_enabled: true,
            },
        );
        let (command, args) = long_running_command();
        let terminal = ctx
            .handle_terminal_create(
                &CreateTerminalRequest::new("session-with-released-terminal", command)
                    .args(args)
                    .cwd(cwd),
            )
            .expect("create terminal");
        let terminal_id = terminal.terminal_id.clone();
        let wait_ctx = Arc::clone(&ctx);

        let started = Instant::now();
        let waiter = std::thread::spawn(move || {
            wait_ctx.handle_terminal_wait(&WaitForTerminalExitRequest::new(
                "session-with-released-terminal",
                terminal_id,
            ))
        });
        std::thread::sleep(Duration::from_millis(100));
        ctx.handle_terminal_release(&ReleaseTerminalRequest::new(
            "session-with-released-terminal",
            terminal.terminal_id,
        ));
        let response = waiter.join().expect("wait thread should finish");

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "wait_for_exit should not wait for natural child exit after release"
        );
        assert!(
            matches!(
                response.exit_status.signal.as_deref(),
                Some("acp-hub-terminal-released" | "acp-hub-terminal-not-found")
            ),
            "unexpected wait_for_exit signal: {:?}",
            response.exit_status.signal
        );
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("acp-hub-{name}-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&path).expect("create temp test dir");
        path
    }

    #[cfg(windows)]
    fn create_file_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }

    #[cfg(unix)]
    fn create_file_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(not(any(windows, unix)))]
    fn create_file_symlink(_original: &Path, _link: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "file symlinks are unsupported on this platform",
        ))
    }

    fn symlink_creation_denied(error: &std::io::Error) -> bool {
        error.kind() == std::io::ErrorKind::PermissionDenied
            || (cfg!(windows) && error.raw_os_error() == Some(1314))
    }

    #[cfg(windows)]
    fn long_running_command() -> (&'static str, Vec<String>) {
        ("cmd", vec!["/C".into(), "ping -n 4 127.0.0.1 >NUL".into()])
    }

    #[cfg(not(windows))]
    fn long_running_command() -> (&'static str, Vec<String>) {
        ("sh", vec!["-c".into(), "sleep 3".into()])
    }

    #[cfg(windows)]
    fn delayed_exit_command(code: u32) -> (&'static str, Vec<String>) {
        (
            "cmd",
            vec![
                "/C".into(),
                format!("ping -n 2 127.0.0.1 >NUL & exit /B {code}"),
            ],
        )
    }

    #[cfg(not(windows))]
    fn delayed_exit_command(code: u32) -> (&'static str, Vec<String>) {
        ("sh", vec!["-c".into(), format!("sleep 0.2; exit {code}")])
    }

    #[cfg(windows)]
    fn idle_child_with_piped_stdout() -> std::process::Child {
        Command::new("cmd")
            .args(["/C", "ping -n 3 127.0.0.1 >NUL"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn idle child")
    }

    #[cfg(not(windows))]
    fn idle_child_with_piped_stdout() -> std::process::Child {
        Command::new("sh")
            .args(["-c", "sleep 3"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn idle child")
    }
}
