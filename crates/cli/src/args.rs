use std::path::PathBuf;

use acp_hub::endpoint::PermissionPolicy;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "acp-hub",
    version,
    about = "ACP Hub: multi-agent ACP client/conductor CLI",
    long_about = "\
ACP Hub is a local operator CLI (and MCP facade) for talking to ACP agents.

Product surface (UX-CORE): send / wait / show / cancel.

Quick start:
  acp-hub doctor
  acp-hub agent add <id> --command <path-or-bin> ...
  acp-hub conv create <id> --cwd <abs>
  acp-hub send <conv_id> --text \"...\"
  acp-hub send <conv_id> --text \"...\" --no-wait   # then: wait <conv_id>
  acp-hub wait <conv_id>
  acp-hub conv show <conv_id>
  acp-hub cancel <conv_id>

Channels: progress/timings go to stderr ([acp-hub] stage=... or JSON lines);
conversation body and final records go to stdout. Use --json for machine I/O.

Paths in agent list/inspect are redacted by default; pass --reveal-paths for
local trusted debugging of command/url strings.

Version note: four-primitive surface ships in 0.2.1-rc.x GitHub prereleases;
crates.io Latest may lag until a stable 0.2.1."
)]
pub(crate) struct Cli {
    /// Hub home directory. Defaults to $ACP_HUB_HOME or ~/.acp-hub.
    #[arg(long, global = true)]
    pub(crate) home: Option<PathBuf>,

    /// Show real command/path strings for local trusted debugging (not for shared logs).
    #[arg(long, global = true)]
    pub(crate) reveal_paths: bool,

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
    /// Send a prompt to a conversation (default: block until done).
    Send(SendArgs),
    /// Attach to an in-flight or finished run and stream Store updates until terminal.
    Wait(WaitArgs),
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
    /// Operator health + four-primitive surface guidance (UX-CORE).
    Doctor {
        #[arg(long)]
        json: bool,
    },
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
        /// Connect agent to refresh capability cache (Phase 3).
        #[arg(long)]
        probe: bool,
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
        /// Full museum. Default: recent workbench slice (HUMAN-READING).
        #[arg(long)]
        all: bool,
        /// Max human table rows (default 20).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Emit JSON instead of a table (full RPC list).
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
    /// Permission callback policy (default: auto-allow for local trusted use).
    #[arg(long, value_enum, default_value = "auto-allow")]
    pub(crate) permission_policy: PermissionPolicyArg,
    /// Advertise fs/read_text_file to the agent (default: true). Pass false to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub(crate) allow_read: bool,
    /// Advertise fs/write_text_file to the agent (default: true). Pass false to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub(crate) allow_write: bool,
    /// Advertise terminal callbacks to the agent (default: true). Pass false to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub(crate) allow_terminal: bool,
    /// One-shot sandbox: reject permissions and disable fs/terminal callbacks.
    /// Overrides --permission-policy / --allow-* when set.
    #[arg(long, default_value_t = false)]
    pub(crate) sandbox: bool,
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
    /// Delete a conversation (hub projection always; remote session when supported).
    ///
    /// Default succeeds even when the agent has no session/delete (e.g. Cursor):
    /// hub soft-deletes locally and reports local_fallback. Use `--local-only` to
    /// skip remote intentionally. Soft-delete keeps transcript for show/audit.
    Delete {
        conv_id: String,
        /// Skip remote session/delete; only remove hub projection.
        #[arg(long)]
        local_only: bool,
    },
    /// Close the remote ACP session and keep the Hub projection.
    Close { conv_id: String },
    /// List stored conversations (default: workbench only).
    List {
        #[arg(long = "agent")]
        agent_id: Option<String>,
        /// Include pure imported (museum) rows.
        #[arg(long = "all")]
        include_all: bool,
        /// Force workbench filter (AND with other flags).
        #[arg(long)]
        workbench: bool,
        #[arg(long = "status")]
        status: Option<String>,
        #[arg(long = "interaction")]
        interaction: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show a conversation and its current messages (full stream by default).
    Show {
        conv_id: String,
        /// Unmerged Store rows (no thought/tool merge).
        #[arg(long)]
        raw: bool,
        /// Emit JSON envelope with transcript view.
        #[arg(long)]
        json: bool,
        /// Pre-merge filter: only this run's messages.
        #[arg(long = "run")]
        run_id: Option<String>,
        /// Closed seq interval lower bound (use with --to-seq).
        #[arg(long = "from-seq")]
        from_seq: Option<i64>,
        /// Closed seq interval upper bound (use with --from-seq).
        #[arg(long = "to-seq")]
        to_seq: Option<i64>,
        /// Keep only the last N view items.
        #[arg(long)]
        tail: Option<usize>,
        /// Keep only the first N view items.
        #[arg(long)]
        head: Option<usize>,
        /// Filter tokens: user,assistant,thought,tool (comma-separated).
        #[arg(long, value_delimiter = ',')]
        kinds: Vec<String>,
        /// Hide tool lines.
        #[arg(long)]
        no_tools: bool,
        /// Truncate each body to at most N characters.
        #[arg(long = "max-chars")]
        max_chars: Option<usize>,
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
#[command(group(ArgGroup::new("wait_mode").args(["wait", "no_wait"]).multiple(false)))]
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
    /// Block until the run finishes (default).
    #[arg(long, default_value_t = false)]
    pub(crate) wait: bool,
    /// Return after prompt is accepted; use `wait` to attach (UX-CORE).
    #[arg(long = "no-wait", default_value_t = false)]
    pub(crate) no_wait: bool,
    /// Emit newline-delimited JSON updates followed by one final JSON object.
    #[arg(long)]
    pub(crate) json: bool,
}

impl SendArgs {
    /// Effective wait flag: default true unless `--no-wait`.
    pub(crate) fn should_wait(&self) -> bool {
        !self.no_wait
    }
}

#[derive(Debug, Args)]
pub(crate) struct WaitArgs {
    pub(crate) conv_id: String,
    /// Attach to this run (default: current in-flight run).
    #[arg(long = "run")]
    pub(crate) run_id: Option<String>,
    /// If nothing is in-flight, replay the latest finished run (short flag, not complex config).
    #[arg(long)]
    pub(crate) last: bool,
    /// Only emit messages with seq greater than this value.
    #[arg(long = "since-seq")]
    pub(crate) since_seq: Option<i64>,
    /// Fail with code timeout after this many seconds.
    #[arg(long)]
    pub(crate) timeout: Option<u64>,
    /// NDJSON message lines + final object.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ParamCommand {
    /// List config options for a conversation (human table; use --json for raw).
    List {
        conv_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Set a config option for a conversation.
    Set {
        conv_id: String,
        config_id: String,
        value: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ModeCommand {
    /// List modes for a conversation (human table; use --json for raw).
    List {
        conv_id: String,
        #[arg(long)]
        json: bool,
    },
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
