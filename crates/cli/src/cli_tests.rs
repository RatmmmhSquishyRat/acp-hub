use acp_hub::HubError;
use acp_hub::endpoint::PermissionPolicy;
use acp_hub::store::{MergeLimits, MessageRow, MessageSource, merge_transcript_with};
use clap::Parser;

use crate::args::{AgentCommand, Cli, Command};
use crate::commands::{build_agent_config, emit_merged_send_view};
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

fn thought_row(seq: i64, body: &str) -> MessageRow {
    MessageRow {
        id: format!("m{seq}"),
        conv_id: "c1".into(),
        run_id: Some("r1".into()),
        source: MessageSource::LocalTurn,
        current_projection: true,
        role: "assistant".into(),
        kind: Some("thought".into()),
        content: serde_json::json!({}),
        body_text: body.into(),
        seq,
        created_at: "t".into(),
    }
}

/// SC-13 on the **send** display path: same merge_transcript as show (PHASE2).
#[test]
fn send_path_merges_thought_chunks_like_show() {
    let rows: Vec<_> = (1..=12)
        .map(|i| thought_row(i, &format!("content type text text chunk{i}")))
        .collect();
    // Same merge API the send path uses (send_run limits).
    let view = merge_transcript_with(&rows, MergeLimits::send_run());
    assert_eq!(view.raw_count, 12);
    assert_eq!(view.view_count, 1);
    assert!(!view.truncated);
    assert!(view.items[0].body_text.contains("chunk1"));
    assert!(view.items[0].body_text.contains("chunk12"));
    assert!(
        !view.items[0]
            .body_text
            .to_ascii_lowercase()
            .starts_with("content type")
    );

    // Drive the shipped send renderer (must not panic; JSON path emits one update).
    let ok = emit_merged_send_view(&rows, true);
    assert!(ok.is_ok(), "emit_merged_send_view failed: {ok:?}");
}

#[test]
fn doctor_command_parses() {
    let cli = Cli::try_parse_from(["acp-hub", "doctor", "--json"]).expect("doctor parses");
    assert!(matches!(cli.command, Command::Doctor { json: true }));
}

#[test]
fn reveal_paths_global_flag_parses() {
    let cli = Cli::try_parse_from(["acp-hub", "--reveal-paths", "agent", "list"])
        .expect("reveal-paths parses");
    assert!(cli.reveal_paths);
}

#[test]
fn human_transcript_line_is_natural_cli_not_dialect() {
    use crate::output::format_human_transcript_line;
    let tool = format_human_transcript_line(
        "assistant",
        Some("tool_call"),
        "fc_abc title Edit File kind edit status in_progress",
    );
    // Indented title only — not "tool   Edit File" protocol tags
    assert_eq!(tool, "  Edit File");
    let reply = format_human_transcript_line("assistant", Some("message"), "Creating the file.");
    assert_eq!(reply, "Creating the file.");
    assert!(!reply.starts_with("say"));
    let think =
        format_human_transcript_line("assistant", Some("thought"), "text Planning the edit text");
    assert!(think.starts_with("  "));
    assert!(!think.contains("think  "));
    assert!(
        !think
            .to_ascii_lowercase()
            .split_whitespace()
            .any(|w| w == "text")
    );
}

#[test]
fn human_done_and_timings_are_scannable() {
    use crate::output::{format_human_done_line, format_human_timings_line};
    let done = format_human_done_line("end_turn", 14886);
    assert!(done.starts_with("Completed in "));
    assert!(done.contains("end_turn"));
    assert!(!done.contains("Some("));
    assert!(!done.starts_with("done  "));
    let t = format_human_timings_line(100, Some(90), None);
    assert!(t.contains("prompt_ms=90"));
    assert!(!t.contains("Some("));
}

#[test]
fn field_reads_camel_case_body_text() {
    // Hub ViewMessage serializes with rename_all = "camelCase" → bodyText.
    // Human show must not blank the conversation because CLI looked for body_text only.
    use crate::output::field;
    use serde_json::json;
    let item = json!({
        "seq": 1,
        "role": "user",
        "kind": "prompt",
        "bodyText": "Create smoke2.txt with SMOKE2-OK",
        "source": "local_turn",
        "mergedCount": 1
    });
    assert_eq!(
        field(&item, "body_text"),
        "Create smoke2.txt with SMOKE2-OK"
    );
    assert_eq!(field(&item, "role"), "user");
    let env = json!({
        "truncated": true,
        "rawCount": 40,
        "viewCount": 20,
        "items": []
    });
    assert_eq!(field(&env, "raw_count"), "40");
    assert_eq!(field(&env, "view_count"), "20");
}

#[test]
fn format_human_show_line_keeps_full_user_and_reply() {
    use crate::output::format_human_show_line;
    let long = "x".repeat(250);
    let thought = format_human_show_line("assistant", Some("thought"), &long);
    assert!(thought.starts_with("  "));
    assert!(
        thought.len() >= 250,
        "show must not truncate thoughts to 200 for completeness"
    );
    let user = format_human_show_line(
        "user",
        Some("prompt"),
        "content type text text Create the file please",
    );
    assert!(user.starts_with("You: "));
    assert!(user.contains("Create the file please"));
    assert!(!user.to_ascii_lowercase().contains("content type"));
    let reply = format_human_show_line("assistant", Some("message"), "Done. Path is smoke2.txt.");
    assert_eq!(reply, "Done. Path is smoke2.txt.");
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
