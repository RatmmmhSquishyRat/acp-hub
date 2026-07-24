use super::*;

#[test]
fn search_finds_appended_message() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "c1", "agent-a");
    msg(
        &store,
        &c,
        "assistant",
        "the hub-search-token lives here",
        1,
    );

    let page = store.search("hub-search-token", None, None, 10, 0).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].conv_id, "c1");
    assert_eq!(page.items[0].agent_id, "agent-a");
    assert!(
        page.items[0].snippet.contains("hub-search-token"),
        "FTS result should include a useful snippet"
    );
}

#[test]
fn session_metadata_patch_initializes_a_null_metadata_column() {
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&NewConversation {
            id: "conv-meta".into(),
            agent_id: "test".into(),
            agent_session_id: "session-meta".into(),
            cwd: None,
            additional_directories: vec![],
            title: None,
        })
        .unwrap();

    let patch = serde_json::json!({"currentMode": {"currentModeId": "ask"}});
    store
        .apply_session_info("conv-meta", None, None, patch.as_object())
        .unwrap();

    assert_eq!(
        store
            .conversation("conv-meta")
            .unwrap()
            .unwrap()
            .session_meta,
        Some(patch)
    );
}

#[test]
fn load_replay_replaces_only_layer_one_and_keeps_layer_two_current() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "c2", "agent-b");
    msg(&store, &c, "user", "original-captured-message", 1);
    msg(&store, &c, "assistant", "original-reply", 2);

    store
        .stage_load_replay(&c, "load-1", &[replay("c2-l1", "old-replay")])
        .unwrap();
    store
        .stage_load_replay(&c, "load-2", &[replay("c2-l2", "new-replay")])
        .unwrap();

    let cur = store.messages(&c, false).unwrap();
    assert_eq!(cur.len(), 3, "both captured rows plus latest replay");
    assert_eq!(
        cur.iter()
            .filter(|row| row.source == MessageSource::LocalTurn)
            .count(),
        2
    );
    assert_eq!(
        cur.iter()
            .filter(|row| row.source == MessageSource::LoadReplay)
            .count(),
        1
    );
    assert!(cur.iter().any(|row| row.body_text == "new-replay"));

    let page = store.search("old-replay", None, None, 10, 0).unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].source.as_deref().unwrap().ends_with(":audit"));
}

#[test]
fn failed_streamed_replay_restores_previous_layer_one() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "c3", "agent-c");
    msg(&store, &c, "user", "captured-layer-two", 1);
    store
        .stage_load_replay(&c, "initial", &[replay("c3-l1", "stable-layer-one")])
        .unwrap();

    let refresh = store.begin_load_replay(&c, "refresh-failed").unwrap();
    store
        .append_message(&NewMessage {
            id: "c3-partial".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({ "text": "partial-replay" }),
            body_text: "partial-replay".into(),
        })
        .unwrap();
    assert!(
        store
            .messages(&c, false)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "stable-layer-one"),
        "the last complete replay must remain current until commit"
    );
    store.rollback_load_replay(refresh).unwrap();

    let current = store.messages(&c, false).unwrap();
    assert_eq!(current.len(), 2);
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "stable-layer-one")
    );
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "captured-layer-two")
    );
    assert!(
        !store
            .messages(&c, true)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "partial-replay")
    );
    assert!(
        store
            .search("partial-replay", None, None, 10, 0)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn successful_streamed_replay_supersedes_old_layer_only_at_commit() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "c4", "agent-d");
    store
        .stage_load_replay(&c, "initial", &[replay("c4-l1", "old-layer-one")])
        .unwrap();

    let refresh = store.begin_load_replay(&c, "refresh-ok").unwrap();
    store
        .append_message(&NewMessage {
            id: "c4-new".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({ "text": "new-layer-one" }),
            body_text: "new-layer-one".into(),
        })
        .unwrap();
    assert_eq!(store.messages(&c, false).unwrap().len(), 2);

    store.commit_load_replay(refresh).unwrap();
    let current = store.messages(&c, false).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].body_text, "new-layer-one");
    let audit = store.messages(&c, true).unwrap();
    assert_eq!(audit.len(), 2);
    assert!(
        audit
            .iter()
            .any(|row| row.body_text == "old-layer-one" && !row.current_projection)
    );
}

