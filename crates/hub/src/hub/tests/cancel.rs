use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::OperationKind;
use super::support::{
    fixture_hub, mark_live_and_bound, prompt, stored_conversation, wait_for_marker,
};
use crate::runtime::SessionState;
use crate::store::{ConvStatus, RunStatus};

/// rc.6 P0-2: cancel marks hub state first; ACP notify failure must not
/// leave the operator without a timely success or roll back the request.
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
    assert_eq!(result.run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(
        hub.store().run_status(&run_id).unwrap(),
        Some(RunStatus::Cancelling)
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
    wait_for_marker(&home.path().join("cancels")).await;

    let second = hub.cancel(&conv.id).await.unwrap();
    assert!(!second.requested);

    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}
