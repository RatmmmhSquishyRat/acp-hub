//! UX-CORE wait / --no-wait tests on shipped CoreHub paths (fixture agent).

use std::fs;
use std::sync::Arc;

use super::SendPromptParams;
use super::WaitRunParams;
use super::support::{fixture_hub, mark_live_and_bound, stored_conversation, wait_for_marker};
use crate::store::RunStatus;
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};

#[tokio::test]
async fn send_prompt_no_wait_returns_accepted_while_run_busy() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-nowait", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let accepted = hub
        .send_prompt(SendPromptParams {
            conv_id: "conv-nowait".into(),
            prompt: vec![ContentBlock::Text(TextContent::new("no-wait body"))],
            params: Vec::new(),
            mode_id: None,
            wait: false,
        })
        .await
        .expect("wait=false must return after accepted enqueue");

    assert_eq!(
        accepted.busy.as_deref(),
        Some("running"),
        "accepted response must expose busy=running"
    );
    assert!(
        accepted.stop_reason.is_empty(),
        "accepted path has no stopReason yet"
    );
    assert!(
        !accepted.run_id.is_empty(),
        "accepted path must return durable runId"
    );
    assert_eq!(
        hub.store().run_status(&accepted.run_id).unwrap(),
        Some(RunStatus::Running)
    );
    assert_eq!(
        hub.store().active_run_id("conv-nowait").unwrap().as_deref(),
        Some(accepted.run_id.as_str())
    );

    // Unblock fixture so worker can finalize (cleanup).
    wait_for_marker(&home.path().join("prompt-ready")).await;
    fs::write(home.path().join("prompt-release"), "").unwrap();
    // Give worker a moment; not asserting join (process may still finish async).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

#[tokio::test]
async fn wait_observes_mid_turn_cancel_finalize() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-wait-cx", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    // Accepted detach — run stays busy while agent blocks.
    let accepted = hub
        .send_prompt(SendPromptParams {
            conv_id: "conv-wait-cx".into(),
            prompt: vec![ContentBlock::Text(TextContent::new("will cancel"))],
            params: Vec::new(),
            mode_id: None,
            wait: false,
        })
        .await
        .expect("no-wait accepted");
    assert_eq!(accepted.busy.as_deref(), Some("running"));
    wait_for_marker(&home.path().join("prompt-ready")).await;

    let run_id = accepted.run_id.clone();
    let wait_hub = Arc::clone(&hub);
    let wait_task = tokio::spawn(async move {
        wait_hub
            .wait_run(WaitRunParams {
                conv_id: "conv-wait-cx".into(),
                run_id: Some(run_id),
                since_seq: None,
                timeout_secs: Some(10),
            })
            .await
    });

    // Mid-wait: request cancel then force Store terminal the way finalize_run_cas does.
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    let cancel = hub.cancel("conv-wait-cx").await.expect("cancel requested");
    assert!(cancel.requested || cancel.run_id.is_some());
    // Agent cancel may race; ensure Store reaches cancelled (shipped finalize path).
    let _ = hub.store().finalize_run_cas(
        &accepted.run_id,
        "conv-wait-cx",
        RunStatus::Cancelled,
        Some("cancelled"),
    );
    fs::write(home.path().join("prompt-release"), "").unwrap();

    let wait_result = wait_task.await.unwrap().expect("wait must complete");
    assert_eq!(wait_result.run.status, "cancelled");
    assert_eq!(wait_result.run.stop_reason.as_deref(), Some("cancelled"));
    assert!(wait_result.run.is_terminal());
}
