use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
use crate::endpoint::AgentTransport;
use crate::store::Store;
use tokio::sync::oneshot;
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

#[tokio::test]
async fn registry_replacement_waits_for_initializer_and_clears_its_generation() {
    let (_home, hub) = fixture_hub("churn", 0);
    let (publish_reached_tx, publish_reached_rx) = oneshot::channel();
    let (publish_release_tx, publish_release_rx) = oneshot::channel();
    *hub.handle_publish_gate.lock() = Some((publish_reached_tx, publish_release_rx));

    let initialize_hub = Arc::clone(&hub);
    let initialize = tokio::spawn(async move { initialize_hub.agent_handle("fixture").await });
    tokio::time::timeout(Duration::from_secs(10), publish_reached_rx)
        .await
        .expect("initializer did not reach publication gate")
        .expect("initializer dropped publication gate");

    let mut replacement = hub.agent_config("fixture").unwrap();
    let AgentTransport::Stdio { args, .. } = &mut replacement.transport else {
        panic!("fixture transport changed");
    };
    args.push("replacement-generation".to_string());
    let replace_hub = Arc::clone(&hub);
    let mut replace =
        tokio::spawn(async move { replace_hub.register_agent("fixture", replacement).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut replace)
            .await
            .is_err(),
        "registry replacement did not wait for the in-flight initializer"
    );

    publish_release_tx
        .send(())
        .expect("initializer publication receiver dropped");
    initialize.await.unwrap().unwrap();
    replace.await.unwrap().unwrap();

    assert!(!hub.handles.lock().await.contains_key("fixture"));
    assert!(hub.store().agent_cache("fixture").unwrap().is_none());
    let current = hub.agent_config("fixture").unwrap();
    let AgentTransport::Stdio { args, .. } = current.transport else {
        panic!("fixture transport changed");
    };
    assert_eq!(
        args.last().map(String::as_str),
        Some("replacement-generation")
    );
}

