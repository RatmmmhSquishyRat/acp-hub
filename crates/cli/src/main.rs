mod mcp;

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
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
#[cfg(test)]
use serde_json::Map;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

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
    /// List sessions known to the agent (ACP session/list).
    Sessions {
        id: String,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
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
    /// Proxy id to apply, in order. Repeat for a chain.
    #[arg(long = "proxy", value_name = "ID")]
    proxy_chain: Vec<String>,
    /// Permission callback policy.
    #[arg(long, value_enum, default_value = "reject")]
    permission_policy: PermissionPolicyArg,
    /// Advertise fs/read_text_file to the agent.
    #[arg(long)]
    allow_read: bool,
    /// Advertise fs/write_text_file to the agent.
    #[arg(long)]
    allow_write: bool,
    /// Advertise terminal callbacks to the agent.
    #[arg(long)]
    allow_terminal: bool,
    /// Filesystem root allowed for callback access. Repeat for multiple roots.
    #[arg(long = "allow-root", value_name = "PATH")]
    allowed_roots: Vec<PathBuf>,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PermissionPolicyArg {
    Reject,
    AutoCancel,
    AutoAllow,
}

impl From<PermissionPolicyArg> for PermissionPolicy {
    fn from(value: PermissionPolicyArg) -> Self {
        match value {
            PermissionPolicyArg::Reject => Self::Reject,
            PermissionPolicyArg::AutoCancel => Self::AutoCancel,
            PermissionPolicyArg::AutoAllow => Self::AutoAllow,
        }
    }
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
}

#[derive(Debug, Args)]
struct ConversationCreateArgs {
    agent_id: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    agent_session_id: Option<String>,
    /// Additional workspace directory exposed to the ACP agent.
    #[arg(long = "additional-directory", value_name = "PATH")]
    additional_directories: Vec<PathBuf>,
    /// ACP MCP server JSON file. Repeat for multiple servers.
    #[arg(long = "mcp-server-json", value_name = "FILE")]
    mcp_server_json: Vec<PathBuf>,
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
    /// Emit newline-delimited JSON updates followed by one final JSON object.
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
    #[arg(long, default_value_t = 50, value_parser = parse_page_limit)]
    limit: usize,
    /// Result offset for deterministic pagination.
    #[arg(long, default_value_t = 0)]
    offset: usize,
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

async fn handle_send(home: &Path, args: SendArgs) -> Result<()> {
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

fn parse_key_val(s: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| "expected KEY=VAL".to_string())?;
    if key.is_empty() {
        return Err("key must not be empty".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

fn parse_page_limit(s: &str) -> std::result::Result<usize, String> {
    let value = s
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if !(1..=200).contains(&value) {
        return Err("limit must be between 1 and 200".to_string());
    }
    Ok(value)
}

fn read_mcp_servers(
    paths: &[PathBuf],
) -> Result<Vec<agent_client_protocol::schema::v1::McpServer>> {
    paths.iter().map(|path| read_json_config(path)).collect()
}

fn print_agent_list(agents: &Value, json_output: bool) -> Result<()> {
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

fn print_proxy_list(proxies: &Value, json_output: bool) -> Result<()> {
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

fn print_inspected_config(config: &Value, json_output: bool) -> Result<()> {
    if json_output {
        print_json(config)
    } else {
        println!("{}", serde_json::to_string_pretty(config)?);
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

async fn emit_new_message_pages(
    client: &HubClient,
    conv_id: &str,
    run_id: &str,
    after_seq: i64,
    json_output: bool,
) -> Result<()> {
    let mut offset = 0;
    loop {
        let page = client
            .messages_page(MessagesPageParams {
                conv_id: conv_id.to_string(),
                include_audit: false,
                after_seq: Some(after_seq),
                run_id: Some(run_id.to_string()),
                limit: 200,
                offset,
            })
            .await?;
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if items.is_empty() && page.get("nextOffset").is_none_or(Value::is_null) {
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
        let Some(next_offset) = page.get("nextOffset").filter(|value| !value.is_null()) else {
            return Ok(());
        };
        let next_offset = next_offset
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .context("message page returned invalid nextOffset")?;
        if next_offset <= offset {
            bail!("message page cursor did not advance");
        }
        offset = next_offset;
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

#[cfg(test)]
pub(crate) fn redacted_value<T: Serialize>(value: &T) -> Result<Value> {
    let mut value = serde_json::to_value(value)?;
    redact(&mut value);
    Ok(value)
}

#[cfg(test)]
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

#[cfg(test)]
fn redact_object(map: &mut Map<String, Value>) {
    for (key, value) in map {
        let normalized = key.to_ascii_lowercase();
        if matches!(normalized.as_str(), "env" | "headers") {
            redact_secret_map(value);
        } else if normalized == "args" {
            if let Some(values) = value.as_array_mut() {
                for item in values {
                    if item.as_str() != Some("<redacted>") {
                        *item = Value::String("<redacted>".to_string());
                    }
                }
            } else if !value.is_null() {
                *value = Value::String("<redacted>".to_string());
            }
        } else if normalized == "command" {
            if value.is_string() {
                *value = Value::String("<redacted-command>".to_string());
            }
        } else if normalized == "url" {
            if let Some(url) = value.as_str() {
                *value = Value::String(sanitize_url(url));
            }
        } else if is_secret_key(key) {
            *value = Value::String("<redacted>".to_string());
        } else {
            redact(value);
        }
    }
}

#[cfg(test)]
fn is_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    [
        "AUTHORIZATION",
        "COOKIE",
        "CREDENTIAL",
        "KEY",
        "PASSWORD",
        "PRIVATE",
        "SECRET",
        "TOKEN",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

#[cfg(test)]
fn redact_secret_map(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        for item in map.values_mut() {
            *item = Value::String("<redacted>".to_string());
        }
    } else if !value.is_null() {
        *value = Value::String("<redacted>".to_string());
    }
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

fn sanitize_terminal_text(input: &str) -> String {
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

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn search_accepts_offset() {
        let cli = Cli::try_parse_from(["acp-hub", "search", "needle", "--offset", "25"])
            .expect("search command parses");
        let Command::Search(args) = cli.command else {
            panic!("expected search command");
        };
        assert_eq!(args.offset, 25);
    }

    #[test]
    fn redaction_covers_registry_credentials_and_local_commands() {
        let input = json!({
            "transport": {
                "type": "http",
                "url": "https://alice:secret@example.test/acp?token=hidden",
                "headers": {
                    "Authorization": "Bearer hidden",
                    "X-Custom": "also-hidden"
                }
            },
            "env": {
                "DATABASE_URL": "postgres://secret",
                "VISIBLE_NAME": "must-still-be-hidden"
            },
            "command": "C:/Users/example/private/agent.exe",
            "args": ["--token", "hidden"]
        });

        let redacted = redacted_value(&input).expect("redacts");
        let text = serde_json::to_string(&redacted).expect("serializes");
        for secret in [
            "alice",
            "secret",
            "Bearer",
            "also-hidden",
            "postgres",
            "private",
            "--token",
        ] {
            assert!(!text.contains(secret), "leaked {secret}: {text}");
        }
        assert!(text.contains("<redacted>"));
        assert_eq!(
            redacted["transport"]["url"],
            "https://example.test/<redacted>"
        );
    }

    #[test]
    fn table_sanitizer_removes_ansi_and_controls() {
        assert_eq!(
            sanitize_terminal_text("\u{1b}[31mdanger\u{1b}[0m\u{7}"),
            "danger"
        );
    }

    #[test]
    fn agent_registration_defaults_to_least_privilege() {
        let cli = Cli::try_parse_from([
            "acp-hub",
            "agent",
            "add",
            "fixture",
            "--command",
            "fixture-agent",
        ])
        .expect("agent add parses");
        let Command::Agent {
            command: AgentCommand::Add(args),
        } = cli.command
        else {
            panic!("expected agent add");
        };
        let config = build_agent_config(&args).expect("config builds");
        assert_eq!(config.permission_policy, PermissionPolicy::Reject);
        assert!(!config.client_capabilities.terminal);
        assert!(!config.client_capabilities.fs.read_text_file);
        assert!(!config.client_capabilities.fs.write_text_file);
    }
}
