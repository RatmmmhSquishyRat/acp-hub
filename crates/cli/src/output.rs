use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

pub(crate) fn print_agent_list(agents: &Value, json_output: bool) -> Result<()> {
    print_agent_list_inner(agents, json_output, false)
}

pub(crate) fn print_agent_list_revealed(agents: &Value, json_output: bool) -> Result<()> {
    print_agent_list_inner(agents, json_output, true)
}

fn print_agent_list_inner(agents: &Value, json_output: bool, reveal: bool) -> Result<()> {
    if json_output {
        print_json(agents)
    } else {
        let Some(map) = agents.as_object() else {
            print_json(agents)?;
            return Ok(());
        };
        if map.is_empty() {
            println!("No agents registered.");
            return Ok(());
        }
        let rows = map
            .iter()
            .map(|(id, config)| {
                vec![
                    id.clone(),
                    transport_type(config),
                    transport_target(config, reveal),
                    proxy_chain(config),
                ]
            })
            .collect();
        print_table(&["ID", "TYPE", "TARGET", "PROXIES"], rows);
        Ok(())
    }
}

pub(crate) fn print_proxy_list(proxies: &Value, json_output: bool) -> Result<()> {
    if json_output {
        print_json(proxies)
    } else {
        let Some(map) = proxies.as_object() else {
            print_json(proxies)?;
            return Ok(());
        };
        if map.is_empty() {
            println!("No proxies registered.");
            return Ok(());
        }
        let rows = map
            .iter()
            .map(|(id, config)| {
                vec![
                    id.clone(),
                    transport_type(config),
                    transport_target(config, false),
                ]
            })
            .collect();
        print_table(&["ID", "TYPE", "TARGET"], rows);
        Ok(())
    }
}

pub(crate) fn print_inspected_config(config: &Value, json_output: bool) -> Result<()> {
    if json_output {
        print_json(config)
    } else {
        println!("{}", serde_json::to_string_pretty(config)?);
        Ok(())
    }
}

pub(crate) fn print_conversation_list(conversations: &Value, json_output: bool) -> Result<()> {
    if json_output {
        print_json(conversations)
    } else {
        // Envelope { items, limit, offset, truncated } or bare array (legacy).
        let items = conversations
            .get("items")
            .and_then(|v| v.as_array())
            .or_else(|| conversations.as_array());
        let Some(items) = items else {
            print_json(conversations)?;
            return Ok(());
        };
        if items.is_empty() {
            println!("No conversations.");
            return Ok(());
        }
        let rows = items
            .iter()
            .map(|item| {
                let ix = field(item, "interaction");
                let ix_short = match ix.as_str() {
                    "writable" => "W".into(),
                    "read_only" => "R".into(),
                    other if !other.is_empty() && other != "-" => other.to_string(),
                    _ => "-".into(),
                };
                vec![
                    field(item, "id"),
                    field(item, "agent_id"),
                    ix_short,
                    field(item, "origin"),
                    field(item, "status"),
                    field(item, "title"),
                    field(item, "updated_at"),
                ]
            })
            .collect();
        print_table(
            &[
                "CONV", "AGENT", "IX", "ORIGIN", "STATUS", "TITLE", "UPDATED",
            ],
            rows,
        );
        Ok(())
    }
}

pub(crate) fn print_conversation_detail(conversation: &Value) -> Result<()> {
    let rows = vec![
        vec!["id".to_string(), field(conversation, "id")],
        vec!["agent".to_string(), field(conversation, "agent_id")],
        vec![
            "agent_session".to_string(),
            field(conversation, "agent_session_id"),
        ],
        vec!["origin".to_string(), field(conversation, "origin")],
        vec![
            "interaction".to_string(),
            field(conversation, "interaction"),
        ],
        vec!["status".to_string(), field(conversation, "status")],
        vec!["phase".to_string(), field(conversation, "phase")],
        vec!["busy".to_string(), field(conversation, "busy")],
        vec![
            "last_outcome".to_string(),
            field(conversation, "last_outcome"),
        ],
        vec!["title".to_string(), field(conversation, "title")],
        vec!["cwd".to_string(), field(conversation, "cwd")],
        vec!["updated".to_string(), field(conversation, "updated_at")],
    ];
    print_table(&["FIELD", "VALUE"], rows);
    println!();
    Ok(())
}

