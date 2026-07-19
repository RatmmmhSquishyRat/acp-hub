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

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::Sha256;

use crate::endpoint::{harden_home, harden_sensitive_file};
use crate::error::HubError;

const MAX_MESSAGE_PAGE_ROWS: usize = 500;
const MAX_MESSAGE_PAGE_BYTES: i64 = 8 * 1024 * 1024;
const MAX_MESSAGE_CURSOR_BYTES: usize = 4096;
const MESSAGE_CURSOR_VERSION: u8 = 1;
const MESSAGE_CURSOR_FILTER: &str = "messages";

type CursorMac = Hmac<Sha256>;

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
    /// Opaque continuation bound to the exact query and projection generation.
    pub next_cursor: Option<String>,
    /// Legacy offset continuation. New callers must use `next_cursor`.
    pub next_offset: Option<usize>,
    pub total: usize,
}

/// Exact query identity for one message page.
pub struct MessagePageQuery<'a> {
    pub conv_id: &'a str,
    pub include_audit: bool,
    pub run_id: Option<&'a str>,
    pub after_seq: Option<i64>,
    pub cursor: Option<&'a str>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageCursorPayload {
    version: u8,
    conversation: String,
    generation: i64,
    last_key: i64,
    include_audit: bool,
    run_id: Option<String>,
    start_after_seq: Option<i64>,
    filter: String,
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

/// Metadata used to atomically stage one discovered agent session.
pub struct AgentSessionImport<'a> {
    pub provisional_conv_id: &'a str,
    pub agent_id: &'a str,
    pub agent_session_id: &'a str,
    pub title: Option<&'a str>,
    pub cwd: &'a str,
    pub additional_directories: &'a [String],
    pub load_id: &'a str,
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

mod lifecycle;
mod paging;
mod replay;
mod snapshots;

// --- helpers --------------------------------------------------------------

fn cursor_hmac_key(conn: &Connection) -> Result<String, HubError> {
    conn.query_row(
        "SELECT value FROM hub_metadata WHERE key = 'message_cursor_hmac_key'",
        [],
        |row| row.get(0),
    )
    .map_err(HubError::from)
}

