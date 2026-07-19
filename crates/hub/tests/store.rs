//! Store validation: two-layer replay, lifecycle integrity, and unified search.

use std::{path::Path, thread, time::Duration};

use acp_hub::error::HubError;
use acp_hub::store::{
    MessagePageQuery, MessageSource, NewConversation, NewMessage, ReplayedMessage, RunStatus, Store,
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

#[path = "store/basic.rs"]
mod basic;
#[path = "store/paging.rs"]
mod paging;
#[path = "store/replay_recovery.rs"]
mod replay_recovery;
