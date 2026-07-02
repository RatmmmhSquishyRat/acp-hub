//! Design 5 — daemon idle-exit test.
//!
//! Verifies that the daemon exits after the idle timeout when no clients
//! are connected and no runs are active.

use std::time::Duration;

#[tokio::test]
async fn daemon_idle_exit_after_timeout() {
    let home = std::env::temp_dir().join(format!("acp-hub-idle-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&home).unwrap();

    // Set a very short idle timeout.
    unsafe { std::env::set_var("ACP_HUB_IDLE_TIMEOUT", "2"); }

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
    unsafe { std::env::remove_var("ACP_HUB_IDLE_TIMEOUT"); }
}

#[tokio::test]
async fn daemon_auto_spawn_and_serve() {
    let home = std::env::temp_dir().join(format!("acp-hub-spawn-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("ACP_HUB_IDLE_TIMEOUT", "3"); }

    // Spawn daemon.
    let h1 = home.clone();
    let handle = tokio::spawn(async move { let _ = acp_hub::daemon::serve(&h1).await; });

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
    unsafe { std::env::remove_var("ACP_HUB_IDLE_TIMEOUT"); }
}

#[tokio::test]
async fn daemon_stale_metadata_cleaned() {
    let home = std::env::temp_dir().join(format!("acp-hub-stale-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&home).unwrap();

    // Write stale metadata pointing to a non-existent endpoint.
    std::fs::write(
        home.join("daemon.json"),
        r#"{"pid":99999,"endpoint":"nonexistent","daemon_id":"stale","started_at":"2020-01-01T00:00:00Z"}"#,
    ).unwrap();

    unsafe { std::env::set_var("ACP_HUB_IDLE_TIMEOUT", "2"); }

    // Spawn daemon — it should detect stale metadata and clean it.
    let h = home.clone();
    let handle = tokio::spawn(async move { let _ = acp_hub::daemon::serve(&h).await; });

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!handle.is_finished(), "daemon should be running despite stale metadata");

    // The new daemon.json should have a different daemon_id.
    let new_meta = std::fs::read_to_string(home.join("daemon.json")).unwrap();
    assert!(!new_meta.contains("stale"), "stale metadata should be replaced");

    // Wait for idle exit.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let _ = std::fs::remove_dir_all(&home);
    unsafe { std::env::remove_var("ACP_HUB_IDLE_TIMEOUT"); }
}
