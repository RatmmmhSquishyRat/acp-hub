//! Operator UX Phases 2–4 shipped-path oracles.

use std::sync::Arc;

use acp_hub::HubError;
use acp_hub::daemon::ActivityTracker;
use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy, Registry,
};
use acp_hub::hub::{CoreHub, PERMISSION_POLICY_REJECT_HINT};
use acp_hub::progress::{ProgressStage, ProgressTracker};
use acp_hub::store::{
    MessageSource, NewConversation, NewMessage, Store, merge_transcript, summary_preview,
};
use serde_json::json;

fn store() -> Store {
    Store::open_memory().expect("memory store")
}

fn hub_with(store: Store) -> CoreHub {
    CoreHub::new(
        tempfile::tempdir().unwrap().path(),
        Registry::default(),
        store,
        Arc::new(ActivityTracker::new()),
    )
}

fn append_thoughts(store: &Store, conv_id: &str, n: i64) {
    store.create_run("run1", conv_id).unwrap();
    for i in 1..=n {
        store
            .append_message(&NewMessage {
                id: format!("t{i}"),
                conv_id: conv_id.into(),
                run_id: Some("run1".into()),
                source: MessageSource::LocalTurn,
                role: "assistant".into(),
                kind: Some("thought".into()),
                content_json: json!({}),
                body_text: format!("content type text text chunk{i}"),
            })
            .unwrap();
    }
}

#[test]
fn sc13_show_conversation_merges_thoughts_on_shipped_path() {
    let store = store();
    store
        .create_conversation(&NewConversation {
            id: "conv-sc13".into(),
            agent_id: "fixture".into(),
            agent_session_id: "sess-sc13".into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: Some("sc13".into()),
        })
        .unwrap();
    append_thoughts(&store, "conv-sc13", 12);

    let hub = hub_with(store);
    let shown = hub
        .show_conversation(&acp_hub::hub::ShowConversationParams::new("conv-sc13"))
        .unwrap();
    assert_eq!(shown["transcript"]["rawCount"], 12);
    assert_eq!(
        shown["transcript"]["viewCount"], 1,
        "SC-13: thoughts must merge"
    );
    let body = shown["transcript"]["items"][0]["bodyText"]
        .as_str()
        .unwrap_or("");
    assert!(body.contains("chunk1") && body.contains("chunk12"));
    assert!(!body.to_ascii_lowercase().starts_with("content type"));

    let raw = hub
        .show_conversation(&acp_hub::hub::ShowConversationParams::new("conv-sc13").with_raw(true))
        .unwrap();
    assert_eq!(raw["transcript"]["viewCount"], 12);
    assert_eq!(shown["layer1Refreshed"], false);
    assert_eq!(
        shown["conversation"]["summaryPreview"].as_str(),
        Some("sc13")
    );
}

#[test]
fn search_hits_include_interaction_origin_updated() {
    let store = store();
    store
        .create_conversation(&NewConversation {
            id: "conv-search".into(),
            agent_id: "fixture".into(),
            agent_session_id: "sess-search".into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: Some("searchable unique_token_xyz".into()),
        })
        .unwrap();
    store
        .append_message(&NewMessage {
            id: "ms1".into(),
            conv_id: "conv-search".into(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "user".into(),
            kind: None,
            content_json: json!({}),
            body_text: "hello unique_token_xyz world".into(),
        })
        .unwrap();

    let page = store.search("unique_token_xyz", None, None, 20, 0).unwrap();
    assert!(!page.items.is_empty());
    let hit = &page.items[0];
    assert_eq!(hit.interaction, "writable");
    assert_eq!(hit.origin, "hub_created");
    assert!(!hit.updated_at.is_empty());
    assert!(hit.snippet.chars().count() <= 120);
}

#[test]
fn list_fills_summary_preview_from_user_message() {
    let store = store();
    store
        .create_conversation(&NewConversation {
            id: "conv-prev".into(),
            agent_id: "fixture".into(),
            agent_session_id: "sess-prev".into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: Some("title-fallback".into()),
        })
        .unwrap();
    store
        .append_message(&NewMessage {
            id: "u1".into(),
            conv_id: "conv-prev".into(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "user".into(),
            kind: None,
            content_json: json!({}),
            body_text: "please fix the flaky test".into(),
        })
        .unwrap();

    let hub = hub_with(store);
    let page = hub
        .list_conversations_filtered(&acp_hub::store::ListConversationsFilter::workbench_default())
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].summary_preview.as_deref(),
        Some("please fix the flaky test")
    );
}

