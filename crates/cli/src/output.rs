use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

pub(crate) fn print_agent_list(agents: &Value, json_output: bool) -> Result<()> {
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
                    transport_target(config),
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
            .map(|(id, config)| vec![id.clone(), transport_type(config), transport_target(config)])
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
        let Some(items) = conversations.as_array() else {
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
                vec![
                    field(item, "id"),
                    field(item, "agent_id"),
                    field(item, "status"),
                    field(item, "title"),
                    field(item, "updated_at"),
                ]
            })
            .collect();
        print_table(&["CONV", "AGENT", "STATUS", "TITLE", "UPDATED"], rows);
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
        vec!["status".to_string(), field(conversation, "status")],
        vec!["title".to_string(), field(conversation, "title")],
        vec!["cwd".to_string(), field(conversation, "cwd")],
        vec!["updated".to_string(), field(conversation, "updated_at")],
    ];
    print_table(&["FIELD", "VALUE"], rows);
    println!();
    Ok(())
}

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
            vec![
                field(item, "kind"),
                field(item, "agent_id"),
                field(item, "conv_id"),
                field(item, "role"),
                shorten(&single_line(&field(item, "snippet")), 100),
            ]
        })
        .collect();
    print_table(&["KIND", "AGENT", "CONV", "ROLE", "SNIPPET"], rows);
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

fn transport_target(config: &Value) -> String {
    let Some(transport) = config.get("transport") else {
        return String::new();
    };
    match transport.get("type").and_then(Value::as_str) {
        Some("stdio") => {
            let command = executable_name(&field(transport, "command"));
            let args = string_array(transport.get("args"));
            if args.is_empty() {
                command
            } else {
                format!("{command} <{} argument(s)>", args.len())
            }
        }
        Some("http") | Some("websocket") => sanitize_url(&field(transport, "url")),
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
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
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
