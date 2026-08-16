use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::OperationKind;
use super::support::{
    fixture_hub, mark_live_and_bound, prompt, stored_conversation, wait_for_marker,
};
use super::{STOP_REASON_HUB_CANCEL_BUDGET, STOP_REASON_HUB_CANCEL_NOTIFY_FAILED};
use crate::runtime::SessionState;
use crate::store::{ConvStatus, RunStatus};

/// Mark-first: notify skip must keep the hub mark and report
/// `acp_notify_enqueued=false`. That is not “agent stopped”.
#[tokio::test]
async fn cancel_marks_requested_even_when_agent_notify_fails() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-cancel-notify-fail", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-notify-fail", "cancel notify fail"))
            .await
    });
    wait_for_marker(&home.path().join("prompt-ready")).await;
    let run_id = {
        let operations = hub.operations.lock();
        let entry = operations.get(&conv.id).unwrap();
        let OperationKind::Prompt(active) = &entry.kind else {
            panic!("prompt operation changed kind");
        };
        active.run_id.clone()
    };

    hub.cancel_notification_fail_once
        .store(true, Ordering::SeqCst);
    let result = hub.cancel(&conv.id).await.unwrap();
    assert!(result.requested);
    assert!(
        !result.acp_notify_enqueued,
        "forced notify skip must report acp_notify_enqueued=false"
    );
    assert_eq!(result.run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(
        hub.store().run_status(&run_id).unwrap(),
        Some(RunStatus::Cancelling),
        "notify skip is mark-only; agent is not treated as stopped"
    );
    assert_eq!(
        hub.store().conversation(&conv.id).unwrap().unwrap().status,
        ConvStatus::Cancelling
    );
    assert!(matches!(
        hub.runtime.get(&conv.id),
        Some((SessionState::Cancelling, _))
    ));
    assert!(hub.operations.lock().get(&conv.id).is_some_and(|entry| {
        matches!(
            &entry.kind,
            OperationKind::Prompt(active) if active.cancel_requested
        )
    }));
    assert!(
        !home.path().join("cancels").exists(),
        "forced notify failure must not reach the agent"
    );

    // Second cancel is one-shot: already requested.
    let retry = hub.cancel(&conv.id).await.unwrap();
    assert!(!retry.requested);
    assert!(!retry.acp_notify_enqueued);

    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancel_is_idempotent_after_successful_request() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-cancel-idempotent", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-idempotent", "cancel idempotent"))
            .await
    });
    wait_for_marker(&home.path().join("prompt-ready")).await;

    let first = hub.cancel(&conv.id).await.unwrap();
    assert!(first.requested);
    assert!(
        first.acp_notify_enqueued,
        "live handle should enqueue session/cancel"
    );
    wait_for_marker(&home.path().join("cancels")).await;

    let second = hub.cancel(&conv.id).await.unwrap();
    assert!(!second.requested);
    assert!(!second.acp_notify_enqueued);
    assert_eq!(second.run_id.as_deref(), first.run_id.as_deref());

    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}

/// Cancel RPC returns the mark immediately; it does not wait the escalation budget.
#[tokio::test]
async fn cancel_rpc_does_not_join_escalation_budget() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-cancel-budget-rpc", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-budget-rpc", "budget rpc"))
            .await
    });
    wait_for_marker(&home.path().join("prompt-ready")).await;

    hub.cancel_escalation_budget_ms
        .store(2_000, Ordering::SeqCst);
    let started = Instant::now();
    let result = hub.cancel(&conv.id).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(result.requested);
    assert!(result.acp_notify_enqueued);
    assert_eq!(
        hub.store()
            .run_status(result.run_id.as_deref().unwrap())
            .unwrap(),
        Some(RunStatus::Cancelling)
    );

    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}

/// Notify skip + short budget: supervisor force-finalizes; wait/run sees cancelled.
#[tokio::test]
async fn cancel_force_finalizes_after_budget_when_notify_skipped() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(
        &hub,
        "conv-cancel-escalate-skip",
        "session-one",
        home.path(),
    );
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-escalate-skip", "escalate skip"))
            .await
    });
    wait_for_marker(&home.path().join("prompt-ready")).await;

    hub.cancel_notification_fail_once
        .store(true, Ordering::SeqCst);
    hub.cancel_escalation_budget_ms.store(40, Ordering::SeqCst);
    let result = hub.cancel(&conv.id).await.unwrap();
    assert!(result.requested);
    assert!(!result.acp_notify_enqueued);
    let run_id = result.run_id.clone().expect("marked run");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if hub.store().run_status(&run_id).unwrap() == Some(RunStatus::Cancelled) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("notify-fail path must force-finalize after the budget");

    let info = hub.store().get_run(&run_id).unwrap().unwrap();
    assert_eq!(info.status, "cancelled");
    assert_eq!(
        info.stop_reason.as_deref(),
        Some(STOP_REASON_HUB_CANCEL_NOTIFY_FAILED)
    );
    assert!(
        !home.path().join("cancels").exists(),
        "escalation must not pretend the agent received session/cancel"
    );

    fs::write(home.path().join("prompt-release"), "").unwrap();
    let _ = prompt_task.await;
}

/// Scheduled notify + agent that never stops: supervisor force-finalizes.
#[tokio::test]
async fn cancel_force_finalizes_after_budget_when_agent_stays_running() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(
        &hub,
        "conv-cancel-escalate-live",
        "session-one",
        home.path(),
    );
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-escalate-live", "escalate live"))
            .await
    });
    wait_for_marker(&home.path().join("prompt-ready")).await;

    hub.cancel_escalation_budget_ms.store(40, Ordering::SeqCst);
    let result = hub.cancel(&conv.id).await.unwrap();
    assert!(result.requested);
    assert!(result.acp_notify_enqueued);
    let run_id = result.run_id.clone().expect("marked run");
    wait_for_marker(&home.path().join("cancels")).await;

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if hub.store().run_status(&run_id).unwrap() == Some(RunStatus::Cancelled) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("still-running agent must be force-finalized after the budget");

    let info = hub.store().get_run(&run_id).unwrap().unwrap();
    assert_eq!(info.status, "cancelled");
    assert_eq!(
        info.stop_reason.as_deref(),
        Some(STOP_REASON_HUB_CANCEL_BUDGET)
    );

    fs::write(home.path().join("prompt-release"), "").unwrap();
    let _ = prompt_task.await;
}
