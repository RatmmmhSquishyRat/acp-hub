use std::time::Duration;

use super::support::{
    assert_operational_registry_fields, assert_registry_read_is_secret_free, registry_with_secrets,
};
use super::{CoreHub, HubClient};
use crate::error::HubError;
use crate::store::{NewConversation, RunStatus, Store};

#[tokio::test]
async fn hub_client_registry_reads_do_not_expose_endpoint_secrets() {
    let home = tempfile::tempdir().unwrap();
    registry_with_secrets().save(home.path()).unwrap();
    let store = Store::open(home.path()).unwrap();
    store
        .upsert_agent_cache(
            "stdio",
            r#"{"name":"cached-agent"}"#,
            r#"{"loadSession":true}"#,
        )
        .unwrap();
    drop(store);

    let daemon_home = home.path().to_path_buf();
    let daemon = tokio::spawn(async move {
        crate::daemon::serve(&daemon_home).await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(15), async {
        while !home.path().join("daemon.json").is_file() {
            assert!(!daemon.is_finished(), "daemon exited before becoming ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("daemon did not become ready");

    let client = HubClient::connect_or_spawn(home.path()).await.unwrap();
    let mut reads = vec![client.list_agents().await.unwrap()];
    for agent_id in [
        "stdio",
        "http",
        "websocket",
        "uppercase-scheme",
        "malformed",
        "unsupported",
        "newline-authority",
        "tab-authority",
        "space-authority",
    ] {
        reads.push(client.inspect_agent(agent_id).await.unwrap());
    }
    reads.push(client.list_proxies().await.unwrap());

    for read in &reads {
        assert_registry_read_is_secret_free(read);
    }
    assert_operational_registry_fields(&reads);

    drop(client);
    daemon.abort();
    let _ = daemon.await;
}

#[tokio::test]
async fn only_the_daemon_lock_owner_recovers_runs_and_second_serve_is_busy() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    store
        .create_conversation(&NewConversation {
            id: "owner-recovery-conv".into(),
            agent_id: "owner-recovery-agent".into(),
            agent_session_id: "owner-recovery-session".into(),
            cwd: Some(home.path().to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    store
        .create_run("orphan-before-daemon", "owner-recovery-conv")
        .unwrap();
    drop(store);

    let daemon_home = home.path().to_path_buf();
    let daemon = tokio::spawn(async move {
        crate::daemon::serve(&daemon_home).await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(15), async {
        while !home.path().join("daemon.json").is_file() {
            assert!(!daemon.is_finished(), "daemon exited before becoming ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("daemon did not become ready");

    let observer = Store::open(home.path()).unwrap();
    assert_eq!(
        observer.run_status("orphan-before-daemon").unwrap(),
        Some(RunStatus::Failed)
    );
    drop(observer);

    let client = HubClient::connect_or_spawn(home.path()).await.unwrap();
    let active = client.create_run("owner-recovery-conv").await.unwrap();

    let secondary = CoreHub::open(home.path()).unwrap();
    assert_eq!(
        secondary.store().run_status(&active.run_id).unwrap(),
        Some(RunStatus::Running)
    );
    drop(secondary);

    let replay_owner = Store::open(home.path()).unwrap();
    let live_refresh = replay_owner
        .begin_load_replay("owner-recovery-conv", "live-owner-refresh")
        .unwrap();
    let secondary = CoreHub::open(home.path()).unwrap();
    drop(secondary);
    replay_owner.rollback_load_replay(live_refresh).unwrap();
    drop(replay_owner);

    let busy = crate::daemon::serve(home.path()).await.unwrap_err();
    assert!(matches!(
        busy,
        HubError::DaemonUnavailable(message)
            if message == "another ACP Hub daemon already holds daemon.lock"
    ));
    let observer = Store::open(home.path()).unwrap();
    assert_eq!(
        observer.run_status(&active.run_id).unwrap(),
        Some(RunStatus::Running)
    );
    drop(observer);

    assert!(
        client
            .finalize_run(
                "owner-recovery-conv",
                &active.run_id,
                &active.owner_token,
                RunStatus::Completed,
                None,
            )
            .await
            .unwrap()
    );
    drop(client);
    daemon.abort();
    let _ = daemon.await;
}
