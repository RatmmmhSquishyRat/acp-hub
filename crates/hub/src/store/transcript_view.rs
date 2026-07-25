//! Operator-facing transcript view (Phase 2) — pure merge over Store rows.
//! Store remains the durable source; this module never mutates the DB.

use super::MessageRow;
use serde::Serialize;

/// One human/agent-readable view node after merge.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ViewMessage {
    pub seq: i64,
    pub role: String,
    pub kind: Option<String>,
    pub body_text: String,
    pub source: String,
    pub merged_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptView {
    pub items: Vec<ViewMessage>,
    pub truncated: bool,
    pub raw_count: usize,
    pub view_count: usize,
}

const MAX_VIEW_NODES: usize = 200;
const MAX_VIEW_BYTES: usize = 256 * 1024;

/// Strip ACP capture noise from body text.
pub fn clean_body(body: &str) -> String {
    let mut s = body.trim().to_string();
    // (?i)^content type\s+
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("content type") {
        let skip = body.len() - rest.len();
        s = body[skip..].trim_start().to_string();
    }
    // collapse repeated "text text"
    while s.contains("text text") {
        s = s.replace("text text", "text");
    }
    s.trim().to_string()
}

fn tool_call_id(content: &serde_json::Value) -> Option<String> {
    content
        .get("toolCallId")
        .or_else(|| content.get("tool_call_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            content
                .pointer("/toolCall/toolCallId")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

fn is_thought(kind: Option<&str>) -> bool {
    matches!(kind, Some("thought"))
}

fn is_toolish(kind: Option<&str>) -> bool {
    matches!(kind, Some("tool_call" | "tool_call_update"))
}

/// Merge Store rows into an operator transcript view (SYSTEM §F.3).
pub fn merge_transcript(rows: &[MessageRow]) -> TranscriptView {
    let raw_count = rows.len();
    let mut items: Vec<ViewMessage> = Vec::new();
    let mut i = 0;
    let mut total_bytes = 0usize;
    let mut truncated = false;

    while i < rows.len() {
        if items.len() >= MAX_VIEW_NODES || total_bytes >= MAX_VIEW_BYTES {
            truncated = true;
            break;
        }
        let row = &rows[i];
        let kind = row.kind.as_deref();

        if is_thought(kind) {
            let start_seq = row.seq;
            let role = row.role.clone();
            let source = source_label(&row.source);
            let mut body = clean_body(&row.body_text);
            let mut merged = 1usize;
            i += 1;
            while i < rows.len() && is_thought(rows[i].kind.as_deref()) {
                let piece = clean_body(&rows[i].body_text);
                if !piece.is_empty() {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&piece);
                }
                merged += 1;
                i += 1;
            }
            total_bytes = total_bytes.saturating_add(body.len());
            items.push(ViewMessage {
                seq: start_seq,
                role,
                kind: Some("thought".into()),
                body_text: body,
                source,
                merged_count: merged,
            });
            continue;
        }

        if is_toolish(kind) {
            let tid = tool_call_id(&row.content);
            let start_seq = row.seq;
            let role = row.role.clone();
            let source = source_label(&row.source);
            let mut body = clean_body(&row.body_text);
            let mut merged = 1usize;
            let mut last_kind = row.kind.clone();
            i += 1;
            while i < rows.len() && is_toolish(rows[i].kind.as_deref()) {
                let next_id = tool_call_id(&rows[i].content);
                if tid.is_some() && next_id != tid {
                    break;
                }
                if tid.is_none() && next_id.is_some() {
                    break;
                }
                let piece = clean_body(&rows[i].body_text);
                if !piece.is_empty() {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&piece);
                }
                last_kind = rows[i].kind.clone();
                merged += 1;
                i += 1;
            }
            total_bytes = total_bytes.saturating_add(body.len());
            items.push(ViewMessage {
                seq: start_seq,
                role,
                kind: last_kind,
                body_text: body,
                source,
                merged_count: merged,
            });
            continue;
        }

        let body = clean_body(&row.body_text);
        total_bytes = total_bytes.saturating_add(body.len());
        items.push(ViewMessage {
            seq: row.seq,
            role: row.role.clone(),
            kind: row.kind.clone(),
            body_text: body,
            source: source_label(&row.source),
            merged_count: 1,
        });
        i += 1;
    }

    if i < rows.len() {
        truncated = true;
    }

    let view_count = items.len();
    TranscriptView {
        items,
        truncated,
        raw_count,
        view_count,
    }
}

fn source_label(source: &super::MessageSource) -> String {
    match source {
        super::MessageSource::LocalTurn => "local_turn".into(),
        super::MessageSource::LoadReplay => "load_replay".into(),
        super::MessageSource::AgentList => "agent_list".into(),
    }
}

/// Unicode-ish char truncate for previews.
pub fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// SYSTEM §F.6 summary_preview from message rows + title.
pub fn summary_preview(rows: &[MessageRow], title: Option<&str>) -> Option<String> {
    let mut last_user: Option<String> = None;
    let mut last_assistant: Option<String> = None;
    for row in rows {
        let body = clean_body(&row.body_text);
        if body.is_empty() {
            continue;
        }
        if row.role == "user" {
            last_user = Some(body);
        } else if row.role == "assistant" && !is_thought(row.kind.as_deref()) {
            last_assistant = Some(body);
        }
    }
    if let Some(u) = last_user {
        return Some(truncate_chars(&u, 80));
    }
    if let Some(a) = last_assistant {
        return Some(truncate_chars(&a, 80));
    }
    title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| truncate_chars(t, 80))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MessageRow, MessageSource};

    fn row(seq: i64, role: &str, kind: Option<&str>, body: &str) -> MessageRow {
        MessageRow {
            id: format!("m{seq}"),
            conv_id: "c1".into(),
            run_id: Some("r1".into()),
            source: MessageSource::LocalTurn,
            current_projection: true,
            role: role.into(),
            kind: kind.map(str::to_string),
            content: serde_json::json!({}),
            body_text: body.into(),
            seq,
            created_at: "t".into(),
        }
    }

    #[test]
    fn sc13_merges_ten_thought_chunks_to_one_node() {
        let rows: Vec<_> = (1..=12)
            .map(|i| row(i, "assistant", Some("thought"), &format!("chunk{i}")))
            .collect();
        let view = merge_transcript(&rows);
        assert_eq!(view.raw_count, 12);
        assert_eq!(view.view_count, 1);
        assert_eq!(view.items[0].merged_count, 12);
        assert_eq!(view.items[0].kind.as_deref(), Some("thought"));
        assert!(view.items[0].body_text.contains("chunk1"));
        assert!(view.items[0].body_text.contains("chunk12"));
    }

    #[test]
    fn clean_body_strips_content_type_noise() {
        assert_eq!(clean_body("content type text text hello"), "text hello");
    }

    #[test]
    fn summary_prefers_latest_user() {
        let rows = vec![
            row(1, "user", None, "first question"),
            row(2, "assistant", Some("thought"), "thinking"),
            row(3, "assistant", None, "answer"),
            row(4, "user", None, "follow up please"),
        ];
        assert_eq!(
            summary_preview(&rows, Some("title")).as_deref(),
            Some("follow up please")
        );
    }
}
