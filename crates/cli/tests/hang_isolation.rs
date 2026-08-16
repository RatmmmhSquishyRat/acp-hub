//! P3 hang-class isolation for the CLI process.
//!
//! H1 (inherit Wait-for-EOF) lives in `windows_spawn` (FALSE-EOF fixture).
//! H3 (`RpcClient` Drop) lives in `rpc/tests.rs`.
//! This file isolates **H4**: a parent waits the CLI **process handle**
//! (not the Job object, not stdout EOF). That handle must signal while
//! `serve` is still alive. Stacking forget + exit + breakaway again must
//! not be able to “fix” a regression that this wait would still catch.

#![cfg(windows)]

use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use acp_hub::daemon::DaemonMetadata;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
}

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const STILL_ACTIVE: u32 = 259;

fn acp_hub() -> Command {
    Command::new(env!("CARGO_BIN_EXE_acp-hub"))
}

fn terminate_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn pid_still_running(pid: u32) -> bool {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

/// Spawn via the real `ensure_daemon` helper inside the CLI, then wait the
/// CLI process handle. `serve` must still be alive afterwards.
///
/// Windows-only (`cfg`). A missed process-handle signal is a failed
/// isolation, not a passed hang-fix.
#[test]
fn cli_process_handle_signals_while_serve_stays_alive() {
    let home = tempfile::tempdir().expect("isolated hub home");
    let mut child = acp_hub()
        .arg("--home")
        .arg(home.path())
        .args(["agent", "list"])
        .env("ACP_HUB_IDLE_TIMEOUT", "60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn CLI agent list");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut stream| {
                        let mut buf = String::new();
                        let _ = stream.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();
                if !status.success() {
                    panic!("CLI agent list failed (not an H4 skip): {status}\nstderr: {stderr}");
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "CLI process handle did not signal within 20s — H4 isolation failed \
                     (waited hProcess, not Job / stdout EOF)"
                );
            }
            Err(err) => panic!("try_wait on CLI process handle: {err}"),
        }
    }

    let meta_path = home.path().join("daemon.json");
    let meta: DaemonMetadata = match fs::read(meta_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).expect("daemon.json is metadata"),
        Err(err) => panic!("serve should have written daemon.json after CLI exit: {err}"),
    };
    let serve_pid = meta.pid;
    assert_ne!(serve_pid, 0, "daemon.json pid");
    assert!(
        pid_still_running(serve_pid),
        "H4: serve pid {serve_pid} must still be alive after the CLI process handle signaled"
    );

    terminate_pid(serve_pid);
}
