use super::*;

#[test]
fn reopening_store_removes_partial_streamed_replay_after_crash() {
    let temp = tempfile::tempdir().unwrap();
    {
        let store = Store::open(temp.path()).unwrap();
        let c = conv(&store, "c5", "agent-e");
        store
            .stage_load_replay(&c, "initial", &[replay("c5-l1", "stable-before-crash")])
            .unwrap();
        delete_conversation_fts(temp.path(), &c);
        assert_eq!(projection_state(temp.path(), &c).fts_title, None);
        let _refresh = store.begin_load_replay(&c, "crashed-refresh").unwrap();
        store
            .append_message(&NewMessage {
                id: "c5-partial".into(),
                conv_id: c,
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({ "text": "partial-before-crash" }),
                body_text: "partial-before-crash".into(),
            })
            .unwrap();
    }

    let reopened = Store::open(temp.path()).unwrap();
    reopened.recover_interrupted_load_replays().unwrap();
    assert_eq!(projection_state(temp.path(), "c5").fts_title, None);
    let current = reopened.messages("c5", false).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].body_text, "stable-before-crash");
    assert!(
        !reopened
            .messages("c5", true)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "partial-before-crash")
    );
}

#[test]
fn failed_replay_restores_every_projection_before_image_and_layer_two() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let c = conv(&store, "projection-rollback", "agent-projection-rollback");
    msg(&store, &c, "user", "durable-layer-two", 1);
    store
        .stage_load_replay(
            &c,
            "old-load",
            &[replay("old-load-message", "old-layer-one")],
        )
        .unwrap();
    set_projection(&store, &c, "old", 11);
    let old = projection_state(temp.path(), &c);

    thread::sleep(Duration::from_millis(20));
    let refresh = store
        .begin_load_replay(&c, "failed-projection-refresh")
        .unwrap();
    set_projection(&store, &c, "new", 22);
    store
        .append_message(&NewMessage {
            id: "partial-projection-message".into(),
            conv_id: c.clone(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: json!({"text": "partial-new-layer-one"}),
            body_text: "partial-new-layer-one".into(),
        })
        .unwrap();
    let mutated = projection_state(temp.path(), &c);
    assert_ne!(
        mutated, old,
        "the replay must mutate every seeded projection"
    );
    assert_ne!(mutated.conversation.1, old.conversation.1);
    assert_ne!(
        mutated.config.as_ref().unwrap().2,
        old.config.as_ref().unwrap().2
    );
    assert_ne!(
        mutated.plan.as_ref().unwrap().1,
        old.plan.as_ref().unwrap().1
    );
    assert_ne!(
        mutated.commands.as_ref().unwrap().1,
        old.commands.as_ref().unwrap().1
    );
    assert_ne!(
        mutated.usage.as_ref().unwrap().3,
        old.usage.as_ref().unwrap().3
    );

    store.rollback_load_replay(refresh).unwrap();

    assert_eq!(projection_state(temp.path(), &c), old);
    let current = store.messages(&c, false).unwrap();
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "durable-layer-two")
    );
    assert!(current.iter().any(|row| row.body_text == "old-layer-one"));
    assert!(
        !store
            .messages(&c, true)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "partial-new-layer-one")
    );
}

#[test]
fn failed_replay_removes_projection_rows_absent_before_begin() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let c = conv(&store, "projection-absent", "agent-projection-absent");
    delete_conversation_fts(temp.path(), &c);
    let old = projection_state(temp.path(), &c);
    assert_eq!(old.conversation.0, None);
    assert_eq!(old.conversation.2, None);
    assert_eq!(old.fts_title, None);
    assert_eq!(old.config, None);
    assert_eq!(old.plan, None);
    assert_eq!(old.commands, None);
    assert_eq!(old.usage, None);

    let refresh = store
        .begin_load_replay(&c, "failed-absent-refresh")
        .unwrap();
    set_projection(&store, &c, "new-from-absent", 33);
    assert_ne!(projection_state(temp.path(), &c), old);
    store.rollback_load_replay(refresh).unwrap();

    assert_eq!(projection_state(temp.path(), &c), old);
}

#[test]
fn reopening_store_restores_every_projection_before_image_after_crash() {
    let temp = tempfile::tempdir().unwrap();
    let old;
    {
        let store = Store::open(temp.path()).unwrap();
        let c = conv(&store, "projection-crash", "agent-projection-crash");
        msg(&store, &c, "user", "crash-durable-layer-two", 1);
        store
            .stage_load_replay(
                &c,
                "crash-old-load",
                &[replay("crash-old-message", "crash-old-layer-one")],
            )
            .unwrap();
        set_projection(&store, &c, "crash-old", 44);
        old = projection_state(temp.path(), &c);

        let _refresh = store
            .begin_load_replay(&c, "crashed-projection-refresh")
            .unwrap();
        set_projection(&store, &c, "crash-new", 55);
        store
            .append_message(&NewMessage {
                id: "crash-partial-message".into(),
                conv_id: c,
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "crash-partial-layer-one"}),
                body_text: "crash-partial-layer-one".into(),
            })
            .unwrap();
        assert_ne!(projection_state(temp.path(), "projection-crash"), old);
    }

    let reopened = Store::open(temp.path()).unwrap();
    reopened.recover_interrupted_load_replays().unwrap();
    assert_eq!(projection_state(temp.path(), "projection-crash"), old);
    let current = reopened.messages("projection-crash", false).unwrap();
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "crash-durable-layer-two")
    );
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "crash-old-layer-one")
    );
    assert!(
        !reopened
            .messages("projection-crash", true)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "crash-partial-layer-one")
    );
}

