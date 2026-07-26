use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy,
    ProxyEndpointConfig, ProxyTransport,
};
use acp_hub::hub::{
    ConfigParam, CreateConversationParams, HubClient, MessagesPageParams, SearchParams,
    SendPromptParams, ShowConversationParams, WaitRunParams,
};
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::args::{
    AgentAddArgs, AgentCommand, AgentTransportKind, ConversationCommand, ModeCommand, ParamCommand,
    ProxyAddArgs, ProxyCommand, SearchArgs, SendArgs, WaitArgs,
};
use std::io::Write;

use crate::output::{
    field, format_human_done_line, format_human_timings_line, print_agent_list,
    print_agent_list_revealed, print_config_section, print_conversation_detail,
    print_conversation_list, print_inspected_config, print_json, print_proxy_list,
    print_search_results, print_table, print_transcript,
};

const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

use std::sync::atomic::{AtomicBool, Ordering};

static REVEAL_PATHS: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_reveal_paths(enabled: bool) {
    REVEAL_PATHS.store(enabled, Ordering::Relaxed);
}

fn reveal_paths_enabled() -> bool {
    REVEAL_PATHS.load(Ordering::Relaxed)
}

pub(crate) async fn handle_agent(home: &Path, command: AgentCommand) -> Result<()> {
    match command {
        AgentCommand::List { json } => {
            if reveal_paths_enabled() {
                // Local trusted reveal: read agents.json without redaction (UX-RC3-3).
                use acp_hub::endpoint::Registry;
                let reg = Registry::load(home)?;
                let mut map = serde_json::Map::new();
                for (id, cfg) in &reg.agents {
                    map.insert(id.clone(), serde_json::to_value(cfg)?);
                }
                return print_agent_list_revealed(&Value::Object(map), json);
            }
            let client = connect(home).await?;
            let agents = client.list_agents().await?;
            print_agent_list(&agents, json)
        }
        AgentCommand::Add(args) => {
            // rc.6 P0-1 / B-REG-01: cold add must never silent-hang.
            // Bound connect+register together; on timeout/failure, write
            // agents.json locally so the typical path always returns.
            const REGISTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
            let id = args.id.clone();
            let config = build_agent_config(&args)?;
            let register_via_daemon = async {
                let client = connect(home).await?;
                client.register_agent(id.clone(), config.clone()).await?;
                Ok::<(), anyhow::Error>(())
            };
            match tokio::time::timeout(REGISTER_TIMEOUT, register_via_daemon).await {
                Ok(Ok(())) => {
                    println!("registered agent {id}");
                    Ok(())
                }
                Ok(Err(err)) => {
                    if agent_on_disk(home, &id) {
                        println!("registered agent {id}");
                        eprintln!(
                            "note: registry already lists {id} after daemon error ({err}); verify with: agent list"
                        );
                        return Ok(());
                    }
                    // Local write so first-time add still succeeds when daemon is sick.
                    register_agent_local(home, &id, config)?;
                    println!("registered agent {id}");
                    eprintln!(
                        "note: wrote agents.json locally after daemon error; if agent list is empty, stop the hub daemon and retry list"
                    );
                    Ok(())
                }
                Err(_elapsed) => {
                    if agent_on_disk(home, &id) {
                        println!("registered agent {id}");
                        eprintln!(
                            "note: registry write confirmed after {secs}s timeout; verify with: agent list",
                            secs = REGISTER_TIMEOUT.as_secs()
                        );
                        return Ok(());
                    }
                    register_agent_local(home, &id, config)?;
                    println!("registered agent {id}");
                    eprintln!(
                        "note: agent add timed out after {}s; wrote agents.json locally; verify with: agent list (restart daemon if list is empty)",
                        REGISTER_TIMEOUT.as_secs()
                    );
                    Ok(())
                }
            }
        }
        AgentCommand::Remove { id } => {
            let client = connect(home).await?;
            client.remove_agent(id.clone()).await?;
            println!("removed agent {id}");
            Ok(())
        }
        AgentCommand::Inspect { id, probe, json } => {
            let client = connect(home).await?;
            let mut inspection = client.inspect_agent_probe(id.clone(), probe).await?;
            if reveal_paths_enabled() {
                use acp_hub::endpoint::Registry;
                if let Ok(reg) = Registry::load(home)
                    && let Some(cfg) = reg.agents.get(&id)
                    && let Some(obj) = inspection.as_object_mut()
                {
                    obj.insert("config".into(), serde_json::to_value(cfg)?);
                    obj.insert("pathsRevealed".into(), json!(true));
                }
            }
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
        AgentCommand::Sessions {
            id,
            all,
            limit,
            json,
        } => {
            let client = connect(home).await?;
            let sessions = client.list_agent_sessions(id.clone()).await?;
            if json {
                // CONTRACT §4.1: JSON = full RPC result
                print_json(&sessions)?;
            } else if let Some(arr) = sessions.as_array() {
                if arr.is_empty() {
                    println!("No remote sessions (museum empty). Create with: conv create {id}");
                } else {
                    let mut ranked: Vec<&Value> = arr.iter().collect();
                    ranked.sort_by_key(|s| {
                        let in_hub = s
                            .get("in_hub_before")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let space = field(s, "space");
                        let title = field(s, "title");
                        (
                            if in_hub { 0 } else { 1 },
                            match space.as_str() {
                                "acp" => 0,
                                "cli" => 1,
                                "ide" => 2,
                                _ => 3,
                            },
                            if title.is_empty() || title == "-" {
                                1
                            } else {
                                0
                            },
                        )
                    });
                    let total = ranked.len();
                    let slice: Vec<&Value> = if all {
                        ranked
                    } else {
                        ranked.into_iter().take(limit.max(1)).collect()
                    };
                    if !all && total > slice.len() {
                        println!(
                            "showing {} of {total} sessions (prefer in-hub/acp). Use --all for museum.",
                            slice.len()
                        );
                    }
                    let rows = slice
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
                            let sid = if sid.chars().count() > 24 {
                                format!("{}...", sid.chars().take(21).collect::<String>())
                            } else {
                                sid
                            };
                            let title = field(session, "title");
                            let title = if title.chars().count() > 36 {
                                format!("{}...", title.chars().take(33).collect::<String>())
                            } else {
                                title
                            };
                            vec![
                                sid,
                                ix_short,
                                field(session, "space"),
                                field(session, "in_hub_before"),
                                field(session, "conv_id"),
                                title,
                            ]
                        })
                        .collect();
                    print_table(&["SESSION", "IX", "SPACE", "IN_HUB", "CONV", "TITLE"], rows);
                }
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
                    "{}",
                    format_human_timings_line(timings.total_ms, None, timings.session_ms)
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
            let mode = client
                .delete_conversation(conv_id.clone(), local_only)
                .await?;
            match mode {
                acp_hub::hub::DeleteMode::Local => {
                    println!("deleted conversation {conv_id} (local-only)");
                }
                acp_hub::hub::DeleteMode::Remote => {
                    println!("deleted conversation {conv_id}");
                }
                acp_hub::hub::DeleteMode::LocalFallback => {
                    println!(
                        "deleted conversation {conv_id} locally (agent has no session delete)"
                    );
                }
            }
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
        ConversationCommand::Show {
            conv_id,
            raw,
            json,
            run_id,
            from_seq,
            to_seq,
            tail,
            head,
            kinds,
            no_tools,
            max_chars,
        } => {
            let client = connect(home).await?;
            let params = ShowConversationParams {
                conv_id: conv_id.clone(),
                raw,
                run_id,
                from_seq,
                to_seq,
                tail,
                head,
                kinds,
                no_tools,
                max_chars,
            };
            let shown = client.show_conversation_params(params).await?;
            if json {
                print_json(&shown)?;
            } else {
                if let Some(conversation) = shown.get("conversation") {
                    print_conversation_detail(conversation)?;
                    let status = field(conversation, "status");
                    let phase = field(conversation, "phase");
                    if status == "deleted" || phase == "deleted" {
                        println!(
                            "note: soft-deleted tombstone — full transcript retained for audit; use search/show to read history (no --purge yet)"
                        );
                        println!();
                    }
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

    let wait = args.should_wait();
    let json_mode = args.json;
    let conv_id = args.conv_id.clone();
    let mode_id = args.mode_id.clone();
    let param_pairs = args.params.clone();
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
    emit_progress(&progress.stage(ProgressStage::DaemonConnect), json_mode);

    let client = connect_with_retry(home).await?;
    let params = param_pairs
        .into_iter()
        .map(|(config_id, value)| ConfigParam { config_id, value })
        .collect();
    let send_params = SendPromptParams {
        conv_id: conv_id.clone(),
        prompt: vec![ContentBlock::Text(TextContent::new(prompt_text))],
        params,
        mode_id,
        wait,
    };

    emit_progress(&progress.stage(ProgressStage::Prompt), json_mode);
    // rc.6 P0-3: one retry when daemon drops mid-send accept.
    let result = match client.send_prompt(send_params.clone()).await {
        Ok(r) => r,
        Err(err) => {
            let msg = err.to_string();
            let retriable = msg.contains("daemon")
                || msg.contains("connection is closed")
                || msg.contains("connection reader stopped");
            if !retriable {
                return Err(err.into());
            }
            eprintln!("note: daemon connection lost during send; retrying once…");
            let client = connect(home).await?;
            client.send_prompt(send_params).await?
        }
    };

    // UX-CORE accepted path: no post-hoc dump.
    if !wait || result.busy.as_deref() == Some("running") {
        let (end, timings) = progress.finish();
        emit_progress(&end, json_mode);
        if json_mode {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "type": "accepted",
                    "convId": result.conv_id,
                    "runId": result.run_id,
                    "promptSeq": result.prompt_seq,
                    "busy": "running",
                    "timings": timings,
                }))?
            );
        } else {
            println!(
                "accepted run={} prompt_seq={} busy=running",
                result.run_id, result.prompt_seq
            );
        }
        return Ok(());
    }

    emit_new_message_pages(
        &client,
        &conv_id,
        &result.run_id,
        result.prompt_seq,
        json_mode,
    )
    .await?;

    let (end, timings) = progress.finish();
    emit_progress(&end, json_mode);

    if json_mode {
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
            "{}",
            format_human_timings_line(timings.total_ms, timings.prompt_ms, None)
        );
        println!(
            "{}",
            format_human_done_line(&result.stop_reason, timings.total_ms)
        );
    }
    Ok(())
}

