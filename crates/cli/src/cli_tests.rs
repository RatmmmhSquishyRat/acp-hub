use acp_hub::HubError;
use acp_hub::endpoint::PermissionPolicy;
use clap::Parser;

use crate::args::{AgentCommand, Cli, Command};
use crate::commands::build_agent_config;
use crate::output::sanitize_terminal_text;

#[test]
fn phase1_cli_error_lines_use_contract_codes() {
    let ro = HubError::read_only_conversation("c1", "imported_list", "read_only", false);
    assert!(
        ro.phase1_cli_line()
            .starts_with("error: read_only_conversation:")
    );
    let busy = HubError::conversation_busy("c2", "running");
    assert!(
        busy.phase1_cli_line()
            .starts_with("error: conversation_busy:")
    );
    let closed = HubError::ConversationClosed {
        conv_id: "c3".into(),
    };
    assert!(
        closed
            .phase1_cli_line()
            .starts_with("error: conversation_closed:")
    );
    let not_busy = HubError::not_busy("c4");
    assert!(not_busy.phase1_cli_line().starts_with("error: not_busy:"));
}

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
fn table_sanitizer_removes_ansi_and_controls() {
    assert_eq!(
        sanitize_terminal_text("\u{1b}[31mdanger\u{1b}[0m\u{7}"),
        "danger"
    );
}

#[test]
fn agent_registration_defaults_to_usable_local_trust() {
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
    assert_eq!(config.permission_policy, PermissionPolicy::AutoAllow);
    assert!(config.client_capabilities.terminal);
    assert!(config.client_capabilities.fs.read_text_file);
    assert!(config.client_capabilities.fs.write_text_file);
}

#[test]
fn agent_registration_sandbox_tightens_all_capabilities() {
    let cli = Cli::try_parse_from([
        "acp-hub",
        "agent",
        "add",
        "fixture",
        "--command",
        "fixture-agent",
        "--sandbox",
    ])
    .expect("agent add --sandbox parses");
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
