use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy,
    ProxyEndpointConfig, ProxyTransport,
};
use acp_hub::hub::{
    ConfigParam, CreateConversationParams, HubClient, MessagesPageParams, SearchParams,
    SendPromptParams,
};
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::args::{
    AgentAddArgs, AgentCommand, AgentTransportKind, ConversationCommand, ModeCommand, ParamCommand,
    ProxyAddArgs, ProxyCommand, SearchArgs, SendArgs,
};
use std::io::Write;

use crate::output::{
    field, print_agent_list, print_config_section, print_conversation_detail,
    print_conversation_list, print_inspected_config, print_json, print_proxy_list,
    print_search_results, print_table, print_transcript,
};

const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn handle_agent(home: &Path, command: AgentCommand) -> Result<()> {
    match command {
        AgentCommand::List { json } => {
            let client = connect(home).await?;
            let agents = client.list_agents().await?;
            print_agent_list(&agents, json)
        }
        AgentCommand::Add(args) => {
            let id = args.id.clone();
            let config = build_agent_config(&args)?;
            let client = connect(home).await?;
            client.register_agent(id.clone(), config).await?;
            println!("registered agent {id}");
            Ok(())
        }
        AgentCommand::Remove { id } => {
            let client = connect(home).await?;
            client.remove_agent(id.clone()).await?;
            println!("removed agent {id}");
            Ok(())
        }
        AgentCommand::Inspect { id, probe, json } => {
            let client = connect(home).await?;
            let inspection = client.inspect_agent_probe(id, probe).await?;
            print_inspected_config(&inspection, json)
        }
        AgentCommand::Auth { id, method_id } => {
            let client = connect(home).await?;
            client
                .authenticate_agent(id.clone(), method_id.clone())
                .await?;
            println!("authenticated agent {id} with method {method_id}");
            Ok(())
        }
        AgentCommand::Logout { id } => {
            let client = connect(home).await?;
            client.logout_agent(id.clone()).await?;
            println!("logged out agent {id}");
            Ok(())
        }
        AgentCommand::Sessions { id, json } => {
            let client = connect(home).await?;
            let sessions = client.list_agent_sessions(id.clone()).await?;
            if json {
                print_json(&sessions)?;
            } else if let Some(arr) = sessions.as_array() {
                let rows = arr
                    .iter()
                    .map(|session| {
                        let ix = field(session, "interaction");
                        let ix_short = match ix.as_str() {
                            "writable" => "W".into(),
                            "read_only" => "R".into(),
                            other => other.to_string(),
                        };
                        let sid = {
                            let a = field(session, "agent_session_id");
                            if a.is_empty() || a == "-" {
                                field(session, "sessionId")
                            } else {
                                a
                            }
                        };
                        vec![
                            sid,
                            ix_short,
                            field(session, "space"),
                            field(session, "in_hub_before"),
                            field(session, "conv_id"),
                            field(session, "title"),
                        ]
                    })
                    .collect();
                print_table(&["SESSION", "IX", "SPACE", "IN_HUB", "CONV", "TITLE"], rows);
            } else {
                print_json(&sessions)?;
            }
            Ok(())
        }
    }
}

pub(crate) async fn handle_proxy(home: &Path, command: ProxyCommand) -> Result<()> {
    match command {
        ProxyCommand::Add(args) => {
            let id = args.id.clone();
            let config = build_proxy_config(&args)?;
            let client = connect(home).await?;
            client.register_proxy(id.clone(), config).await?;
            println!("registered proxy {id}");
            Ok(())
        }
        ProxyCommand::Remove { id } => {
            let client = connect(home).await?;
            client.remove_proxy(id.clone()).await?;
            println!("removed proxy {id}");
            Ok(())
        }
        ProxyCommand::List { json } => {
            let client = connect(home).await?;
            let proxies = client.list_proxies().await?;
            print_proxy_list(&proxies, json)
        }
    }
}