#[test]
fn stale_replay_rollback_after_commit_is_rejected_before_mutation() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "stale-after-commit", "agent-stale-after-commit");
    store
        .stage_load_replay(
            &c,
            "stale-old",
            &[replay("stale-old-message", "stale-old-layer-one")],
        )
        .unwrap();
    let refresh = store.begin_load_replay(&c, "stale-commit-refresh").unwrap();
    store
        .append_message(&NewMessage {
            id: "stale-commit-new-message".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "stale-commit-new-layer-one"}),
            body_text: "stale-commit-new-layer-one".into(),
        })
        .unwrap();
    store.commit_load_replay(&refresh).unwrap();

    let error = store.rollback_load_replay(refresh).unwrap_err();
    assert!(matches!(&error, HubError::Conflict(id) if id == &c));
    let current = store.messages(&c, false).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].body_text, "stale-commit-new-layer-one");
}

#[test]
fn stale_replay_commit_after_rollback_is_rejected_before_mutation() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "stale-after-rollback", "agent-stale-after-rollback");
    store
        .stage_load_replay(
            &c,
            "rollback-old",
            &[replay("rollback-old-message", "rollback-old-layer-one")],
        )
        .unwrap();
    let refresh = store
        .begin_load_replay(&c, "stale-rollback-refresh")
        .unwrap();
    store
        .append_message(&NewMessage {
            id: "stale-rollback-partial-message".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "stale-rollback-partial-layer-one"}),
            body_text: "stale-rollback-partial-layer-one".into(),
        })
        .unwrap();
    store.rollback_load_replay(&refresh).unwrap();

    let error = store.commit_load_replay(refresh).unwrap_err();
    assert!(matches!(&error, HubError::Conflict(id) if id == &c));
    let current = store.messages(&c, false).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].body_text, "rollback-old-layer-one");
    assert!(
        !store
            .messages(&c, true)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "stale-rollback-partial-layer-one")
    );
}

#[test]
fn stale_replay_token_cannot_finalize_reused_id_after_empty_commit() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let c = conv(&store, "reuse-after-commit", "agent-reuse-after-commit");
    let first = store.begin_load_replay(&c, "reused-load-id").unwrap();
    store.commit_load_replay(&first).unwrap();
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));
    // Test needs id reuse; operator delete is soft — hard-delete for fixture reset.
    store.hard_delete_conversation(&c).unwrap();
    let recreated = conv(
        &store,
        "reuse-after-commit",
        "agent-reuse-after-commit-recreated",
    );
    assert_eq!(recreated, c);

    let second = store.begin_load_replay(&c, "reused-load-id").unwrap();
    assert_eq!(replay_metadata_counts(temp.path()), (1, 1));
    let error = store.rollback_load_replay(first).unwrap_err();
    assert!(matches!(&error, HubError::Conflict(id) if id == &c));
    assert_eq!(
        replay_metadata_counts(temp.path()),
        (1, 1),
        "the stale token must not delete the second refresh"
    );
    store
        .append_message(&NewMessage {
            id: "reused-commit-message".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "reused-commit-layer-one"}),
            body_text: "reused-commit-layer-one".into(),
        })
        .unwrap();
    store.commit_load_replay(second).unwrap();
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));
    let current = store.messages(&c, false).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].body_text, "reused-commit-layer-one");
}

