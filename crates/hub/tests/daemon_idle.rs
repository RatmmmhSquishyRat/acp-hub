//! Design 5 — daemon idle-exit test.
//!
//! Verifies that the daemon exits after the idle timeout when no clients
//! are connected and no runs are active.

use serial_test::serial;
use std::path::PathBuf;
use std::time::Duration;

/// Short temp home paths keep Unix `sun_path` within platform limits even when
/// the runner's `temp_dir()` is already deep (common on macOS CI).
fn short_test_home(prefix: &str) -> PathBuf {
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let home = std::env::temp_dir().join(format!("{prefix}-{id}"));
    std::fs::create_dir_all(&home).unwrap();
    home
}

#[tokio::test]
#[serial]
async fn daemon_idle_exit_after_timeout() {
    let home = short_test_home("ah-idle");

    // Set a very short idle timeout.
    unsafe {
        std::env::set_var("ACP_HUB_IDLE_TIMEOUT", "2");
    }

    // Spawn the daemon.
    let home_clone = home.clone();
    let handle = tokio::spawn(async move {
        let _ = acp_hub::daemon::serve(&home_clone).await;
    });

    // Wait for the idle timeout (2s) plus a margin.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // The daemon should have exited on its own. Check that the handle
    // resolved (task completed).
    let finished = handle.is_finished();
    assert!(finished, "daemon should have exited after idle timeout");

    // Clean up.
    let _ = std::fs::remove_dir_all(&home);
    unsafe {
        std::env::remove_var("ACP_HUB_IDLE_TIMEOUT");
    }
}

#[tokio::test]
#[serial]
async fn daemon_auto_spawn_and_serve() {
    let home = short_test_home("ah-spawn");
    unsafe {
        std::env::set_var("ACP_HUB_IDLE_TIMEOUT", "3");
    }

    // Spawn daemon.
    let h1 = home.clone();
    let handle = tokio::spawn(async move {
        let _ = acp_hub::daemon::serve(&h1).await;
    });

    // Wait for daemon to start.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!handle.is_finished(), "daemon should be running");

    // Daemon metadata should exist.
    let metadata = std::fs::read_to_string(home.join("daemon.json"));
    assert!(metadata.is_ok(), "daemon.json should exist");

    // Wait for idle exit.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(handle.is_finished(), "daemon should exit after idle");

    let _ = std::fs::remove_dir_all(&home);
    unsafe {
        std::env::remove_var("ACP_HUB_IDLE_TIMEOUT");
    }
}

/// Read the `daemon_id` field from the home's daemon.json (None if missing/unparseable).
fn read_daemon_id(home: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join("daemon.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("daemon_id")?
        .as_str()
        .map(str::to_owned)
}

#[tokio::test]
#[serial]
async fn daemon_stale_metadata_cleaned() {
    let home = short_test_home("ah-stale");

    // Write stale metadata pointing to a non-existent endpoint.
    std::fs::write(
        home.join("daemon.json"),
        r#"{"pid":99999,"endpoint":"nonexistent","daemon_id":"stale","started_at":"2020-01-01T00:00:00Z"}"#,
    ).unwrap();

    unsafe {
        std::env::set_var("ACP_HUB_IDLE_TIMEOUT", "2");
    }

    // Spawn daemon — it should detect stale metadata and take over.
    let h = home.clone();
    let handle = tokio::spawn(async move { acp_hub::daemon::serve(&h).await });

    // Poll for either metadata replacement or an early serve() failure. A fixed
    // sleep plus discarding serve()'s Result previously masked the real cause.
    let mut replaced = false;
    for _ in 0..30 {
        if handle.is_finished() {
            let outcome = handle.await.expect("daemon task panicked");
            panic!("daemon exited unexpectedly instead of taking over: {outcome:?}");
        }
        if read_daemon_id(&home).is_some_and(|id| id != "stale") {
            replaced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        replaced,
        "stale metadata was not replaced (daemon did not take over)"
    );

    // Wait for idle exit.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let _ = std::fs::remove_dir_all(&home);
    unsafe {
        std::env::remove_var("ACP_HUB_IDLE_TIMEOUT");
    }
}
