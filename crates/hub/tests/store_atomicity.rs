use std::path::Path;

use acp_hub::store::{AgentSessionImport, MessageSource, NewConversation, NewMessage, Store};
use rusqlite::Connection;
use serde_json::json;

fn create_conversation(store: &Store, id: &str, session_id: &str) {
    store
        .create_conversation(&NewConversation {
            id: id.to_string(),
            agent_id: "agent".to_string(),
            agent_session_id: session_id.to_string(),
            cwd: Some("/old/cwd".to_string()),
            additional_directories: vec!["/old/root".to_string()],
            title: Some("old title".to_string()),
        })
        .unwrap();
}

fn database(home: &Path) -> Connection {
    Connection::open(home.join("hub.db")).unwrap()
}

#[test]
fn partial_initial_schema_without_marker_recovers_idempotently() {
    let home = tempfile::tempdir().unwrap();
    database(home.path())
        .execute_batch(
            "CREATE TABLE schema_migrations(
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             CREATE TABLE agent_cache(
                 id TEXT PRIMARY KEY,
                 agent_info_json TEXT,
                 capabilities_json TEXT,
                 inspected_at TEXT
             );",
        )
        .unwrap();

    let _store = Store::open(home.path()).unwrap();
    let conn = database(home.path());
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    let conversations: bool = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'conversations'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 7);
    assert!(conversations);
}

#[test]
fn agent_session_upsert_rolls_back_metadata_and_fts_together() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    create_conversation(&store, "conv-upsert", "session-upsert");
    database(home.path())
        .execute_batch("DROP TABLE conversations_fts;")
        .unwrap();

    assert!(
        store
            .upsert_agent_session(
                "agent",
                "session-upsert",
                Some("new title"),
                Some("/new/cwd"),
                &["/new/root".to_string()],
            )
            .is_err()
    );
    let row = store.conversation("conv-upsert").unwrap().unwrap();
    assert_eq!(row.title.as_deref(), Some("old title"));
    assert_eq!(row.cwd.as_deref(), Some("/old/cwd"));
    assert_eq!(row.additional_directories, vec!["/old/root"]);
}

#[test]
fn interrupted_discovery_import_restores_existing_and_removes_new_rows() {
    let home = tempfile::tempdir().unwrap();
    {
        let store = Store::open(home.path()).unwrap();
        create_conversation(&store, "conv-existing", "session-existing");
        store
            .set_config_snapshot(
                "conv-existing",
                &json!([{"id": "old-config"}]),
                Some(&json!({"currentModeId": "old-mode"})),
            )
            .unwrap();
        let _existing = store
            .begin_agent_session_import(AgentSessionImport {
                provisional_conv_id: "unused-provisional",
                agent_id: "agent",
                agent_session_id: "session-existing",
                title: Some("new title"),
                cwd: "/new/cwd",
                additional_directories: &["/new/root".to_string()],
                load_id: "load-existing",
            })
            .unwrap();
        let _new = store
            .begin_agent_session_import(AgentSessionImport {
                provisional_conv_id: "conv-provisional",
                agent_id: "agent",
                agent_session_id: "session-new",
                title: Some("new session"),
                cwd: "/new/cwd",
                additional_directories: &[],
                load_id: "load-new",
            })
            .unwrap();
    }

    let reopened = Store::open(home.path()).unwrap();
    reopened.recover_interrupted_load_replays().unwrap();
    let existing = reopened.conversation("conv-existing").unwrap().unwrap();
    assert_eq!(existing.title.as_deref(), Some("old title"));
    assert_eq!(existing.cwd.as_deref(), Some("/old/cwd"));
    assert_eq!(existing.additional_directories, vec!["/old/root"]);
    assert_eq!(
        reopened.config_snapshot("conv-existing").unwrap(),
        Some(json!([{"id": "old-config"}]))
    );
    assert_eq!(
        reopened.modes_snapshot("conv-existing").unwrap(),
        Some(json!({"currentModeId": "old-mode"}))
    );
    assert!(reopened.conversation("conv-provisional").unwrap().is_none());
}

#[test]
fn modes_only_refresh_replaces_the_complete_static_snapshot_set() {
    let store = Store::open_memory().unwrap();
    create_conversation(&store, "conv-static", "session-static");
    store
        .set_config_snapshot(
            "conv-static",
            &json!([{"id": "old-config"}]),
            Some(&json!({"currentModeId": "old-mode"})),
        )
        .unwrap();
    store
        .set_plan_snapshot("conv-static", &json!({"entries": ["old-plan"]}))
        .unwrap();
    store
        .set_available_commands_snapshot("conv-static", &json!({"commands": ["old-command"]}))
        .unwrap();
    store
        .upsert_usage_snapshot("conv-static", 1, 10, Some(&json!({"cost": 1})))
        .unwrap();

    let refresh = store
        .begin_load_replay("conv-static", "load-static")
        .unwrap();
    store
        .commit_load_replay_with_static(refresh, None, Some(&json!({"currentModeId": "new-mode"})))
        .unwrap();

    assert_eq!(store.config_snapshot("conv-static").unwrap(), None);
    assert_eq!(
        store.modes_snapshot("conv-static").unwrap(),
        Some(json!({"currentModeId": "new-mode"}))
    );
    assert_eq!(store.plan_snapshot("conv-static").unwrap(), None);
    assert_eq!(store.commands_snapshot("conv-static").unwrap(), None);
    assert_eq!(store.usage_snapshot("conv-static").unwrap(), None);
}

#[test]
fn corrupt_persisted_json_and_enums_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    create_conversation(&store, "conv-corrupt", "session-corrupt");
    store
        .append_message(&NewMessage {
            id: "message-corrupt".to_string(),
            conv_id: "conv-corrupt".to_string(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "assistant".to_string(),
            kind: None,
            content_json: json!({"text": "valid"}),
            body_text: "valid".to_string(),
        })
        .unwrap();

    let conn = database(home.path());
    conn.execute(
        "UPDATE conversations
         SET additional_directories_json = 'not-json'
         WHERE id = 'conv-corrupt'",
        [],
    )
    .unwrap();
    assert!(store.conversation("conv-corrupt").is_err());
    conn.execute(
        "UPDATE conversations
         SET additional_directories_json = '[]'
         WHERE id = 'conv-corrupt'",
        [],
    )
    .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "UPDATE conversations SET status = 'unknown' WHERE id = 'conv-corrupt'",
        [],
    )
    .unwrap();
    assert!(store.conversation("conv-corrupt").is_err());
    conn.execute(
        "UPDATE conversations SET status = 'idle' WHERE id = 'conv-corrupt'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE messages SET source = 'unknown' WHERE id = 'message-corrupt'",
        [],
    )
    .unwrap();
    assert!(store.messages("conv-corrupt", true).is_err());
}
