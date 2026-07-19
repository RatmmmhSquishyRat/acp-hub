//! SQLite projection store (Spec 2, FAQ).
//!
//! Stores Hub-owned snapshots of conversations, messages, runs, and the
//! per-conversation config/mode/plan/commands/usage snapshots. Full-text search
//! is provided by FTS5 (compiled in by the `bundled` rusqlite feature).
//!
//! Concurrency: every message-projection mutation allocates `seq` inside a
//! `BEGIN IMMEDIATE` transaction guarded by `UNIQUE(conv_id, seq)`. The
//! higher-level per-conversation mutex (runtime/CoreHub) serializes turns, so
//! the store is safe under concurrent callers regardless.

use std::{borrow::Borrow, path::Path};

use parking_lot::Mutex;
use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter,
};
use serde::{Deserialize, Serialize};

use crate::endpoint::{harden_home, harden_sensitive_file};
use crate::error::HubError;

const MAX_MESSAGE_PAGE_ROWS: usize = 500;
const MAX_MESSAGE_PAGE_BYTES: i64 = 8 * 1024 * 1024;

/// Message provenance inside the projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    LocalTurn,
    LoadReplay,
    AgentList,
}

impl MessageSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalTurn => "local_turn",
            Self::LoadReplay => "load_replay",
            Self::AgentList => "agent_list",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "local_turn" => Some(Self::LocalTurn),
            "load_replay" => Some(Self::LoadReplay),
            "agent_list" => Some(Self::AgentList),
            _ => None,
        }
    }
}

/// Conversation lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvStatus {
    Idle,
    Running,
    Cancelling,
    Cancelled,
    Failed,
    Completed,
    Deleted,
}

