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

/// Caps for operator transcript merge (SYSTEM §F.3 defaults for show).
#[derive(Debug, Clone, Copy)]
pub struct MergeLimits {
    pub max_view_nodes: Option<usize>,
    pub max_view_bytes: Option<usize>,
}

impl MergeLimits {
    /// Show default: 200 nodes or 256 KiB.
    pub const fn show_default() -> Self {
        Self {
            max_view_nodes: Some(MAX_VIEW_NODES),
            max_view_bytes: Some(MAX_VIEW_BYTES),
        }
    }

    /// Send end-state for one run: same merge rules, no global byte/node cap
    /// (operator must see the turn they just triggered; page budget already limited fetch).
    pub const fn send_run() -> Self {
        Self {
            max_view_nodes: None,
            max_view_bytes: None,
        }
    }

    fn exceeded(self, nodes: usize, bytes: usize) -> bool {
        self.max_view_nodes.is_some_and(|max| nodes >= max)
            || self.max_view_bytes.is_some_and(|max| bytes >= max)
    }
}

/// Strip ACP / vendor capture noise (HUMAN-READING-CONTRACT §1.1).
///
/// Preserves line breaks so `conv show` can display full multi-paragraph turns.
/// Drops content-type prefixes and standalone vendor tokens `text` / leftover `type`.
pub fn clean_body(body: &str) -> String {
    let cleaned = body
        .lines()
        .map(clean_body_line)
        .collect::<Vec<_>>()
        .join("\n");
    // Collapse runs of empty lines but keep paragraph structure.
    let mut out = String::new();
    let mut blank = 0usize;
    for line in cleaned.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 1 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blank = 0;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out.trim().to_string()
}