#[allow(dead_code)] // retained for raw message arrays outside transcript envelope
pub(crate) fn print_messages(messages: &Value) -> Result<()> {
    let Some(items) = messages.as_array() else {
        print_json(messages)?;
        return Ok(());
    };
    if items.is_empty() {
        println!("No messages.");
        return Ok(());
    }
    let rows = items
        .iter()
        .map(|item| {
            let src = field(item, "source");
            let label = match src.as_str() {
                "load_replay" => "[agent-original]",
                "local_turn" => "[hub-capture]",
                "agent_list" => "[agent-meta]",
                _ => "",
            };
            vec![
                field(item, "seq"),
                label.to_string(),
                field(item, "role"),
                shorten(&single_line(&field(item, "body_text")), 100),
            ]
        })
        .collect();
    print_table(&["SEQ", "SOURCE", "ROLE", "BODY"], rows);
    Ok(())
}

/// Natural CLI line for one transcript item (HUMAN-READING-CONTRACT v2).
/// Not a custom language: plain speech, indented thinking/tools.
///
/// `full`: when true (e.g. `conv show`), keep complete thought/reply text —
/// do not collapse thoughts to 200 chars. Tools stay title-only either way.
pub(crate) fn format_human_transcript_line(role: &str, kind: Option<&str>, body: &str) -> String {
    format_human_transcript_line_inner(role, kind, body, false)
}

pub(crate) fn format_human_show_line(role: &str, kind: Option<&str>, body: &str) -> String {
    format_human_transcript_line_inner(role, kind, body, true)
}

fn format_human_transcript_line_inner(
    role: &str,
    kind: Option<&str>,
    body: &str,
    full: bool,
) -> String {
    use acp_hub::store::{clean_body, compact_human_body};
    let b = match kind {
        Some("tool_call" | "tool_call_update") => compact_human_body(kind, body),
        Some("thought") if full => clean_body(body),
        Some("thought") => compact_human_body(kind, body),
        _ if full => clean_body(body),
        _ => compact_human_body(kind, body),
    };
    if b.is_empty() {
        return String::new();
    }
    match kind {
        Some("thought") | Some("tool_call" | "tool_call_update") => format!("  {b}"),
        _ if role == "user" => format!("You: {b}"),
        _ => b,
    }
}

pub(crate) fn format_human_done_line(stop_reason: &str, total_ms: u64) -> String {
    let secs = total_ms as f64 / 1000.0;
    format!("Completed in {secs:.1}s ({stop_reason})")
}

pub(crate) fn format_human_timings_line(
    total_ms: u64,
    prompt_ms: Option<u64>,
    session_ms: Option<u64>,
) -> String {
    let mut parts = vec![format!("total_ms={total_ms}")];
    if let Some(p) = prompt_ms {
        parts.push(format!("prompt_ms={p}"));
    }
    if let Some(s) = session_ms {
        parts.push(format!("session_ms={s}"));
    }
    format!("[acp-hub] timings {}", parts.join(" "))
}

/// Phase-2 merged transcript: full conversation as a readable stream.
///
/// Not a truncated ROLE/BODY table. Wire JSON uses camelCase (`bodyText`);
/// `field` accepts both snake and camel so bodies never go silently blank.
pub(crate) fn print_transcript(transcript: &Value) -> Result<()> {
    let Some(items) = transcript
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| transcript.as_array())
    else {
        print_json(transcript)?;
        return Ok(());
    };
    if items.is_empty() {
        println!("No messages.");
        return Ok(());
    }
    let mut printed = 0usize;
    for item in items {
        let kind = field(item, "kind");
        let role = field(item, "role");
        let kind_opt = if kind.is_empty() || kind == "-" {
            None
        } else {
            Some(kind.as_str())
        };
        // Hub ViewMessage serializes body_text → bodyText (camelCase).
        let body = field(item, "body_text");
        let line = format_human_show_line(&role, kind_opt, &body);
        if line.is_empty() {
            continue;
        }
        if printed > 0 {
            println!();
        }
        println!("{}", sanitize_terminal_text(&line));
        printed += 1;
    }
    if printed == 0 {
        println!("No messages.");
    }
    if transcript
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        println!(
            "\n(truncated: showing {} of {} raw rows)",
            field(transcript, "view_count"),
            field(transcript, "raw_count"),
        );
    }
    Ok(())
}

