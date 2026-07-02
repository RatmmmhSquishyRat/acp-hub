//! P5/P4 integration — full capture path validation through the driver +
//! callbacks + store against the in-process Testy agent.
//!
//! Validates: connection spawn → CreateSession (binds session) → SendPrompt
//! (captures every session/update via notification handler) → store has the
//! echoed text → search finds it.

use std::path::PathBuf;
use std::time::Duration;

use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use agent_client_protocol::{Client, DynConnectTo};
use agent_client_protocol_test::testy::{Testy, TestyCommand};
use acp_hub::acp::{spawn_agent_connection, AgentCommand, PromptDone};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::Store;

#[tokio::test]
async fn testy_echo_captured_and_searchable() {
    let store = Store::open_memory().unwrap();

    // Create a conversation row so the store knows about conv-1.
    store
        .create_conversation(&acp_hub::store::NewConversation {
            id: "conv-1".into(),
            agent_id: "testy".into(),
            agent_session_id: "pending".into(),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();

    let ctx = HubCtx::new(store);

    // Spawn a Testy connection (in-process).
    let component: DynConnectTo<Client> = DynConnectTo::new(Testy::new());
    let handle_rx = spawn_agent_connection(component, "testy".into(), ctx.clone());
    let handle = tokio::time::timeout(Duration::from_secs(10), handle_rx)
        .await
        .expect("spawn timed out")
        .expect("spawn channel dropped")
        .expect("spawn failed");

    // Create a session.
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "conv-1".into(),
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

    // Update the conversation's agent_session_id (the driver bound the session
    // to the real session_id returned by Testy).
    let agent_sid = session.agent_session_id.clone();

    // Send an Echo prompt.
    let prompt = vec![ContentBlock::Text(TextContent::new(
        TestyCommand::Echo {
            message: "hub-integration-token".into(),
        }
        .to_prompt(),
    ))];

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "conv-1".into(),
            agent_session_id: agent_sid.clone(),
            prompt,
            params: vec![],
            mode_id: None,
            reply: tx,
        })
        .await
        .unwrap();

    let done: PromptDone = tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // The prompt completed.
    assert!(
        format!("{:?}", done.stop_reason).contains("EndTurn"),
        "expected EndTurn, got {:?}",
        done.stop_reason
    );

    // The store should have captured messages for conv-1.
    let messages = ctx.store().messages("conv-1", true).unwrap();
    assert!(
        !messages.is_empty(),
        "expected captured messages in store"
    );

    // Search should find the echoed token.
    tokio::time::sleep(Duration::from_millis(100)).await; // let writes settle
    let page = ctx
        .store()
        .search("hub-integration-token", None, None, 10, 0)
        .unwrap();
    assert!(
        !page.items.is_empty(),
        "search should find the echoed token"
    );
}