impl ConvStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "idle" => Some(Self::Idle),
            "running" => Some(Self::Running),
            "cancelling" => Some(Self::Cancelling),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            "completed" => Some(Self::Completed),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// Run lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "cancelling" => Some(Self::Cancelling),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewConversation {
    pub id: String,
    pub agent_id: String,
    pub agent_session_id: String,
    pub cwd: Option<String>,
    pub additional_directories: Vec<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationRow {
    pub id: String,
    pub agent_id: String,
    pub agent_session_id: String,
    pub title: Option<String>,
    pub status: ConvStatus,
    pub cwd: Option<String>,
    pub additional_directories: Vec<String>,
    pub session_meta: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewMessage {
    pub id: String,
    pub conv_id: String,
    pub run_id: Option<String>,
    pub source: MessageSource,
    pub role: String,
    pub kind: Option<String>,
    pub content_json: serde_json::Value,
    pub body_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub conv_id: String,
    pub run_id: Option<String>,
    pub source: MessageSource,
    pub current_projection: bool,
    pub role: String,
    pub kind: Option<String>,
    pub content: serde_json::Value,
    pub body_text: String,
    pub seq: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub items: Vec<MessageRow>,
    pub next_offset: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct ReplayedMessage {
    pub id: String,
    pub role: String,
    pub kind: Option<String>,
    pub content_json: serde_json::Value,
    pub body_text: String,
    pub message_key: Option<String>,
}

/// Transaction token for a streamed `session/load` replay refresh.
///
/// `session/update` notifications are persisted while the ACP request is in
/// flight, so the refresh cannot be one long SQLite transaction. The token
/// records the boundary needed to restore the previous Layer 1 projection if
/// the request fails, without touching independently captured `local_turn`
/// rows.
#[derive(Debug)]
pub struct ReplayRefresh {
    conv_id: String,
    load_id: String,
    starting_seq: i64,
    generation_nonce: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub kind: String,
    pub rank: f64,
    pub agent_id: String,
    pub conv_id: String,
    pub conv_title: Option<String>,
    pub message_id: Option<String>,
    pub run_id: Option<String>,
    pub seq: Option<i64>,
    pub role: Option<String>,
    pub source: Option<String>,
    pub created_at: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub items: Vec<SearchHit>,
    pub next_offset: Option<usize>,
}

/// The SQLite-backed projection store.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(home: &Path) -> Result<Self, HubError> {
        harden_home(home)?;
        let database = home.join("hub.db");
        let conn = Connection::open(&database)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        harden_sensitive_file(&database)?;
        for suffix in ["hub.db-wal", "hub.db-shm"] {
            let sidecar = home.join(suffix);
            if sidecar.exists() {
                harden_sensitive_file(&sidecar)?;
            }
        }
        Self::migrate(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.recover_interrupted_load_replays()?;
        store.recover_interrupted_runs()?;
        Ok(store)
    }

    pub fn open_memory() -> Result<Self, HubError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::migrate(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.recover_interrupted_load_replays()?;
        store.recover_interrupted_runs()?;
        Ok(store)
    }

    fn migrate(conn: &Connection) -> Result<(), HubError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
                version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )?;
        let current: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if current < 1 {
            conn.execute_batch(MIGRATION_1)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?)",
                params![now_iso()],
            )?;
        }
        if current < 2 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS load_replay_refreshes(
                    conv_id TEXT PRIMARY KEY,
                    load_id TEXT NOT NULL UNIQUE,
                    starting_seq INTEGER NOT NULL,
                    started_at TEXT NOT NULL,
                    FOREIGN KEY(conv_id) REFERENCES conversations(id) ON DELETE CASCADE
                );",
            )?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?)",
                params![now_iso()],
            )?;
        }
        if current < 3 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS load_replay_projection_before_images(
                    load_id TEXT PRIMARY KEY
                        REFERENCES load_replay_refreshes(load_id) ON DELETE CASCADE,
                    conv_id TEXT NOT NULL UNIQUE
                        REFERENCES conversations(id) ON DELETE CASCADE,
                    conversation_title TEXT,
                    conversation_updated_at TEXT NOT NULL,
                    session_meta_json TEXT,
                    fts_title_present INTEGER NOT NULL CHECK(fts_title_present IN (0, 1)),
                    fts_title TEXT,
                    config_present INTEGER NOT NULL CHECK(config_present IN (0, 1)),
                    config_options_json TEXT,
                    config_modes_json TEXT,
                    config_updated_at TEXT,
                    plan_present INTEGER NOT NULL CHECK(plan_present IN (0, 1)),
                    plan_entries_json TEXT,
                    plan_updated_at TEXT,
                    commands_present INTEGER NOT NULL CHECK(commands_present IN (0, 1)),
                    commands_json TEXT,
                    commands_updated_at TEXT,
                    usage_present INTEGER NOT NULL CHECK(usage_present IN (0, 1)),
                    usage_used INTEGER,
                    usage_size INTEGER,
                    usage_cost_json TEXT,
                    usage_updated_at TEXT
                );",
            )?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (3, ?)",
                params![now_iso()],
            )?;
        }
        if current < 4 {
            let has_generation_nonce: bool = conn.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM pragma_table_info('load_replay_refreshes')
                     WHERE name = 'generation_nonce'
                 )",
                [],
                |row| row.get(0),
            )?;
            let tx = conn.unchecked_transaction()?;
            if !has_generation_nonce {
                tx.execute_batch(
                    "ALTER TABLE load_replay_refreshes
                         ADD COLUMN generation_nonce TEXT;",
                )?;
            }
            tx.execute(
                "UPDATE load_replay_refreshes
                 SET generation_nonce = lower(hex(randomblob(16)))
                 WHERE generation_nonce IS NULL",
                [],
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (4, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    // --- agent_cache -------------------------------------------------------

    pub fn upsert_agent_cache(
        &self,
        id: &str,
        agent_info_json: &str,
        capabilities_json: &str,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO agent_cache(id, agent_info_json, capabilities_json, inspected_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               agent_info_json = excluded.agent_info_json,
               capabilities_json = excluded.capabilities_json,
               inspected_at = excluded.inspected_at",
            params![id, agent_info_json, capabilities_json, now_iso()],
        )?;
        Ok(())
    }

    pub fn agent_cache(&self, id: &str) -> Result<Option<(String, String)>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT agent_info_json, capabilities_json FROM agent_cache WHERE id = ?",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?)
    }

    // --- conversations -----------------------------------------------------

    pub fn create_conversation(&self, c: &NewConversation) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dirs = serde_json::to_string(&c.additional_directories)?;
        let ts = now_iso();
        tx.execute(
            "INSERT INTO conversations(
                 id, agent_id, agent_session_id, title, status,
                 cwd, additional_directories_json, session_meta_json,
                 created_at, updated_at)
             VALUES (?, ?, ?, ?, 'idle', ?, ?, NULL, ?, ?)",
            params![
                c.id,
                c.agent_id,
                c.agent_session_id,
                c.title,
                c.cwd,
                dirs,
                ts,
                ts
            ],
        )?;
        let fts_title = c.title.as_deref().unwrap_or("");
        tx.execute(
            "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
            params![c.id, fts_title],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn conversation(&self, conv_id: &str) -> Result<Option<ConversationRow>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(CONV_SELECT[0], params![conv_id], map_conversation)
            .optional()?)
    }

    pub fn conversation_by_agent_session(
        &self,
        agent_id: &str,
        agent_session_id: &str,
    ) -> Result<Option<ConversationRow>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                CONV_SELECT[1],
                params![agent_id, agent_session_id],
                map_conversation,
            )
            .optional()?)
    }

    pub fn list_conversations(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<ConversationRow>, HubError> {
        let conn = self.conn.lock();
        let sql = if agent_id.is_some() {
            CONV_SELECT[2]
        } else {
            CONV_SELECT[3]
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(a) = agent_id {
            stmt.query_map(params![a], map_conversation)?
        } else {
            stmt.query_map([], map_conversation)?
        };
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn set_conv_status(&self, conv_id: &str, status: ConvStatus) -> Result<(), HubError> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE conversations SET status = ?, updated_at = ? WHERE id = ?",
            params![status.as_str(), now_iso(), conv_id],
        )?;
        Ok(())
    }

    pub fn touch_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        Ok(())
    }

    /// Apply a partial `session_info_update`: title/updatedAt/_meta only.
    pub fn apply_session_info(
        &self,
        conv_id: &str,
        title: Option<&str>,
        updated_at: Option<&str>,
        meta_patch: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        if let Some(t) = title {
            tx.execute(
                "UPDATE conversations SET title = ? WHERE id = ?",
                params![t, conv_id],
            )?;
            tx.execute(
                "DELETE FROM conversations_fts WHERE conv_id = ?",
                params![conv_id],
            )?;
            tx.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![conv_id, t],
            )?;
        }
        if updated_at.is_some() {
            tx.execute(
                "UPDATE conversations SET updated_at = ? WHERE id = ?",
                params![now_iso(), conv_id],
            )?;
        }
        if let Some(patch) = meta_patch {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT session_meta_json FROM conversations WHERE id = ?",
                    params![conv_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            let mut merged: serde_json::Map<String, serde_json::Value> = existing
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            for (k, v) in patch {
                if v.is_null() {
                    merged.remove(k);
                } else {
                    merged.insert(k.clone(), v.clone());
                }
            }
            let serialized = if merged.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&serde_json::Value::Object(merged))?)
            };
            tx.execute(
                "UPDATE conversations SET session_meta_json = ? WHERE id = ?",
                params![serialized, conv_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace the complete `additionalDirectories` list (never merges omitted).
    pub fn set_additional_directories(
        &self,
        conv_id: &str,
        dirs: &[String],
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let serialized = serde_json::to_string(dirs)?;
        conn.execute(
            "UPDATE conversations SET additional_directories_json = ?, updated_at = ? WHERE id = ?",
            params![serialized, now_iso(), conv_id],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM runs
                 WHERE conv_id = ? AND status IN ('running','cancelling')
             )",
            params![conv_id],
            |r| r.get(0),
        )?;
        if active {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        tx.execute(
            "DELETE FROM messages_fts WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "DELETE FROM conversations_fts WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute("DELETE FROM conversations WHERE id = ?", params![conv_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Upsert a conversation row discovered via agent `session/list`.
    /// Creates a new row if the (agent_id, agent_session_id) pair doesn't
    /// exist; otherwise updates title/cwd/directories/meta. Does NOT touch
    /// messages — use `stage_load_replay` to import message history.
    pub fn upsert_agent_session(
        &self,
        agent_id: &str,
        agent_session_id: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        additional_directories: &[String],
    ) -> Result<String, HubError> {
        let conn = self.conn.lock();
        let existing_id: Option<String> = conn
            .query_row(
                "SELECT id FROM conversations WHERE agent_id = ? AND agent_session_id = ?",
                params![agent_id, agent_session_id],
                |r| r.get(0),
            )
            .optional()?;
        let dirs = serde_json::to_string(additional_directories)?;
        let ts = now_iso();
        if let Some(id) = existing_id {
            // Update metadata.
            if let Some(t) = title {
                conn.execute(
                    "UPDATE conversations SET title = ?, updated_at = ? WHERE id = ?",
                    params![t, ts, id],
                )?;
                conn.execute(
                    "DELETE FROM conversations_fts WHERE conv_id = ?",
                    params![id],
                )?;
                conn.execute(
                    "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                    params![id, t],
                )?;
            }
            if let Some(c) = cwd {
                conn.execute(
                    "UPDATE conversations SET cwd = ? WHERE id = ?",
                    params![c, id],
                )?;
            }
            conn.execute(
                "UPDATE conversations SET additional_directories_json = ? WHERE id = ?",
                params![dirs, id],
            )?;
            Ok(id)
        } else {
            // Create new conversation row from agent-side discovery.
            let conv_id = format!("conv-{}", uuid::Uuid::new_v4().simple());
            conn.execute(
                "INSERT INTO conversations(
                     id, agent_id, agent_session_id, title, status,
                     cwd, additional_directories_json, session_meta_json,
                     created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'idle', ?, ?, NULL, ?, ?)",
                params![
                    conv_id,
                    agent_id,
                    agent_session_id,
                    title,
                    cwd,
                    dirs,
                    ts,
                    ts
                ],
            )?;
            conn.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![conv_id, title.unwrap_or("")],
            )?;
            Ok(conv_id)
        }
    }

    // --- runs --------------------------------------------------------------

    pub fn create_run(&self, run_id: &str, conv_id: &str) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM runs
                 WHERE conv_id = ? AND status IN ('running','cancelling')
             )",
            params![conv_id],
            |r| r.get(0),
        )?;
        if active {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        tx.execute(
            "INSERT INTO runs(id, conv_id, status, started_at) VALUES (?, ?, 'running', ?)",
            params![run_id, conv_id, now_iso()],
        )?;
        tx.execute(
            "UPDATE conversations SET status = 'running', updated_at = ? WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Compare-and-set finalize: only updates if status is running/cancelling.
    pub fn finalize_run_cas(
        &self,
        run_id: &str,
        conv_id: &str,
        status: RunStatus,
        stop_reason: Option<&str>,
    ) -> Result<bool, HubError> {
        if status == RunStatus::Running {
            return Err(HubError::other(
                "finalize_run cannot transition a run to running",
            ));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual_conv: Option<String> = tx
            .query_row(
                "SELECT conv_id FROM runs WHERE id = ?",
                params![run_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(actual_conv) = actual_conv else {
            return Ok(false);
        };
        if actual_conv != conv_id {
            return Err(HubError::other(format!(
                "run {run_id} belongs to conversation {actual_conv}, not {conv_id}"
            )));
        }
        let ended_at = (status != RunStatus::Cancelling).then(now_iso);
        let updated = tx.execute(
            "UPDATE runs SET status = ?, stop_reason = ?, ended_at = ?
             WHERE id = ? AND conv_id = ? AND status IN ('running','cancelling')",
            params![status.as_str(), stop_reason, ended_at, run_id, conv_id],
        )?;
        if updated > 0 {
            let conv_status = match status {
                RunStatus::Cancelling => ConvStatus::Cancelling,
                RunStatus::Running => ConvStatus::Running,
                _ => ConvStatus::Idle,
            };
            tx.execute(
                "UPDATE conversations SET status = ?, updated_at = ? WHERE id = ?",
                params![conv_status.as_str(), now_iso(), conv_id],
            )?;
        }
        tx.commit()?;
        Ok(updated > 0)
    }

    /// Resolve run/conversation state left behind by an unclean daemon exit.
    ///
    /// No ACP command survives a daemon process, so persisted non-terminal
    /// runs cannot truthfully remain `running` after startup.
    pub fn recover_interrupted_runs(&self) -> Result<usize, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = now_iso();
        let recovered = tx.execute(
            "UPDATE runs
             SET status = 'failed',
                 stop_reason = COALESCE(stop_reason, 'daemon_restarted'),
                 ended_at = ?
             WHERE status IN ('running','cancelling')",
            params![ts],
        )?;
        tx.execute(
            "UPDATE conversations
             SET status = 'failed', updated_at = ?
             WHERE status IN ('running','cancelling')",
            params![now_iso()],
        )?;
        tx.commit()?;
        Ok(recovered)
    }

    pub fn run_status(&self, run_id: &str) -> Result<Option<RunStatus>, HubError> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT status FROM runs WHERE id = ?",
                params![run_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(row.as_deref().and_then(RunStatus::parse))
    }

    // --- messages ----------------------------------------------------------

    /// Append a message, allocating `seq` atomically inside `BEGIN IMMEDIATE`.
    pub fn append_message(&self, m: &NewMessage) -> Result<i64, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE conv_id = ?",
            params![m.conv_id],
            |r| r.get(0),
        )?;
        let content = serde_json::to_string(&m.content_json)?;
        tx.execute(
            "INSERT INTO messages(
                 id, conv_id, run_id, source, current_projection, message_key,
                 superseded_by_load_id, role, kind, content_json, body_text,
                 seq, created_at)
             VALUES (?, ?, ?, ?, 1, NULL, NULL, ?, ?, ?, ?, ?, ?)",
            params![
                m.id,
                m.conv_id,
                m.run_id,
                m.source.as_str(),
                m.role,
                m.kind,
                content,
                m.body_text,
                seq,
                now_iso(),
            ],
        )?;
        tx.execute(
            "INSERT INTO messages_fts(message_id, conv_id, body) VALUES (?, ?, ?)",
            params![m.id, m.conv_id, m.body_text],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    /// Begin a streamed `session/load` refresh.
    ///
    /// The previous Layer 1 projection remains current while the remote load is
    /// in flight. New replay rows are appended after `starting_seq`; commit
    /// atomically supersedes the prior Layer 1, while rollback removes the new
    /// rows. This ordering ensures a daemon crash cannot hide the last complete
    /// replay snapshot.
    pub fn begin_load_replay(
        &self,
        conv_id: &str,
        load_id: &str,
    ) -> Result<ReplayRefresh, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?)",
            params![conv_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(HubError::not_found("conversation", conv_id));
        }
        let starting_seq = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE conv_id = ?",
            params![conv_id],
            |r| r.get(0),
        )?;
        let generation_nonce = uuid::Uuid::new_v4().simple().to_string();
        let started_at = now_iso();
        tx.execute(
            "INSERT INTO load_replay_refreshes(
                 conv_id, load_id, starting_seq, started_at, generation_nonce
             ) VALUES (?, ?, ?, ?, ?)",
            params![
                conv_id,
                load_id,
                starting_seq,
                started_at.as_str(),
                generation_nonce.as_str()
            ],
        )?;
        tx.execute(
            "INSERT INTO load_replay_projection_before_images(
                 load_id,
                 conv_id,
                 conversation_title,
                 conversation_updated_at,
                 session_meta_json,
                 fts_title_present,
                 fts_title,
                 config_present,
                 config_options_json,
                 config_modes_json,
                 config_updated_at,
                 plan_present,
                 plan_entries_json,
                 plan_updated_at,
                 commands_present,
                 commands_json,
                 commands_updated_at,
                 usage_present,
                 usage_used,
                 usage_size,
                 usage_cost_json,
                 usage_updated_at
             )
             SELECT ?,
                    c.id,
                    c.title,
                    c.updated_at,
                    c.session_meta_json,
                    EXISTS(
                        SELECT 1 FROM conversations_fts f WHERE f.conv_id = c.id
                    ),
                    (
                        SELECT f.title FROM conversations_fts f
                        WHERE f.conv_id = c.id LIMIT 1
                    ),
                    EXISTS(
                        SELECT 1 FROM config_snapshots s WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.config_options_json FROM config_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.modes_json FROM config_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM config_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    EXISTS(
                        SELECT 1 FROM plan_snapshots s WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.entries_json FROM plan_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM plan_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    EXISTS(
                        SELECT 1 FROM available_command_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.commands_json FROM available_command_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM available_command_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    EXISTS(
                        SELECT 1 FROM usage_snapshots s WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.used FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.size FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.cost_json FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    )
             FROM conversations c
             WHERE c.id = ?",
            params![load_id, conv_id],
        )?;
        tx.commit()?;
        Ok(ReplayRefresh {
            conv_id: conv_id.to_string(),
            load_id: load_id.to_string(),
            starting_seq,
            generation_nonce,
        })
    }

    fn validate_load_replay_refresh(
        tx: &Transaction<'_>,
        refresh: &ReplayRefresh,
    ) -> Result<(), HubError> {
        let exact_marker_exists: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM load_replay_refreshes
                 WHERE conv_id = ?
                   AND load_id = ?
                   AND starting_seq = ?
                   AND generation_nonce = ?
             )",
            params![
                refresh.conv_id.as_str(),
                refresh.load_id.as_str(),
                refresh.starting_seq,
                refresh.generation_nonce.as_str()
            ],
            |row| row.get(0),
        )?;
        if !exact_marker_exists {
            return Err(HubError::Conflict(refresh.conv_id.clone()));
        }
        Ok(())
    }

    /// Commit a streamed replay refresh.
    ///
    /// Notification rows are already durable. Superseding the previous Layer 1
    /// happens only here, after the remote load and snapshot persistence have
    /// succeeded.
    pub fn commit_load_replay<R>(&self, refresh: R) -> Result<(), HubError>
    where
        R: Borrow<ReplayRefresh>,
    {
        let refresh = refresh.borrow();
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::validate_load_replay_refresh(&tx, refresh)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?)",
            params![refresh.conv_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(HubError::not_found("conversation", refresh.conv_id.clone()));
        }
        tx.execute(
            "UPDATE messages
             SET current_projection = 0, superseded_by_load_id = ?
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND current_projection = 1
               AND seq <= ?",
            params![
                refresh.load_id.as_str(),
                refresh.conv_id.as_str(),
                refresh.starting_seq
            ],
        )?;
        tx.execute(
            "DELETE FROM load_replay_refreshes
             WHERE conv_id = ? AND load_id = ?",
            params![refresh.conv_id.as_str(), refresh.load_id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn restore_load_replay_projection(
        tx: &Transaction<'_>,
        conv_id: &str,
        load_id: &str,
    ) -> Result<(), HubError> {
        let before_image_exists: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM load_replay_projection_before_images
                 WHERE conv_id = ? AND load_id = ?
             )",
            params![conv_id, load_id],
            |row| row.get(0),
        )?;
        if !before_image_exists {
            return Ok(());
        }

        tx.execute(
            "UPDATE conversations
             SET title = (
                     SELECT conversation_title
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 updated_at = (
                     SELECT conversation_updated_at
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 session_meta_json = (
                     SELECT session_meta_json
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 )
             WHERE id = ?",
            params![
                conv_id, load_id, conv_id, load_id, conv_id, load_id, conv_id
            ],
        )?;
        tx.execute(
            "DELETE FROM conversations_fts WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO conversations_fts(conv_id, title)
             SELECT conv_id, fts_title
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND fts_title_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM config_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO config_snapshots(
                 conv_id, config_options_json, modes_json, updated_at
             )
             SELECT conv_id, config_options_json, config_modes_json, config_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND config_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM plan_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO plan_snapshots(conv_id, entries_json, updated_at)
             SELECT conv_id, plan_entries_json, plan_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND plan_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM available_command_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO available_command_snapshots(conv_id, commands_json, updated_at)
             SELECT conv_id, commands_json, commands_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND commands_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM usage_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO usage_snapshots(
                 conv_id, used, size, cost_json, updated_at
             )
             SELECT conv_id, usage_used, usage_size, usage_cost_json, usage_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND usage_present = 1",
            params![conv_id, load_id],
        )?;
        Ok(())
    }

    /// Roll back a failed streamed replay refresh.
    ///
    /// Newly captured Layer 1 rows are removed from both the base table and
    /// FTS, then the exact previous Layer 1 projection is restored. Layer 2 is
    /// never changed.
    pub fn rollback_load_replay<R>(&self, refresh: R) -> Result<(), HubError>
    where
        R: Borrow<ReplayRefresh>,
    {
        let refresh = refresh.borrow();
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::validate_load_replay_refresh(&tx, refresh)?;
        tx.execute(
            "DELETE FROM messages_fts
             WHERE message_id IN (
                 SELECT id FROM messages
                 WHERE conv_id = ?
                   AND source = 'load_replay'
                   AND seq > ?
             )",
            params![refresh.conv_id.as_str(), refresh.starting_seq],
        )?;
        tx.execute(
            "DELETE FROM messages
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND seq > ?",
            params![refresh.conv_id.as_str(), refresh.starting_seq],
        )?;
        Self::restore_load_replay_projection(
            &tx,
            refresh.conv_id.as_str(),
            refresh.load_id.as_str(),
        )?;
        tx.execute(
            "DELETE FROM load_replay_refreshes
             WHERE conv_id = ? AND load_id = ?",
            params![refresh.conv_id.as_str(), refresh.load_id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Restore partial Layer 1 rows and projection snapshots left by a daemon
    /// crash during `session/load`.
    ///
    /// The last complete Layer 1 and the projection before-image both remain
    /// durable until commit.
    pub fn recover_interrupted_load_replays(&self) -> Result<usize, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let refreshes = {
            let mut stmt = tx.prepare(
                "SELECT conv_id, load_id, starting_seq
                 FROM load_replay_refreshes",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (conv_id, load_id, starting_seq) in &refreshes {
            let has_before_image: bool = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 )",
                params![conv_id, load_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "DELETE FROM messages_fts
                 WHERE message_id IN (
                     SELECT id FROM messages
                     WHERE conv_id = ?
                       AND source = 'load_replay'
                       AND seq > ?
                 )",
                params![conv_id, starting_seq],
            )?;
            tx.execute(
                "DELETE FROM messages
                 WHERE conv_id = ?
                   AND source = 'load_replay'
                   AND seq > ?",
                params![conv_id, starting_seq],
            )?;
            if !has_before_image {
                tx.execute(
                    "UPDATE messages
                     SET current_projection = 1, superseded_by_load_id = NULL
                     WHERE conv_id = ?
                       AND source = 'load_replay'
                       AND current_projection = 0
                       AND seq <= ?
                       AND superseded_by_load_id = ?",
                    params![conv_id, starting_seq, load_id],
                )?;
            }
            Self::restore_load_replay_projection(&tx, conv_id, load_id)?;
        }
        tx.execute("DELETE FROM load_replay_refreshes", [])?;
        tx.commit()?;
        Ok(refreshes.len())
    }

    /// Non-destructive `session/load` replay: insert new `load_replay` rows as
    /// the current Layer 1 projection and supersede only prior Layer 1 rows.
    /// Hub-captured Layer 2 rows remain current and independently visible.
    pub fn stage_load_replay(
        &self,
        conv_id: &str,
        load_id: &str,
        messages: &[ReplayedMessage],
    ) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE messages SET current_projection = 0, superseded_by_load_id = ?
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND current_projection = 1",
            params![load_id, conv_id],
        )?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE conv_id = ?",
            params![conv_id],
            |r| r.get(0),
        )?;
        for m in messages {
            seq += 1;
            let content = serde_json::to_string(&m.content_json)?;
            tx.execute(
                "INSERT INTO messages(
                     id, conv_id, run_id, source, current_projection, message_key,
                     superseded_by_load_id, role, kind, content_json, body_text,
                     seq, created_at)
                 VALUES (?, ?, NULL, 'load_replay', 1, ?, NULL, ?, ?, ?, ?, ?, ?)",
                params![
                    m.id,
                    conv_id,
                    m.message_key,
                    m.role,
                    m.kind,
                    content,
                    m.body_text,
                    seq,
                    now_iso(),
                ],
            )?;
            tx.execute(
                "INSERT INTO messages_fts(message_id, conv_id, body) VALUES (?, ?, ?)",
                params![m.id, conv_id, m.body_text],
            )?;
        }
        tx.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn messages(
        &self,
        conv_id: &str,
        include_audit: bool,
    ) -> Result<Vec<MessageRow>, HubError> {
        let conn = self.conn.lock();
        let filter = if include_audit {
            ""
        } else {
            " AND current_projection = 1"
        };
        let sql = format!(
            "SELECT id, conv_id, run_id, source, current_projection, message_key,
                    role, kind, content_json, body_text, seq, created_at
             FROM messages WHERE conv_id = ?{filter} ORDER BY seq ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![conv_id], map_message)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn messages_page(
        &self,
        conv_id: &str,
        include_audit: bool,
        run_id: Option<&str>,
        after_seq: Option<i64>,
        limit: usize,
        offset: usize,
    ) -> Result<MessagePage, HubError> {
        if limit == 0 {
            return Err(HubError::other("message page limit must be positive"));
        }
        let limit = limit.min(MAX_MESSAGE_PAGE_ROWS);
        let conn = self.conn.lock();
        let filter = if include_audit {
            ""
        } else {
            " AND current_projection = 1"
        };
        let total: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM messages
                 WHERE conv_id = ?{filter}
                   AND (? IS NULL OR run_id = ?)
                   AND (? IS NULL OR seq > ?)"
            ),
            params![conv_id, run_id, run_id, after_seq, after_seq],
            |row| row.get(0),
        )?;
        let total = usize::try_from(total).unwrap_or(usize::MAX);
        let Ok(sql_offset) = i64::try_from(offset) else {
            return Ok(MessagePage {
                items: Vec::new(),
                next_offset: None,
                total,
            });
        };
        let sql = format!(
            "WITH page_candidates AS (
                 SELECT id, conv_id, run_id, source, current_projection, message_key,
                        role, kind, content_json, body_text, seq, created_at,
                        length(CAST(content_json AS BLOB))
                            + length(CAST(body_text AS BLOB)) + 512 AS row_bytes
                 FROM messages
                 WHERE conv_id = ?{filter}
                   AND (? IS NULL OR run_id = ?)
                   AND (? IS NULL OR seq > ?)
                 ORDER BY seq ASC LIMIT ? OFFSET ?
             ),
             budgeted AS (
                 SELECT *,
                        SUM(row_bytes) OVER (ORDER BY seq ASC) AS cumulative_bytes
                 FROM page_candidates
             )
             SELECT id, conv_id, run_id, source, current_projection, message_key,
                    role, kind, content_json, body_text, seq, created_at
             FROM budgeted
             WHERE cumulative_bytes <= ?
             ORDER BY seq ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                conv_id,
                run_id,
                run_id,
                after_seq,
                after_seq,
                limit as i64,
                sql_offset,
                MAX_MESSAGE_PAGE_BYTES
            ],
            map_message,
        )?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        if items.is_empty() && offset < total {
            return Err(HubError::other(format!(
                "message at offset {offset} exceeds the {MAX_MESSAGE_PAGE_BYTES}-byte page budget"
            )));
        }
        let consumed = offset.saturating_add(items.len());
        let next_offset = (consumed < total).then_some(consumed);
        Ok(MessagePage {
            items,
            next_offset,
            total,
        })
    }

    pub fn max_message_seq(&self, conv_id: &str) -> Result<i64, HubError> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE conv_id = ?",
            params![conv_id],
            |row| row.get(0),
        )
        .map_err(HubError::from)
    }

    // --- snapshots ---------------------------------------------------------

    pub fn set_config_snapshot(
        &self,
        conv_id: &str,
        config_options: &serde_json::Value,
        modes: Option<&serde_json::Value>,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let opts = serde_json::to_string(config_options)?;
        let modes = match modes {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        conn.execute(
            "INSERT INTO config_snapshots(conv_id, config_options_json, modes_json, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(conv_id) DO UPDATE SET
               config_options_json = excluded.config_options_json,
               modes_json = excluded.modes_json,
               updated_at = excluded.updated_at",
            params![conv_id, opts, modes, now_iso()],
        )?;
        Ok(())
    }

    pub fn set_plan_snapshot(
        &self,
        conv_id: &str,
        entries: &serde_json::Value,
    ) -> Result<(), HubError> {
        replace_json_snapshot(
            &self.conn.lock(),
            "plan_snapshots",
            "entries_json",
            conv_id,
            entries,
        )
    }

    pub fn set_available_commands_snapshot(
        &self,
        conv_id: &str,
        commands: &serde_json::Value,
    ) -> Result<(), HubError> {
        replace_json_snapshot(
            &self.conn.lock(),
            "available_command_snapshots",
            "commands_json",
            conv_id,
            commands,
        )
    }

    pub fn upsert_usage_snapshot(
        &self,
        conv_id: &str,
        used: i64,
        size: i64,
        cost: Option<&serde_json::Value>,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let cost = match cost {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        conn.execute(
            "INSERT INTO usage_snapshots(conv_id, used, size, cost_json, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(conv_id) DO UPDATE SET
               used = excluded.used, size = excluded.size,
               cost_json = excluded.cost_json, updated_at = excluded.updated_at",
            params![conv_id, used, size, cost, now_iso()],
        )?;
        Ok(())
    }

    pub fn config_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(
            &self.conn.lock(),
            "config_snapshots",
            "config_options_json",
            conv_id,
        )
    }

    pub fn modes_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(&self.conn.lock(), "config_snapshots", "modes_json", conv_id)
    }

    pub fn plan_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(&self.conn.lock(), "plan_snapshots", "entries_json", conv_id)
    }

    pub fn commands_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(
            &self.conn.lock(),
            "available_command_snapshots",
            "commands_json",
            conv_id,
        )
    }

    pub fn usage_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT used, size, cost_json FROM usage_snapshots WHERE conv_id = ?",
                params![conv_id],
                |r| {
                    Ok(serde_json::json!({
                        "used": r.get::<_, i64>(0)?,
                        "size": r.get::<_, i64>(1)?,
                        "cost": r.get::<_, Option<String>>(2)?
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                    }))
                },
            )
            .optional()?;
        Ok(row)
    }

    // --- search ------------------------------------------------------------

    pub fn search(
        &self,
        query: &str,
        agent_id: Option<&str>,
        conv_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<SearchPage, HubError> {
        if limit == 0 {
            return Ok(SearchPage {
                items: Vec::new(),
                next_offset: None,
            });
        }
        let limit = limit.min(500);
        let offset_for_next = offset;
        let Ok(sql_offset) = i64::try_from(offset) else {
            return Ok(SearchPage {
                items: Vec::new(),
                next_offset: None,
            });
        };
        let conn = self.conn.lock();
        let fts = sanitize_fts(query);
        let mut sql = String::from(
            "WITH hits AS (
                 SELECT 'message' AS kind,
                        bm25(messages_fts) AS rank,
                        conversations.agent_id AS agent_id,
                        messages_fts.conv_id AS conv_id,
                        conversations.title AS conv_title,
                        messages_fts.message_id AS message_id,
                        m.run_id AS run_id,
                        m.seq AS seq,
                        m.role AS role,
                        m.source ||
                            CASE WHEN m.current_projection = 1 THEN '' ELSE ':audit' END
                            AS source,
                        m.created_at AS created_at,
                        snippet(messages_fts, 2, '[', ']', '…', 18) AS snippet
                 FROM messages_fts
                 JOIN messages m ON m.id = messages_fts.message_id
                 JOIN conversations ON conversations.id = messages_fts.conv_id
                 WHERE messages_fts MATCH ?
                   AND conversations.status != 'deleted'",
        );
        let mut pv: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text(fts.clone())];
        if let Some(a) = agent_id {
            sql.push_str(" AND conversations.agent_id = ?");
            pv.push(rusqlite::types::Value::Text(a.to_string()));
        }
        if let Some(c) = conv_id {
            sql.push_str(" AND messages_fts.conv_id = ?");
            pv.push(rusqlite::types::Value::Text(c.to_string()));
        }
        sql.push_str(
            " UNION ALL
                 SELECT 'conversation' AS kind,
                        bm25(conversations_fts) AS rank,
                        conversations.agent_id AS agent_id,
                        conversations_fts.conv_id AS conv_id,
                        conversations.title AS conv_title,
                        NULL AS message_id,
                        NULL AS run_id,
                        NULL AS seq,
                        NULL AS role,
                        NULL AS source,
                        conversations.updated_at AS created_at,
                        snippet(conversations_fts, 1, '[', ']', '…', 18) AS snippet
                 FROM conversations_fts
                 JOIN conversations ON conversations.id = conversations_fts.conv_id
                 WHERE conversations_fts MATCH ?
                   AND conversations.status != 'deleted'",
        );
        pv.push(rusqlite::types::Value::Text(fts));
        if let Some(a) = agent_id {
            sql.push_str(" AND conversations.agent_id = ?");
            pv.push(rusqlite::types::Value::Text(a.to_string()));
        }
        if let Some(c) = conv_id {
            sql.push_str(" AND conversations_fts.conv_id = ?");
            pv.push(rusqlite::types::Value::Text(c.to_string()));
        }
        sql.push_str(
            " )
             SELECT kind, rank, agent_id, conv_id, conv_title, message_id,
                    run_id, seq, role, source, created_at, snippet
             FROM hits
             ORDER BY rank ASC, kind ASC, conv_id ASC, COALESCE(message_id, '')
             LIMIT ? OFFSET ?",
        );
        pv.push(rusqlite::types::Value::Integer((limit + 1) as i64));
        pv.push(rusqlite::types::Value::Integer(sql_offset));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(pv.iter()), |r| {
            Ok(SearchHit {
                kind: r.get(0)?,
                rank: r.get(1)?,
                agent_id: r.get(2)?,
                conv_id: r.get(3)?,
                conv_title: r.get(4)?,
                message_id: r.get(5)?,
                run_id: r.get(6)?,
                seq: r.get(7)?,
                role: r.get(8)?,
                source: r.get(9)?,
                created_at: r.get(10)?,
                snippet: r.get(11)?,
            })
        })?;
        let mut items = Vec::new();
        for r in rows {
            items.push(r?);
        }
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_offset = has_more.then(|| offset_for_next.saturating_add(limit));
        Ok(SearchPage { items, next_offset })
    }
}

