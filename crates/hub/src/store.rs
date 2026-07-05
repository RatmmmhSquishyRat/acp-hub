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

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::error::HubError;

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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone)]
pub struct ReplayedMessage {
    pub id: String,
    pub role: String,
    pub kind: Option<String>,
    pub content_json: serde_json::Value,
    pub body_text: String,
    pub message_key: Option<String>,
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
        std::fs::create_dir_all(home)?;
        let mut conn = Connection::open(home.join("hub.db"))?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        Self::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_memory() -> Result<Self, HubError> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn migrate(conn: &mut Connection) -> Result<(), HubError> {
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
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION_1)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?)",
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
        let dirs = serde_json::to_string(&c.additional_directories)?;
        let ts = now_iso();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
                    |r| r.get(0),
                )
                .optional()?;
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
        let tx = conn.transaction()?;
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
        let mut conn = self.conn.lock();
        let dirs = serde_json::to_string(additional_directories)?;
        let ts = now_iso();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing_id: Option<String> = tx
            .query_row(
                "SELECT id FROM conversations WHERE agent_id = ? AND agent_session_id = ?",
                params![agent_id, agent_session_id],
                |r| r.get(0),
            )
            .optional()?;
        let conv_id = if let Some(id) = existing_id {
            // Update metadata.
            if let Some(t) = title {
                tx.execute(
                    "UPDATE conversations SET title = ?, updated_at = ? WHERE id = ?",
                    params![t, ts, &id],
                )?;
                tx.execute(
                    "DELETE FROM conversations_fts WHERE conv_id = ?",
                    params![&id],
                )?;
                tx.execute(
                    "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                    params![&id, t],
                )?;
            }
            if let Some(c) = cwd {
                tx.execute(
                    "UPDATE conversations SET cwd = ? WHERE id = ?",
                    params![c, &id],
                )?;
            }
            tx.execute(
                "UPDATE conversations SET additional_directories_json = ? WHERE id = ?",
                params![dirs, &id],
            )?;
            id
        } else {
            // Create new conversation row from agent-side discovery.
            let conv_id = format!("conv-{}", uuid::Uuid::new_v4().simple());
            tx.execute(
                "INSERT INTO conversations(
                     id, agent_id, agent_session_id, title, status,
                     cwd, additional_directories_json, session_meta_json,
                     created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'idle', ?, ?, NULL, ?, ?)",
                params![
                    &conv_id,
                    agent_id,
                    agent_session_id,
                    title,
                    cwd,
                    dirs,
                    ts,
                    ts
                ],
            )?;
            tx.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![&conv_id, title.unwrap_or("")],
            )?;
            conv_id
        };
        tx.commit()?;
        Ok(conv_id)
    }

    // --- runs --------------------------------------------------------------

    pub fn create_run(&self, run_id: &str, conv_id: &str) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let updated = tx.execute(
            "UPDATE runs SET status = ?, stop_reason = ?, ended_at = ?
             WHERE id = ? AND status IN ('running','cancelling')",
            params![status.as_str(), stop_reason, now_iso(), run_id],
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

    /// Non-destructive `session/load` replay: insert new `load_replay` rows as
    /// the current projection and supersede prior rows. Unmatched Hub-captured
    /// rows remain searchable audit.
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
             WHERE conv_id = ? AND current_projection = 1",
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

    #[cfg(test)]
    fn assert_messages_fts_consistent(&self) -> Result<(), String> {
        let conn = self.conn.lock();
        let (message_rows, fts_rows, missing_or_stale_rows, extra_rows, body_mismatch_rows): (
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM messages),
                    (SELECT COUNT(*) FROM messages_fts),
                    (SELECT COUNT(*) FROM messages m
                     WHERE NOT EXISTS (
                         SELECT 1 FROM messages_fts f
                         WHERE f.message_id = m.id
                           AND f.conv_id = m.conv_id
                           AND f.body IS m.body_text
                     )),
                    (SELECT COUNT(*) FROM messages_fts f
                     WHERE NOT EXISTS (
                         SELECT 1 FROM messages m
                         WHERE m.id = f.message_id
                           AND m.conv_id = f.conv_id
                     )),
                    (SELECT COUNT(*) FROM messages m
                     JOIN messages_fts f
                       ON f.message_id = m.id AND f.conv_id = m.conv_id
                     WHERE f.body IS NOT m.body_text)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(|e| e.to_string())?;

        if message_rows != fts_rows
            || missing_or_stale_rows != 0
            || extra_rows != 0
            || body_mismatch_rows != 0
        {
            return Err(format!(
                "messages_fts inconsistent: messages={message_rows}, fts={fts_rows}, \
                 missing_or_stale={missing_or_stale_rows}, extra={extra_rows}, \
                 body mismatch={body_mismatch_rows}"
            ));
        }

        Ok(())
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
        let conn = self.conn.lock();
        let fts = sanitize_fts(query);
        let mut sql = String::from(
            "SELECT bm25(messages_fts) AS rank,
                    snippet(messages_fts, 2, '<<', '>>', '...', 20) AS snip,
                    messages_fts.conv_id, messages_fts.message_id,
                    m.run_id, m.source, m.current_projection, m.role, m.seq, m.created_at,
                    conversations.agent_id, conversations.title
             FROM messages_fts
             JOIN messages m ON m.id = messages_fts.message_id
             JOIN conversations ON conversations.id = messages_fts.conv_id
             WHERE messages_fts MATCH ? AND conversations.status != 'deleted'",
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
        sql.push_str(" ORDER BY rank LIMIT ? OFFSET ?");
        pv.push(rusqlite::types::Value::Integer(limit as i64));
        pv.push(rusqlite::types::Value::Integer(offset as i64));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(pv.iter()), |r| {
            let rank: f64 = r.get(0)?;
            let snip: String = r.get::<_, Option<String>>(1)?.unwrap_or_default();
            let cid: String = r.get(2)?;
            let mid: String = r.get(3)?;
            let run_id: Option<String> = r.get(4)?;
            let source: Option<String> = r.get(5)?;
            let current: bool = r.get::<_, i64>(6)? != 0;
            let role: Option<String> = r.get(7)?;
            let seq: Option<i64> = r.get(8)?;
            let created_at: Option<String> = r.get(9)?;
            let agent_id: String = r.get(10)?;
            let conv_title: Option<String> = r.get(11)?;
            Ok(SearchHit {
                kind: "message".to_string(),
                rank,
                agent_id,
                conv_id: cid,
                conv_title,
                message_id: Some(mid),
                run_id,
                role,
                seq,
                source: source.map(|s| format!("{s}{}", if current { "" } else { ":audit" })),
                created_at,
                snippet: snip,
            })
        })?;
        let mut items = Vec::new();
        for r in rows {
            items.push(r?);
        }
        let message_count = items.len();
        // M3: Only append conversation-title hits on the first page so they
        // don't repeat on every page or break offset calculation.
        if offset == 0 {
            if let Some(extra) = self.search_conversations(&conn, &fts, agent_id, conv_id)? {
                items.extend(extra);
            }
        }
        let next_offset = (message_count >= limit).then(|| offset + limit);
        Ok(SearchPage { items, next_offset })
    }

    /// Search conversations by title via conversations_fts.
    fn search_conversations(
        &self,
        conn: &Connection,
        fts: &str,
        agent_id: Option<&str>,
        conv_id: Option<&str>,
    ) -> Result<Option<Vec<SearchHit>>, HubError> {
        let mut sql = String::from(
            "SELECT bm25(conversations_fts) AS rank,
                    conversations_fts.conv_id,
                    conversations.agent_id, conversations.title, conversations.updated_at
             FROM conversations_fts
             JOIN conversations ON conversations.id = conversations_fts.conv_id
             WHERE conversations_fts MATCH ? AND conversations.status != 'deleted'",
        );
        let mut pv: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Text(fts.to_string())];
        if let Some(a) = agent_id {
            sql.push_str(" AND conversations.agent_id = ?");
            pv.push(rusqlite::types::Value::Text(a.to_string()));
        }
        if let Some(c) = conv_id {
            sql.push_str(" AND conversations_fts.conv_id = ?");
            pv.push(rusqlite::types::Value::Text(c.to_string()));
        }
        sql.push_str(" LIMIT 10");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(pv.iter()), |r| {
            Ok(SearchHit {
                kind: "conversation".to_string(),
                rank: r.get(0)?,
                agent_id: r.get::<_, String>(2)?,
                conv_id: r.get::<_, String>(1)?,
                conv_title: r.get(3)?,
                message_id: None,
                run_id: None,
                seq: None,
                role: None,
                source: None,
                created_at: r.get::<_, Option<String>>(4)?,
                snippet: String::new(),
            })
        })?;
        let mut hits = Vec::new();
        for r in rows {
            hits.push(r?);
        }
        if hits.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hits))
        }
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
    let row: Option<Option<String>> = conn
        .query_row(&sql, params![conv_id], |r| r.get::<_, Option<String>>(0))
        .optional()?;
    Ok(row.flatten().and_then(|s| serde_json::from_str(&s).ok()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_agent_session_rolls_back_when_conversation_fts_write_fails() {
        let store = Store::open_memory().unwrap();
        let existing_id = store
            .upsert_agent_session("agent-a", "session-1", Some("old title"), Some("/tmp"), &[])
            .unwrap();

        store
            .conn
            .lock()
            .execute_batch("DROP TABLE conversations_fts;")
            .unwrap();

        assert!(
            store
                .upsert_agent_session("agent-a", "session-1", Some("new title"), Some("/tmp"), &[])
                .is_err()
        );
        assert_eq!(
            store
                .conversation(&existing_id)
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("old title")
        );

        assert!(
            store
                .upsert_agent_session(
                    "agent-a",
                    "session-2",
                    Some("new session"),
                    Some("/tmp"),
                    &[]
                )
                .is_err()
        );
        assert!(
            store
                .conversation_by_agent_session("agent-a", "session-2")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn messages_fts_consistency_guard_covers_append_and_load_replay() {
        let store = Store::open_memory().unwrap();
        store
            .create_conversation(&NewConversation {
                id: "conv-fts".to_string(),
                agent_id: "agent-a".to_string(),
                agent_session_id: "session-fts".to_string(),
                cwd: Some("/tmp".to_string()),
                additional_directories: vec![],
                title: None,
            })
            .unwrap();

        store.assert_messages_fts_consistent().unwrap();
        store
            .append_message(&NewMessage {
                id: "conv-fts-m1".to_string(),
                conv_id: "conv-fts".to_string(),
                run_id: None,
                source: MessageSource::LocalTurn,
                role: "user".to_string(),
                kind: None,
                content_json: serde_json::json!({ "text": "captured searchable" }),
                body_text: "captured searchable".to_string(),
            })
            .unwrap();
        store.assert_messages_fts_consistent().unwrap();

        store
            .stage_load_replay(
                "conv-fts",
                "load-1",
                &[ReplayedMessage {
                    id: "conv-fts-load-1".to_string(),
                    role: "assistant".to_string(),
                    kind: None,
                    content_json: serde_json::json!({ "text": "replayed searchable" }),
                    body_text: "replayed searchable".to_string(),
                    message_key: Some("agent-msg-1".to_string()),
                }],
            )
            .unwrap();
        store.assert_messages_fts_consistent().unwrap();

        {
            let conn = store.conn.lock();
            conn.execute(
                "DELETE FROM messages_fts WHERE message_id = ?",
                params!["conv-fts-load-1"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages_fts(message_id, conv_id, body) VALUES (?, ?, ?)",
                params!["conv-fts-load-1", "conv-fts", "drifted body"],
            )
            .unwrap();
        }
        let err = store.assert_messages_fts_consistent().unwrap_err();
        assert!(err.contains("body mismatch"), "{err}");
    }
}
