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
async fn soft_delete_revives_on_discover_without_load() {
    // Phase 1: discover is metadata-only; soft-deleted rows revive as imported_list.
    let (_home, hub) = fixture_hub("refresh-error-block", 0);
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
    hub.store().delete_conversation(&existing.id).unwrap();
    let deleted = hub.store().conversation(&existing.id).unwrap().unwrap();
    assert_eq!(deleted.phase.as_str(), "deleted");

    let sessions = hub.list_agent_sessions("fixture").await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].get("in_hub_before").and_then(|v| v.as_bool()),
        Some(false)
    );
    let revived = hub.store().conversation(&existing.id).unwrap().unwrap();
    assert_eq!(revived.phase.as_str(), "open");
    assert_eq!(revived.origin.as_str(), "imported_list");
    assert_eq!(revived.interaction.as_str(), "read_only");
}

#[tokio::test]
async fn session_list_rejects_relative_paths_before_storage_and_deduplicates_imports() {
    let (_home, relative_hub) = fixture_hub("relative-list", 0);
    assert!(relative_hub.list_agent_sessions("fixture").await.is_err());
    // Workbench default empty; museum list also empty (no durable import on reject).
    assert!(
        relative_hub
            .store()
            .list_conversations_filtered(&crate::store::ListConversationsFilter {
                include_imported: true,
                workbench: false,
                limit: 100,
                ..Default::default()
            })
            .unwrap()
            .items
            .is_empty()
    );

    let (_home, duplicate_hub) = fixture_hub("duplicate-list", 0);
    // Phase 1: metadata-only discover (no session/load).
    let sessions = duplicate_hub.list_agent_sessions("fixture").await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].get("interaction").and_then(|v| v.as_str()),
        Some("read_only")
    );
    assert_eq!(
        duplicate_hub
            .store()
            .list_conversations_filtered(&crate::store::ListConversationsFilter {
                agent_id: Some("fixture".into()),
                include_imported: true,
                workbench: false,
                limit: 100,
                ..Default::default()
            })
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn session_discover_metadata_only_no_load_and_no_downgrade() {
    let (_home, hub) = fixture_hub("churn", 0);
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

    // Metadata discover succeeds even while a refresh operation is reserved
    // (no session/load, so no OperationKind::Refresh on discover path).
    let sessions = hub.list_agent_sessions("fixture").await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].get("in_hub_before").and_then(|v| v.as_bool()),
        Some(true)
    );
    let restored = hub.store().conversation(&existing.id).unwrap().unwrap();
    // imported_list discover prefers remote title/cwd when remote is non-empty.
    assert_eq!(restored.origin.as_str(), "imported_list");
    assert_eq!(restored.interaction.as_str(), "read_only");
    assert_eq!(restored.id, existing.id);
}

#[tokio::test]
async fn bind_promotes_discovered_row_and_stays_single_identity() {
    // Phase 1: discover metadata first, then promote + bind without hanging load.
    // Use non-blocking fixture (churn) so session/load completes.
    let (home, hub) = fixture_hub("churn", 0);
    let sessions = hub.list_agent_sessions("fixture").await.unwrap();
    assert_eq!(sessions.len(), 1);
    let conv_id = sessions[0]
        .get("conv_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let row = hub.store().conversation(&conv_id).unwrap().unwrap();
    assert_eq!(row.origin.as_str(), "imported_list");
    assert_eq!(row.interaction.as_str(), "read_only");

    let created = hub
        .create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: Some(row.agent_session_id.clone()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(created.conv_id, conv_id);
    let bound = hub.store().conversation(&conv_id).unwrap().unwrap();
    assert_eq!(bound.origin.as_str(), "bound");
    let museum = hub
        .store()
        .list_conversations_filtered(&crate::store::ListConversationsFilter {
            agent_id: Some("fixture".into()),
            include_imported: true,
            workbench: false,
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(museum.items.len(), 1);
}

#[tokio::test]
async fn independent_session_identity_owners_do_not_block_each_other() {
    let (home, hub) = fixture_hub("refresh-block", 0);
    let independent_config = hub.agent_config("fixture").unwrap();
    hub.registry
        .write()
        .agents
        .insert("independent".to_string(), independent_config);

    // Discover on fixture (metadata-only) does not block create on independent agent.
    let list_hub = Arc::clone(&hub);
    let listing = tokio::spawn(async move { list_hub.list_agent_sessions("fixture").await });

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
    listing.await.unwrap().unwrap();
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
