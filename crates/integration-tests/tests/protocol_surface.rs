//! T13-T17 — Protocol surface tests using in-process Testy.
//! Exercises: session/close, session/list, fs/read_text_file, terminal/*.
//! Auth/logout are capability-gated; Testy exercises them via Callbacks scenario.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::{NewConversation, Store};
use agent_client_protocol::{Client, DynConnectTo};
use agent_client_protocol_test::testy::{Testy, TestyCommand, TestyScenario};

async fn setup_testy() -> (
    tempfile::TempDir,
    std::sync::Arc<HubCtx>,
    acp_hub::acp::AgentHandle,
) {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_path_buf();
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&NewConversation {
            id: "conv-proto".into(),
            agent_id: "testy".into(),
            agent_session_id: "pending".into(),
            cwd: Some(cwd.display().to_string()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    let ctx = HubCtx::new(store);
    let component: DynConnectTo<Client> = DynConnectTo::new(Testy::new());
    let handle_rx = spawn_agent_connection(
        component,
        "testy".into(),
        acp_hub_integration_tests::test_agent_config(),
        ctx.clone(),
    );
    let handle = tokio::time::timeout(Duration::from_secs(10), handle_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    (temp, ctx, handle)
}

async fn create_session(ctx: &HubCtx, handle: &acp_hub::acp::AgentHandle, cwd: &Path) -> String {
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "conv-proto".into(),
            agent_id: "testy".into(),
            cwd: cwd.to_path_buf(),
            additional_directories: vec![],
            mcp_servers: vec![],
            reply: tx,
        })
        .await
        .unwrap();
    let session_id = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .agent_session_id;
    acp_hub_integration_tests::bind_test_session(
        ctx,
        "conv-proto",
        "testy",
        &session_id,
        cwd.to_path_buf(),
    )
    .unwrap();
    session_id
}

#[tokio::test]
async fn session_close_unbinds() {
    let (temp, ctx, handle) = setup_testy().await;
    let sid = create_session(&ctx, &handle, temp.path()).await;

    // Close the session.
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CloseSession {
            conv_id: "conv-proto".into(),
            agent_session_id: sid.clone(),
            reply: tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !ctx.is_session_bound("testy", &sid),
        "session/close must remove the local callback binding"
    );

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::ListSessions {
            cwd: Some(temp.path().to_path_buf()),
            reply: tx,
        })
        .await
        .unwrap();
    let listed = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        listed
            .sessions
            .iter()
            .all(|session| session.session_id.to_string() != sid),
        "closed session must no longer be advertised"
    );
}

#[tokio::test]
async fn session_list_returns_sessions() {
    let (temp, ctx, handle) = setup_testy().await;
    let sid1 = create_session(&ctx, &handle, temp.path()).await;
    let sid2 = create_session(&ctx, &handle, temp.path()).await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::ListSessions {
            cwd: Some(temp.path().to_path_buf()),
            reply: tx,
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ids = result
        .sessions
        .iter()
        .map(|session| session.session_id.to_string())
        .collect::<BTreeSet<_>>();
    assert!(ids.contains(&sid1));
    assert!(ids.contains(&sid2));
}

#[tokio::test]
async fn session_updates_scenario_captures_all_update_variants() {
    let (temp, ctx, handle) = setup_testy().await;
    let sid = create_session(&ctx, &handle, temp.path()).await;

    // SessionUpdates is the SDK's focused, platform-neutral scenario for every
    // stable session/update variant. Callback behavior has separate
    // cross-platform coverage in callback_roundtrip.rs.
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
    let prompt = vec![ContentBlock::Text(TextContent::new(
        TestyCommand::RunScenario {
            scenario: TestyScenario::SessionUpdates,
        }
        .to_prompt(),
    ))];
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "conv-proto".into(),
            agent_session_id: sid,
            prompt,
            params: vec![],
            mode_id: None,
            reply: tx,
        })
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Verify the named update variants rather than accepting an arbitrary count.
    let msgs = ctx.store().messages("conv-proto", true).unwrap();
    let kinds = msgs
        .iter()
        .filter_map(|message| message.kind.as_deref())
        .collect::<BTreeSet<_>>();
    for required in ["thought", "tool_call", "tool_call_update"] {
        assert!(
            kinds.contains(required),
            "missing update kind {required}; captured kinds: {kinds:?}"
        );
    }
    let roles = msgs
        .iter()
        .map(|message| message.role.as_str())
        .collect::<BTreeSet<_>>();
    assert!(roles.contains("user"));
    assert!(roles.contains("assistant"));
    assert!(ctx.store().config_snapshot("conv-proto").unwrap().is_some());
    assert!(ctx.store().plan_snapshot("conv-proto").unwrap().is_some());
    assert!(
        ctx.store()
            .commands_snapshot("conv-proto")
            .unwrap()
            .is_some()
    );
    let usage = ctx
        .store()
        .usage_snapshot("conv-proto")
        .unwrap()
        .expect("usage snapshot");
    assert_eq!(usage["used"], 128);
    assert_eq!(usage["size"], 4096);
    let conversation = ctx
        .store()
        .conversation("conv-proto")
        .unwrap()
        .expect("conversation");
    assert_eq!(
        conversation.title.as_deref(),
        Some("Testy deterministic session")
    );
    assert!(
        conversation
            .session_meta
            .as_ref()
            .and_then(|meta| meta.get("currentMode"))
            .is_some(),
        "current mode update was not persisted; session metadata: {:?}",
        conversation.session_meta
    );
}