/// UX-CORE wait: Store-poll with **incremental emit each poll** (V3 attach).
///
/// MCP may batch via `HubClient::wait_run`; CLI must stream while in-flight.
pub(crate) async fn handle_wait(home: &Path, args: WaitArgs) -> Result<()> {
    let client = connect(home).await?;
    let started = std::time::Instant::now();
    let json_mode = args.json;
    let result = client
        .wait_run_with_emit(
            WaitRunParams {
                conv_id: args.conv_id.clone(),
                run_id: args.run_id,
                prefer_last: args.last,
                since_seq: args.since_seq,
                timeout_secs: args.timeout,
            },
            |views| {
                for item in views {
                    if json_mode {
                        // Best-effort: ignore serialize errors mid-stream (final still returns).
                        if let Ok(line) = serde_json::to_string(&json!({
                            "type": "message",
                            "seq": item.seq,
                            "role": item.role,
                            "kind": item.kind,
                            "bodyText": item.body_text,
                        })) {
                            println!("{line}");
                        }
                    } else {
                        let line = crate::output::format_human_show_line(
                            &item.role,
                            item.kind.as_deref(),
                            &item.body_text,
                        );
                        if !line.is_empty() {
                            println!("{line}");
                        }
                    }
                }
            },
        )
        .await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "type": "final",
                "convId": result.conv_id,
                "runId": result.run.run_id,
                "status": result.run.status,
                "stopReason": result.run.stop_reason,
            }))?
        );
    } else {
        let reason = result
            .run
            .stop_reason
            .as_deref()
            .unwrap_or(&result.run.status);
        println!(
            "{}",
            format_human_done_line(reason, started.elapsed().as_millis() as u64)
        );
    }
    // Terminal observed (including failed) → process exit 0 (UX-CORE Q7).
    Ok(())
}

