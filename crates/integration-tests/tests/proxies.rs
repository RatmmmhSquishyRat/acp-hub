//! Spec 5 — proxy chain assembly test.
//!
//! Verifies that the Hub's conductor integration correctly assembles a proxy
//! chain and that prompts flow through the proxy to the agent and back.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::daemon::ActivityTracker;
use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ProxyEndpointConfig, ProxyTransport, Registry,
};
use acp_hub::hub::{CoreHub, CreateConversationParams, SendPromptParams};
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

#[tokio::test]
async fn physical_bounded_proxy_legs_record_reservation_ack_and_saturation() {
    const TEST_FLOW_LIMIT: usize = 8 * 1024;
    acp_hub::test_flow_ledger::reset_test_flow_ledger(64, TEST_FLOW_LIMIT);

    let temp = tempfile::tempdir().unwrap();
    let registry = Registry {
        agents: BTreeMap::from([(
            "physical".to_string(),
            AgentEndpointConfig {
                transport: AgentTransport::Stdio {
                    command: env!("CARGO_BIN_EXE_flow_ledger_agent").to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
                proxy_chain: vec!["expander".to_string()],
                permission_policy: Default::default(),
                client_capabilities: Default::default(),
            },
        )]),
        proxies: BTreeMap::from([(
            "expander".to_string(),
            ProxyEndpointConfig {
                transport: ProxyTransport::Stdio {
                    command: env!("CARGO_BIN_EXE_flow_ledger_proxy").to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            },
        )]),
    };
    let hub = CoreHub::new(
        temp.path(),
        registry,
        Store::open_memory().unwrap(),
        Arc::new(ActivityTracker::new()),
    );
    let conversation = hub
        .create_conversation(CreateConversationParams {
            agent_id: "physical".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            agent_session_id: None,
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        })
        .await
        .expect("physical bounded proxy chain must initialize and create a session");

    let setup_events = acp_hub::test_flow_ledger::test_flow_ledger_snapshot();
    let setup_flows = setup_events
        .iter()
        .filter(|event| event.action == "reserve")
        .map(|event| event.flow_id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        setup_flows.len(),
        2,
        "both physical stdio legs must reserve and acknowledge frames"
    );
    for flow_id in &setup_flows {
        assert!(
            setup_events
                .iter()
                .any(|event| event.flow_id == *flow_id && event.action == "ack"),
            "physical leg {flow_id} never recorded an ACK"
        );
    }
    assert!(
        setup_events
            .iter()
            .all(|event| event.action != "ack_mismatch"),
        "transparent setup traffic must not release a non-matching physical frame"
    );
    let mut acknowledged_reservations = std::collections::BTreeSet::new();
    for acknowledgement in setup_events.iter().filter(|event| event.action == "ack") {
        let token = acknowledgement
            .reservation_token
            .expect("every ACK must name its physical reservation token");
        let identity = acknowledgement
            .identity
            .as_deref()
            .expect("every ACK must name its physical frame identity");
        let reservation = setup_events
            .iter()
            .find(|event| {
                event.flow_id == acknowledgement.flow_id
                    && event.action == "reserve"
                    && event.reservation_token == Some(token)
            })
            .expect("every ACK must correspond to an earlier reservation on the same leg");
        assert_eq!(
            reservation.identity.as_deref(),
            Some(identity),
            "ACK identity must equal the reserved physical frame identity"
        );
        assert!(
            reservation.sequence < acknowledgement.sequence,
            "a physical frame cannot be acknowledged before it is reserved"
        );
        assert!(
            acknowledged_reservations.insert((acknowledgement.flow_id, token)),
            "a physical reservation token must be released at most once"
        );
    }

    acp_hub::test_flow_ledger::reset_test_flow_ledger(64, TEST_FLOW_LIMIT);
    acp_hub::test_flow_ledger::pause_test_flow_acknowledgements(true);
    let error = hub
        .send_prompt(SendPromptParams {
            conv_id: conversation.conv_id,
            prompt: vec![agent_client_protocol::schema::v1::ContentBlock::Text(
                agent_client_protocol::schema::v1::TextContent::new("expand"),
            )],
            params: Vec::new(),
            mode_id: None,
        })
        .await
        .expect_err("expanded proxy output must saturate the controlled outer-leg byte budget");
    assert!(
        error.to_string().contains("connection")
            || error.to_string().contains("flow")
            || error.to_string().contains("closed")
            || error.to_string().contains("dropped"),
        "unexpected saturation error: {error}"
    );

    let saturated = acp_hub::test_flow_ledger::test_flow_ledger_snapshot();
    assert!(
        saturated.iter().any(|event| event.action == "reserve"),
        "the inner physical leg did not reserve the pre-transform frame"
    );
    assert!(
        saturated.iter().any(|event| event.action == "reject"),
        "the expanded outer physical leg did not reject at its fixed byte ceiling"
    );
    assert!(
        saturated
            .iter()
            .all(|event| event.bytes.saturating_add(event.partial_bytes) <= TEST_FLOW_LIMIT),
        "a physical leg exceeded the controlled retained-byte ceiling"
    );
}
