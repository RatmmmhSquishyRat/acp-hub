//! Phase-1 SC oracles against shipped store/policy paths (PHASE1-CONTRACT §9).

use acp_hub::store::{
    ConvOrigin, ConversationListPage, Interaction, ListConversationsFilter, MessageSource,
    NewConversation, NewConversationOptions, NewMessage, RunStatus, Store,
};
use serde_json::json;

fn store() -> Store {
    Store::open_memory().expect("memory store")
}

fn hub_create(store: &Store, id: &str, sid: &str) {
    store
        .create_conversation(&NewConversation {
            id: id.into(),
            agent_id: "fixture".into(),
            agent_session_id: sid.into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: Some("work".into()),
        })
        .unwrap();
}

#[test]
fn sc_ide_ro_option_a_and_bind_stays_read_only() {
    let store = store();
    let meta = json!({"cursor-adapter": {"space": "ide"}});
    let upsert = store
        .upsert_agent_session_discover(
            "fixture",
            "ide-1",
            Some("IDE chat"),
            Some("/tmp"),
            &[],
            Some(&meta),
        )
        .unwrap();
    assert_eq!(upsert.origin, ConvOrigin::ImportedList);
    assert_eq!(upsert.interaction, Interaction::ReadOnly);
    assert!(!upsert.in_hub_before);

    let all = store
        .list_conversations_filtered(&ListConversationsFilter {
            workbench: false,
            include_imported: true,
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.items.len(), 1);
    assert_eq!(all.items[0].interaction, Interaction::ReadOnly);
    assert_eq!(all.items[0].origin, ConvOrigin::ImportedList);

    // Workbench default hides pure imports
    let wb = store
        .list_conversations_filtered(&ListConversationsFilter::workbench_default())
        .unwrap();
    assert_eq!(wb.items.len(), 0);

    // Bind promote — IDE stays RO
    store
        .promote_conversation_bind(&upsert.conv_id, Some(&meta))
        .unwrap();
    let row = store.conversation(&upsert.conv_id).unwrap().unwrap();
    assert_eq!(row.origin, ConvOrigin::Bound);
    assert_eq!(row.interaction, Interaction::ReadOnly);
}

#[test]
fn sc_bind_acp_becomes_writable() {
    let store = store();
    let meta = json!({"cursor-adapter": {"space": "acp"}});
    let upsert = store
        .upsert_agent_session_discover(
            "fixture",
            "acp-1",
            Some("ACP"),
            Some("/tmp"),
            &[],
            Some(&meta),
        )
        .unwrap();
    assert_eq!(upsert.interaction, Interaction::ReadOnly);
    store
        .promote_conversation_bind(&upsert.conv_id, Some(&meta))
        .unwrap();
    let row = store.conversation(&upsert.conv_id).unwrap().unwrap();
    assert_eq!(row.origin, ConvOrigin::Bound);
    assert_eq!(row.interaction, Interaction::Writable);
}

#[test]
fn sc_nodegrade_discover_keeps_hub_created() {
    let store = store();
    hub_create(&store, "conv-h", "sid-shared");
    let before = store.conversation("conv-h").unwrap().unwrap();
    assert_eq!(before.origin, ConvOrigin::HubCreated);

    let meta = json!({"cursor-adapter": {"space": "acp"}});
    let upsert = store
        .upsert_agent_session_discover(
            "fixture",
            "sid-shared",
            Some("remote title"),
            Some("/tmp"),
            &[],
            Some(&meta),
        )
        .unwrap();
    assert!(upsert.in_hub_before);
    assert_eq!(upsert.origin, ConvOrigin::HubCreated);
    let after = store.conversation("conv-h").unwrap().unwrap();
    assert_eq!(after.origin, ConvOrigin::HubCreated);
    assert_eq!(after.interaction, Interaction::Writable);
    // Title merge: hub_created keeps local when both non-empty
    assert_eq!(after.title.as_deref(), Some("work"));
}

#[test]
fn sc_flood_workbench_vs_all() {
    let store = store();
    for i in 0..120 {
        store
            .upsert_agent_session_discover(
                "fixture",
                &format!("sid-{i}"),
                Some(&format!("t{i}")),
                Some("/tmp"),
                &[],
                None,
            )
            .unwrap();
    }
    let wb = store
        .list_conversations_filtered(&ListConversationsFilter::workbench_default())
        .unwrap();
    assert_eq!(wb.items.len(), 0);

    let all = store
        .list_conversations_filtered(&ListConversationsFilter {
            workbench: false,
            include_imported: true,
            limit: 100,
            offset: 0,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.items.len(), 100);
    assert!(all.truncated);

    hub_create(&store, "conv-wb", "new-hub");
    let wb2 = store
        .list_conversations_filtered(&ListConversationsFilter::workbench_default())
        .unwrap();
    assert_eq!(wb2.items.len(), 1);
    assert_eq!(wb2.items[0].id, "conv-wb");
}

#[test]
fn sc_mk_busy_second_run_conflicts() {
    let store = store();
    hub_create(&store, "conv-b", "sid-b");
    store.create_run("run-1", "conv-b").unwrap();
    let err = store.create_run("run-2", "conv-b").unwrap_err();
    assert!(matches!(err, acp_hub::HubError::Conflict(_)));
    let row = store.conversation("conv-b").unwrap().unwrap();
    assert_eq!(row.busy.as_str(), "running");
    assert_eq!(row.status.as_str(), "running");
}

#[test]
fn sc_daemon_recovery_clears_busy_sets_failed() {
    let store = store();
    hub_create(&store, "conv-d", "sid-d");
    store.create_run("run-d", "conv-d").unwrap();
    let n = store.recover_interrupted_runs().unwrap();
    assert!(n >= 1);
    let row = store.conversation("conv-d").unwrap().unwrap();
    assert_eq!(row.busy.as_str(), "none");
    assert_eq!(row.last_outcome.as_str(), "failed");
    assert_eq!(row.status.as_str(), "failed");
}

#[test]
fn finalize_run_sets_last_outcome() {
    let store = store();
    hub_create(&store, "conv-f", "sid-f");
    store.create_run("run-f", "conv-f").unwrap();
    assert!(
        store
            .finalize_run_cas("run-f", "conv-f", RunStatus::Completed, Some("end_turn"))
            .unwrap()
    );
    let row = store.conversation("conv-f").unwrap().unwrap();
    assert_eq!(row.busy.as_str(), "none");
    assert_eq!(row.last_outcome.as_str(), "completed");
    assert_eq!(row.status.as_str(), "completed");
}

#[test]
fn soft_delete_keeps_unique_and_hides_from_list() {
    let store = store();
    hub_create(&store, "conv-del", "sid-del");
    store.delete_conversation("conv-del").unwrap();
    let row = store.conversation("conv-del").unwrap().unwrap();
    assert_eq!(row.phase.as_str(), "deleted");
    let wb = store
        .list_conversations_filtered(&ListConversationsFilter {
            include_imported: true,
            workbench: false,
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert!(wb.items.iter().all(|c| c.id != "conv-del"));
    // Revive via discover same sid
    let up = store
        .upsert_agent_session_discover(
            "fixture",
            "sid-del",
            Some("revived"),
            Some("/tmp"),
            &[],
            None,
        )
        .unwrap();
    assert!(!up.in_hub_before);
    assert_eq!(up.origin, ConvOrigin::ImportedList);
}

#[test]
fn close_while_busy_sets_failed_outcome() {
    let store = store();
    hub_create(&store, "conv-c", "sid-c");
    store.create_run("run-c", "conv-c").unwrap();
    store.close_conversation_local("conv-c", true).unwrap();
    let row = store.conversation("conv-c").unwrap().unwrap();
    assert_eq!(row.phase.as_str(), "closed");
    assert_eq!(row.busy.as_str(), "none");
    assert_eq!(row.last_outcome.as_str(), "failed");
    assert_eq!(row.status.as_str(), "closed");
}

#[test]
fn migration_backfill_origin_from_layer2() {
    let store = store();
    hub_create(&store, "conv-m", "sid-m");
    store
        .append_message(&NewMessage {
            id: "m1".into(),
            conv_id: "conv-m".into(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "user".into(),
            kind: None,
            content_json: json!({"text": "hi"}),
            body_text: "hi".into(),
        })
        .unwrap();
    let row = store.conversation("conv-m").unwrap().unwrap();
    assert_eq!(row.origin, ConvOrigin::HubCreated);
    // Layer2 presence keeps workbench even if we hypothetically listed
    let page: ConversationListPage = store
        .list_conversations_filtered(&ListConversationsFilter::workbench_default())
        .unwrap();
    assert_eq!(page.items.len(), 1);
}

#[test]
fn bind_create_options_origin_bound() {
    let store = store();
    store
        .create_conversation_with_options(
            &NewConversation {
                id: "conv-bound".into(),
                agent_id: "fixture".into(),
                agent_session_id: "remote-x".into(),
                cwd: Some("/tmp".into()),
                additional_directories: vec![],
                title: None,
            },
            &NewConversationOptions {
                origin: ConvOrigin::Bound,
                session_meta: Some(json!({"cursor-adapter": {"space": "acp"}})),
            },
        )
        .unwrap();
    let row = store.conversation("conv-bound").unwrap().unwrap();
    assert_eq!(row.origin, ConvOrigin::Bound);
    assert_eq!(row.interaction, Interaction::Writable);
}

#[test]
fn write_gate_helpers_reject_read_only() {
    use acp_hub::HubError;
    let err = HubError::read_only_conversation("c1", "imported_list", "read_only", false);
    match err {
        HubError::ReadOnlyConversation {
            conv_id,
            origin,
            interaction,
            message,
        } => {
            assert_eq!(conv_id, "c1");
            assert_eq!(origin, "imported_list");
            assert_eq!(interaction, "read_only");
            assert!(message.contains("read-only"));
        }
        other => panic!("unexpected {other:?}"),
    }
    let ide = HubError::read_only_conversation("c2", "bound", "read_only", true);
    assert!(ide.to_string().contains("IDE"));
}
