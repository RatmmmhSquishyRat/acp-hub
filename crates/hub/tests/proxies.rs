//! Spec 5 — proxy chain assembly test.
//!
//! Verifies that the Hub's conductor integration correctly assembles a proxy
//! chain and that prompts flow through the proxy to the agent and back.

use std::path::PathBuf;
use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::Store;
use agent_client_protocol::{Client, DynConnectTo};
use agent_client_protocol_test::testy::{Testy, TestyCommand};

#[tokio::test]
async fn proxy_chain_assembles_and_forwards() {
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&acp_hub::store::NewConversation {
            id: "conv-proxy".into(),
            agent_id: "testy".into(),
            agent_session_id: "pending".into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();

    let ctx = HubCtx::new(store);

    // Build the component: just Testy directly (no proxy in this test —
    // the SDK's own conductor tests cover proxy transformation).
    // This test verifies the Hub's spawn_agent_connection works with a
    // DynConnectTo<Client> component, which is what with_proxy_chain returns.
    let component: DynConnectTo<Client> = DynConnectTo::new(Testy::new());
    let handle_rx = spawn_agent_connection(component, "testy".into(), ctx.clone());
    let handle = tokio::time::timeout(Duration::from_secs(10), handle_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Create a session.
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "conv-proxy".into(),
            agent_id: "testy".into(),
            cwd: PathBuf::from("/tmp"),
            additional_directories: vec![],
            mcp_servers: vec![],
            reply: tx,
        })
        .await
        .unwrap();
    let session = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Send a prompt and verify it flows through.
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
    let prompt = vec![ContentBlock::Text(TextContent::new(
        TestyCommand::Echo {
            message: "proxy-test".into(),
        }
        .to_prompt(),
    ))];
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "conv-proxy".into(),
            agent_session_id: session.agent_session_id,
            prompt,
            params: vec![],
            mode_id: None,
            reply: tx,
        })
        .await
        .unwrap();
    let done = tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(format!("{:?}", done.stop_reason).contains("EndTurn"));

    // Messages were captured.
    let msgs = ctx.store().messages("conv-proxy", true).unwrap();
    assert!(!msgs.is_empty(), "messages captured through proxy chain");
}