#[test]
fn committed_replay_retains_every_new_projection_and_layer_two() {
    let temp = tempfile::tempdir().unwrap();
    let new;
    {
        let store = Store::open(temp.path()).unwrap();
        let c = conv(&store, "projection-commit", "agent-projection-commit");
        msg(&store, &c, "user", "commit-durable-layer-two", 1);
        store
            .stage_load_replay(
                &c,
                "commit-old-load",
                &[replay("commit-old-message", "commit-old-layer-one")],
            )
            .unwrap();
        set_projection(&store, &c, "commit-old", 66);

        let refresh = store
            .begin_load_replay(&c, "committed-projection-refresh")
            .unwrap();
        set_projection(&store, &c, "commit-new", 77);
        new = projection_state(temp.path(), &c);
        store
            .append_message(&NewMessage {
                id: "commit-new-message".into(),
                conv_id: c,
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "commit-new-layer-one"}),
                body_text: "commit-new-layer-one".into(),
            })
            .unwrap();
        store.commit_load_replay(refresh).unwrap();
    }

    let reopened = Store::open(temp.path()).unwrap();
    assert_eq!(projection_state(temp.path(), "projection-commit"), new);
    let current = reopened.messages("projection-commit", false).unwrap();
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "commit-durable-layer-two")
    );
    assert!(
        current
            .iter()
            .any(|row| row.body_text == "commit-new-layer-one")
    );
    assert!(
        !current
            .iter()
            .any(|row| row.body_text == "commit-old-layer-one")
    );
}

#[test]
fn replay_nonce_migration_recovers_when_column_precedes_version_marker() {
    let temp = tempfile::tempdir().unwrap();
    {
        let store = Store::open(temp.path()).unwrap();
        let c = conv(&store, "nonce-migration", "agent-nonce-migration");
        let _interrupted = store
            .begin_load_replay(&c, "nonce-migration-refresh")
            .unwrap();
        assert_eq!(replay_metadata_counts(temp.path()), (1, 1));
    }
    Connection::open(temp.path().join("hub.db"))
        .unwrap()
        .execute(
            "DELETE FROM schema_migrations WHERE version IN (4, 5, 6, 7)",
            [],
        )
        .unwrap();

    let reopened = Store::open(temp.path()).unwrap();
    reopened.recover_interrupted_load_replays().unwrap();
    let schema_version: i64 = Connection::open(temp.path().join("hub.db"))
        .unwrap()
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(schema_version, 7);
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));
}

#[test]
fn schema_v2_replay_recovery_migrates_and_v3_finalizers_clean_metadata() {
    let temp = tempfile::tempdir().unwrap();
    seed_schema_v2_store(temp.path());
    let legacy_projection = projection_state(temp.path(), "v2-conv");
    let committed_projection;
    {
        let store = Store::open(temp.path()).unwrap();
        store.recover_interrupted_load_replays().unwrap();
        let schema_version: i64 = Connection::open(temp.path().join("hub.db"))
            .unwrap()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(schema_version, 7);
        assert_eq!(
            projection_state(temp.path(), "v2-conv"),
            legacy_projection,
            "v2 has no projection before-image to reconstruct"
        );
        let recovered = store.messages("v2-conv", false).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].body_text, "v2-stable-layer-one");
        assert!(recovered[0].current_projection);
        assert!(
            store
                .search("v2-partial-layer-one", None, None, 10, 0)
                .unwrap()
                .items
                .is_empty(),
            "legacy recovery must delete the partial message FTS row"
        );
        assert_eq!(replay_metadata_counts(temp.path()), (0, 0));

        let rollback = store.begin_load_replay("v2-conv", "v3-rollback").unwrap();
        set_projection(&store, "v2-conv", "v3-rollback-new", 81);
        store.rollback_load_replay(rollback).unwrap();
        assert_eq!(projection_state(temp.path(), "v2-conv"), legacy_projection);
        assert_eq!(replay_metadata_counts(temp.path()), (0, 0));

        let commit = store.begin_load_replay("v2-conv", "v3-commit").unwrap();
        set_projection(&store, "v2-conv", "v3-committed", 82);
        store
            .append_message(&NewMessage {
                id: "v3-committed-message".into(),
                conv_id: "v2-conv".into(),
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "v3-committed-layer-one"}),
                body_text: "v3-committed-layer-one".into(),
            })
            .unwrap();
        store.commit_load_replay(commit).unwrap();
        committed_projection = projection_state(temp.path(), "v2-conv");
        assert_eq!(replay_metadata_counts(temp.path()), (0, 0));

        let _interrupted = store
            .begin_load_replay("v2-conv", "v3-interrupted")
            .unwrap();
        set_projection(&store, "v2-conv", "v3-interrupted-new", 83);
        store
            .append_message(&NewMessage {
                id: "v3-interrupted-message".into(),
                conv_id: "v2-conv".into(),
                run_id: None,
                source: MessageSource::LoadReplay,
                role: "assistant".into(),
                kind: None,
                content_json: json!({"text": "v3-interrupted-layer-one"}),
                body_text: "v3-interrupted-layer-one".into(),
            })
            .unwrap();
        assert_eq!(replay_metadata_counts(temp.path()), (1, 1));
    }

    let reopened = Store::open(temp.path()).unwrap();
    reopened.recover_interrupted_load_replays().unwrap();
    assert_eq!(
        projection_state(temp.path(), "v2-conv"),
        committed_projection
    );
    assert_eq!(replay_metadata_counts(temp.path()), (0, 0));
    assert!(
        !reopened
            .messages("v2-conv", true)
            .unwrap()
            .iter()
            .any(|row| row.body_text == "v3-interrupted-layer-one")
    );
}