// --- helpers --------------------------------------------------------------

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

const CONV_SELECT: &[&str] = &[
    // 0: by id
    "SELECT id, agent_id, agent_session_id, title, status, cwd,
            additional_directories_json, session_meta_json, created_at, updated_at
     FROM conversations WHERE id = ?",
    // 1: by agent+session
    "SELECT id, agent_id, agent_session_id, title, status, cwd,
            additional_directories_json, session_meta_json, created_at, updated_at
     FROM conversations WHERE agent_id = ? AND agent_session_id = ?",
    // 2: by agent
    "SELECT id, agent_id, agent_session_id, title, status, cwd,
            additional_directories_json, session_meta_json, created_at, updated_at
     FROM conversations WHERE status != 'deleted' AND agent_id = ? ORDER BY updated_at DESC",
    // 3: all
    "SELECT id, agent_id, agent_session_id, title, status, cwd,
            additional_directories_json, session_meta_json, created_at, updated_at
     FROM conversations WHERE status != 'deleted' ORDER BY updated_at DESC",
];

fn map_conversation(r: &rusqlite::Row) -> rusqlite::Result<ConversationRow> {
    let dirs_json: String = r.get(6)?;
    let dirs: Vec<String> = serde_json::from_str(&dirs_json).unwrap_or_default();
    let meta_json: Option<String> = r.get(7)?;
    Ok(ConversationRow {
        id: r.get(0)?,
        agent_id: r.get(1)?,
        agent_session_id: r.get(2)?,
        title: r.get(3)?,
        status: ConvStatus::parse(&r.get::<_, String>(4)?).unwrap_or(ConvStatus::Idle),
        cwd: r.get(5)?,
        additional_directories: dirs,
        session_meta: meta_json.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn map_message(r: &rusqlite::Row) -> rusqlite::Result<MessageRow> {
    let content_str: String = r.get(8)?;
    let content: serde_json::Value =
        serde_json::from_str(&content_str).unwrap_or(serde_json::Value::Null);
    Ok(MessageRow {
        id: r.get(0)?,
        conv_id: r.get(1)?,
        run_id: r.get(2)?,
        source: MessageSource::parse(&r.get::<_, String>(3)?).unwrap_or(MessageSource::LocalTurn),
        current_projection: r.get::<_, i64>(4)? != 0,
        role: r.get(6)?,
        kind: r.get(7)?,
        content,
        body_text: r.get(9)?,
        seq: r.get(10)?,
        created_at: r.get(11)?,
    })
}

fn replace_json_snapshot(
    conn: &Connection,
    table: &str,
    json_col: &str,
    conv_id: &str,
    value: &serde_json::Value,
) -> Result<(), HubError> {
    let v = serde_json::to_string(value)?;
    let sql = format!(
        "INSERT INTO {table}(conv_id, {json_col}, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(conv_id) DO UPDATE SET
           {json_col} = excluded.{json_col}, updated_at = excluded.updated_at"
    );
    conn.execute(&sql, params![conv_id, v, now_iso()])?;
    Ok(())
}

fn snapshot_json(
    conn: &Connection,
    table: &str,
    col: &str,
    conv_id: &str,
) -> Result<Option<serde_json::Value>, HubError> {
    let sql = format!("SELECT {col} FROM {table} WHERE conv_id = ?");
    let row = conn
        .query_row(&sql, params![conv_id], |r| r.get::<_, String>(0))
        .optional()?;
    Ok(row.and_then(|s| serde_json::from_str(&s).ok()))
}

/// Quote a user query for FTS5 MATCH as a phrase prefix search.
fn sanitize_fts(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let trimmed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return "zzznomatchzzz".to_string();
    }
    format!("\"{trimmed}\"*")
}

/// Deterministic searchable-body extractor over raw ACP JSON. Recursively
/// collects string/number leaves, skipping base64/blob payloads so image/audio
/// data is never indexed. Makes every `session/update` variant searchable.
pub fn search_body(value: &serde_json::Value) -> String {
    let mut out = String::new();
    collect_strings(value, &mut out);
    out.trim().to_string()
}

fn collect_strings(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::String(s) if !looks_like_base64_blob(s) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(s);
        }
        serde_json::Value::Number(n) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&n.to_string());
        }
        serde_json::Value::Array(a) => {
            for v in a {
                collect_strings(v, out);
            }
        }
        serde_json::Value::Object(o) => {
            for (k, v) in o {
                let lk = k.to_ascii_lowercase();
                // Skip binary data payloads entirely.
                if lk == "data" {
                    continue;
                }
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(k);
                collect_strings(v, out);
            }
        }
        _ => {}
    }
}

fn looks_like_base64_blob(s: &str) -> bool {
    s.len() > 256
        && !s.contains(' ')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

const MIGRATION_1: &str = r#"
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
    status TEXT NOT NULL CHECK(status IN ('idle','running','cancelling','cancelled','failed','completed','deleted')),
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
    status TEXT NOT NULL CHECK(status IN ('running','cancelling','completed','cancelled','failed')),
    stop_reason TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT
);
CREATE TABLE messages(
    id TEXT PRIMARY KEY,
    conv_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    source TEXT NOT NULL CHECK(source IN ('local_turn','load_replay','agent_list')),
    current_projection INTEGER NOT NULL DEFAULT 1 CHECK(current_projection IN (0,1)),
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
CREATE VIRTUAL TABLE messages_fts USING fts5(message_id UNINDEXED, conv_id UNINDEXED, body);
CREATE VIRTUAL TABLE conversations_fts USING fts5(conv_id UNINDEXED, title);
CREATE INDEX idx_messages_proj ON messages(conv_id, current_projection, seq);
CREATE INDEX idx_runs_conv ON runs(conv_id, started_at);
"#;