pub(crate) async fn handle_conversation(home: &Path, command: ConversationCommand) -> Result<()> {
    match command {
        ConversationCommand::Create(args) => {
            use acp_hub::progress::{ProgressStage, ProgressTracker};
            let mut progress = ProgressTracker::new();
            emit_progress(&progress.stage(ProgressStage::DaemonConnect), args.json);
            let client = connect(home).await?;
            let cwd = resolve_conversation_cwd(args.cwd)?;
            let mcp_servers = read_mcp_servers(&args.mcp_server_json)?;
            let additional_directories = args
                .additional_directories
                .into_iter()
                .map(|path| resolve_existing_directory(&path))
                .collect::<Result<Vec<_>>>()?;
            emit_progress(&progress.stage(ProgressStage::SessionOp), args.json);
            let created = client
                .create_conversation(CreateConversationParams {
                    agent_id: args.agent_id,
                    cwd: Some(cwd),
                    agent_session_id: args.agent_session_id,
                    mcp_servers,
                    additional_directories,
                })
                .await?;
            let (end, timings) = progress.finish();
            emit_progress(&end, args.json);
            if args.json {
                let mut value = serde_json::to_value(&created)?;
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("timings".into(), serde_json::to_value(&timings)?);
                }
                print_json(&value)?;
            } else {
                eprintln!(
                    "[acp-hub] timings total_ms={} session_ms={:?}",
                    timings.total_ms, timings.session_ms
                );
                println!("{}", created.conv_id);
            }
            Ok(())
        }
        ConversationCommand::Delete {
            conv_id,
            local_only,
        } => {
            let client = connect(home).await?;
            client
                .delete_conversation(conv_id.clone(), local_only)
                .await?;
            println!("deleted conversation {conv_id}");
            Ok(())
        }
        ConversationCommand::Close { conv_id } => {
            let client = connect(home).await?;
            client.close_conversation(conv_id.clone()).await?;
            println!("closed conversation {conv_id}");
            Ok(())
        }
        ConversationCommand::List {
            agent_id,
            include_all,
            workbench,
            status,
            interaction,
            limit,
            offset,
            json,
        } => {
            let client = connect(home).await?;
            use acp_hub::hub::ListConversationsParams;
            // Default workbench on; --all or --status turn default workbench off unless --workbench.
            let params = ListConversationsParams {
                agent_id,
                workbench: if include_all || status.is_some() {
                    workbench
                } else {
                    true
                },
                include_imported: include_all,
                status,
                interaction,
                limit,
                offset,
            };
            let conversations = client.list_conversations_filtered(params).await?;
            print_conversation_list(&conversations, json)
        }
        ConversationCommand::Show { conv_id, raw, json } => {
            let client = connect(home).await?;
            let shown = client.show_conversation(conv_id.clone(), raw).await?;
            if json {
                print_json(&shown)?;
            } else {
                if let Some(conversation) = shown.get("conversation") {
                    print_conversation_detail(conversation)?;
                } else {
                    println!("conversation {conv_id}");
                }
                if let Some(transcript) = shown.get("transcript") {
                    print_transcript(transcript)?;
                }
            }
            Ok(())
        }
    }
}

pub(crate) async fn handle_send(home: &Path, args: SendArgs) -> Result<()> {
    use acp_hub::progress::{ProgressStage, ProgressTracker};

    let prompt_text = match (args.text, args.stdin) {
        (Some(text), false) => text,
        (None, true) => {
            let mut input = String::new();
            tokio::io::stdin()
                .take((MAX_STDIN_BYTES + 1) as u64)
                .read_to_string(&mut input)
                .await
                .context("reading stdin")?;
            if input.len() > MAX_STDIN_BYTES {
                bail!("stdin prompt exceeds {MAX_STDIN_BYTES} bytes");
            }
            input
        }
        _ => bail!("choose exactly one of --text or --stdin"),
    };

    let mut progress = ProgressTracker::new();
    emit_progress(&progress.stage(ProgressStage::DaemonConnect), args.json);

    let conv_id = args.conv_id.clone();
    let client = connect(home).await?;
    let params = args
        .params
        .into_iter()
        .map(|(config_id, value)| ConfigParam { config_id, value })
        .collect();
    let send_params = SendPromptParams {
        conv_id: conv_id.clone(),
        prompt: vec![ContentBlock::Text(TextContent::new(prompt_text))],
        params,
        mode_id: args.mode_id,
    };

    emit_progress(&progress.stage(ProgressStage::Prompt), args.json);
    let result = client.send_prompt(send_params).await?;
    emit_new_message_pages(
        &client,
        &conv_id,
        &result.run_id,
        result.prompt_seq,
        args.json,
    )
    .await?;

    let (end, timings) = progress.finish();
    emit_progress(&end, args.json);

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "type": "final",
                "convId": result.conv_id,
                "runId": result.run_id,
                "stopReason": result.stop_reason,
                "promptSeq": result.prompt_seq,
                "timings": timings,
            }))?
        );
    } else {
        eprintln!(
            "[acp-hub] timings total_ms={} prompt_ms={:?}",
            timings.total_ms, timings.prompt_ms
        );
        println!(
            "final: conv={} run={} stop_reason={}",
            result.conv_id, result.run_id, result.stop_reason
        );
    }
    Ok(())
}