pub(crate) fn print_search_results(results: &Value) -> Result<()> {
    let Some(items) = results.get("items").and_then(Value::as_array) else {
        print_json(results)?;
        return Ok(());
    };
    if items.is_empty() {
        println!("No results.");
        return Ok(());
    }
    let rows = items
        .iter()
        .map(|item| {
            let ix = field(item, "interaction");
            let ix_short = match ix.as_str() {
                "writable" => "W".to_string(),
                "read_only" => "R".to_string(),
                other if !other.is_empty() && other != "-" => other.to_string(),
                _ => "-".to_string(),
            };
            let snip = field(item, "snippet");
            let snip = snip
                .replace("content type text text", "")
                .replace("content type text", "");
            vec![
                field(item, "kind"),
                field(item, "agent_id"),
                field(item, "conv_id"),
                ix_short,
                field(item, "origin"),
                shorten(&single_line(&snip), 80),
            ]
        })
        .collect();
    print_table(&["KIND", "AGENT", "CONV", "IX", "ORIGIN", "SNIPPET"], rows);
    if let Some(next) = results.get("next_offset").and_then(Value::as_u64) {
        println!("next offset: {next}");
    }
    Ok(())
}

pub(crate) fn print_config_section(value: Option<&Value>, empty: &str) -> Result<()> {
    match value {
        Some(value) if !value.is_null() => print_json(value),
        _ => {
            println!("{empty}");
            Ok(())
        }
    }
}

fn transport_type(config: &Value) -> String {
    match config
        .get("transport")
        .and_then(|transport| transport.get("type"))
        .and_then(Value::as_str)
    {
        Some("websocket") => "ws".to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn transport_target(config: &Value, reveal: bool) -> String {
    let Some(transport) = config.get("transport") else {
        return String::new();
    };
    match transport.get("type").and_then(Value::as_str) {
        Some("stdio") => {
            let command = field(transport, "command");
            let args = string_array(transport.get("args"));
            if reveal {
                if args.is_empty() {
                    command
                } else {
                    format!("{command} {}", args.join(" "))
                }
            } else {
                let short = executable_name(&command);
                if args.is_empty() {
                    short
                } else {
                    format!("{short} <{} argument(s)>", args.len())
                }
            }
        }
        Some("http") | Some("websocket") => {
            let url = field(transport, "url");
            if reveal { url } else { sanitize_url(&url) }
        }
        _ => String::new(),
    }
}

fn proxy_chain(config: &Value) -> String {
    let value = config
        .get("proxy_chain")
        .or_else(|| config.get("proxyChain"));
    string_array(value).join(",")
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

pub(crate) fn field(value: &Value, key: &str) -> String {
    if let Some(v) = value.get(key) {
        return value_as_display(v);
    }
    // ViewMessage / envelopes often serialize as camelCase (bodyText, rawCount…).
    if key.contains('_') {
        let camel = snake_to_camel(key);
        if let Some(v) = value.get(&camel) {
            return value_as_display(v);
        }
    }
    String::new()
}

fn value_as_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn snake_to_camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for ch in key.chars() {
        if ch == '_' {
            upper = true;
            continue;
        }
        if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shorten(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let shortened: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

pub(crate) fn print_table(headers: &[&str], rows: Vec<Vec<String>>) {
    let rows = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell| sanitize_terminal_text(&cell))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut widths = headers.iter().map(|h| h.len()).collect::<Vec<_>>();
    for row in &rows {
        for (idx, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(idx) {
                *width = (*width).max(cell.len());
            }
        }
    }
    print_row(headers.iter().map(|s| s.to_string()).collect(), &widths);
    print_row(
        widths.iter().map(|width| "-".repeat(*width)).collect(),
        &widths,
    );
    for row in rows {
        print_row(row, &widths);
    }
}

fn print_row(row: Vec<String>, widths: &[usize]) {
    for (idx, cell) in row.iter().enumerate() {
        if idx > 0 {
            print!("  ");
        }
        let width = widths.get(idx).copied().unwrap_or_default();
        print!("{cell:<width$}");
    }
    println!();
}

fn sanitize_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "<redacted-url>".to_string();
    };
    let authority_and_path = rest.rsplit_once('@').map_or(rest, |(_, tail)| tail);
    let authority = authority_and_path
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        "<redacted-url>".to_string()
    } else {
        format!("{scheme}://{authority}/<redacted>")
    }
}

fn executable_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("<command>")
        .to_string()
}

pub(crate) fn sanitize_terminal_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        if !ch.is_control() {
            output.push(ch);
        }
    }
    output
}

pub(crate) fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
