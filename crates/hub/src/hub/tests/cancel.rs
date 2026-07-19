use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::OperationKind;
use super::support::{
    fixture_hub, mark_live_and_bound, prompt, stored_conversation, wait_for_marker,
};
use crate::runtime::SessionState;
use crate::store::{ConvStatus, RunStatus};

#[tokio::test]
async fn failed_cancel_notification_rolls_back_persisted_and_runtime_state() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-cancel-rollback", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-rollback", "cancel rollback"))
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
    let error = hub.cancel(&conv.id).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("forced cancel notification failure")
    );
    assert_eq!(
        hub.store().run_status(&run_id).unwrap(),
        Some(RunStatus::Running)
    );
    assert_eq!(
        hub.store().conversation(&conv.id).unwrap().unwrap().status,
        ConvStatus::Running
    );
    assert!(matches!(
        hub.runtime.get(&conv.id),
        Some((SessionState::Live, _))
    ));
    assert!(hub.operations.lock().get(&conv.id).is_some_and(|entry| {
        matches!(
            &entry.kind,
            OperationKind::Prompt(active) if !active.cancel_requested
        )
    }));
    assert!(
        !home.path().join("cancels").exists(),
        "failed notification must not reach the agent"
    );

    let retry = hub.cancel(&conv.id).await.unwrap();
    assert!(retry.requested);
    assert_eq!(retry.run_id.as_deref(), Some(run_id.as_str()));
    wait_for_marker(&home.path().join("cancels")).await;
    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn failed_cancel_rollback_keeps_the_run_fail_closed_as_cancelling() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-cancel-fail-closed", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cancel-fail-closed", "cancel fail closed"))
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
    hub.cancel_rollback_fail_once.store(true, Ordering::SeqCst);
    let error = hub.cancel(&conv.id).await.unwrap_err().to_string();
    assert!(error.contains("forced cancel notification failure"));
    assert!(error.contains("forced cancel rollback failure"));
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
        "failed notification must not reach the agent"
    );
    let retry = hub.cancel(&conv.id).await.unwrap();
    assert!(!retry.requested);

    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}