pub(crate) async fn handle_doctor(home: &Path, json: bool) -> Result<()> {
    use acp_hub::endpoint::{PermissionPolicy, Registry};
    use acp_hub::hub::PERMISSION_POLICY_REJECT_HINT;
    use acp_hub::store::Store;

    let mut checks = Vec::new();
    match Registry::load(home) {
        Ok(reg) => {
            if reg.agents.is_empty() {
                checks.push(json!({
                    "id": "agents_empty",
                    "severity": "warn",
                    "message": "no agents registered; next: agent add <id> --command …",
                }));
            } else {
                for (id, cfg) in &reg.agents {
                    if cfg.permission_policy == PermissionPolicy::Reject {
                        checks.push(json!({
                            "id": "permission_policy_reject",
                            "severity": "warn",
                            "agentId": id,
                            "message": PERMISSION_POLICY_REJECT_HINT,
                        }));
                    }
                }
                checks.push(json!({
                    "id": "agents_present",
                    "severity": "info",
                    "message": format!("{} agent(s) registered; next: agent inspect <id> --probe", reg.agents.len()),
                }));
                // PHASE4: agent-cache-empty info without rewriting registry.
                match Store::open(home) {
                    Ok(store) => {
                        for id in reg.agents.keys() {
                            match store.agent_cache(id) {
                                Ok(None) => {
                                    checks.push(json!({
                                        "id": "agent_cache_empty",
                                        "severity": "info",
                                        "agentId": id,
                                        "message": format!(
                                            "capability cache empty; next: agent inspect {id} --probe"
                                        ),
                                    }));
                                }
                                Ok(Some(_)) => {}
                                Err(err) => {
                                    checks.push(json!({
                                        "id": "agent_cache_error",
                                        "severity": "warn",
                                        "agentId": id,
                                        "message": format!("cannot read agent_cache: {err}"),
                                    }));
                                }
                            }
                        }
                    }
                    Err(err) => {
                        checks.push(json!({
                            "id": "store_open_error",
                            "severity": "warn",
                            "message": format!("cannot open hub store for cache scan: {err}"),
                        }));
                    }
                }
            }
        }
        Err(err) => {
            checks.push(json!({
                "id": "registry_error",
                "severity": "warn",
                "message": format!("cannot load agents.json: {err}; next: agent add"),
            }));
        }
    }

    let journey = [
        "1. acp-hub (on PATH) / cargo install --path crates/cli",
        "2. agent add <id> --command …",
        "3. agent inspect <id> --probe",
        "4. conv create <id> --cwd <abs>",
        "5. send <conv_id> --text \"…\"",
        "6. conv show <conv_id>  |  conv list  |  search \"…\"",
        "7. agent sessions <id> (museum RO) → bind only for writable ACP history; IDE = show only / new create to work",
    ];

    if json {
        print_json(&json!({
            "checks": checks,
            "journey": journey,
        }))?;
    } else {
        println!("acp-hub doctor — operator journey (G.0)");
        for line in journey {
            println!("  {line}");
        }
        println!();
        println!("checks:");
        if checks.is_empty() {
            println!("  (none)");
        } else {
            for c in &checks {
                let sev = field(c, "severity");
                let msg = field(c, "message");
                let agent = field(c, "agentId");
                if agent.is_empty() || agent == "-" {
                    println!("  [{sev}] {msg}");
                } else {
                    println!("  [{sev}] {agent}: {msg}");
                }
            }
        }
        println!();
        println!("note: doctor never rewrites agents.json (no silent reject→auto-allow).");
    }
    Ok(())
}

fn emit_progress(event: &acp_hub::progress::ProgressEvent, json_output: bool) {
    if json_output {
        let _ = writeln!(
            std::io::stderr(),
            "{}",
            serde_json::to_string(event).unwrap_or_default()
        );
    } else {
        eprintln!(
            "{}",
            acp_hub::progress::ProgressTracker::human_stage_line(&event.stage)
        );
    }
}

pub(crate) async fn handle_param(home: &Path, command: ParamCommand) -> Result<()> {
    match command {
        ParamCommand::List { conv_id } => {
            let client = connect(home).await?;
            let snapshot = client.get_config(conv_id).await?;
            print_config_section(snapshot.config_options.as_ref(), "No config options")
        }
        ParamCommand::Set {
            conv_id,
            config_id,
            value,
        } => {
            let client = connect(home).await?;
            client
                .set_param(conv_id.clone(), config_id.clone(), value.clone())
                .await?;
            println!("set {config_id}={value} for {conv_id}");
            Ok(())
        }
    }
}

