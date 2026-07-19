use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::support::{
    assert_operational_registry_fields, assert_registry_read_is_secret_free, core_registry_reads,
    fixture_hub, registry_with_secrets, stored_conversation, wait_for_marker,
};
use super::{
    CoreHub, OperationEntry, OperationKind, OperationMap, PromptOperation, ReplayMethod,
    reject_active_agents, require_absolute_cwd,
};
use crate::daemon::ActivityTracker;
use crate::store::Store;
use uuid::Uuid;

#[tokio::test]
async fn core_registry_reads_do_not_expose_endpoint_secrets() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_memory().unwrap();
    store
        .upsert_agent_cache(
            "stdio",
            r#"{"name":"cached-agent"}"#,
            r#"{"loadSession":true}"#,
        )
        .unwrap();
    let hub = CoreHub::new(
        home.path(),
        registry_with_secrets(),
        store,
        Arc::new(ActivityTracker::new()),
    );

    let reads = core_registry_reads(&hub).await;
    for read in &reads {
        assert_registry_read_is_secret_free(read);
    }
    assert_operational_registry_fields(&reads);
}
#[test]
fn cwd_must_be_explicit_and_absolute() {
    assert!(require_absolute_cwd(None).is_err());
    assert!(require_absolute_cwd(Some(PathBuf::from("relative"))).is_err());

    let absolute = std::env::temp_dir();
    assert_eq!(
        require_absolute_cwd(Some(absolute.clone())).unwrap(),
        absolute
    );
}

#[test]
fn registry_mutations_reject_agents_with_active_runs() {
    let mut operations = OperationMap::new();
    operations.insert(
        "conv-active".into(),
        OperationEntry {
            token: Uuid::new_v4(),
            agent_id: "agent-a".into(),
            kind: OperationKind::Prompt(PromptOperation {
                run_id: "run-active".into(),
                agent_session_id: "session-active".into(),
                cancel_requested: false,
            }),
        },
    );

    let error = reject_active_agents(&operations, &["agent-a".into()]).unwrap_err();
    assert!(matches!(&error, super::HubError::Conflict(id) if id == "conv-active"));
    assert!(reject_active_agents(&operations, &["agent-b".into()]).is_ok());
}
#[tokio::test]
async fn endpoint_removal_waits_for_old_load_then_revokes_its_binding() {
    let (home, hub) = fixture_hub("refresh-block", 0);
    let conv = stored_conversation(&hub, "conv-remove-race", "refresh-session", home.path());
    let handle = hub.agent_handle("fixture").await.unwrap();
    let load_hub = Arc::clone(&hub);
    let load_handle = Arc::clone(&handle);
    let load_conv = conv.clone();
    let load_cwd = home.path().to_path_buf();
    let load = tokio::spawn(async move {
        load_hub
            .refresh_session_projection(&load_handle, &load_conv, load_cwd, ReplayMethod::Load)
            .await
    });
    wait_for_marker(&home.path().join("load-ready")).await;
    assert!(hub.ctx.is_session_bound("fixture", "refresh-session"));

    let remove_hub = Arc::clone(&hub);
    let mut remove = tokio::spawn(async move { remove_hub.remove_agent("fixture").await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut remove)
            .await
            .is_err(),
        "endpoint removal did not wait for the old agent command"
    );

    fs::write(home.path().join("load-release"), "").unwrap();
    load.await.unwrap().unwrap();
    remove.await.unwrap().unwrap();
    assert!(!hub.ctx.is_session_bound("fixture", "refresh-session"));
    assert!(!hub.registry.read().agents.contains_key("fixture"));
    assert!(!hub.handles.lock().await.contains_key("fixture"));
}