#[tokio::test]
async fn failed_registry_save_preserves_the_previous_agent_cache() {
    let (home, hub) = fixture_hub("churn", 0);
    let current_registry = hub.registry.read().clone();
    current_registry.save(home.path()).unwrap();
    *hub.registry_fingerprint.write() =
        crate::endpoint::Registry::fingerprint(home.path()).unwrap();
    hub.store()
        .upsert_agent_cache("fixture", r#"{"name":"stable"}"#, r#"{"loadSession":true}"#)
        .unwrap();
    hub.registry_save_fail_once.store(true, Ordering::Release);

    let mut replacement = hub.agent_config("fixture").unwrap();
    let AgentTransport::Stdio { args, .. } = &mut replacement.transport else {
        panic!("fixture transport changed");
    };
    args.push("must-not-commit".to_string());
    assert!(hub.register_agent("fixture", replacement).await.is_err());

    assert!(
        hub.store().agent_cache("fixture").unwrap().is_some(),
        "a failed pre-commit save must not invalidate the live registry cache"
    );
    assert_eq!(*hub.registry.read(), current_registry);
    assert_eq!(
        crate::endpoint::Registry::load(home.path()).unwrap(),
        current_registry
    );
}

#[tokio::test]
async fn successful_registry_save_publishes_despite_reload_verification_failure() {
    let (home, hub) = fixture_hub("churn", 0);
    let current_registry = hub.registry.read().clone();
    current_registry.save(home.path()).unwrap();
    *hub.registry_fingerprint.write() =
        crate::endpoint::Registry::fingerprint(home.path()).unwrap();
    hub.agent_handle("fixture").await.unwrap();
    assert!(hub.handles.lock().await.contains_key("fixture"));
    assert!(hub.store().agent_cache("fixture").unwrap().is_some());
    hub.registry_verify_fail_once.store(true, Ordering::Release);

    let mut replacement = hub.agent_config("fixture").unwrap();
    let AgentTransport::Stdio { args, .. } = &mut replacement.transport else {
        panic!("fixture transport changed");
    };
    args.push("verified-after-reload-failure".to_string());
    hub.register_agent("fixture", replacement.clone())
        .await
        .unwrap();

    assert_eq!(
        hub.registry.read().agents.get("fixture"),
        Some(&replacement)
    );
    assert_eq!(
        crate::endpoint::Registry::load(home.path())
            .unwrap()
            .agents
            .get("fixture"),
        Some(&replacement)
    );
    assert!(!hub.handles.lock().await.contains_key("fixture"));
    assert!(hub.store().agent_cache("fixture").unwrap().is_none());
}

#[tokio::test]
async fn failed_session_list_import_removes_ghost_and_restores_existing_metadata() {
    let (home, hub) = fixture_hub("refresh-error-block", 0);
    hub.store()
        .upsert_agent_session(
            "fixture",
            "refresh-session",
            Some("stable title"),
            Some("/stable/cwd"),
            &["/stable/root".to_string()],
        )
        .unwrap();
    let existing = hub
        .store()
        .conversation_by_agent_session("fixture", "refresh-session")
        .unwrap()
        .unwrap();

    let list_hub = Arc::clone(&hub);
    let listing = tokio::spawn(async move { list_hub.list_agent_sessions("fixture").await });
    wait_for_marker(&home.path().join("load-ready")).await;
    let provisional = hub.store().conversation(&existing.id).unwrap().unwrap();
    assert_eq!(provisional.cwd.as_deref(), home.path().to_str());
    fs::write(home.path().join("load-release"), "").unwrap();
    assert!(listing.await.unwrap().is_err());

    let restored = hub.store().conversation(&existing.id).unwrap().unwrap();
    assert_eq!(restored.title.as_deref(), Some("stable title"));
    assert_eq!(restored.cwd.as_deref(), Some("/stable/cwd"));
    assert_eq!(restored.additional_directories, vec!["/stable/root"]);

    hub.store().delete_conversation(&existing.id).unwrap();
    fs::remove_file(home.path().join("load-release")).unwrap();
    fs::remove_file(home.path().join("load-ready")).unwrap();
    let list_hub = Arc::clone(&hub);
    let listing = tokio::spawn(async move { list_hub.list_agent_sessions("fixture").await });
    wait_for_marker(&home.path().join("load-ready")).await;
    fs::write(home.path().join("load-release"), "").unwrap();
    assert!(listing.await.unwrap().is_err());
    assert!(
        hub.store()
            .conversation_by_agent_session("fixture", "refresh-session")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn session_list_rejects_relative_paths_before_storage_and_deduplicates_imports() {
    let (_home, relative_hub) = fixture_hub("relative-list", 0);
    assert!(relative_hub.list_agent_sessions("fixture").await.is_err());
    assert!(
        relative_hub
            .store()
            .list_conversations(None)
            .unwrap()
            .is_empty()
    );

    let (home, duplicate_hub) = fixture_hub("duplicate-list", 0);
    duplicate_hub.list_agent_sessions("fixture").await.unwrap();
    assert_eq!(
        duplicate_hub
            .store()
            .list_conversations(Some("fixture"))
            .unwrap()
            .len(),
        1
    );
    let methods = fs::read_to_string(home.path().join("methods")).unwrap();
    assert_eq!(
        methods
            .lines()
            .filter(|line| *line == "load:duplicate-session")
            .count(),
        1
    );
}

#[tokio::test]
async fn session_import_admission_precedes_replay_for_an_existing_conversation() {
    let (home, hub) = fixture_hub("churn", 0);
    hub.store()
        .upsert_agent_session(
            "fixture",
            "refresh-session",
            Some("stable title"),
            Some("/stable/cwd"),
            &["/stable/root".to_string()],
        )
        .unwrap();
    let existing = hub
        .store()
        .conversation_by_agent_session("fixture", "refresh-session")
        .unwrap()
        .unwrap();
    let explicit_create_operation = hub
        .reserve_operation(&existing.id, "fixture", OperationKind::Refresh)
        .unwrap();

    let error = hub.list_agent_sessions("fixture").await.unwrap_err();
    assert!(matches!(error, super::HubError::Conflict(id) if id == existing.id));
    {
        let operations = hub.operations.lock();
        assert_eq!(operations.len(), 1);
        assert!(operations.contains_key(&existing.id));
    }
    assert_eq!(
        hub.store()
            .list_conversations(Some("fixture"))
            .unwrap()
            .len(),
        1,
        "the losing import must not leave a provisional conversation"
    );
    let restored = hub.store().conversation(&existing.id).unwrap().unwrap();
    assert_eq!(restored.title.as_deref(), Some("stable title"));
    assert_eq!(restored.cwd.as_deref(), Some("/stable/cwd"));
    assert_eq!(restored.additional_directories, vec!["/stable/root"]);

    drop(explicit_create_operation);
    hub.list_agent_sessions("fixture").await.unwrap();
    let methods = fs::read_to_string(home.path().join("methods")).unwrap();
    assert_eq!(
        methods
            .lines()
            .filter(|line| *line == "load:refresh-session")
            .count(),
        1
    );
}

#[tokio::test]
async fn explicit_supplied_session_create_conflicts_with_discovery_identity_owner() {
    let (home, hub) = fixture_hub("refresh-block", 0);
    let list_hub = Arc::clone(&hub);
    let listing = tokio::spawn(async move { list_hub.list_agent_sessions("fixture").await });
    wait_for_marker(&home.path().join("load-ready")).await;

    let create_hub = Arc::clone(&hub);
    let create_cwd = home.path().to_path_buf();
    let creating = tokio::spawn(async move {
        create_hub
            .create_conversation(super::CreateConversationParams {
                agent_id: "fixture".to_string(),
                cwd: Some(create_cwd),
                agent_session_id: Some("refresh-session".to_string()),
                mcp_servers: Vec::new(),
                additional_directories: Vec::new(),
            })
            .await
    });
    let create_error = tokio::time::timeout(Duration::from_millis(300), creating)
        .await
        .expect("explicit create did not reject the discovery identity owner")
        .unwrap()
        .unwrap_err();
    let durable = hub
        .store()
        .conversation_by_agent_session("fixture", "refresh-session")
        .unwrap()
        .unwrap();
    assert!(matches!(create_error, super::HubError::Conflict(id) if id == durable.id));

    fs::write(home.path().join("load-release"), "").unwrap();
    listing.await.unwrap().unwrap();
    let created = hub
        .create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: Some("refresh-session".to_string()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(created.conv_id, durable.id);
    assert_eq!(
        hub.store()
            .list_conversations(Some("fixture"))
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn independent_session_identity_owners_do_not_block_each_other() {
    let (home, hub) = fixture_hub("refresh-block", 0);
    let independent_config = hub.agent_config("fixture").unwrap();
    hub.registry
        .write()
        .agents
        .insert("independent".to_string(), independent_config);
    let list_hub = Arc::clone(&hub);
    let listing = tokio::spawn(async move { list_hub.list_agent_sessions("fixture").await });
    wait_for_marker(&home.path().join("load-ready")).await;

    let created = tokio::time::timeout(
        Duration::from_secs(2),
        hub.create_conversation(super::CreateConversationParams {
            agent_id: "independent".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: Some("independent-session".to_string()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        }),
    )
    .await
    .expect("unrelated session identity was head-of-line blocked")
    .unwrap();
    assert_eq!(created.agent_session_id, "independent-session");
    assert_eq!(hub.session_identities.lock().len(), 1);

    fs::write(home.path().join("load-release"), "").unwrap();
    listing.await.unwrap().unwrap();
    assert!(hub.session_identities.lock().is_empty());
}

#[tokio::test]
async fn session_new_projection_joins_the_same_identity_ownership_domain() {
    let (home, hub) = fixture_hub("churn", 0);
    let identity = hub
        .reserve_session_identity("fixture", "new-session", "conv-owned")
        .unwrap();

    let error = hub
        .create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: None,
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        })
        .await
        .unwrap_err();
    assert!(matches!(error, super::HubError::Conflict(id) if id == "conv-owned"));
    assert!(
        hub.store()
            .conversation_by_agent_session("fixture", "new-session")
            .unwrap()
            .is_none()
    );

    drop(identity);
    assert!(hub.session_identities.lock().is_empty());
}
