mod mcp;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy,
    ProxyEndpointConfig, ProxyTransport,
};
use acp_hub::hub::{
    ConfigParam, CreateConversationParams, HubClient, SearchParams, SendPromptParams,
};
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;

#[derive(Debug, Parser)]
#[command(name = "acp-hub", version, about = "ACP Hub daemon and CLI")]
struct Cli {
    /// Hub home directory. Defaults to $ACP_HUB_HOME or ~/.acp-hub.
    #[arg(long, global = true)]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the singleton Hub daemon for a home directory.
    Serve,
    /// Manage registered ACP agent endpoints.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Manage registered ACP proxy endpoints.
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    /// Manage Hub conversations.
    Conv {
        #[command(subcommand)]
        command: ConversationCommand,
    },
    /// Send a prompt to a conversation.
    Send(SendArgs),
    /// Read or set conversation config parameters.
    Param {
        #[command(subcommand)]
        command: ParamCommand,
    },
    /// Read or set conversation modes.
    Mode {
        #[command(subcommand)]
        command: ModeCommand,
    },
    /// Cancel the active run for a conversation.
    Cancel { conv_id: String },
    /// Search stored conversations and messages.
    Search(SearchArgs),
    /// Run the MCP stdio facade.
    Mcp,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// List registered agents.
    List {
        /// Emit redacted JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Register or replace an agent endpoint.
    Add(AgentAddArgs),
    /// Remove an agent endpoint.
    Remove { id: String },
    /// Show one registered agent endpoint.
    Inspect {
        id: String,
        /// Emit redacted JSON instead of pretty text.
        #[arg(long)]
        json: bool,
    },
    /// Authenticate an agent with an advertised auth method id.
    Auth { id: String, method_id: String },
    /// Logout an agent.
    Logout { id: String },
    /// List sessions known to the agent (ACP session/list). Fast: no session/load.
    Sessions {
        id: String,
        /// Also batch-import session messages via session/load.
        #[arg(long)]
        import: bool,
        /// Limit how many sessions to import (only with --import).
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Debug, Args)]
struct AgentAddArgs {
    id: String,
    /// Agent transport type.
    #[arg(long = "type", value_enum, default_value = "stdio")]
    transport_type: AgentTransportKind,
    /// Stdio command. Required for --type stdio unless --json is supplied.
    #[arg(long)]
    command: Option<String>,
    /// Stdio command arguments.
    #[arg(long = "args", value_name = "ARG", num_args = 1..)]
    args: Vec<String>,
    /// Stdio environment entries.
    #[arg(long = "env", value_name = "KEY=VAL", value_parser = parse_key_val)]
    env: Vec<(String, String)>,
    /// HTTP/WebSocket endpoint URL. Required for --type http or --type ws unless --json is supplied.
    #[arg(long)]
    url: Option<String>,
    /// HTTP/WebSocket header entries.
    #[arg(long = "header", value_name = "KEY=VAL", value_parser = parse_key_val)]
    headers: Vec<(String, String)>,
    /// Read the full AgentEndpointConfig from a JSON file.
    #[arg(long = "json", value_name = "FILE")]
    json_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentTransportKind {
    Stdio,
    Http,
    Ws,
}

#[derive(Debug, Subcommand)]
enum ProxyCommand {
    /// Register or replace a proxy endpoint.
    Add(ProxyAddArgs),
    /// Remove a proxy endpoint.
    Remove { id: String },
    /// List registered proxies.
    List {
        /// Emit redacted JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
struct ProxyAddArgs {
    id: String,
    /// Stdio proxy command. Required unless --json is supplied.
    #[arg(long)]
    command: Option<String>,
    /// Stdio command arguments.
    #[arg(long = "args", value_name = "ARG", num_args = 1..)]
    args: Vec<String>,
    /// Stdio environment entries.
    #[arg(long = "env", value_name = "KEY=VAL", value_parser = parse_key_val)]
    env: Vec<(String, String)>,
    /// Read the full ProxyEndpointConfig from a JSON file.
    #[arg(long = "json", value_name = "FILE")]
    json_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ConversationCommand {
    /// Create a new Hub conversation or bind an existing agent session.
    Create(ConversationCreateArgs),
    /// Delete a conversation projection, optionally without deleting the agent session.
    Delete {
        conv_id: String,
        #[arg(long)]
        local_only: bool,
    },
    /// Close the remote ACP session and keep the Hub projection.
    Close { conv_id: String },
    /// List stored conversations.
    List {
        #[arg(long = "agent")]
        agent_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show a conversation and its current messages.
    Show {
        conv_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Send a prompt to this conversation.
    Send(SendArgs),
    /// Search messages and conversations.
    Search(SearchArgs),
    /// Read or set config parameters for a conversation.
    Config {
        #[command(subcommand)]
        command: ParamCommand,
    },
    /// Read or set modes for a conversation.
    Mode {
        #[command(subcommand)]
        command: ModeCommand,
    },
    /// Cancel the active run for this conversation.
    Cancel { conv_id: String },
}

#[derive(Debug, Args)]
struct ConversationCreateArgs {
    agent_id: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    agent_session_id: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("input").required(true).args(["text", "stdin"])))]
struct SendArgs {
    conv_id: String,
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    stdin: bool,
    #[arg(long = "param", value_name = "CONFIG_ID=VALUE", value_parser = parse_key_val)]
    params: Vec<(String, String)>,
    #[arg(long = "mode")]
    mode_id: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ParamCommand {
    /// List config options for a conversation.
    List { conv_id: String },
    /// Set a config option for a conversation.
    Set {
        conv_id: String,
        config_id: String,
        value: String,
    },
}

#[derive(Debug, Subcommand)]
enum ModeCommand {
    /// List modes for a conversation.
    List { conv_id: String },
    /// Set the current mode for a conversation.
    Set { conv_id: String, mode_id: String },
}

#[derive(Debug, Args)]
struct SearchArgs {
    query: String,
    #[arg(long = "agent")]
    agent_id: Option<String>,
    #[arg(long = "conv")]
    conv_id: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let home = match cli.home {
        Some(home) => home,
        None => acp_hub::home_dir()?,
    };

    match cli.command {
        Command::Serve => acp_hub::daemon::serve(home).await?,
        Command::Agent { command } => handle_agent(&home, command).await?,
        Command::Proxy { command } => handle_proxy(&home, command).await?,
        Command::Conv { command } => handle_conversation(&home, command).await?,
        Command::Send(args) => handle_send(&home, args).await?,
        Command::Param { command } => handle_param(&home, command).await?,
        Command::Mode { command } => handle_mode(&home, command).await?,
        Command::Cancel { conv_id } => handle_cancel(&home, conv_id).await?,
        Command::Search(args) => handle_search(&home, args).await?,
        Command::Mcp => mcp::run(home)
            .await
            .map_err(|err| anyhow::anyhow!("{err}"))?,
    }

    Ok(())
}

async fn handle_agent(home: &Path, command: AgentCommand) -> Result<()> {
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
        AgentCommand::Sessions { id, import, limit } => {
            let client = connect(home).await?;
            let sessions = client.list_agent_sessions(id.clone()).await?;
            if let Some(arr) = sessions.as_array() {
                println!("{:<40} {:<20} UPDATED", "SESSION ID", "TITLE");
                println!("{}", "-".repeat(80));
                for s in arr {
                    let sid = s.get("sessionId").and_then(|v| v.as_str()).unwrap_or("?");
                    let title = s.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let updated = s.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("");
                    println!("{:<40} {:<20} {}", sid, title, updated);
                }
                println!("\n[{} sessions]", arr.len());
            } else {
                println!("{sessions:#?}");
            }
            if import {
                eprintln!("\nImporting session messages (limit: {:?})...", limit);
                let result = client.import_agent_sessions(id.clone(), limit).await?;
                if let Some(n) = result.get("imported").and_then(|v| v.as_u64()) {
                    println!("Imported {n} sessions.");
                }
            }
            Ok(())
        }
    }
}

async fn handle_proxy(home: &Path, command: ProxyCommand) -> Result<()> {
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

async fn handle_conversation(home: &Path, command: ConversationCommand) -> Result<()> {
    match command {
        ConversationCommand::Create(args) => {
            let client = connect(home).await?;
            let created = client
                .create_conversation(CreateConversationParams {
                    agent_id: args.agent_id,
                    cwd: args.cwd,
                    agent_session_id: args.agent_session_id,
                    mcp_servers: Vec::new(),
                    additional_directories: Vec::new(),
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
        ConversationCommand::Send(args) => handle_send(home, args).await,
        ConversationCommand::Search(args) => handle_search(home, args).await,
        ConversationCommand::Config { command } => handle_param(home, command).await,
        ConversationCommand::Mode { command } => handle_mode(home, command).await,
        ConversationCommand::Cancel { conv_id } => handle_cancel(home, conv_id).await,
    }
}

async fn handle_send(home: &Path, args: SendArgs) -> Result<()> {
    let prompt_text = match (args.text, args.stdin) {
        (Some(text), false) => text,
        (None, true) => {
            let mut input = String::new();
            tokio::io::stdin()
                .read_to_string(&mut input)
                .await
                .context("reading stdin")?;
            input
        }
        _ => bail!("choose exactly one of --text or --stdin"),
    };

    let conv_id = args.conv_id.clone();
    let client = connect(home).await?;
    let poll_client = connect(home).await?;
    let before = poll_client.messages(conv_id.clone(), false).await?;
    let mut seen = message_ids(&before);
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

    let mut send_task = tokio::spawn(async move { client.send_prompt(send_params).await });
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let result = loop {
        tokio::select! {
            send_result = &mut send_task => {
                let result = send_result.context("send task failed")??;
                if let Ok(messages) = poll_client.messages(conv_id.clone(), false).await {
                    emit_new_messages(&messages, &mut seen, args.json)?;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Ok(messages) = poll_client.messages(conv_id.clone(), false).await {
                    emit_new_messages(&messages, &mut seen, args.json)?;
                }
                break result;
            }
            _ = interval.tick() => {
                if let Ok(messages) = poll_client.messages(conv_id.clone(), false).await {
                    emit_new_messages(&messages, &mut seen, args.json)?;
                }
            }
        }
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "type": "final",
                "convId": result.conv_id,
                "runId": result.run_id,
                "stopReason": result.stop_reason,
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

async fn handle_param(home: &Path, command: ParamCommand) -> Result<()> {
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

async fn handle_mode(home: &Path, command: ModeCommand) -> Result<()> {
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

async fn handle_cancel(home: &Path, conv_id: String) -> Result<()> {
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

async fn handle_search(home: &Path, args: SearchArgs) -> Result<()> {
    let client = connect(home).await?;
    let results = client
        .search(SearchParams {
            query: args.query,
            agent_id: args.agent_id,
            conv_id: args.conv_id,
            limit: args.limit,
            offset: 0,
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

fn build_agent_config(args: &AgentAddArgs) -> Result<AgentEndpointConfig> {
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

    Ok(AgentEndpointConfig {
        transport,
        proxy_chain: Vec::new(),
        permission_policy: PermissionPolicy::default(),
        client_capabilities: ClientCapabilityConfig::default(),
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

fn parse_key_val(s: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| "expected KEY=VAL".to_string())?;
    if key.is_empty() {
        return Err("key must not be empty".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

fn print_agent_list(agents: &Value, json_output: bool) -> Result<()> {
    if json_output {
        print_json(&redacted_value(agents)?)
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

fn print_proxy_list(proxies: &Value, json_output: bool) -> Result<()> {
    if json_output {
        print_json(&redacted_value(proxies)?)
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

fn print_inspected_config(config: &Value, json_output: bool) -> Result<()> {
    let redacted = redacted_value(config)?;
    if json_output {
        print_json(&redacted)
    } else {
        println!("{}", serde_json::to_string_pretty(&redacted)?);
        Ok(())
    }
}

fn print_conversation_list(conversations: &Value, json_output: bool) -> Result<()> {
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

fn print_conversation_detail(conversation: &Value) -> Result<()> {
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

fn print_messages(messages: &Value) -> Result<()> {
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

fn print_search_results(results: &Value) -> Result<()> {
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

fn print_config_section(value: Option<&Value>, empty: &str) -> Result<()> {
    match value {
        Some(value) if !value.is_null() => print_json(value),
        _ => {
            println!("{empty}");
            Ok(())
        }
    }
}

fn emit_new_messages(
    messages: &Value,
    seen: &mut BTreeSet<String>,
    json_output: bool,
) -> Result<()> {
    let Some(items) = messages.as_array() else {
        return Ok(());
    };
    for item in items {
        let id = message_id(item);
        if !seen.insert(id) {
            continue;
        }
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
    Ok(())
}

fn message_ids(messages: &Value) -> BTreeSet<String> {
    messages
        .as_array()
        .into_iter()
        .flatten()
        .map(message_id)
        .collect()
}

fn message_id(message: &Value) -> String {
    field(message, "id").if_empty_then(|| format!("seq:{}", field(message, "seq")))
}

trait EmptyExt {
    fn if_empty_then(self, fallback: impl FnOnce() -> String) -> String;
}

impl EmptyExt for String {
    fn if_empty_then(self, fallback: impl FnOnce() -> String) -> String {
        if self.is_empty() { fallback() } else { self }
    }
}

fn find_object_by_id<'a>(items: &'a Value, id: &str) -> Option<&'a Value> {
    items
        .as_array()?
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
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
            let command = field(transport, "command");
            let args = string_array(transport.get("args"));
            if args.is_empty() {
                command
            } else {
                format!("{command} {}", args.join(" "))
            }
        }
        Some("http") | Some("websocket") => field(transport, "url"),
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

fn field(value: &Value, key: &str) -> String {
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

fn print_table(headers: &[&str], rows: Vec<Vec<String>>) {
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

fn redacted_value<T: Serialize>(value: &T) -> Result<Value> {
    let mut value = serde_json::to_value(value)?;
    redact(&mut value);
    Ok(value)
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => redact_object(map),
        Value::Array(items) => {
            for item in items {
                redact(item);
            }
        }
        _ => {}
    }
}

fn redact_object(map: &mut Map<String, Value>) {
    for (key, value) in map {
        if is_secret_key(key) {
            *value = Value::String("<redacted>".to_string());
        } else {
            redact(value);
        }
    }
}

fn is_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD"]
        .iter()
        .any(|needle| upper.contains(needle))
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
