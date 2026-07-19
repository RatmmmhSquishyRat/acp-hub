//! Spec 5 — proxy chain assembly test.
//!
//! Verifies that the Hub's conductor integration correctly assembles a proxy
//! chain and that prompts flow through the proxy to the agent and back.

use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::Store;
use agent_client_protocol::{Client, Conductor, ConnectTo, DynConnectTo, Proxy};
use agent_client_protocol_test::testy::{Testy, TestyCommand};

struct InProcessArrowProxy;

impl ConnectTo<Conductor> for InProcessArrowProxy {
    async fn connect_to(
        self,
        client: impl ConnectTo<Proxy>,
    ) -> Result<(), agent_client_protocol::Error> {
        agent_client_protocol_test::arrow_proxy::run_arrow_proxy(client).await
    }
}

#[tokio::test]
async fn proxy_chain_assembles_and_forwards() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&acp_hub::store::NewConversation {
            id: "conv-proxy".into(),
            agent_id: "testy".into(),
            agent_session_id: "pending".into(),
            cwd: Some(temp.path().display().to_string()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();

    let ctx = HubCtx::new(store);

    let agent: DynConnectTo<Client> = DynConnectTo::new(Testy::new());
    let proxy: DynConnectTo<Conductor> = DynConnectTo::new(InProcessArrowProxy);
    let component = acp_hub::transport::with_proxy_chain(agent, vec![proxy]);
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

    // Create a session.
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "conv-proxy".into(),
            agent_id: "testy".into(),
            cwd: temp.path().to_path_buf(),
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
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "conv-proxy",
        "testy",
        &session.agent_session_id,
        temp.path().to_path_buf(),
    )
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
    assert!(
        msgs.iter()
            .any(|message| message.body_text.contains(">proxy-test")),
        "the in-process proxy transformation was not observed"
    );
}