fn clean_body_line(line: &str) -> String {
    let mut s = line.trim().to_string();
    // May appear on each vendor chunk line.
    loop {
        let lower = s.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content type") {
            let skip = s.len() - rest.len();
            s = s[skip..].trim_start().to_string();
            continue;
        }
        // Residue when "content " was already stripped elsewhere → "type text …".
        if let Some(rest) = lower.strip_prefix("type ") {
            let skip = s.len() - rest.len();
            s = s[skip..].trim_start().to_string();
            continue;
        }
        break;
    }
    s.split_whitespace()
        .filter(|w| {
            let t = w.trim_matches(|c: char| {
                c == '.' || c == ',' || c == ';' || c == ':' || c == '。' || c == '，'
            });
            !t.eq_ignore_ascii_case("text") && !t.eq_ignore_ascii_case("type")
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// Assistant answer chunks that should glue together (UX-RC3-10 split replies).
fn is_mergeable_assistant_message(role: &str, kind: Option<&str>) -> bool {
    if role != "assistant" {
        return false;
    }
    matches!(
        kind,
        None | Some("") | Some("message") | Some("agent_message_chunk") | Some("text")
    )
}

/// HUMAN-READING-CONTRACT §1.2
pub fn human_role_label(role: &str, kind: Option<&str>) -> &'static str {
    match kind {
        Some("thought") => "think",
        Some("tool_call" | "tool_call_update") => "tool",
        _ if role == "user" => "you",
        _ => "say",
    }
}

/// HUMAN-READING-CONTRACT §1.3
pub fn compact_human_body(kind: Option<&str>, body: &str) -> String {
    compact_human_body_with_content(kind, body, None)
}

pub fn compact_human_body_with_content(
    kind: Option<&str>,
    body: &str,
    content: Option<&serde_json::Value>,
) -> String {
    let cleaned = clean_body(body);
    match kind {
        Some("thought") => truncate_chars(&single_line(&cleaned), 200),
        Some("tool_call" | "tool_call_update") => {
            if let Some(title) = tool_title_from_content(content) {
                return truncate_chars(&title, 80);
            }
            human_tool_title_from_body(&cleaned)
        }
        _ => cleaned,
    }
}

fn tool_title_from_content(content: Option<&serde_json::Value>) -> Option<String> {
    let c = content?;
    c.get("title")
        .or_else(|| c.pointer("/toolCall/title"))
        .or_else(|| c.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn human_tool_title_from_body(body: &str) -> String {
    let line = single_line(body);
    let lower = line.to_ascii_lowercase();
    if let Some(pos) = lower.find("title ") {
        let rest = &line[pos + "title ".len()..];
        let end = [" kind ", " raw", " status ", " toolcallid ", " rawinput"]
            .iter()
            .filter_map(|m| rest.to_ascii_lowercase().find(m))
            .min()
            .unwrap_or(rest.len());
        let title = rest[..end].trim();
        if !title.is_empty() {
            return truncate_chars(title, 80);
        }
    }
    let kept: Vec<&str> = line
        .split_whitespace()
        .filter(|w| {
            let l = w.to_ascii_lowercase();
            !l.contains("toolcallid")
                && !l.starts_with("fc_")
                && !looks_like_id(w)
                && !matches!(
                    l.as_str(),
                    "kind" | "status" | "in_progress" | "completed" | "rawinput" | "title"
                )
        })
        .take(6)
        .collect();
    if kept.is_empty() {
        "tool".into()
    } else {
        truncate_chars(&kept.join(" "), 80)
    }
}

fn looks_like_id(w: &str) -> bool {
    let alnum: String = w.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if alnum.len() >= 16 && alnum.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    w.contains('-') && w.len() >= 20
}

fn single_line(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Merge Store rows into an operator transcript view (SYSTEM §F.3 show defaults).
pub fn merge_transcript(rows: &[MessageRow]) -> TranscriptView {
    merge_transcript_with(rows, MergeLimits::show_default())
}

/// Same merge algorithm (thought/tool collapse + clean_body) with explicit caps.
pub fn merge_transcript_with(rows: &[MessageRow], limits: MergeLimits) -> TranscriptView {
    let raw_count = rows.len();
    let mut items: Vec<ViewMessage> = Vec::new();
    let mut i = 0;
    let mut total_bytes = 0usize;
    let mut truncated = false;

    while i < rows.len() {
        if limits.exceeded(items.len(), total_bytes) {
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

        // Merge short consecutive assistant chunks only (split "UX-RC3-" / "ASK-OK").
        // Do NOT glue large full messages (byte-budget paging / multi-MB bodies).
        const SHORT_CHUNK: usize = 512;
        if is_mergeable_assistant_message(&row.role, kind) {
            let start_seq = row.seq;
            let role = row.role.clone();
            let source = source_label(&row.source);
            let mut body = clean_body(&row.body_text);
            let mut merged = 1usize;
            let mut last_kind = row.kind.clone();
            i += 1;
            // Only attempt glue when the first piece is a short stream fragment.
            if body.chars().count() <= SHORT_CHUNK {
                while i < rows.len()
                    && is_mergeable_assistant_message(&rows[i].role, rows[i].kind.as_deref())
                {
                    let piece = clean_body(&rows[i].body_text);
                    if piece.chars().count() > SHORT_CHUNK {
                        break;
                    }
                    if !piece.is_empty() {
                        body.push_str(&piece);
                    }
                    last_kind = rows[i].kind.clone();
                    merged += 1;
                    i += 1;
                }
            }
            total_bytes = total_bytes.saturating_add(body.len());
            items.push(ViewMessage {
                seq: start_seq,
                role,
                kind: last_kind.or_else(|| Some("message".into())),
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

/// UX-CORE §6.3 post-merge filters (kinds / tail / head / max_chars).
pub fn apply_show_view_filters(
    transcript: &mut TranscriptView,
    kinds: &[String],
    no_tools: bool,
    tail: Option<usize>,
    head: Option<usize>,
    max_chars: Option<usize>,
) {
    if no_tools || !kinds.is_empty() {
        let tokens: Vec<String> = kinds.iter().map(|k| k.to_ascii_lowercase()).collect();
        transcript.items.retain(|item| {
            if no_tools && is_toolish(item.kind.as_deref()) {
                return false;
            }
            if tokens.is_empty() {
                return true;
            }
            view_item_matches_kinds(item, &tokens)
        });
        transcript.view_count = transcript.items.len();
    }
    // Priority: range already applied pre-merge; tail/head after kinds.
    if let Some(n) = tail {
        if transcript.items.len() > n {
            let skip = transcript.items.len() - n;
            transcript.items = transcript.items.split_off(skip);
            transcript.view_count = transcript.items.len();
            transcript.truncated = true;
        }
    } else if let Some(n) = head {
        if transcript.items.len() > n {
            transcript.items.truncate(n);
            transcript.view_count = transcript.items.len();
            transcript.truncated = true;
        }
    }
    if let Some(max) = max_chars.filter(|m| *m > 0) {
        for item in &mut transcript.items {
            item.body_text = truncate_chars(&item.body_text, max);
        }
    }
}

fn view_item_matches_kinds(item: &ViewMessage, tokens: &[String]) -> bool {
    for t in tokens {
        match t.as_str() {
            "user" if item.role == "user" => return true,
            "assistant"
                if item.role == "assistant"
                    && !is_thought(item.kind.as_deref())
                    && !is_toolish(item.kind.as_deref()) =>
            {
                return true;
            }
            "thought" | "thinking" if is_thought(item.kind.as_deref()) => return true,
            "tool" | "tool_call" | "tool_call_update" if is_toolish(item.kind.as_deref()) => {
                return true;
            }
            other
                if item
                    .kind
                    .as_deref()
                    .is_some_and(|k| k.eq_ignore_ascii_case(other)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
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
    fn clean_body_strips_content_type_and_text_tokens() {
        assert_eq!(clean_body("content type text text hello"), "hello");
        assert_eq!(
            clean_body("text Creating ux-rc4.txt text with the line"),
            "Creating ux-rc4.txt with the line"
        );
        assert_eq!(
            clean_body("type text Create a file named smoke2.txt"),
            "Create a file named smoke2.txt"
        );
        let multi = clean_body("content type text line one\ncontent type text line two");
        assert!(multi.contains('\n'), "preserve paragraph breaks for show");
        assert!(multi.contains("line one"));
        assert!(multi.contains("line two"));
        assert!(!multi.to_ascii_lowercase().contains("content type"));
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

    #[test]
    fn merges_split_assistant_answer_chunks() {
        let rows = vec![
            row(1, "assistant", Some("message"), "UX-RC3-"),
            row(2, "assistant", Some("message"), "ASK-OK"),
        ];
        let view = merge_transcript(&rows);
        assert_eq!(view.view_count, 1);
        assert_eq!(view.items[0].body_text, "UX-RC3-ASK-OK");
        assert_eq!(view.items[0].merged_count, 2);
    }

    #[test]
    fn tool_human_line_prefers_title_not_id() {
        let body = "fc_abc123deadbeef title Edit File kind edit rawInput | path status in_progress";
        assert_eq!(compact_human_body(Some("tool_call"), body), "Edit File");
    }

    #[test]
    fn show_view_filters_tail_and_kinds() {
        let rows = vec![
            row(1, "user", Some("prompt"), "q1"),
            row(2, "assistant", Some("thought"), "thinking"),
            row(3, "assistant", Some("message"), "a1"),
            row(
                4,
                "assistant",
                Some("tool_call"),
                "title Edit File kind edit",
            ),
            row(5, "assistant", Some("message"), "a2"),
        ];
        let mut view = merge_transcript(&rows);
        apply_show_view_filters(&mut view, &["assistant".into()], false, Some(1), None, None);
        assert_eq!(view.items.len(), 1);
        assert_eq!(view.items[0].body_text, "a2");
        assert!(view.truncated);

        let mut view2 = merge_transcript(&rows);
        apply_show_view_filters(&mut view2, &[], true, None, None, None);
        assert!(
            view2
                .items
                .iter()
                .all(|i| !matches!(i.kind.as_deref(), Some("tool_call" | "tool_call_update")))
        );
    }

    #[test]
    fn compact_human_strips_toolcall_noise() {
        let body = "toolCallId abc-123\nRead file path";
        let c = compact_human_body(Some("tool_call"), body);
        assert!(!c.to_ascii_lowercase().contains("toolcallid"));
        assert!(c.contains("Read") || c.contains("path") || c == "tool");
    }

    #[test]
    fn large_assistant_bodies_are_not_glued() {
        let big = "x".repeat(600);
        let rows = vec![
            row(1, "assistant", Some("message"), &big),
            row(2, "assistant", Some("message"), &big),
        ];
        let view = merge_transcript(&rows);
        assert_eq!(view.view_count, 2);
    }
}
