//! Design 5 — daemon idle-exit test.
//!
//! Verifies that the daemon exits after the idle timeout when no clients
//! are connected and no runs are active.

use serial_test::serial;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Short temp home paths keep Unix `sun_path` within platform limits even when
/// the runner's `temp_dir()` is already deep (common on macOS CI).
fn short_test_home(prefix: &str) -> PathBuf {
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let home = std::env::temp_dir().join(format!("{prefix}-{id}"));
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// Read the `daemon_id` field from the home's daemon.json (None if missing/unparseable).
fn read_daemon_id(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join("daemon.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("daemon_id")?
        .as_str()
        .map(str::to_owned)
}

/// Poll until the daemon has written live metadata (ready to accept clients).
async fn wait_until_daemon_ready(home: &Path, handle: &JoinHandle<()>, deadline: Duration) {
    let started = Instant::now();
    while started.elapsed() < deadline {
        assert!(
            !handle.is_finished(),
            "daemon exited before becoming ready (no stable daemon.json)"
        );
        if read_daemon_id(home).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not become ready within {deadline:?} (no daemon.json / daemon_id)");
}

/// Poll until the serve task finishes (idle exit or error).
async fn wait_until_daemon_exits(handle: &JoinHandle<()>, deadline: Duration) {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if handle.is_finished() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        handle.is_finished(),
        "daemon should have exited within {deadline:?} after idle timeout"
    );
}

struct IdleEnvGuard;
impl Drop for IdleEnvGuard {
    fn drop(&mut self) {
        // SAFETY: test-only env mutation, serialized by #[serial].
        unsafe {
            std::env::remove_var("ACP_HUB_IDLE_TIMEOUT");
        }
    }
}

fn set_idle_timeout_secs(secs: u64) -> IdleEnvGuard {
    // SAFETY: test-only env mutation, serialized by #[serial].
    unsafe {
        std::env::set_var("ACP_HUB_IDLE_TIMEOUT", secs.to_string());
    }
    IdleEnvGuard
}

#[tokio::test]
#[serial]
async fn daemon_idle_exit_after_timeout() {
    let home = short_test_home("ah-idle");
    let _env = set_idle_timeout_secs(2);

    let home_clone = home.clone();
    let handle = tokio::spawn(async move {
        let _ = acp_hub::daemon::serve(&home_clone).await;
    });

    // Wait for listen/metadata first — cold CI runners can take >1s to bind/store.
    wait_until_daemon_ready(&home, &handle, Duration::from_secs(15)).await;

    // Idle timer starts once the server is running; allow timeout + CI slack.
    wait_until_daemon_exits(&handle, Duration::from_secs(12)).await;
    let _ = handle.await;

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
#[serial]
async fn daemon_auto_spawn_and_serve() {
    let home = short_test_home("ah-spawn");
    let _env = set_idle_timeout_secs(2);

    let h1 = home.clone();
    let handle = tokio::spawn(async move {
        let _ = acp_hub::daemon::serve(&h1).await;
    });

    wait_until_daemon_ready(&home, &handle, Duration::from_secs(15)).await;
    assert!(
        !handle.is_finished(),
        "daemon should still be running after ready"
    );

    let metadata = std::fs::read_to_string(home.join("daemon.json"));
    assert!(metadata.is_ok(), "daemon.json should exist");

    wait_until_daemon_exits(&handle, Duration::from_secs(12)).await;
    let _ = handle.await;

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
#[serial]
async fn daemon_stale_metadata_cleaned() {
    let home = short_test_home("ah-stale");

    // Write stale metadata pointing to a non-existent endpoint.
    std::fs::write(
        home.join("daemon.json"),
        r#"{"pid":99999,"endpoint":"nonexistent","daemon_id":"stale","started_at":"2020-01-01T00:00:00Z"}"#,
    )
    .unwrap();

    let _env = set_idle_timeout_secs(2);

    let h = home.clone();
    let handle = tokio::spawn(async move {
        let _ = acp_hub::daemon::serve(&h).await;
    });

    // Poll for metadata replacement or an early serve() failure.
    let mut replaced = false;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        assert!(
            !handle.is_finished(),
            "daemon exited unexpectedly instead of taking over"
        );
        if read_daemon_id(&home).is_some_and(|id| id != "stale") {
            replaced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        replaced,
        "stale metadata was not replaced (daemon did not take over)"
    );

    wait_until_daemon_exits(&handle, Duration::from_secs(12)).await;
    let _ = handle.await;

    let _ = std::fs::remove_dir_all(&home);
}