#[test]
fn stale_replay_token_cannot_finalize_reused_id_after_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let c = conv(&store, "reuse-after-rollback", "agent-reuse-after-rollback");
    store
        .stage_load_replay(
            &c,
            "initial-load",
            &[replay("reuse-stable-message", "reuse-stable-layer-one")],
        )
        .unwrap();
    let first = store.begin_load_replay(&c, "reused-rollback-id").unwrap();
    store
        .append_message(&NewMessage {
            id: "reuse-partial-message".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "reuse-partial-layer-one"}),
            body_text: "reuse-partial-layer-one".into(),
        })
        .unwrap();
    store.rollback_load_replay(&first).unwrap();
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));

    let second = store.begin_load_replay(&c, "reused-rollback-id").unwrap();
    assert_eq!(replay_metadata_counts(temp.path()), (1, 1));
    let error = store.commit_load_replay(first).unwrap_err();
    assert!(matches!(&error, HubError::Conflict(id) if id == &c));
    assert_eq!(
        replay_metadata_counts(temp.path()),
        (1, 1),
        "the stale token must not commit or delete the second refresh"
    );
    store
        .append_message(&NewMessage {
            id: "reused-rollback-message".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "reused-rollback-layer-one"}),
            body_text: "reused-rollback-layer-one".into(),
        })
        .unwrap();
    store.commit_load_replay(second).unwrap();
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));
    let current = store.messages(&c, false).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].body_text, "reused-rollback-layer-one");
}

#[test]
fn reused_replay_load_id_never_reactivates_historical_rows() {
    let temp = tempfile::tempdir().unwrap();
    {
        let store = Store::open(temp.path()).unwrap();
        let c = conv(&store, "reused-id-history", "agent-reused-id-history");
        store
            .stage_load_replay(
                &c,
                "initial-load",
                &[replay("reused-id-old", "reused-id-old-layer-one")],
            )
            .unwrap();

        let first = store.begin_load_replay(&c, "shared-load-id").unwrap();
        store
            .append_message(&NewMessage {
                id: "reused-id-committed".into(),
                conv_id: c.clone(),
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "reused-id-committed-layer-one"}),
                body_text: "reused-id-committed-layer-one".into(),
            })
            .unwrap();
        store.commit_load_replay(first).unwrap();

        let second = store.begin_load_replay(&c, "shared-load-id").unwrap();
        store
            .append_message(&NewMessage {
                id: "reused-id-rollback-partial".into(),
                conv_id: c.clone(),
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "reused-id-rollback-partial"}),
                body_text: "reused-id-rollback-partial".into(),
            })
            .unwrap();
        store.rollback_load_replay(second).unwrap();
        let audit = store.messages(&c, true).unwrap();
        assert!(
            audit
                .iter()
                .any(|row| row.body_text == "reused-id-old-layer-one" && !row.current_projection)
        );
        assert!(
            audit
                .iter()
                .any(|row| row.body_text == "reused-id-committed-layer-one"
                    && row.current_projection)
        );
        assert!(
            !audit
                .iter()
                .any(|row| row.body_text == "reused-id-rollback-partial")
        );
        assert_eq!(replay_metadata_counts(temp.path()), (0, 0));

        let _interrupted = store.begin_load_replay(&c, "shared-load-id").unwrap();
        store
            .append_message(&NewMessage {
                id: "reused-id-recovery-partial".into(),
                conv_id: c,
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "reused-id-recovery-partial"}),
                body_text: "reused-id-recovery-partial".into(),
            })
            .unwrap();
        assert_eq!(replay_metadata_counts(temp.path()), (1, 1));
    }

    let reopened = Store::open(temp.path()).unwrap();
    reopened.recover_interrupted_load_replays().unwrap();
    let audit = reopened.messages("reused-id-history", true).unwrap();
    assert!(
        audit
            .iter()
            .any(|row| row.body_text == "reused-id-old-layer-one" && !row.current_projection)
    );
    assert!(
        audit
            .iter()
            .any(|row| row.body_text == "reused-id-committed-layer-one" && row.current_projection)
    );
    assert!(
        !audit
            .iter()
            .any(|row| row.body_text == "reused-id-recovery-partial")
    );
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));
}
