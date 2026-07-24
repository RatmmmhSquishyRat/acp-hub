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
    assert!(matches!(
        err,
        acp_hub::HubError::ConversationBusy { ref busy, .. } if busy == "running"
    ));
    assert_eq!(err.phase1_code(), Some("conversation_busy"));
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
fn write_gate_and_send_reject_imported_list_and_ide() {
    use acp_hub::HubError;
    use acp_hub::daemon::ActivityTracker;
    use acp_hub::endpoint::Registry;
    use acp_hub::hub::{CoreHub, SendPromptParams};
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
    use std::sync::Arc;

    let store = Store::open_memory().unwrap();
    // imported_list RO
    store
        .upsert_agent_session_discover(
            "fixture",
            "ide-send",
            Some("IDE"),
            Some("/tmp"),
            &[],
            Some(&json!({"cursor-adapter": {"space": "ide"}})),
        )
        .unwrap();
    let imported = store
        .conversation_by_agent_session("fixture", "ide-send")
        .unwrap()
        .unwrap();
    assert_eq!(imported.interaction, Interaction::ReadOnly);

    // Shipped gate used by send/param/mode
    let gate = CoreHub::assert_write_gate(&imported);
    match gate {
        Err(HubError::ReadOnlyConversation {
            conv_id,
            origin,
            interaction,
            message,
        }) => {
            assert_eq!(conv_id, imported.id);
            assert_eq!(origin, "imported_list");
            assert_eq!(interaction, "read_only");
            assert!(
                message.contains("IDE") || message.contains("read-only"),
                "{message}"
            );
        }
        other => panic!("expected ReadOnlyConversation from assert_write_gate, got {other:?}"),
    }

    // After bind, IDE stays RO — gate still rejects
    store
        .promote_conversation_bind(
            &imported.id,
            Some(&json!({"cursor-adapter": {"space": "ide"}})),
        )
        .unwrap();
    let bound = store.conversation(&imported.id).unwrap().unwrap();
    assert_eq!(bound.origin, ConvOrigin::Bound);
    assert_eq!(bound.interaction, Interaction::ReadOnly);
    assert!(matches!(
        CoreHub::assert_write_gate(&bound),
        Err(HubError::ReadOnlyConversation { .. })
    ));

    // send_prompt takes the real gate path before agent I/O
    let hub = CoreHub::new(
        tempfile::tempdir().unwrap().path(),
        Registry::default(),
        store,
        Arc::new(ActivityTracker::new()),
    );
    let err = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(hub.send_prompt(SendPromptParams {
            conv_id: bound.id.clone(),
            prompt: vec![ContentBlock::Text(TextContent::new("hi"))],
            params: vec![],
            mode_id: None,
        }))
        .unwrap_err();
    match &err {
        HubError::ReadOnlyConversation {
            origin,
            interaction,
            message,
            ..
        } => {
            assert_eq!(origin, "bound");
            assert_eq!(interaction, "read_only");
            assert!(
                message.contains("IDE") || message.contains("read-only"),
                "{message}"
            );
            assert_eq!(err.phase1_code(), Some("read_only_conversation"));
            assert!(
                err.phase1_cli_line()
                    .starts_with("error: read_only_conversation:")
            );
        }
        other => panic!("send_prompt must hit write gate, got {other:?}"),
    }

    // set_param / set_mode share the same gate
    let err_param = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(hub.set_param(&bound.id, "temperature", "1"))
        .unwrap_err();
    assert!(matches!(err_param, HubError::ReadOnlyConversation { .. }));
    let err_mode = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(hub.set_mode(&bound.id, "plan"))
        .unwrap_err();
    assert!(matches!(err_mode, HubError::ReadOnlyConversation { .. }));
}

#[test]
fn create_run_busy_and_cli_codes() {
    use acp_hub::HubError;
    let store = store();
    hub_create(&store, "conv-busy2", "sid-busy2");
    store.create_run("run-a", "conv-busy2").unwrap();
    let err = store.create_run("run-b", "conv-busy2").unwrap_err();
    match err {
        HubError::ConversationBusy {
            ref conv_id,
            ref busy,
        } => {
            assert_eq!(conv_id, "conv-busy2");
            assert_eq!(busy, "running");
            assert_eq!(err.phase1_code(), Some("conversation_busy"));
            assert!(
                err.phase1_cli_line()
                    .starts_with("error: conversation_busy:")
            );
        }
        other => panic!("expected ConversationBusy, got {other:?}"),
    }
}

#[test]
fn phase1_cli_line_closed_and_not_busy() {
    use acp_hub::HubError;
    let closed = HubError::ConversationClosed {
        conv_id: "c".into(),
    };
    assert_eq!(closed.phase1_code(), Some("conversation_closed"));
    assert!(
        closed
            .phase1_cli_line()
            .starts_with("error: conversation_closed:")
    );
    let not_busy = HubError::not_busy("c");
    assert_eq!(not_busy.phase1_code(), Some("not_busy"));
    assert!(not_busy.phase1_cli_line().starts_with("error: not_busy:"));
}
