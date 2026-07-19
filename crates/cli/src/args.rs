use std::path::PathBuf;

use acp_hub::endpoint::PermissionPolicy;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "acp-hub", version, about = "ACP Hub daemon and CLI")]
pub(crate) struct Cli {
    /// Hub home directory. Defaults to $ACP_HUB_HOME or ~/.acp-hub.
    #[arg(long, global = true)]
    pub(crate) home: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
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
pub(crate) enum AgentCommand {
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
pub(crate) struct AgentAddArgs {
    pub(crate) id: String,
    /// Agent transport type.
    #[arg(long = "type", value_enum, default_value = "stdio")]
    pub(crate) transport_type: AgentTransportKind,
    /// Stdio command. Required for --type stdio unless --json is supplied.
    #[arg(long)]
    pub(crate) command: Option<String>,
    /// Stdio command arguments.
    #[arg(long = "args", value_name = "ARG", num_args = 1..)]
    pub(crate) args: Vec<String>,
    /// Stdio environment entries.
    #[arg(long = "env", value_name = "KEY=VAL", value_parser = parse_key_val)]
    pub(crate) env: Vec<(String, String)>,
    /// HTTP/WebSocket endpoint URL. Required for --type http or --type ws unless --json is supplied.
    #[arg(long)]
    pub(crate) url: Option<String>,
    /// HTTP/WebSocket header entries.
    #[arg(long = "header", value_name = "KEY=VAL", value_parser = parse_key_val)]
    pub(crate) headers: Vec<(String, String)>,
    /// Proxy id to apply, in order. Repeat for a chain.
    #[arg(long = "proxy", value_name = "ID")]
    pub(crate) proxy_chain: Vec<String>,
    /// Permission callback policy.
    #[arg(long, value_enum, default_value = "reject")]
    pub(crate) permission_policy: PermissionPolicyArg,
    /// Advertise fs/read_text_file to the agent.
    #[arg(long)]
    pub(crate) allow_read: bool,
    /// Advertise fs/write_text_file to the agent.
    #[arg(long)]
    pub(crate) allow_write: bool,
    /// Advertise terminal callbacks to the agent.
    #[arg(long)]
    pub(crate) allow_terminal: bool,
    /// Filesystem root allowed for callback access. Repeat for multiple roots.
    #[arg(long = "allow-root", value_name = "PATH")]
    pub(crate) allowed_roots: Vec<PathBuf>,
    /// Read the full AgentEndpointConfig from a JSON file.
    #[arg(long = "json", value_name = "FILE")]
    pub(crate) json_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum AgentTransportKind {
    Stdio,
    Http,
    Ws,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum PermissionPolicyArg {
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
pub(crate) enum ProxyCommand {
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
pub(crate) struct ProxyAddArgs {
    pub(crate) id: String,
    /// Stdio proxy command. Required unless --json is supplied.
    #[arg(long)]
    pub(crate) command: Option<String>,
    /// Stdio command arguments.
    #[arg(long = "args", value_name = "ARG", num_args = 1..)]
    pub(crate) args: Vec<String>,
    /// Stdio environment entries.
    #[arg(long = "env", value_name = "KEY=VAL", value_parser = parse_key_val)]
    pub(crate) env: Vec<(String, String)>,
    /// Read the full ProxyEndpointConfig from a JSON file.
    #[arg(long = "json", value_name = "FILE")]
    pub(crate) json_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConversationCommand {
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
pub(crate) struct ConversationCreateArgs {
    pub(crate) agent_id: String,
    #[arg(long)]
    pub(crate) cwd: Option<PathBuf>,
    #[arg(long)]
    pub(crate) agent_session_id: Option<String>,
    /// Additional workspace directory exposed to the ACP agent.
    #[arg(long = "additional-directory", value_name = "PATH")]
    pub(crate) additional_directories: Vec<PathBuf>,
    /// ACP MCP server JSON file. Repeat for multiple servers.
    #[arg(long = "mcp-server-json", value_name = "FILE")]
    pub(crate) mcp_server_json: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("input").required(true).args(["text", "stdin"])))]
pub(crate) struct SendArgs {
    pub(crate) conv_id: String,
    #[arg(long)]
    pub(crate) text: Option<String>,
    #[arg(long)]
    pub(crate) stdin: bool,
    #[arg(long = "param", value_name = "CONFIG_ID=VALUE", value_parser = parse_key_val)]
    pub(crate) params: Vec<(String, String)>,
    #[arg(long = "mode")]
    pub(crate) mode_id: Option<String>,
    /// Emit newline-delimited JSON updates followed by one final JSON object.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ParamCommand {
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
pub(crate) enum ModeCommand {
    /// List modes for a conversation.
    List { conv_id: String },
    /// Set the current mode for a conversation.
    Set { conv_id: String, mode_id: String },
}

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    pub(crate) query: String,
    #[arg(long = "agent")]
    pub(crate) agent_id: Option<String>,
    #[arg(long = "conv")]
    pub(crate) conv_id: Option<String>,
    #[arg(long, default_value_t = 50, value_parser = parse_page_limit)]
    pub(crate) limit: usize,
    /// Result offset for deterministic pagination.
    #[arg(long, default_value_t = 0)]
    pub(crate) offset: usize,
    #[arg(long)]
    pub(crate) json: bool,
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
