//! P5/P6 concurrency invariants (plan verification step 9).
//!
//! Tests the load-bearing concurrency guarantees:
//! - Store CAS finalize (only running/cancelling → terminal)
//! - seq allocation correctness (UNIQUE constraint + BEGIN IMMEDIATE)
//! - Non-destructive load replay doesn't corrupt seq ordering
//! - Driver send+cancel: prompt returns with cancelled stop reason
//! - Driver load success: session binding and replay/update capture remain live

use std::path::PathBuf;
use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::{ConvStatus, MessageSource, NewConversation, NewMessage, RunStatus, Store};
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use agent_client_protocol::{Client, DynConnectTo};
use agent_client_protocol_test::testy::{Testy, TestyCommand};

fn make_store_with_conv() -> Store {
    make_store_with_conv_at(PathBuf::from("/tmp"))
}

fn make_store_with_conv_at(cwd: PathBuf) -> Store {
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&NewConversation {
            id: "c1".into(),
            agent_id: "a1".into(),
            agent_session_id: "s1".into(),
            cwd: Some(cwd.display().to_string()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    store
}

#[test]
fn cas_finalize_only_from_running_or_cancelling() {
    let store = make_store_with_conv();
    store.create_run("r1", "c1").unwrap();

    // running → completed: should succeed.
    assert!(
        store
            .finalize_run_cas("r1", "c1", RunStatus::Completed, Some("EndTurn"))
            .unwrap()
    );
    assert_eq!(
        store.conversation("c1").unwrap().unwrap().status,
        ConvStatus::Completed,
        "terminal run outcome must mirror onto the conversation"
    );

    // Already completed → cannot transition again.
    assert!(
        !store
            .finalize_run_cas("r1", "c1", RunStatus::Cancelled, None)
            .unwrap()
    );
}

#[test]
fn cas_finalize_failed_marks_conversation_failed() {
    let store = make_store_with_conv();
    store.create_run("r-fail", "c1").unwrap();
    assert!(
        store
            .finalize_run_cas("r-fail", "c1", RunStatus::Failed, None)
            .unwrap()
    );
    assert_eq!(
        store.conversation("c1").unwrap().unwrap().status,
        ConvStatus::Failed
    );
    // Next run may start after a failed conversation (busy = active run only).
    store.create_run("r-retry", "c1").unwrap();
    assert_eq!(
        store.conversation("c1").unwrap().unwrap().status,
        ConvStatus::Running
    );
}

#[test]
fn seq_allocation_is_contiguous() {
    let store = make_store_with_conv();
    let s1 = store
        .append_message(&NewMessage {
            id: "m1".into(),
            conv_id: "c1".into(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "user".into(),
            kind: None,
            content_json: serde_json::json!({"text": "first"}),
            body_text: "first".into(),
        })
        .unwrap();
    let s2 = store
        .append_message(&NewMessage {
            id: "m2".into(),
            conv_id: "c1".into(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "assistant".into(),
            kind: None,
            content_json: serde_json::json!({"text": "second"}),
            body_text: "second".into(),
        })
        .unwrap();
    assert_eq!(s1, 1);
    assert_eq!(s2, 2);
}

#[test]
fn load_replay_preserves_seq_ordering() {
    let store = make_store_with_conv();
    // Original messages.
    store
        .append_message(&NewMessage {
            id: "orig1".into(),
            conv_id: "c1".into(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: "user".into(),
            kind: None,
            content_json: serde_json::json!({"text": "orig"}),
            body_text: "orig".into(),
        })
        .unwrap();

    // Load replay replaces only Layer 1 and preserves the Hub-captured turn.
    store
        .stage_load_replay(
            "c1",
            "load-1",
            &[
                acp_hub::store::ReplayedMessage {
                    id: "rp1".into(),
                    role: "agent".into(),
                    kind: None,
                    content_json: serde_json::json!({"text": "replayed"}),
                    body_text: "replayed".into(),
                    message_key: Some("mk".into()),
                },
                acp_hub::store::ReplayedMessage {
                    id: "rp2".into(),
                    role: "agent".into(),
                    kind: None,
                    content_json: serde_json::json!({"text": "replayed2"}),
                    body_text: "replayed2".into(),
                    message_key: Some("mk2".into()),
                },
            ],
        )
        .unwrap();

    // Current projection = one Hub-captured message plus two replay messages.
    let cur = store.messages("c1", false).unwrap();
    assert_eq!(cur.len(), 3);
    assert_eq!(cur[0].seq, 1);
    assert_eq!(cur[0].source, MessageSource::LocalTurn);
    assert_eq!(cur[1].seq, 2);
    assert_eq!(cur[1].source, MessageSource::LoadReplay);
    assert_eq!(cur[2].seq, 3);
    assert_eq!(cur[2].source, MessageSource::LoadReplay);

    // The independent local layer remains searchable.
    let page = store.search("orig", None, None, 10, 0).unwrap();
    assert!(!page.items.is_empty());
}

#[tokio::test]
async fn driver_load_creates_and_binds_session_with_update_capture() {
    let temp = tempfile::tempdir().unwrap();
    let store = make_store_with_conv_at(temp.path().to_path_buf());
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

    // Testy deterministically accepts and creates this session on load.
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::LoadSession {
            conv_id: "c1".into(),
            agent_id: "testy".into(),
            agent_session_id: "loaded-session-1".into(),
            cwd: temp.path().to_path_buf(),
            reply: tx,
        })
        .await
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap();

    let created = result.expect("Testy session/load must succeed");
    assert_eq!(created.agent_session_id, "loaded-session-1");
    assert!(ctx.is_session_bound("testy", "loaded-session-1"));
    let prompt = vec![ContentBlock::Text(TextContent::new(
        TestyCommand::Echo {
            message: "loaded-session-binding".into(),
        }
        .to_prompt(),
    ))];
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "c1".into(),
            agent_session_id: created.agent_session_id,
            prompt,
            params: vec![],
            mode_id: None,
            reply: tx,
        })
        .await
        .unwrap();
    let done = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(format!("{:?}", done.stop_reason).contains("EndTurn"));
    let messages = ctx.store().messages("c1", true).unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.body_text.contains("loaded-session-binding"))
    );
}