pub(crate) async fn handle_mode(home: &Path, command: ModeCommand) -> Result<()> {
    match command {
        ModeCommand::List { conv_id } => {
            let client = connect(home).await?;
            let snapshot = client.get_config(conv_id).await?;
            print_config_section(snapshot.modes.as_ref(), "No modes")
        }
        ModeCommand::Set { conv_id, mode_id } => {
            let client = connect(home).await?;
            client.set_mode(conv_id.clone(), mode_id.clone()).await?;
            println!("set mode {mode_id} for {conv_id}");
            Ok(())
        }
    }
}

pub(crate) async fn handle_cancel(home: &Path, conv_id: String) -> Result<()> {
    let client = connect(home).await?;
    let cancelled = client.cancel(conv_id).await?;
    if cancelled.requested {
        if let Some(run_id) = cancelled.run_id {
            println!(
                "requested cancellation for {} run {}",
                cancelled.conv_id, run_id
            );
        } else {
            println!("requested cancellation for {}", cancelled.conv_id);
        }
    } else {
        println!("no active run for {}", cancelled.conv_id);
    }
    Ok(())
}

pub(crate) async fn handle_search(home: &Path, args: SearchArgs) -> Result<()> {
    let client = connect(home).await?;
    let results = client
        .search(SearchParams {
            query: args.query,
            agent_id: args.agent_id,
            conv_id: args.conv_id,
            limit: args.limit,
            offset: args.offset,
        })
        .await?;
    if args.json {
        print_json(&results)
    } else {
        print_search_results(&results)
    }
}

async fn connect(home: &Path) -> Result<HubClient> {
    Ok(HubClient::connect_or_spawn(home).await?)
}

pub(crate) fn build_agent_config(args: &AgentAddArgs) -> Result<AgentEndpointConfig> {
    if let Some(path) = &args.json_file {
        return read_json_config(path);
    }

    let transport = match args.transport_type {
        AgentTransportKind::Stdio => AgentTransport::Stdio {
            command: args
                .command
                .clone()
                .context("--command is required for --type stdio")?,
            args: args.args.clone(),
            env: kv_map(&args.env),
        },
        AgentTransportKind::Http => AgentTransport::Http {
            url: args
                .url
                .clone()
                .context("--url is required for --type http")?,
            headers: kv_map(&args.headers),
        },
        AgentTransportKind::Ws => AgentTransport::WebSocket {
            url: args
                .url
                .clone()
                .context("--url is required for --type ws")?,
            headers: kv_map(&args.headers),
        },
    };

    let allowed_roots = args
        .allowed_roots
        .iter()
        .map(|path| resolve_existing_directory(path))
        .collect::<Result<Vec<_>>>()?;

    let (permission_policy, allow_read, allow_write, allow_terminal) = if args.sandbox {
        (PermissionPolicy::Reject, false, false, false)
    } else {
        (
            args.permission_policy.into(),
            args.allow_read,
            args.allow_write,
            args.allow_terminal,
        )
    };

    Ok(AgentEndpointConfig {
        transport,
        proxy_chain: args.proxy_chain.clone(),
        permission_policy,
        client_capabilities: ClientCapabilityConfig {
            fs: acp_hub::endpoint::FsConfig {
                read_text_file: allow_read,
                write_text_file: allow_write,
                allowed_roots,
            },
            terminal: allow_terminal,
        },
    })
}

fn build_proxy_config(args: &ProxyAddArgs) -> Result<ProxyEndpointConfig> {
    if let Some(path) = &args.json_file {
        return read_json_config(path);
    }
    Ok(ProxyEndpointConfig {
        transport: ProxyTransport::Stdio {
            command: args
                .command
                .clone()
                .context("--command is required for proxy add")?,
            args: args.args.clone(),
            env: kv_map(&args.env),
        },
    })
}

fn read_json_config<T>(path: &Path) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing JSON from {}", path.display()))?;
    let config = value.get("config").cloned().unwrap_or(value);
    Ok(serde_json::from_value(config)?)
}

fn kv_map(values: &[(String, String)]) -> BTreeMap<String, String> {
    values.iter().cloned().collect()
}

fn read_mcp_servers(
    paths: &[PathBuf],
) -> Result<Vec<agent_client_protocol::schema::v1::McpServer>> {
    paths.iter().map(|path| read_json_config(path)).collect()
}

