//! Store validation: two-layer replay, lifecycle integrity, and unified search.

use std::{path::Path, thread, time::Duration};

use acp_hub::error::HubError;
use acp_hub::store::{
    MessageSource, NewConversation, NewMessage, ReplayedMessage, RunStatus, Store,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

fn conv(store: &Store, id: &str, agent: &str) -> String {
    store
        .create_conversation(&NewConversation {
            id: id.to_string(),
            agent_id: agent.to_string(),
            agent_session_id: format!("{agent}-session"),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    id.to_string()
}

fn msg(store: &Store, conv: &str, role: &str, body: &str, n: usize) {
    store
        .append_message(&NewMessage {
            id: format!("{conv}-m{n}"),
            conv_id: conv.to_string(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: role.into(),
            kind: None,
            content_json: json!({ "text": body }),
            body_text: body.into(),
        })
        .unwrap();
}

fn replay(id: &str, body: &str) -> ReplayedMessage {
    ReplayedMessage {
        id: id.into(),
        role: "assistant".into(),
        kind: None,
        content_json: json!({ "text": body }),
        body_text: body.into(),
        message_key: Some(format!("key-{id}")),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ProjectionState {
    conversation: (Option<String>, String, Option<String>),
    fts_title: Option<String>,
    config: Option<(String, Option<String>, String)>,
    plan: Option<(String, String)>,
    commands: Option<(String, String)>,
    usage: Option<(i64, i64, Option<String>, String)>,
}

fn projection_state(home: &Path, conv_id: &str) -> ProjectionState {
    let conn = Connection::open(home.join("hub.db")).unwrap();
    let conversation = conn
        .query_row(
            "SELECT title, updated_at, session_meta_json
             FROM conversations WHERE id = ?",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let fts_title = conn
        .query_row(
            "SELECT title FROM conversations_fts WHERE conv_id = ?",
            params![conv_id],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    let config = conn
        .query_row(
            "SELECT config_options_json, modes_json, updated_at
             FROM config_snapshots WHERE conv_id = ?",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .unwrap();
    let plan = conn
        .query_row(
            "SELECT entries_json, updated_at FROM plan_snapshots WHERE conv_id = ?",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap();
    let commands = conn
        .query_row(
            "SELECT commands_json, updated_at
             FROM available_command_snapshots WHERE conv_id = ?",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap();
    let usage = conn
        .query_row(
            "SELECT used, size, cost_json, updated_at
             FROM usage_snapshots WHERE conv_id = ?",
            params![conv_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .unwrap();
    ProjectionState {
        conversation,
        fts_title,
        config,
        plan,
        commands,
        usage,
    }
}

fn set_projection(store: &Store, conv_id: &str, generation: &str, used: i64) {
    let meta = json!({
        "currentMode": {"currentModeId": format!("{generation}-mode")},
        "marker": generation,
    });
    store
        .apply_session_info(
            conv_id,
            Some(&format!("{generation}-projection-title")),
            Some(&format!("{generation}-updated-at")),
            meta.as_object(),
        )
        .unwrap();
    store
        .set_plan_snapshot(
            conv_id,
            &json!({"entries": [{"content": format!("{generation}-plan")}]}),
        )
        .unwrap();
    store
        .set_available_commands_snapshot(
            conv_id,
            &json!({"availableCommands": [{"name": format!("{generation}-command")}]}),
        )
        .unwrap();
    store
        .set_config_snapshot(
            conv_id,
            &json!([{"id": format!("{generation}-config")}]),
            Some(&json!({
                "currentModeId": format!("{generation}-mode"),
                "availableModes": [{"id": format!("{generation}-mode")}],
            })),
        )
        .unwrap();
    store
        .upsert_usage_snapshot(
            conv_id,
            used,
            used + 100,
            Some(&json!({"amount": used, "currency": generation})),
        )
        .unwrap();
}

fn delete_conversation_fts(home: &Path, conv_id: &str) {
    Connection::open(home.join("hub.db"))
        .unwrap()
        .execute(
            "DELETE FROM conversations_fts WHERE conv_id = ?",
            params![conv_id],
        )
        .unwrap();
}

fn replay_metadata_counts(home: &Path) -> (i64, i64) {
    let conn = Connection::open(home.join("hub.db")).unwrap();
    (
        conn.query_row("SELECT COUNT(*) FROM load_replay_refreshes", [], |row| {
            row.get(0)
        })
        .unwrap(),
        conn.query_row(
            "SELECT COUNT(*) FROM load_replay_projection_before_images",
            [],
            |row| row.get(0),
        )
        .unwrap(),
    )
}

fn seed_schema_v2_store(home: &Path) {
    let conn = Connection::open(home.join("hub.db")).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE schema_migrations(
             version INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL
         );
         INSERT INTO schema_migrations(version, applied_at)
         VALUES (1, 'v1'), (2, 'v2');
         CREATE TABLE agent_cache(
             id TEXT PRIMARY KEY,
             agent_info_json TEXT,
             capabilities_json TEXT,
             inspected_at TEXT
         );
         CREATE TABLE conversations(
             id TEXT PRIMARY KEY,
             agent_id TEXT NOT NULL,
             agent_session_id TEXT NOT NULL,
             title TEXT,
             status TEXT NOT NULL CHECK(status IN (
                 'idle', 'running', 'cancelling', 'cancelled',
                 'failed', 'completed', 'deleted'
             )),
             cwd TEXT,
             additional_directories_json TEXT NOT NULL DEFAULT '[]',
             session_meta_json TEXT,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             UNIQUE(agent_id, agent_session_id)
         );
         CREATE TABLE runs(
             id TEXT PRIMARY KEY,
             conv_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
             status TEXT NOT NULL CHECK(status IN (
                 'running', 'cancelling', 'completed', 'cancelled', 'failed'
             )),
             stop_reason TEXT,
             started_at TEXT NOT NULL,
             ended_at TEXT
         );
         CREATE TABLE messages(
             id TEXT PRIMARY KEY,
             conv_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
             run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
             source TEXT NOT NULL CHECK(source IN (
                 'local_turn', 'load_replay', 'agent_list'
             )),
             current_projection INTEGER NOT NULL DEFAULT 1
                 CHECK(current_projection IN (0, 1)),
             message_key TEXT,
             superseded_by_load_id TEXT,
             role TEXT NOT NULL,
             kind TEXT,
             content_json TEXT NOT NULL,
             body_text TEXT NOT NULL DEFAULT '',
             seq INTEGER NOT NULL,
             created_at TEXT NOT NULL,
             UNIQUE(conv_id, seq)
         );
         CREATE TABLE config_snapshots(
             conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
             config_options_json TEXT NOT NULL,
             modes_json TEXT,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE plan_snapshots(
             conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
             entries_json TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE available_command_snapshots(
             conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
             commands_json TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE usage_snapshots(
             conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
             used INTEGER NOT NULL,
             size INTEGER NOT NULL,
             cost_json TEXT,
             updated_at TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE messages_fts
             USING fts5(message_id UNINDEXED, conv_id UNINDEXED, body);
         CREATE VIRTUAL TABLE conversations_fts
             USING fts5(conv_id UNINDEXED, title);
         CREATE INDEX idx_messages_proj
             ON messages(conv_id, current_projection, seq);
         CREATE INDEX idx_runs_conv ON runs(conv_id, started_at);
         CREATE TABLE load_replay_refreshes(
             conv_id TEXT PRIMARY KEY,
             load_id TEXT NOT NULL UNIQUE,
             starting_seq INTEGER NOT NULL,
             started_at TEXT NOT NULL,
             FOREIGN KEY(conv_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         INSERT INTO conversations(
             id, agent_id, agent_session_id, title, status, cwd,
             additional_directories_json, session_meta_json, created_at, updated_at
         ) VALUES (
             'v2-conv', 'v2-agent', 'v2-session', 'v2-projection-title', 'idle', '/v2',
             '[]', '{\"currentMode\":{\"currentModeId\":\"v2-mode\"}}',
             'v2-created', 'v2-updated'
         );
         INSERT INTO conversations_fts(conv_id, title)
         VALUES ('v2-conv', 'v2-projection-title');
         INSERT INTO messages(
             id, conv_id, run_id, source, current_projection, message_key,
             superseded_by_load_id, role, kind, content_json, body_text, seq, created_at
         ) VALUES
             (
                 'v2-stable', 'v2-conv', NULL, 'load_replay', 0, 'v2-stable-key',
                 'v2-interrupted', 'assistant', NULL, '{\"text\":\"v2-stable-layer-one\"}',
                 'v2-stable-layer-one', 1, 'v2-message-old'
             ),
             (
                 'v2-partial', 'v2-conv', NULL, 'load_replay', 1, 'v2-partial-key',
                 NULL, 'assistant', NULL, '{\"text\":\"v2-partial-layer-one\"}',
                 'v2-partial-layer-one', 2, 'v2-message-new'
             );
         INSERT INTO messages_fts(message_id, conv_id, body) VALUES
             ('v2-stable', 'v2-conv', 'v2-stable-layer-one'),
             ('v2-partial', 'v2-conv', 'v2-partial-layer-one');
         INSERT INTO config_snapshots(
             conv_id, config_options_json, modes_json, updated_at
         ) VALUES (
             'v2-conv', '[{\"id\":\"v2-config\"}]',
             '{\"currentModeId\":\"v2-mode\"}', 'v2-config-updated'
         );
         INSERT INTO plan_snapshots(conv_id, entries_json, updated_at)
         VALUES ('v2-conv', '{\"entries\":[{\"content\":\"v2-plan\"}]}', 'v2-plan-updated');
         INSERT INTO available_command_snapshots(conv_id, commands_json, updated_at)
         VALUES (
             'v2-conv', '{\"availableCommands\":[{\"name\":\"v2-command\"}]}',
             'v2-commands-updated'
         );
         INSERT INTO usage_snapshots(conv_id, used, size, cost_json, updated_at)
         VALUES (
             'v2-conv', 8, 108, '{\"amount\":8,\"currency\":\"v2\"}',
             'v2-usage-updated'
         );
         INSERT INTO load_replay_refreshes(conv_id, load_id, starting_seq, started_at)
         VALUES ('v2-conv', 'v2-interrupted', 1, 'v2-replay-started');",
    )
    .unwrap();
}

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
    store.delete_conversation(&c).unwrap();
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
        .execute("DELETE FROM schema_migrations WHERE version = 4", [])
        .unwrap();

    let _reopened = Store::open(temp.path()).unwrap();
    let schema_version: i64 = Connection::open(temp.path().join("hub.db"))
        .unwrap()
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(schema_version, 4);
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
        let schema_version: i64 = Connection::open(temp.path().join("hub.db"))
            .unwrap()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(schema_version, 4);
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
fn message_pages_are_bounded_on_the_server_and_support_sequence_cursors() {
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
    assert_eq!(first.total, 5);

    let after = store
        .messages_page("page-c1", false, None, Some(first.items[1].seq), 2, 0)
        .unwrap();
    assert_eq!(after.items.len(), 2);
    assert!(after.items.iter().all(|item| item.seq > first.items[1].seq));

    let overflow = store
        .messages_page("page-c1", false, None, None, 2, usize::MAX)
        .unwrap();
    assert!(overflow.items.is_empty());
    assert_eq!(overflow.next_offset, None);
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