#[test]
fn inspect_without_probe_skipped_when_cache_empty() {
    let store = store();
    let mut reg = Registry::default();
    reg.agents.insert(
        "fixture".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "true".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::AutoAllow,
            client_capabilities: ClientCapabilityConfig::default(),
        },
    );
    let hub = CoreHub::new(
        tempfile::tempdir().unwrap().path(),
        reg,
        store,
        Arc::new(ActivityTracker::new()),
    );
    let inspection = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(hub.inspect_agent("fixture", false))
        .unwrap();
    assert_eq!(inspection.probe_status, "skipped");
    assert!(!inspection.cache_populated);
    let msg = inspection.message.as_deref().unwrap_or("");
    assert!(msg.contains("probe") || msg.contains("skipped"), "{msg}");
    assert_eq!(inspection.permission_policy, "auto-allow");
}

#[test]
fn inspect_reject_policy_fixed_substring() {
    let store = store();
    let mut reg = Registry::default();
    reg.agents.insert(
        "rejecty".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "true".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::Reject,
            client_capabilities: ClientCapabilityConfig::default(),
        },
    );
    let hub = CoreHub::new(
        tempfile::tempdir().unwrap().path(),
        reg,
        store,
        Arc::new(ActivityTracker::new()),
    );
    let inspection = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(hub.inspect_agent("rejecty", false))
        .unwrap();
    assert_eq!(inspection.permission_policy, "reject");
    let msg = inspection.message.as_deref().unwrap_or("");
    assert!(msg.contains(PERMISSION_POLICY_REJECT_HINT), "{msg}");
}

#[test]
fn progress_tracker_create_and_send_stages() {
    let mut create = ProgressTracker::new();
    create.stage(ProgressStage::DaemonConnect);
    create.stage(ProgressStage::SessionOp);
    let (end, t) = create.finish();
    assert_eq!(end.stage, "end");
    assert!(t.daemon_ms.is_some());
    assert!(t.session_ms.is_some());
    assert!(t.prompt_ms.is_none());
    assert_eq!(
        ProgressTracker::human_stage_line("daemon_connect"),
        "[acp-hub] stage=daemon_connect"
    );

    let mut send = ProgressTracker::new();
    send.stage(ProgressStage::DaemonConnect);
    send.stage(ProgressStage::Prompt);
    let (_, st) = send.finish();
    assert!(st.prompt_ms.is_some());
}

#[test]
fn pure_merge_api_is_public_entry() {
    let store = store();
    store
        .create_conversation(&NewConversation {
            id: "c".into(),
            agent_id: "a".into(),
            agent_session_id: "s".into(),
            cwd: None,
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    append_thoughts(&store, "c", 10);
    let rows = store.messages("c", false).unwrap();
    let view = merge_transcript(&rows);
    assert_eq!(view.view_count, 1);
    assert_eq!(view.raw_count, 10);
    assert_eq!(summary_preview(&rows, Some("t")).as_deref(), Some("t"));
}

#[test]
fn doctor_reject_hint_constant_matches_error_surface() {
    // Same constant used by CLI doctor and inspect.
    assert!(PERMISSION_POLICY_REJECT_HINT.contains("permission_policy=reject"));
    let err = HubError::PermissionPolicyReject {
        message: PERMISSION_POLICY_REJECT_HINT.into(),
    };
    let line = err.phase1_cli_line();
    assert!(
        line.contains("permission_policy_reject") || line.contains("reject"),
        "{line}"
    );
}