fn encode_message_cursor(
    conn: &Connection,
    payload: &MessageCursorPayload,
) -> Result<String, HubError> {
    let payload_json = serde_json::to_vec(payload)?;
    let key = cursor_hmac_key(conn)?;
    let mut mac = CursorMac::new_from_slice(key.as_bytes())
        .map_err(|_| HubError::other("invalid persisted message cursor key"))?;
    mac.update(&payload_json);
    let signature = mac.finalize().into_bytes();
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload_json),
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn decode_message_cursor(
    conn: &Connection,
    cursor: &str,
) -> Result<MessageCursorPayload, HubError> {
    if cursor.is_empty() || cursor.len() > MAX_MESSAGE_CURSOR_BYTES {
        return Err(HubError::invalid_cursor("malformed cursor"));
    }
    let (payload, signature) = cursor
        .split_once('.')
        .filter(|(_, signature)| !signature.contains('.'))
        .ok_or_else(|| HubError::invalid_cursor("malformed cursor"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| HubError::invalid_cursor("malformed cursor"))?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| HubError::invalid_cursor("malformed cursor"))?;
    let key = cursor_hmac_key(conn)?;
    let mut mac = CursorMac::new_from_slice(key.as_bytes())
        .map_err(|_| HubError::other("invalid persisted message cursor key"))?;
    mac.update(&payload);
    mac.verify_slice(&signature)
        .map_err(|_| HubError::invalid_cursor("cursor authentication failed"))?;
    let payload: MessageCursorPayload = serde_json::from_slice(&payload)
        .map_err(|_| HubError::invalid_cursor("malformed cursor payload"))?;
    if payload.version != MESSAGE_CURSOR_VERSION || payload.filter != MESSAGE_CURSOR_FILTER {
        return Err(HubError::invalid_cursor(
            "unsupported cursor version or filter",
        ));
    }
    if payload.last_key < 0 || payload.generation < 0 {
        return Err(HubError::invalid_cursor(
            "cursor contains an invalid projection position",
        ));
    }
    Ok(payload)
}

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
    let dirs: Vec<String> = parse_sql_json(6, &dirs_json)?;
    let meta_json: Option<String> = r.get(7)?;
    let status_raw: String = r.get(4)?;
    let status = ConvStatus::parse(&status_raw)
        .ok_or_else(|| invalid_persisted_value(4, "conversation status", &status_raw))?;
    Ok(ConversationRow {
        id: r.get(0)?,
        agent_id: r.get(1)?,
        agent_session_id: r.get(2)?,
        title: r.get(3)?,
        status,
        cwd: r.get(5)?,
        additional_directories: dirs,
        session_meta: meta_json
            .map(|value| parse_sql_json(7, &value))
            .transpose()?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn map_message(r: &rusqlite::Row) -> rusqlite::Result<MessageRow> {
    let content_str: String = r.get(8)?;
    let content: serde_json::Value = parse_sql_json(8, &content_str)?;
    let source_raw: String = r.get(3)?;
    let source = MessageSource::parse(&source_raw)
        .ok_or_else(|| invalid_persisted_value(3, "message source", &source_raw))?;
    let current_projection_raw: i64 = r.get(4)?;
    if !matches!(current_projection_raw, 0 | 1) {
        return Err(invalid_persisted_value(
            4,
            "current projection flag",
            &current_projection_raw.to_string(),
        ));
    }
    Ok(MessageRow {
        id: r.get(0)?,
        conv_id: r.get(1)?,
        run_id: r.get(2)?,
        source,
        current_projection: current_projection_raw == 1,
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
    row.flatten()
        .map(|value| serde_json::from_str(&value).map_err(HubError::Json))
        .transpose()
}

fn parse_sql_json<T: DeserializeOwned>(column: usize, value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn invalid_persisted_value(column: usize, kind: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("corrupt persisted {kind}: {value:?}"),
        )),
    )
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, HubError> {
    let exists = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info(?) WHERE name = ?
         )",
        params![table, column],
        |row| row.get(0),
    )?;
    Ok(exists)
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
CREATE TABLE IF NOT EXISTS agent_cache(
    id TEXT PRIMARY KEY,
    agent_info_json TEXT,
    capabilities_json TEXT,
    inspected_at TEXT
);
CREATE TABLE IF NOT EXISTS hub_metadata(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO hub_metadata(key, value)
VALUES ('message_cursor_hmac_key', lower(hex(randomblob(32))));
CREATE TABLE IF NOT EXISTS conversations(
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    agent_session_id TEXT NOT NULL,
    title TEXT,
    status TEXT NOT NULL CHECK(status IN ('idle','running','cancelling','cancelled','failed','completed','deleted')),
    cwd TEXT,
    additional_directories_json TEXT NOT NULL DEFAULT '[]',
    session_meta_json TEXT,
    projection_generation INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(agent_id, agent_session_id)
);
CREATE TABLE IF NOT EXISTS runs(
    id TEXT PRIMARY KEY,
    conv_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('running','cancelling','completed','cancelled','failed')),
    stop_reason TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT
);
CREATE TABLE IF NOT EXISTS messages(
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
CREATE TABLE IF NOT EXISTS config_snapshots(
    conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    config_options_json TEXT NOT NULL,
    modes_json TEXT,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS plan_snapshots(
    conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    entries_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS available_command_snapshots(
    conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    commands_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS usage_snapshots(
    conv_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    used INTEGER NOT NULL,
    size INTEGER NOT NULL,
    cost_json TEXT,
    updated_at TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    message_id UNINDEXED, conv_id UNINDEXED, body
);
CREATE VIRTUAL TABLE IF NOT EXISTS conversations_fts USING fts5(
    conv_id UNINDEXED, title
);
CREATE INDEX IF NOT EXISTS idx_messages_proj ON messages(conv_id, current_projection, seq);
CREATE INDEX IF NOT EXISTS idx_runs_conv ON runs(conv_id, started_at);
"#;
