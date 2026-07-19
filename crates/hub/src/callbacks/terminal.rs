use super::*;

const DEFAULT_TERMINAL_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_TERMINAL_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TERMINALS_GLOBAL: usize = 64;
const MAX_TERMINALS_PER_SESSION: usize = 8;

#[derive(Default)]
pub(super) struct TerminalOutput {
    pub(super) text: String,
    pub(super) truncated: bool,
}

#[cfg(unix)]
pub(super) struct ProcessTree {
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

    pub(in super::super) struct ProcessTree {
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
pub(super) use windows_process_tree::ProcessTree;

pub(super) struct TerminalHandle {
    pub(super) owner: SessionKey,
    pub(super) child: Option<Child>,
    pub(super) process_tree: Option<ProcessTree>,
    pub(super) readers: Vec<thread::JoinHandle<()>>,
    pub(super) output: Arc<Mutex<TerminalOutput>>,
    pub(super) exit_status: Option<TerminalExitStatus>,
    pub(super) _activity: Option<ActivityLease>,
    #[cfg(test)]
    pub(super) reaped: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    pub(super) cleanup_failures_remaining: usize,
}

impl TerminalHandle {
    pub(super) fn cleanup(&mut self) -> Result<(), HubError> {
        #[cfg(test)]
        if self.cleanup_failures_remaining > 0 {
            self.cleanup_failures_remaining -= 1;
            return Err(HubError::other("forced terminal cleanup failure"));
        }
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
        #[cfg(test)]
        if let Some(reaped) = &self.reaped {
            reaped.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

impl HubCtx {
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
            #[cfg(test)]
            cleanup_failures_remaining: 0,
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

    pub(super) fn verify_terminal_owner(
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

pub(super) fn truncate_from_start(state: &mut TerminalOutput, limit: usize) {
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
