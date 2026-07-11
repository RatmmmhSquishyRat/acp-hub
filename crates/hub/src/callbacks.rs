//! Client-side callback handlers and session/update capture.
//!
//! The Hub is the ACP *Client*. It answers agent-to-client requests
//! (`session/request_permission`, `fs/*`, `terminal/*`) and captures every
//! `session/update` notification into the projection store. All handlers share
//! `Arc<HubCtx>`, keyed by the agent's `session_id`.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

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
}

struct TerminalHandle {
    child: Option<Child>,
    output: String,
    truncated: bool,
    byte_limit: usize,
    exit_status: Option<TerminalExitStatus>,
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
        let source = if self.is_loading(&sid) {
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
                let _ = self
                    .store
                    .set_plan_snapshot(&conv_id, &serde_json::to_value(&p).unwrap_or_default());
            }
            SessionUpdate::AvailableCommandsUpdate(cmds) => {
                let _ = self.store.set_available_commands_snapshot(
                    &conv_id,
                    &serde_json::to_value(&cmds).unwrap_or_default(),
                );
            }
            SessionUpdate::CurrentModeUpdate(m) => {
                let mut patch = serde_json::Map::new();
                patch.insert(
                    "currentMode".into(),
                    serde_json::to_value(&m).unwrap_or_default(),
                );
                let _ = self
                    .store
                    .apply_session_info(&conv_id, None, None, Some(&patch));
            }
            SessionUpdate::ConfigOptionUpdate(c) => {
                let v = serde_json::to_value(&c.config_options).unwrap_or_default();
                let _ = self.store.set_config_snapshot(&conv_id, &v, None);
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                let title: Option<String> = info.title.value().map(|s| s.to_string());
                let updated: Option<String> = info.updated_at.value().map(|s| s.to_string());
                let t = title.as_deref();
                let u = updated.as_deref();
                let meta: Option<&serde_json::Map<String, serde_json::Value>> = info.meta.as_ref();
                let _ = self.store.apply_session_info(&conv_id, t, u, meta);
            }
            SessionUpdate::UsageUpdate(u) => {
                let cost = serde_json::to_value(&u.cost).ok();
                let _ = self.store.upsert_usage_snapshot(
                    &conv_id,
                    u.used as i64,
                    u.size as i64,
                    cost.as_ref(),
                );
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
        req: &CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, HubError> {
        let cwd = req
            .cwd
            .clone()
            .or_else(|| {
                self.sessions
                    .read()
                    .get(req.session_id.to_string().as_str())
                    .map(|b| b.cwd.clone())
            })
            .ok_or_else(|| HubError::other("no cwd for terminal"))?;
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
        let child = self
            .terminals
            .lock()
            .get_mut(&tid)
            .and_then(|h| h.child.take());
        let exit = wait_child(child);
        let mut terms = self.terminals.lock();
        if let Some(h) = terms.get_mut(&tid) {
            h.exit_status = exit.clone();
        }
        WaitForTerminalExitResponse::new(exit.unwrap_or_default())
    }

    pub fn handle_terminal_kill(&self, req: &KillTerminalRequest) -> KillTerminalResponse {
        let mut terms = self.terminals.lock();
        if let Some(h) = terms.get_mut(req.terminal_id.to_string().as_str()) {
            if let Some(c) = &mut h.child {
                let _ = c.kill();
                let _ = c.try_wait();
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

fn drain(h: &mut TerminalHandle) {
    if let Some(c) = h.child.as_mut() {
        read_to(&mut c.stdout, &mut h.output, h.byte_limit, &mut h.truncated);
        read_to(&mut c.stderr, &mut h.output, h.byte_limit, &mut h.truncated);
        if let Ok(Some(s)) = c.try_wait() {
            h.exit_status = make_exit(s);
        }
    }
}

fn read_to(src: &mut Option<impl Read>, dst: &mut String, limit: usize, truncated: &mut bool) {
    let Some(r) = src else { return };
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
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

fn wait_child(child: Option<Child>) -> Option<TerminalExitStatus> {
    let mut c = child?;
    let status = c.wait().ok()?;
    Some(TerminalExitStatus::new().exit_code(exit_code(&status)))
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
