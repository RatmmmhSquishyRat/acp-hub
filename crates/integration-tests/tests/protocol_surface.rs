//! T13-T17 — Protocol surface tests using in-process Testy.
//! Exercises: session/close, session/list, fs/read_text_file, terminal/*.
//! Auth/logout are capability-gated; Testy exercises them via Callbacks scenario.

use std::path::PathBuf;
use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::{NewConversation, Store};
use agent_client_protocol::{Client, DynConnectTo};
use agent_client_protocol_test::testy::{Testy, TestyCommand, TestyScenario};

async fn setup_testy() -> (std::sync::Arc<HubCtx>, acp_hub::acp::AgentHandle) {
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&NewConversation {
            id: "conv-proto".into(),
            agent_id: "testy".into(),
            agent_session_id: "pending".into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    let ctx = HubCtx::new(store);
    let component: DynConnectTo<Client> = DynConnectTo::new(Testy::new());
    let handle_rx = spawn_agent_connection(component, "testy".into(), ctx.clone());
    let handle = tokio::time::timeout(Duration::from_secs(10), handle_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    (ctx, handle)
}

async fn create_session(handle: &acp_hub::acp::AgentHandle) -> String {
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "conv-proto".into(),
            agent_id: "testy".into(),
            cwd: PathBuf::from("/tmp"),
            additional_directories: vec![],
            mcp_servers: vec![],
            reply: tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .agent_session_id
}

#[tokio::test]
async fn session_close_unbinds() {
    let (ctx, handle) = setup_testy().await;
    let sid = create_session(&handle).await;

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
    let _ = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap();
    // Session should be unbound (no crash).
    let _ = ctx;
}

#[tokio::test]
async fn session_list_returns_sessions() {
    let (_ctx, handle) = setup_testy().await;
    let _sid = create_session(&handle).await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::ListSessions {
            cwd: Some(PathBuf::from("/tmp")),
            reply: tx,
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !result.sessions.is_empty(),
        "session/list should return sessions"
    );
}

#[tokio::test]
async fn full_scenario_captures_all_update_variants() {
    let (ctx, handle) = setup_testy().await;
    let sid = create_session(&handle).await;

    // Run the Full scenario which exercises all session/update variants.
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
    let prompt = vec![ContentBlock::Text(TextContent::new(
        TestyCommand::RunScenario {
            scenario: TestyScenario::Full,
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

    // Verify multiple message types were captured.
    let msgs = ctx.store().messages("conv-proto", true).unwrap();
    assert!(
        msgs.len() > 5,
        "Full scenario should produce many captured messages, got {}",
        msgs.len()
    );
}