#[tokio::test]
async fn driver_send_cancel_returns_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let store = make_store_with_conv_at(temp.path().to_path_buf());
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

    // Create a session.
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "c1".into(),
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
    let agent_sid = session.agent_session_id.clone();
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "c1",
        "testy",
        &agent_sid,
        temp.path().to_path_buf(),
    )
    .unwrap();

    // Send a Full scenario (longer-running), then cancel mid-flight.
    let prompt = vec![ContentBlock::Text(TextContent::new(
        TestyCommand::RunScenario {
            scenario: agent_client_protocol_test::testy::TestyScenario::Full,
        }
        .to_prompt(),
    ))];

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "c1".into(),
            agent_session_id: agent_sid.clone(),
            prompt,
            params: vec![],
            mode_id: None,
            reply: tx,
        })
        .await
        .unwrap();

    // Cancel after a short delay via the cloned cx (bypassing the blocked loop).
    let cx_clone = handle.cx.clone();
    let sid_clone = agent_sid.clone();
    tokio::spawn(async move {
        let _ =
            cx_clone.send_notification(agent_client_protocol::schema::v1::CancelNotification::new(
                agent_client_protocol::schema::v1::SessionId::new(sid_clone.as_str()),
            ));
    });

    let done = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .expect("prompt timed out")
        .unwrap()
        .unwrap();

    // The stop reason should reflect cancellation (not a normal EndTurn).
    let reason = format!("{:?}", done.stop_reason);
    assert!(
        reason.contains("Cancel"),
        "cancel test completed normally instead of proving cancellation: {reason}"
    );
}
