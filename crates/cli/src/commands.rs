use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, ProxyEndpointConfig,
    ProxyTransport,
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
use crate::output::{
    field, print_agent_list, print_config_section, print_conversation_detail,
    print_conversation_list, print_inspected_config, print_json, print_messages, print_proxy_list,
    print_search_results, print_table,
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
        AgentCommand::Inspect { id, json } => {
            let client = connect(home).await?;
            let inspection = client.inspect_agent(id).await?;
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
                        vec![
                            field(session, "sessionId"),
                            field(session, "title"),
                            field(session, "updatedAt"),
                        ]
                    })
                    .collect();
                print_table(&["SESSION ID", "TITLE", "UPDATED"], rows);
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
            let client = connect(home).await?;
            let cwd = resolve_conversation_cwd(args.cwd)?;
            let mcp_servers = read_mcp_servers(&args.mcp_server_json)?;
            let additional_directories = args
                .additional_directories
                .into_iter()
                .map(|path| resolve_existing_directory(&path))
                .collect::<Result<Vec<_>>>()?;
            let created = client
                .create_conversation(CreateConversationParams {
                    agent_id: args.agent_id,
                    cwd: Some(cwd),
                    agent_session_id: args.agent_session_id,
                    mcp_servers,
                    additional_directories,
                })
                .await?;
            if args.json {
                print_json(&created)?;
            } else {
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
        ConversationCommand::List { agent_id, json } => {
            let client = connect(home).await?;
            let conversations = client.list_conversations(agent_id).await?;
            print_conversation_list(&conversations, json)
        }
        ConversationCommand::Show { conv_id, json } => {
            let client = connect(home).await?;
            let conversations = client.list_conversations(None).await?;
            let conversation = find_object_by_id(&conversations, &conv_id).cloned();
            let messages = client.messages(conv_id.clone(), false).await?;
            if json {
                print_json(&json!({
                    "conversation": conversation,
                    "messages": messages,
                }))?;
            } else {
                if let Some(conversation) = conversation {
                    print_conversation_detail(&conversation)?;
                } else {
                    println!("conversation {conv_id}");
                }
                print_messages(&messages)?;
            }
            Ok(())
        }
    }
}

pub(crate) async fn handle_send(home: &Path, args: SendArgs) -> Result<()> {
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

    let result = client.send_prompt(send_params).await?;
    emit_new_message_pages(
        &client,
        &conv_id,
        &result.run_id,
        result.prompt_seq,
        args.json,
    )
    .await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "type": "final",
                "convId": result.conv_id,
                "runId": result.run_id,
                "stopReason": result.stop_reason,
                "promptSeq": result.prompt_seq,
            }))?
        );
    } else {
        println!(
            "final: conv={} run={} stop_reason={}",
            result.conv_id, result.run_id, result.stop_reason
        );
    }
    Ok(())
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

    Ok(AgentEndpointConfig {
        transport,
        proxy_chain: args.proxy_chain.clone(),
        permission_policy: args.permission_policy.into(),
        client_capabilities: ClientCapabilityConfig {
            fs: acp_hub::endpoint::FsConfig {
                read_text_file: args.allow_read,
                write_text_file: args.allow_write,
                allowed_roots,
            },
            terminal: args.allow_terminal,
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

async fn emit_new_message_pages(
    client: &HubClient,
    conv_id: &str,
    run_id: &str,
    after_seq: i64,
    json_output: bool,
) -> Result<()> {
    let mut cursor: Option<String> = None;
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
            bail!("message page cursor did not advance");
        }
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if items.is_empty() && next_cursor.is_none() {
            return Ok(());
        }
        for item in &items {
            if field(item, "role") == "user" {
                continue;
            }
            let body = field(item, "body_text");
            if body.trim().is_empty() {
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
                let role = field(item, "role");
                let kind = field(item, "kind");
                if kind.is_empty() {
                    println!("[{role}] {body}");
                } else {
                    println!("[{role}/{kind}] {body}");
                }
            }
        }
        let Some(next_cursor) = next_cursor else {
            return Ok(());
        };
        cursor = Some(next_cursor);
    }
}

fn find_object_by_id<'a>(items: &'a Value, id: &str) -> Option<&'a Value> {
    items
        .as_array()?
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
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