/// Collect run-scoped Store rows then emit the **same merged transcript** as
/// `conv show` (SYSTEM §F.3 / PHASE2: send end-state shares show merge).
async fn emit_new_message_pages(
    client: &HubClient,
    conv_id: &str,
    run_id: &str,
    after_seq: i64,
    json_output: bool,
) -> Result<()> {
    use acp_hub::store::MessageRow;

    let mut cursor: Option<String> = None;
    let mut rows: Vec<MessageRow> = Vec::new();
    // MessageRow used only as row type; construction via message_row_from_page_item.
    loop {
        let page = client
            .messages_page(MessagesPageParams {
                conv_id: conv_id.to_string(),
                include_audit: false,
                after_seq: Some(after_seq),
                run_id: Some(run_id.to_string()),
                cursor: cursor.clone(),
                limit: 200,
                offset: 0,
            })
            .await?;
        let next_cursor = match page.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(next)) => Some(next.clone()),
            Some(_) => bail!("message page returned invalid nextCursor"),
        };
        if next_cursor.is_some() && next_cursor == cursor {
            // Emit whatever we already collected so the operator saw progress,
            // then fail safely (non-advancing cursor must not loop).
            emit_merged_send_view(&rows, json_output)?;
            bail!("message page cursor did not advance");
        }
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if items.is_empty() && next_cursor.is_none() {
            break;
        }
        for item in items {
            rows.push(message_row_from_page_item(&item)?);
        }
        let Some(next_cursor) = next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    emit_merged_send_view(&rows, json_output)
}

/// Shipped send display path: same merge algorithm as show (`clean_body`, thought/tool
/// collapse) via `merge_transcript_with(..., MergeLimits::send_run())`.
pub(crate) fn emit_merged_send_view(
    rows: &[acp_hub::store::MessageRow],
    json_output: bool,
) -> Result<()> {
    use acp_hub::store::{MergeLimits, merge_transcript_with};

    let view = merge_transcript_with(rows, MergeLimits::send_run());
    for item in &view.items {
        if item.role == "user" {
            continue;
        }
        if item.body_text.trim().is_empty() {
            continue;
        }
        if json_output {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "type": "update",
                    "message": item,
                }))?
            );
        } else {
            let role = &item.role;
            match item.kind.as_deref() {
                None | Some("") => println!("[{role}] {}", item.body_text),
                Some(kind) => println!("[{role}/{kind}] {}", item.body_text),
            }
        }
    }
    Ok(())
}

/// Best-effort page item → MessageRow (daemon fixtures may omit optional fields).
fn message_row_from_page_item(item: &Value) -> Result<acp_hub::store::MessageRow> {
    use acp_hub::store::{MessageRow, MessageSource};

    // Prefer full serde when the daemon returns a complete row.
    if let Ok(row) = serde_json::from_value::<MessageRow>(item.clone()) {
        return Ok(row);
    }

    let seq = item
        .get("seq")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("message page item missing seq"))?;
    let role = item
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant")
        .to_string();
    let body_text = item
        .get("body_text")
        .or_else(|| item.get("bodyText"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let source = item
        .get("source")
        .and_then(Value::as_str)
        .and_then(|s| match s {
            "local_turn" => Some(MessageSource::LocalTurn),
            "load_replay" => Some(MessageSource::LoadReplay),
            "agent_list" => Some(MessageSource::AgentList),
            _ => None,
        })
        .unwrap_or(MessageSource::LocalTurn);
    let kind = item.get("kind").and_then(Value::as_str).map(str::to_string);
    let content = item.get("content").cloned().unwrap_or_else(|| json!({}));
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("msg-{seq}"));
    let conv_id = item
        .get("conv_id")
        .or_else(|| item.get("convId"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let run_id = item
        .get("run_id")
        .or_else(|| item.get("runId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let created_at = item
        .get("created_at")
        .or_else(|| item.get("createdAt"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let current_projection = item
        .get("current_projection")
        .or_else(|| item.get("currentProjection"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    Ok(MessageRow {
        id,
        conv_id,
        run_id,
        source,
        current_projection,
        role,
        kind,
        content,
        body_text,
        seq,
        created_at,
    })
}

fn resolve_conversation_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir().context("resolving caller current directory")?,
    };
    let cwd = dunce::canonicalize(&cwd)
        .with_context(|| format!("resolving conversation cwd {}", cwd.display()))?;
    if !cwd.is_dir() {
        bail!("conversation cwd is not a directory: {}", cwd.display());
    }
    Ok(cwd)
}

fn resolve_existing_directory(path: &Path) -> Result<PathBuf> {
    let path = dunce::canonicalize(path)
        .with_context(|| format!("resolving directory {}", path.display()))?;
    if !path.is_dir() {
        bail!("not a directory: {}", path.display());
    }
    Ok(path)
}
