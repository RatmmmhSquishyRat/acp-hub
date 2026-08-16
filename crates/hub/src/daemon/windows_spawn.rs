//! Windows `CreateProcessW` helper for `ensure_daemon`.
//!
//! Stable `std::process::Command` always sets `bInheritHandles=TRUE`
//! (`inherit_handles` is still unstable — rust-lang/rust#146407). That leaks
//! every inheritable handle in the parent — including a redirected CI stdout
//! write-end and `daemon.lock` while `ensure_daemon` holds it — so a parent
//! Wait-for-EOF outlives the CLI. This wrapper matches sccache / nginx:
//! inherit nothing, do not set `STARTF_USESTDHANDLES`, and do not wait the
//! child process handle. Ready remains `daemon.json` + connect.

use std::{
    ffi::OsStr,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{FromRawHandle, OwnedHandle, RawHandle},
    },
    path::Path,
    ptr,
};

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};

/// Opt-in Job breakaway. Off by default; in-job no-inherit spawn is allowed.
pub(super) const BREAKAWAY_ENV: &str = "ACP_HUB_DAEMON_BREAKAWAY";

pub(super) fn breakaway_requested() -> bool {
    std::env::var(BREAKAWAY_ENV)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

/// `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT`.
/// Never ORs `DETACHED_PROCESS` (`CREATE_NO_WINDOW` is ignored if it does).
pub(super) fn creation_flags(breakaway: bool) -> u32 {
    let mut flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT;
    if breakaway {
        flags |= CREATE_BREAKAWAY_FROM_JOB;
    }
    flags
}

pub(super) struct SpawnedProcess {
    pub pid: u32,
    /// Closed on drop; not waited. Tests may terminate through this handle.
    #[allow(dead_code)]
    process: OwnedHandle,
}

impl SpawnedProcess {
    /// Close the process handle without waiting. The daemon lifetime is the
    /// ready handshake, not this handle.
    pub(super) fn detach(self) {
        drop(self);
    }
}

/// Spawn `program args…` with `bInheritHandles=FALSE` and no stdio handles.
///
/// The returned handle is the child process object. Callers that only need
/// "spawn started" must [`SpawnedProcess::detach`] (close, do not wait).
pub(super) fn spawn_no_inherit(
    program: &Path,
    args: &[&OsStr],
    flags: u32,
) -> io::Result<SpawnedProcess> {
    let application = program.is_absolute().then(|| wide_nul(program.as_os_str()));
    let application_ptr = application
        .as_ref()
        .map_or(ptr::null(), |value| value.as_ptr());
    let mut command_line = command_line(program.as_os_str(), args);
    command_line.push(0);

    // dwFlags stays 0: do not set STARTF_USESTDHANDLES. Combined with
    // bInheritHandles=FALSE the child gets no inherited handles at all.
    let startup = STARTUPINFOW {
        cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).expect("STARTUPINFOW fits in u32"),
        ..STARTUPINFOW::default()
    };
    let mut information = PROCESS_INFORMATION::default();

    // SAFETY: `application` / `command_line` are valid NUL-terminated UTF-16
    // for the duration of the call. `command_line` is writable as required by
    // CreateProcessW. STARTUPINFOW has cb set and no STARTF_USESTDHANDLES, so
    // hStd* are unused. bInheritHandles is FALSE. On success the process and
    // thread handles in `information` are owned and must be closed.
    let created = unsafe {
        CreateProcessW(
            application_ptr,
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            FALSE,
            flags,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: CreateProcessW succeeded, so both handles are valid and unique.
    let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread as RawHandle) };
    let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess as RawHandle) };
    drop(thread);

    Ok(SpawnedProcess {
        pid: information.dwProcessId,
        process,
    })
}

