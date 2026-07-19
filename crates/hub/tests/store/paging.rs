use super::*;

#[test]
fn run_finalization_checks_conversation_ownership_and_delete_conflicts() {
    let store = Store::open_memory().unwrap();
    conv(&store, "run-c1", "agent-run-1");
    conv(&store, "run-c2", "agent-run-2");
    store.create_run("run-1", "run-c1").unwrap();

    let err = store
        .finalize_run_cas("run-1", "run-c2", RunStatus::Completed, None)
        .unwrap_err();
    assert!(err.to_string().contains("belongs to conversation run-c1"));
    assert_eq!(store.run_status("run-1").unwrap(), Some(RunStatus::Running));
    assert!(store.delete_conversation("run-c1").is_err());

    assert!(
        store
            .finalize_run_cas("run-1", "run-c1", RunStatus::Completed, None)
            .unwrap()
    );
    store.delete_conversation("run-c1").unwrap();
}

#[test]
fn crash_recovery_terminalizes_orphaned_runs() {
    let store = Store::open_memory().unwrap();
    conv(&store, "recovery-c1", "agent-recovery");
    store.create_run("recovery-run", "recovery-c1").unwrap();

    assert_eq!(store.recover_interrupted_runs().unwrap(), 1);
    assert_eq!(
        store.run_status("recovery-run").unwrap(),
        Some(RunStatus::Failed)
    );
    assert_eq!(
        store.conversation("recovery-c1").unwrap().unwrap().status,
        acp_hub::store::ConvStatus::Failed
    );
}

#[test]
fn search_uses_one_limit_and_offset_across_titles_and_messages() {
    let store = Store::open_memory().unwrap();
    for index in 0..3 {
        let id = format!("search-c{index}");
        store
            .create_conversation(&NewConversation {
                id: id.clone(),
                agent_id: "agent-search".into(),
                agent_session_id: format!("session-{index}"),
                cwd: Some("/tmp".into()),
                additional_directories: vec![],
                title: Some(format!("unified token title {index}")),
            })
            .unwrap();
        msg(
            &store,
            &id,
            "assistant",
            &format!("unified token message {index}"),
            index,
        );
    }

    let first = store.search("unified token", None, None, 2, 0).unwrap();
    let second = store
        .search("unified token", None, None, 2, first.next_offset.unwrap())
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(second.items.len(), 2);
    let first_keys: Vec<_> = first
        .items
        .iter()
        .map(|hit| (hit.kind.clone(), hit.conv_id.clone()))
        .collect();
    assert!(
        second
            .items
            .iter()
            .all(|hit| !first_keys.contains(&(hit.kind.clone(), hit.conv_id.clone())))
    );
    assert!(first.items.iter().all(|hit| !hit.snippet.is_empty()));
    assert!(second.items.iter().all(|hit| !hit.snippet.is_empty()));

    let beyond_sqlite_offset = store
        .search("unified token", None, None, 2, usize::MAX)
        .unwrap();
    assert!(beyond_sqlite_offset.items.is_empty());
    assert_eq!(beyond_sqlite_offset.next_offset, None);
}

#[test]
fn message_pages_are_bounded_and_support_opaque_continuations() {
    let store = Store::open_memory().unwrap();
    conv(&store, "page-c1", "agent-page");
    for index in 0..5 {
        msg(
            &store,
            "page-c1",
            "assistant",
            &format!("message-{index}"),
            index,
        );
    }

    let first = store
        .messages_page("page-c1", false, None, None, 2, 0)
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_offset, Some(2));
    assert!(first.next_cursor.is_some());
    assert_eq!(first.total, 5);

    let after = store
        .messages_page_query(MessagePageQuery {
            conv_id: "page-c1",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: first.next_cursor.as_deref(),
            limit: 2,
            offset: 0,
        })
        .unwrap();
    assert_eq!(after.items.len(), 2);
    assert!(after.items.iter().all(|item| item.seq > first.items[1].seq));
    assert_eq!(after.next_offset, None);
    assert!(after.next_cursor.is_some());

    let overflow = store
        .messages_page("page-c1", false, None, None, 2, usize::MAX)
        .unwrap();
    assert!(overflow.items.is_empty());
    assert_eq!(overflow.next_offset, None);
}

#[test]
fn message_cursor_rejects_tampering_query_changes_and_stale_projections() {
    let store = Store::open_memory().unwrap();
    conv(&store, "cursor-c1", "agent-cursor");
    conv(&store, "cursor-c2", "agent-cursor-2");
    for index in 0..3 {
        msg(
            &store,
            "cursor-c1",
            "assistant",
            &format!("cursor-message-{index}"),
            index,
        );
    }
    let first = store
        .messages_page_query(MessagePageQuery {
            conv_id: "cursor-c1",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: None,
            limit: 1,
            offset: 0,
        })
        .unwrap();
    let cursor = first.next_cursor.unwrap();

    let mut tampered = cursor.clone().into_bytes();
    let last = tampered.last_mut().unwrap();
    *last = if *last == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: "cursor-c1",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: Some(&tampered),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::InvalidCursor { .. })
    ));
    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: "cursor-c2",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::InvalidCursor { .. })
    ));
    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: "cursor-c1",
            include_audit: true,
            run_id: None,
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::InvalidCursor { .. })
    ));
    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: "cursor-c1",
            include_audit: false,
            run_id: Some("another-run"),
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::InvalidCursor { .. })
    ));
    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: "cursor-c1",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 1,
        }),
        Err(HubError::InvalidCursor { .. })
    ));

    store
        .stage_load_replay(
            "cursor-c1",
            "cursor-load",
            &[replay("cursor-replay", "refreshed")],
        )
        .unwrap();
    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: "cursor-c1",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::StaleCursor {
            conv_id,
            expected_generation: 0,
            current_generation: 1,
        }) if conv_id == "cursor-c1"
    ));
}