pub(crate) async fn handle_doctor(home: &Path, json: bool) -> Result<()> {
    use acp_hub::endpoint::{PermissionPolicy, Registry};
    use acp_hub::hub::PERMISSION_POLICY_REJECT_HINT;
    use acp_hub::store::Store;

    // ASCII-safe copy for Windows consoles that are not UTF-8 (UX-RC3-5).
    let mut checks = Vec::new();
    match Registry::load(home) {
        Ok(reg) => {
            if reg.agents.is_empty() {
                checks.push(json!({
                    "id": "agents_empty",
                    "severity": "warn",
                    "message": "no agents registered; next: agent add <id> --command ...",
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
                    "message": format!(
                        "{} agent(s) registered; next: conv create <id> --cwd <abs> (or agent sessions <id>)",
                        reg.agents.len()
                    ),
                }));
                // Cache-aware next steps (UX-RC3-4): only nudge probe when empty.
                match Store::open(home) {
                    Ok(store) => {
                        let mut empty = 0usize;
                        let mut hot = 0usize;
                        for id in reg.agents.keys() {
                            match store.agent_cache(id) {
                                Ok(None) => {
                                    empty += 1;
                                    checks.push(json!({
                                        "id": "agent_cache_empty",
                                        "severity": "info",
                                        "agentId": id,
                                        "message": format!(
                                            "capability cache empty; next: agent inspect {id} --probe"
                                        ),
                                    }));
                                }
                                Ok(Some(_)) => {
                                    hot += 1;
                                    checks.push(json!({
                                        "id": "agent_cache_ready",
                                        "severity": "info",
                                        "agentId": id,
                                        "message": format!(
                                            "capability cache present; probe optional (agent inspect {id} --probe to refresh)"
                                        ),
                                    }));
                                }
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
                        checks.push(json!({
                            "id": "agent_cache_summary",
                            "severity": "info",
                            "message": format!(
                                "capability cache: {hot} ready, {empty} empty (probe only needed when empty)"
                            ),
                        }));
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

    checks.push(json!({
        "id": "lifecycle_hint",
        "severity": "info",
        "message": "lifecycle: cancel = stop active run; close = end remote session keep local; delete = remove hub projection (auto local if agent has no session delete). Paths redacted by default; use --reveal-paths for local command paths.",
    }));
    checks.push(json!({
        "id": "progress_channels",
        "severity": "info",
        "message": "channels: human progress/timings on stderr ([acp-hub] stage=...); conversation body on stdout. JSON mode: progress NDJSON on stderr, final/result on stdout.",
    }));
    checks.push(json!({
        "id": "wait_hint",
        "severity": "info",
        "message": "wait defaults to in-flight run only; finished runs: wait --run <id> or wait --last",
    }));

    let journey = [
        "1. agent add <id> --command ...",
        "2. conv create <id> --cwd <abs>",
        "3. send <conv_id> --text \"...\"   (or --no-wait then wait <conv_id>)",
        "4. conv show <conv_id>  |  wait <conv_id>  |  cancel <conv_id>",
        "5. conv close <conv_id> ; conv delete <conv_id>  (default local ok without session delete)",
    ];

    if json {
        print_json(&json!({
            "checks": checks,
            "surface": ["send", "wait", "show", "cancel"],
            "journey": journey,
        }))?;
    } else {
        // ASCII hyphen only (Windows consoles may garble U+2014 em dash).
        println!("acp-hub doctor - UX-CORE surface: send / wait / show / cancel");
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
        println!("note: doctor never rewrites agents.json (no silent reject->auto-allow).");
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
        ParamCommand::List { conv_id, json } => {
            let client = connect(home).await?;
            let snapshot = client.get_config(conv_id).await?;
            if json {
                print_json(&snapshot.config_options)?;
            } else {
                print_config_section(snapshot.config_options.as_ref(), "No config options")?;
            }
            Ok(())
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
        ModeCommand::List { conv_id, json } => {
            let client = connect(home).await?;
            let snapshot = client.get_config(conv_id).await?;
            if json {
                print_json(&snapshot.modes)?;
            } else {
                print_config_section(snapshot.modes.as_ref(), "No modes")?;
            }
            Ok(())
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
    // rc.6 P0-2: CLI hard bound even if daemon/agent path stalls.
    const CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
    let cancelled = match tokio::time::timeout(CANCEL_TIMEOUT, async {
        let client = connect_with_retry(home).await?;
        Ok::<_, anyhow::Error>(client.cancel(conv_id.clone()).await?)
    })
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(err)) => return Err(err),
        Err(_elapsed) => {
            bail!(
                "cancel timed out after {}s for {conv_id}; hub may still be cancelling — try: wait --last {conv_id}  or  cancel {conv_id} again",
                CANCEL_TIMEOUT.as_secs()
            );
        }
    };
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

/// rc.6 P0-3: one automatic reconnect after daemon_unavailable / closed connection.
async fn connect_with_retry(home: &Path) -> Result<HubClient> {
    match connect(home).await {
        Ok(c) => Ok(c),
        Err(err) => {
            let is_daemon = err
                .downcast_ref::<acp_hub::HubError>()
                .map(|e| matches!(e, acp_hub::HubError::DaemonUnavailable(_)))
                .unwrap_or_else(|| {
                    err.chain().any(|c| {
                        c.downcast_ref::<acp_hub::HubError>()
                            .is_some_and(|e| matches!(e, acp_hub::HubError::DaemonUnavailable(_)))
                    })
                });
            if !is_daemon {
                return Err(err);
            }
            eprintln!("note: daemon unavailable; reconnecting once…");
            connect(home).await
        }
    }
}

fn agent_on_disk(home: &Path, id: &str) -> bool {
    use acp_hub::endpoint::Registry;
    Registry::load(home)
        .map(|r| r.agents.contains_key(id))
        .unwrap_or(false)
}

fn register_agent_local(home: &Path, id: &str, config: AgentEndpointConfig) -> Result<()> {
    use acp_hub::endpoint::Registry;
    let mut reg = Registry::load(home).unwrap_or_default();
    reg.register_agent(id.to_string(), config)?;
    reg.save(home)?;
    Ok(())
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
/// Human mode uses compact tool/thought lines (UX-RC3-1); JSON keeps full view nodes.
pub(crate) fn emit_merged_send_view(
    rows: &[acp_hub::store::MessageRow],
    json_output: bool,
) -> Result<()> {
    use acp_hub::store::{MergeLimits, merge_transcript_with};

    let view = merge_transcript_with(rows, MergeLimits::send_run());
    let mut prev_was_plain_reply = false;
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
            continue;
        }
        let kind = item.kind.as_deref();
        let line = crate::output::format_human_transcript_line(&item.role, kind, &item.body_text);
        if line.is_empty() {
            continue;
        }
        let is_tool = matches!(kind, Some("tool_call" | "tool_call_update"));
        let is_thought = matches!(kind, Some("thought"));
        let is_plain = !is_tool && !is_thought;
        // Natural spacing: blank line before tool block after main reply.
        if is_tool && prev_was_plain_reply {
            println!();
        }
        println!("{line}");
        prev_was_plain_reply = is_plain;
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