fn wide_nul(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

/// Windows argv quoting (same rules as rustc `std::sys::process::windows`).
fn command_line(program: &OsStr, args: &[&OsStr]) -> Vec<u16> {
    let mut cmd = Vec::new();
    append_quoted(&mut cmd, program);
    for arg in args {
        cmd.push(u16::from(b' '));
        append_quoted(&mut cmd, arg);
    }
    cmd
}

fn append_quoted(cmd: &mut Vec<u16>, arg: &OsStr) {
    let arg: Vec<u16> = arg.encode_wide().collect();
    let quote = arg.is_empty()
        || arg
            .iter()
            .any(|&c| c == u16::from(b' ') || c == u16::from(b'\t'));
    if quote {
        cmd.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for &c in &arg {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                cmd.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            }
            backslashes = 0;
        }
        cmd.push(c);
    }
    if quote {
        cmd.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        cmd.push(u16::from(b'"'));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::{
        DETACHED_PROCESS, GetCurrentProcess, TerminateProcess, WaitForSingleObject,
    };

    fn utf16_line(program: &str, args: &[&str]) -> String {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let refs: Vec<&OsStr> = os_args.iter().map(OsStr::new).collect();
        let line = command_line(OsStr::new(program), &refs);
        String::from_utf16(&line).expect("command line is UTF-16")
    }

    #[test]
    fn default_flags_are_no_window_group_unicode() {
        let flags = creation_flags(false);
        assert_eq!(
            flags,
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT
        );
        assert_eq!(flags & DETACHED_PROCESS, 0);
        assert_eq!(flags & CREATE_BREAKAWAY_FROM_JOB, 0);
    }

    #[test]
    fn breakaway_opt_in_does_not_add_detached() {
        let flags = creation_flags(true);
        assert_ne!(flags & CREATE_BREAKAWAY_FROM_JOB, 0);
        assert_ne!(flags & CREATE_NO_WINDOW, 0);
        assert_eq!(flags & DETACHED_PROCESS, 0);
    }

    #[test]
    fn command_line_quotes_spaces_only_when_needed() {
        assert_eq!(
            utf16_line(
                r"C:\Program Files\acp-hub.exe",
                &["serve", "--home", r"C:\Users\me\My Home"]
            ),
            r#""C:\Program Files\acp-hub.exe" serve --home "C:\Users\me\My Home""#
        );
        assert_eq!(
            utf16_line(r"C:\acp-hub.exe", &["serve", "--home", r"C:\hub"]),
            r"C:\acp-hub.exe serve --home C:\hub"
        );
    }

    fn system32(name: &str) -> std::path::PathBuf {
        let system_root =
            std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        Path::new(&system_root).join("System32").join(name)
    }

    fn process_in_any_job(handle: RawHandle) -> bool {
        let mut in_job = 0i32;
        let ok = unsafe { IsProcessInJob(handle.cast(), ptr::null_mut(), &mut in_job) };
        assert_ne!(ok, 0, "IsProcessInJob: {}", io::Error::last_os_error());
        in_job != 0
    }

    /// H1 isolation (Wait-for-EOF).
    ///
    /// An inheritable pipe write-end stays open if the child inherited it
    /// (`bInheritHandles=TRUE`). That is the hang: a parent redirected
    /// Wait-for-EOF outlives the CLI even after `RpcClient` Drop (H3) and
    /// even if the child stays in or leaves the Job (H4). We do **not** spawn
    /// inherit=TRUE in CI — that would leak this harness's stdout write-end
    /// into the child and hang cargo's own Wait-for-EOF. The shipped path is
    /// FALSE: once the parent drops its copy, the pipe EOFs while the child
    /// is still alive. Job breakaway cannot substitute for this assertion.
    #[test]
    fn no_inherit_spawn_does_not_hold_parent_pipe_write_end() {
        let (mut reader, writer) = std::io::pipe().expect("anonymous pipe");
        let set = unsafe {
            SetHandleInformation(
                writer.as_raw_handle().cast(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        };
        assert_ne!(
            set,
            0,
            "make pipe write-end inheritable: {}",
            io::Error::last_os_error()
        );

        let ping = system32("ping.exe");
        let args = [OsStr::new("-n"), OsStr::new("30"), OsStr::new("127.0.0.1")];
        let child = spawn_no_inherit(&ping, &args, creation_flags(false))
            .expect("spawn ping.exe with no-inherit CreateProcessW");
        assert_ne!(child.pid, 0);

        drop(writer);

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1];
            let _ = tx.send(reader.read(&mut buf));
        });
        let read = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("no-inherit child must not hold the pipe write-end (Wait-for-EOF)");
        assert_eq!(
            read.expect("pipe read"),
            0,
            "parent read must see EOF once its write-end is dropped"
        );

        unsafe {
            TerminateProcess(child.process.as_raw_handle().cast(), 1);
        }
    }

    /// H4 isolation: wait the CLI **process handle**, not the Job object and
    /// not stdout EOF. The handle must signal while the no-inherit child
    /// (serve stand-in) is still alive. Breakaway / `DETACHED` / `start /B`
    /// cannot be the thing that makes this wait return — we never set them.
    #[test]
    fn cli_process_handle_signals_while_no_inherit_child_stays_alive() {
        let ping = system32("ping.exe");
        let ping_args = [OsStr::new("-n"), OsStr::new("30"), OsStr::new("127.0.0.1")];
        let serve = spawn_no_inherit(&ping, &ping_args, creation_flags(false))
            .expect("spawn serve stand-in with the real no-inherit helper");

        let cmd = system32("cmd.exe");
        let cli_args = [OsStr::new("/c"), OsStr::new("exit"), OsStr::new("0")];
        let cli = spawn_no_inherit(&cmd, &cli_args, creation_flags(false))
            .expect("spawn CLI-equivalent process");

        let cli_wait = unsafe { WaitForSingleObject(cli.process.as_raw_handle().cast(), 2_000) };
        assert_eq!(
            cli_wait, WAIT_OBJECT_0,
            "CLI process handle must signal (H4: wait hProcess, not the Job, not pipe EOF)"
        );

        let serve_wait = unsafe { WaitForSingleObject(serve.process.as_raw_handle().cast(), 0) };
        assert_eq!(
            serve_wait, WAIT_TIMEOUT,
            "no-inherit child must still be alive after the CLI handle signals"
        );

        if process_in_any_job(unsafe { GetCurrentProcess() as RawHandle }) {
            assert!(
                process_in_any_job(serve.process.as_raw_handle()),
                "default spawn must stay in-job (H4: do not claim detach / breakaway)"
            );
        }

        unsafe {
            TerminateProcess(serve.process.as_raw_handle().cast(), 1);
        }
    }

    #[test]
    fn default_spawn_does_not_claim_job_detach() {
        assert_eq!(creation_flags(false) & CREATE_BREAKAWAY_FROM_JOB, 0);
        assert_eq!(creation_flags(false) & DETACHED_PROCESS, 0);
        let src = include_str!("windows_spawn.rs");
        let helper = src
            .split("pub(super) fn spawn_no_inherit")
            .nth(1)
            .and_then(|rest| rest.split("fn wide_nul").next())
            .expect("spawn_no_inherit body");
        assert!(
            helper.contains("bInheritHandles is FALSE"),
            "H1 fix is no-inherit, not Job breakaway"
        );
        assert!(
            !helper.contains("start /B"),
            "rc.9 start /B cascade must stay deleted"
        );
    }
}