#[test]
fn message_pages_apply_a_total_byte_budget() {
    let store = Store::open_memory().unwrap();
    conv(&store, "page-bytes", "agent-page");
    let body = "x".repeat(600 * 1024);
    for index in 0..20 {
        msg(&store, "page-bytes", "assistant", &body, index);
    }

    let first = store
        .messages_page("page-bytes", false, None, None, 500, 0)
        .unwrap();
    assert!(!first.items.is_empty());
    assert!(first.items.len() < 20);
    assert_eq!(first.next_offset, Some(first.items.len()));
    let serialized = serde_json::to_vec(&first).unwrap();
    assert!(
        serialized.len() < 10 * 1024 * 1024,
        "serialized page escaped the intended response budget"
    );
}

#[test]
fn rollback_invalidates_cursor_before_deleted_sequence_is_reused() {
    let store = Store::open_memory().unwrap();
    let conv_id = conv(&store, "cursor-rollback", "agent-cursor-rollback");
    store
        .stage_load_replay(
            &conv_id,
            "cursor-rollback-stable",
            &[replay("cursor-rollback-stable-message", "stable")],
        )
        .unwrap();
    let refresh = store
        .begin_load_replay(&conv_id, "cursor-rollback-partial")
        .unwrap();
    store
        .append_message(&NewMessage {
            id: "cursor-rollback-partial-message".into(),
            conv_id: conv_id.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "partial"}),
            body_text: "partial".into(),
        })
        .unwrap();
    let cursor = store
        .messages_page(&conv_id, false, None, None, 1, 0)
        .unwrap()
        .next_cursor
        .expect("partial projection must have a continuation cursor");

    store.rollback_load_replay(refresh).unwrap();
    msg(&store, &conv_id, "assistant", "replacement", 99);

    assert!(matches!(
        store.messages_page_query(MessagePageQuery {
            conv_id: &conv_id,
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::StaleCursor {
            conv_id: stale_conv_id,
            expected_generation: 1,
            current_generation: 2,
        }) if stale_conv_id == conv_id
    ));
}

#[test]
fn crash_recovery_invalidates_cursor_before_deleted_sequence_is_reused() {
    let temp = tempfile::tempdir().unwrap();
    let cursor = {
        let store = Store::open(temp.path()).unwrap();
        let conv_id = conv(&store, "cursor-recovery", "agent-cursor-recovery");
        store
            .stage_load_replay(
                &conv_id,
                "cursor-recovery-stable",
                &[replay("cursor-recovery-stable-message", "stable")],
            )
            .unwrap();
        let _refresh = store
            .begin_load_replay(&conv_id, "cursor-recovery-partial")
            .unwrap();
        store
            .append_message(&NewMessage {
                id: "cursor-recovery-partial-message".into(),
                conv_id: conv_id.clone(),
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "partial"}),
                body_text: "partial".into(),
            })
            .unwrap();
        store
            .messages_page(&conv_id, false, None, None, 1, 0)
            .unwrap()
            .next_cursor
            .expect("partial projection must have a continuation cursor")
    };

    let reopened = Store::open(temp.path()).unwrap();
    assert_eq!(reopened.recover_interrupted_load_replays().unwrap(), 1);
    msg(&reopened, "cursor-recovery", "assistant", "replacement", 99);

    assert!(matches!(
        reopened.messages_page_query(MessagePageQuery {
            conv_id: "cursor-recovery",
            include_audit: false,
            run_id: None,
            after_seq: None,
            cursor: Some(&cursor),
            limit: 1,
            offset: 0,
        }),
        Err(HubError::StaleCursor {
            conv_id,
            expected_generation: 1,
            current_generation: 2,
        }) if conv_id == "cursor-recovery"
    ));
}

#[test]
fn message_pages_filter_run_before_pagination_and_byte_budgeting() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "page-runs", "agent-page-runs");
    store.create_run("page-run-1", &c).unwrap();
    store
        .finalize_run_cas("page-run-1", &c, RunStatus::Completed, None)
        .unwrap();
    store.create_run("page-run-2", &c).unwrap();
    let oversized_excluded_body = "x".repeat(9 * 1024 * 1024);
    for (index, (run_id, body)) in [
        ("page-run-2", oversized_excluded_body.as_str()),
        ("page-run-1", "run-one-first"),
        ("page-run-2", "run-two-interleaved"),
        ("page-run-1", "run-one-appended"),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .append_message(&NewMessage {
                id: format!("page-run-message-{index}"),
                conv_id: c.clone(),
                run_id: Some(run_id.into()),
                source: MessageSource::LocalTurn,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": body}),
                body_text: body.into(),
            })
            .unwrap();
    }

    let first = store
        .messages_page(&c, false, Some("page-run-1"), None, 1, 0)
        .unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.next_offset, Some(1));
    assert_eq!(first.items[0].run_id.as_deref(), Some("page-run-1"));
    assert_eq!(first.items[0].body_text, "run-one-first");

    let second = store
        .messages_page(&c, false, Some("page-run-1"), None, 1, 1)
        .unwrap();
    assert_eq!(second.total, 2);
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.next_offset, None);
    assert_eq!(second.items[0].run_id.as_deref(), Some("page-run-1"));
    assert_eq!(second.items[0].body_text, "run-one-appended");
}
