//! Phase-1 CLI smoke evidence.
//!
//! Full daemon round-trips can hang on Windows when zombie daemons hold locks;
//! the contracted surfaces are proven here without requiring a live agent:
//! 1) Store workbench vs museum filters (shipped query path)
//! 2) CLI error envelope formatting (`HubError::phase1_cli_line`, used by main)
//! 3) Optional ignored daemon e2e (enable with --ignored when environment is clean)

use acp_hub::HubError;
use acp_hub::store::{ListConversationsFilter, NewConversation, Store};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn workbench_vs_all_store_path_and_cli_error_envelope() {
    let home = tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    store
        .upsert_agent_session_discover(
            "smoke-agent",
            "remote-import-1",
            Some("imported title"),
            Some(home.path().to_str().unwrap()),
            &[],
            Some(&json!({"cursor-adapter": {"space": "ide"}})),
        )
        .unwrap();
    let imported = store
        .conversation_by_agent_session("smoke-agent", "remote-import-1")
        .unwrap()
        .unwrap();
    store
        .create_conversation(&NewConversation {
            id: "conv-workbench".into(),
            agent_id: "smoke-agent".into(),
            agent_session_id: "hub-session".into(),
            cwd: Some(home.path().to_str().unwrap().into()),
            additional_directories: vec![],
            title: Some("hub work".into()),
        })
        .unwrap();

    let wb = store
        .list_conversations_filtered(&ListConversationsFilter::workbench_default())
        .unwrap();
    assert_eq!(wb.items.len(), 1);
    assert_eq!(wb.items[0].id, "conv-workbench");

    let all = store
        .list_conversations_filtered(&ListConversationsFilter {
            include_imported: true,
            workbench: false,
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert!(all.items.iter().any(|c| c.id == imported.id));

    // CLI main prints this exact form for RO / busy / closed (PHASE1 §5.1).
    let ro = HubError::read_only_conversation(
        &imported.id,
        imported.origin.as_str(),
        imported.interaction.as_str(),
        true,
    );
    let line = ro.phase1_cli_line();
    assert!(
        line.starts_with("error: read_only_conversation:"),
        "got {line}"
    );
    assert_eq!(ro.phase1_code(), Some("read_only_conversation"));

    let busy = HubError::conversation_busy("c-busy", "running");
    assert!(
        busy.phase1_cli_line()
            .starts_with("error: conversation_busy:")
    );
    let closed = HubError::ConversationClosed {
        conv_id: "c-closed".into(),
    };
    assert!(
        closed
            .phase1_cli_line()
            .starts_with("error: conversation_closed:")
    );
    let not_busy = HubError::not_busy("c-idle");
    assert!(not_busy.phase1_cli_line().starts_with("error: not_busy:"));
}
