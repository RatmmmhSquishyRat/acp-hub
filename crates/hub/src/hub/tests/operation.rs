use std::fs;
use std::sync::Arc;
use std::time::Duration;

use super::support::{
    fixture_hub, fixture_hub_with_blocked_operation, mark_live_and_bound, prompt,
    stored_conversation, wait_for_marker,
};
use super::{CoreHub, OperationEntry, OperationKind, ReplayMethod};
use crate::daemon::ActivityTracker;
use crate::endpoint::Registry;
use crate::store::{NewConversation, Store};
use agent_client_protocol::schema::v1::{ContentBlock, ImageContent};
use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

#[tokio::test]
async fn prompt_admission_is_bounded_per_agent_connection() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let first_conv = stored_conversation(&hub, "conv-one", "session-one", home.path());
    let second_conv = stored_conversation(&hub, "conv-two", "session-two", home.path());
    mark_live_and_bound(&hub, &first_conv);
    mark_live_and_bound(&hub, &second_conv);

    let first_hub = Arc::clone(&hub);
    let first =
        tokio::spawn(async move { first_hub.send_prompt(prompt("conv-one", "first")).await });
    wait_for_marker(&home.path().join("prompt-ready")).await;

    let second_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.send_prompt(prompt("conv-two", "second")),
    )
    .await
    .expect("second prompt admission must fail immediately")
    .unwrap_err();
    assert!(
        matches!(&second_error, super::HubError::Conflict(id) if id == "conv-two"),
        "unexpected second prompt error: {second_error}"
    );
    let second_cancel = hub.cancel("conv-two").await.unwrap();
    assert!(!second_cancel.requested);
    assert!(second_cancel.run_id.is_none());
    assert!(!home.path().join("second-prompt-reached").exists());

    fs::write(home.path().join("prompt-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("first prompt did not complete")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancel_is_one_shot_for_each_active_run() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-one", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let send_hub = Arc::clone(&hub);
    let send =
        tokio::spawn(async move { send_hub.send_prompt(prompt("conv-one", "cancel")).await });
    wait_for_marker(&home.path().join("prompt-ready")).await;

    let (first, second) = tokio::join!(hub.cancel("conv-one"), hub.cancel("conv-one"));
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.requested, second.requested);
    assert!(first.run_id.is_some());
    assert_eq!(first.run_id, second.run_id);
    wait_for_marker(&home.path().join("cancels")).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancellations = fs::read_to_string(home.path().join("cancels")).unwrap();
    assert_eq!(
        cancellations.lines().collect::<Vec<_>>(),
        vec!["session-one"]
    );

    fs::write(home.path().join("prompt-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), send)
        .await
        .expect("cancelled prompt did not complete")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn external_refresh_blocks_incompatible_conversation_operations() {
    let (home, hub) = fixture_hub("refresh-block", 0);
    let list_hub = Arc::clone(&hub);
    let listing = tokio::spawn(async move { list_hub.list_agent_sessions("fixture").await });
    wait_for_marker(&home.path().join("load-ready")).await;
    let conv = hub
        .store()
        .conversation_by_agent_session("fixture", "refresh-session")
        .unwrap()
        .unwrap();

    let send_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.send_prompt(prompt(&conv.id, "blocked")),
    )
    .await
    .expect("send must reject a refreshing conversation immediately")
    .unwrap_err();
    assert!(matches!(&send_error, super::HubError::Conflict(id) if id == &conv.id));

    let delete_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.delete_conversation(&conv.id, true),
    )
    .await
    .expect("delete must reject a refreshing conversation immediately")
    .unwrap_err();
    assert!(matches!(&delete_error, super::HubError::Conflict(id) if id == &conv.id));

    let param_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.set_param(&conv.id, "temperature", "1"),
    )
    .await
    .expect("set-param must reject a refreshing conversation immediately")
    .unwrap_err();
    assert!(matches!(&param_error, super::HubError::Conflict(id) if id == &conv.id));

    let mode_error =
        tokio::time::timeout(Duration::from_millis(300), hub.set_mode(&conv.id, "plan"))
            .await
            .expect("set-mode must reject a refreshing conversation immediately")
            .unwrap_err();
    assert!(matches!(&mode_error, super::HubError::Conflict(id) if id == &conv.id));

    let close_error =
        tokio::time::timeout(Duration::from_millis(300), hub.close_conversation(&conv.id))
            .await
            .expect("close must reject a refreshing conversation immediately")
            .unwrap_err();
    assert!(matches!(&close_error, super::HubError::Conflict(id) if id == &conv.id));

    let refresh_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: Some("refresh-session".to_string()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        }),
    )
    .await
    .expect("second refresh must reject immediately")
    .unwrap_err();
    assert!(matches!(&refresh_error, super::HubError::Conflict(id) if id == &conv.id));

    fs::write(home.path().join("load-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), listing)
        .await
        .expect("session listing did not complete")
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn failed_agent_resolution_does_not_leak_active_run() {
    let home = tempfile::tempdir().unwrap();
    let hub = CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::new(ActivityTracker::new()),
    );
    hub.store()
        .create_conversation(&NewConversation {
            id: "conv-missing-agent".to_string(),
            agent_id: "missing-agent".to_string(),
            agent_session_id: "missing-session".to_string(),
            cwd: Some(home.path().to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();

    let error = hub
        .send_prompt(prompt("conv-missing-agent", "missing"))
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        super::HubError::NotFound { kind, .. } if *kind == "agent"
    ));
    assert!(
        hub.operations.lock().is_empty(),
        "agent lookup failure leaked an admitted prompt"
    );
}

#[tokio::test]
async fn unsupported_prompt_content_is_rejected_before_session_or_run_side_effects() {
    let (home, hub) = fixture_hub("ordinary", 0);
    stored_conversation(&hub, "conv-image", "session-image", home.path());
    let error = hub
        .send_prompt(super::SendPromptParams {
            conv_id: "conv-image".to_string(),
            prompt: vec![ContentBlock::Image(ImageContent::new("", "image/png"))],
            params: Vec::new(),
            mode_id: None,
        })
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        super::HubError::UnsupportedCapability {
            operation: "session/prompt",
            required_capability: "prompt_capabilities.image",
            ..
        }
    ));
    assert!(
        hub.operations.lock().is_empty(),
        "capability rejection leaked an admitted prompt operation"
    );
    assert!(
        !home.path().join("methods").exists(),
        "capability rejection reached session load or prompt transport"
    );
    let conversation = hub.store().conversation("conv-image").unwrap().unwrap();
    assert_eq!(conversation.status, crate::store::ConvStatus::Idle);
}
#[derive(Clone, Copy)]
enum BlockingConversationOperation {
    SetParam,
    SetMode,
    Close,
    Delete,
}

impl BlockingConversationOperation {
    fn method(self) -> &'static str {
        match self {
            Self::SetParam => "session/set_config_option",
            Self::SetMode => "session/set_mode",
            Self::Close => "session/close",
            Self::Delete => "session/delete",
        }
    }
}

async fn assert_operation_and_refresh_are_mutually_exclusive(
    operation: BlockingConversationOperation,
) {
    let (home, hub) = fixture_hub_with_blocked_operation("operation-block", 0, operation.method());
    let conv = stored_conversation(&hub, "conv-operation", "operation-session", home.path());
    mark_live_and_bound(&hub, &conv);

    let operation_hub = Arc::clone(&hub);
    let mut operation_task = tokio::spawn(async move {
        match operation {
            BlockingConversationOperation::SetParam => {
                operation_hub
                    .set_param("conv-operation", "temperature", "1")
                    .await
            }
            BlockingConversationOperation::SetMode => {
                operation_hub.set_mode("conv-operation", "plan").await
            }
            BlockingConversationOperation::Close => {
                operation_hub.close_conversation("conv-operation").await
            }
            BlockingConversationOperation::Delete => {
                operation_hub
                    .delete_conversation("conv-operation", false)
                    .await
            }
        }
    });
    let operation_ready = home.path().join("operation-ready");
    tokio::select! {
        result = &mut operation_task => {
            panic!("operation completed before blocking: {result:?}");
        }
        () = wait_for_marker(&operation_ready) => {}
    }

    let refresh_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: Some("operation-session".to_string()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        }),
    )
    .await
    .expect("refresh must reject an admitted operation immediately")
    .unwrap_err();
    assert!(matches!(
        &refresh_error,
        super::HubError::Conflict(id) if id == "conv-operation"
    ));

    operation_task.abort();
    assert!(operation_task.await.unwrap_err().is_cancelled());
    assert!(
        hub.operations.lock().contains_key(&conv.id),
        "outer cancellation released an in-flight operation"
    );
    let blocked_retry = hub
        .create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),
            agent_session_id: Some("operation-session".to_string()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        blocked_retry,
        super::HubError::Conflict(ref id) if id == &conv.id
    ));

    fs::write(home.path().join("operation-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.operations.lock().contains_key(&conv.id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("operation worker did not release admission");

    if matches!(operation, BlockingConversationOperation::Delete) {
        assert!(hub.store().conversation(&conv.id).unwrap().is_none());
    } else {
        hub.create_conversation(super::CreateConversationParams {
            agent_id: "fixture".to_string(),
            cwd: Some(home.path().to_path_buf()),

            agent_session_id: Some("operation-session".to_string()),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn aborted_create_session_worker_finishes_projection_and_releases_admission() {
    let (home, hub) = fixture_hub_with_blocked_operation("operation-block", 0, "session/new");
    let create_hub = Arc::clone(&hub);
    let create_cwd = home.path().to_path_buf();
    let create = tokio::spawn(async move {
        create_hub
            .create_conversation(super::CreateConversationParams {
                agent_id: "fixture".to_string(),
                cwd: Some(create_cwd),
                agent_session_id: None,
                mcp_servers: Vec::new(),
                additional_directories: Vec::new(),
            })
            .await
    });
    wait_for_marker(&home.path().join("operation-ready")).await;

    let conv_id = hub
        .operations
        .lock()
        .iter()
        .find_map(|(conv_id, entry)| {
            (entry.agent_id == "fixture" && matches!(entry.kind, OperationKind::Refresh))
                .then(|| conv_id.clone())
        })
        .expect("create-session worker lost admission");
    create.abort();
    assert!(create.await.unwrap_err().is_cancelled());
    assert!(hub.operations.lock().contains_key(&conv_id));

    fs::write(home.path().join("operation-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.operations.lock().contains_key(&conv_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("create-session worker did not release admission");

    let conv = hub
        .store()
        .conversation(&conv_id)
        .unwrap()
        .expect("create-session worker did not persist its conversation");
    assert_eq!(conv.agent_session_id, "new-session");
    assert!(hub.ctx.is_session_bound("fixture", "new-session"));
    assert!(matches!(
        hub.runtime.get(&conv_id),
        Some((crate::runtime::SessionState::Live, _))
    ));
}

#[tokio::test]
async fn aborted_provisional_load_error_removes_row_binding_runtime_and_admission() {
    let (home, hub) = fixture_hub("refresh-error-block", 0);
    let load_hub = Arc::clone(&hub);
    let load_cwd = home.path().to_path_buf();
    let load = tokio::spawn(async move {
        load_hub
            .create_conversation(super::CreateConversationParams {
                agent_id: "fixture".to_string(),
                cwd: Some(load_cwd),
                agent_session_id: Some("provisional-session".to_string()),
                mcp_servers: Vec::new(),
                additional_directories: Vec::new(),
            })
            .await
    });
    wait_for_marker(&home.path().join("load-ready")).await;
    let provisional = hub
        .store()
        .conversation_by_agent_session("fixture", "provisional-session")
        .unwrap()
        .expect("provisional load did not create a row");
    assert!(hub.operations.lock().contains_key(&provisional.id));

    load.abort();
    assert!(load.await.unwrap_err().is_cancelled());
    assert!(hub.operations.lock().contains_key(&provisional.id));
    fs::write(home.path().join("load-release"), "").unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.operations.lock().contains_key(&provisional.id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("failed provisional load did not release admission");
    assert!(
        hub.store()
            .conversation_by_agent_session("fixture", "provisional-session")
            .unwrap()
            .is_none()
    );
    assert!(!hub.ctx.is_session_bound("fixture", "provisional-session"));
    assert!(hub.runtime.get(&provisional.id).is_none());
}

#[tokio::test]
async fn conversation_operations_block_refresh_in_the_reverse_order_and_release_on_abort() {
    for operation in [
        BlockingConversationOperation::SetParam,
        BlockingConversationOperation::SetMode,
        BlockingConversationOperation::Close,
        BlockingConversationOperation::Delete,
    ] {
        assert_operation_and_refresh_are_mutually_exclusive(operation).await;
    }
}

#[tokio::test]
async fn aborted_prompt_releases_its_operation_admission() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-one", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let first_hub = Arc::clone(&hub);
    let first =
        tokio::spawn(async move { first_hub.send_prompt(prompt("conv-one", "first")).await });
    wait_for_marker(&home.path().join("prompt-ready")).await;
    let first_run_id = {
        let operations = hub.operations.lock();
        let entry = operations.get(&conv.id).expect("prompt lost admission");
        let OperationKind::Prompt(active) = &entry.kind else {
            panic!("prompt admission changed kind");
        };
        active.run_id.clone()
    };
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    assert!(
        matches!(
            hub.operations.lock().get(&conv.id),
            Some(OperationEntry {
                kind: OperationKind::Prompt(active),
                ..
            }) if active.run_id == first_run_id
        ),
        "outer cancellation released an in-flight prompt"
    );
    let retry_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.send_prompt(prompt("conv-one", "too-early")),
    )
    .await
    .expect("busy prompt retry did not reject immediately")
    .unwrap_err();
    assert!(matches!(
        retry_error,
        super::HubError::Conflict(ref id) if id == &conv.id
    ));

    fs::write(home.path().join("prompt-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let released = !hub.operations.lock().contains_key(&conv.id);
            let terminal = hub
                .store()
                .conversation(&conv.id)
                .unwrap()
                .is_some_and(|row| {
                    matches!(
                        row.status,
                        crate::store::ConvStatus::Completed
                            | crate::store::ConvStatus::Cancelled
                            | crate::store::ConvStatus::Failed
                            | crate::store::ConvStatus::Idle
                    )
                });
            if released && terminal {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("prompt worker did not finalize after backend release");

    let retry = hub.send_prompt(prompt("conv-one", "retry")).await.unwrap();
    assert_ne!(retry.run_id, first_run_id);
    let user_run_ids = hub
        .store()
        .messages(&conv.id, false)
        .unwrap()
        .into_iter()
        .filter(|message| message.role == "user")
        .filter_map(|message| message.run_id)
        .collect::<Vec<_>>();
    assert_eq!(user_run_ids, vec![first_run_id, retry.run_id]);
}

#[tokio::test]
async fn external_refresh_publishes_binding_and_runtime_before_returning() {
    let (home, hub) = fixture_hub("refresh-block", 0);
    let conv = stored_conversation(&hub, "conv-publish", "refresh-session", home.path());
    let (publish_reached_tx, publish_reached_rx) = oneshot::channel();
    let (publish_release_tx, publish_release_rx) = oneshot::channel();
    {
        let mut gate = hub.refresh_publish_gate.lock();
        assert!(gate.is_none());
        *gate = Some((publish_reached_tx, publish_release_rx));
    }

    let refresh_hub = Arc::clone(&hub);
    let refresh_conv = conv.clone();
    let refresh_cwd = home.path().to_path_buf();
    let refresh = tokio::spawn(async move {
        refresh_hub
            .refresh_session_projection_external(&refresh_conv, refresh_cwd, ReplayMethod::Load)
            .await
    });
    wait_for_marker(&home.path().join("load-ready")).await;
    fs::write(home.path().join("load-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), publish_reached_rx)
        .await
        .expect("refresh did not reach its publication gate")
        .expect("refresh dropped its publication gate");

    assert!(
        matches!(
            hub.operations.lock().get(&conv.id),
            Some(entry) if matches!(&entry.kind, OperationKind::Refresh)
        ),
        "refresh released admission before publication"
    );
    assert!(
        hub.runtime.get(&conv.id).is_none(),
        "refresh published runtime state before its publication phase"
    );
    let delete_error = tokio::time::timeout(
        Duration::from_millis(300),
        hub.delete_conversation(&conv.id, true),
    )
    .await
    .expect("delete must reject while refresh publication is pending")
    .unwrap_err();
    assert!(matches!(
        &delete_error,
        super::HubError::Conflict(id) if id == &conv.id
    ));

    publish_release_tx
        .send(())
        .expect("refresh publication gate receiver dropped");
    refresh.await.unwrap().unwrap();

    assert!(
        hub.ctx
            .is_session_bound(&conv.agent_id, &conv.agent_session_id),
        "external refresh returned before publishing its callback binding"
    );
    assert!(
        matches!(
            hub.runtime.get(&conv.id),
            Some((crate::runtime::SessionState::Live, _))
        ),
        "external refresh returned before publishing Live runtime state"
    );
    assert!(
        !hub.operations.lock().contains_key(&conv.id),
        "external refresh returned before releasing admission"
    );
}

#[tokio::test]
async fn stale_cancel_does_not_target_a_replacement_prompt() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-one", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let old_hub = Arc::clone(&hub);
    let old = tokio::spawn(async move { old_hub.send_prompt(prompt("conv-one", "old")).await });
    wait_for_marker(&home.path().join("prompt-ready")).await;
    let old_token = {
        hub.operations
            .lock()
            .get(&conv.id)
            .expect("old prompt lost admission")
            .token
    };
    let (snapshot_reached_tx, snapshot_reached_rx) = oneshot::channel();
    let (snapshot_release_tx, snapshot_release_rx) = oneshot::channel();
    {
        let mut gate = hub.cancel_snapshot_gate.lock();
        assert!(gate.is_none());
        *gate = Some((snapshot_reached_tx, snapshot_release_rx));
    }

    let cancel_hub = Arc::clone(&hub);
    let cancel = tokio::spawn(async move { cancel_hub.cancel("conv-one").await });
    tokio::time::timeout(Duration::from_secs(10), snapshot_reached_rx)
        .await
        .expect("cancel did not reach its post-snapshot gate")
        .expect("cancel dropped its post-snapshot gate");

    fs::write(home.path().join("prompt-release"), "").unwrap();
    old.await.unwrap().unwrap();
    fs::remove_file(home.path().join("prompt-release")).unwrap();

    let replacement_hub = Arc::clone(&hub);
    let replacement = tokio::spawn(async move {
        replacement_hub
            .send_prompt(prompt("conv-one", "replacement"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let replacement_reserved = hub
                .operations
                .lock()
                .get(&conv.id)
                .is_some_and(|entry| entry.token != old_token);
            if replacement_reserved {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacement prompt did not reserve admission");

    snapshot_release_tx
        .send(())
        .expect("cancel snapshot gate receiver dropped");
    let cancelled = cancel.await.unwrap().unwrap();
    assert!(!cancelled.requested);
    assert!(
        hub.operations.lock().get(&conv.id).is_some_and(|entry| {
            matches!(
                &entry.kind,
                OperationKind::Prompt(active) if !active.cancel_requested
            )
        }),
        "stale cancel mutated the replacement prompt"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !home.path().join("cancels").exists(),
        "stale cancel emitted a session-scoped notification"
    );

    fs::write(home.path().join("prompt-release"), "").unwrap();
    replacement.await.unwrap().unwrap();
}
#[tokio::test]
async fn aborted_cancel_attempt_resets_the_matching_prompt_flag() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-one", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task =
        tokio::spawn(async move { prompt_hub.send_prompt(prompt("conv-one", "cancel")).await });
    wait_for_marker(&home.path().join("prompt-ready")).await;
    let (snapshot_reached_tx, snapshot_reached_rx) = oneshot::channel();
    let (snapshot_release_tx, snapshot_release_rx) = oneshot::channel();
    {
        let mut gate = hub.cancel_snapshot_gate.lock();
        assert!(gate.is_none());
        *gate = Some((snapshot_reached_tx, snapshot_release_rx));
    }

    let cancel_hub = Arc::clone(&hub);
    let cancel_task = tokio::spawn(async move { cancel_hub.cancel("conv-one").await });
    tokio::time::timeout(Duration::from_secs(10), snapshot_reached_rx)
        .await
        .expect("cancel did not reach its post-snapshot gate")
        .expect("cancel dropped its post-snapshot gate");
    assert!(
        hub.operations.lock().get("conv-one").is_some_and(|entry| {
            matches!(
                &entry.kind,
                OperationKind::Prompt(active) if !active.cancel_requested
            )
        }),
        "snapshot phase mutated the prompt cancellation flag"
    );

    cancel_task.abort();
    assert!(cancel_task.await.unwrap_err().is_cancelled());
    drop(snapshot_release_tx);

    let retry = hub.cancel("conv-one").await.unwrap();
    assert!(retry.requested);
    wait_for_marker(&home.path().join("cancels")).await;
    fs::write(home.path().join("prompt-release"), "").unwrap();
    prompt_task.await.unwrap().unwrap();
}
#[test]
fn stale_operation_lease_cannot_remove_a_newer_generation() {
    let home = tempfile::tempdir().unwrap();
    let hub = CoreHub::new(
        home.path(),
        Registry::default(),
        Store::open_memory().unwrap(),
        Arc::new(ActivityTracker::new()),
    );
    let stale = hub
        .reserve_operation("conv-generation", "agent-a", OperationKind::Refresh)
        .unwrap();
    let newer_token = Uuid::new_v4();
    hub.operations.lock().insert(
        "conv-generation".to_string(),
        OperationEntry {
            token: newer_token,
            agent_id: "agent-a".to_string(),
            kind: OperationKind::SetMode,
        },
    );

    drop(stale);

    assert_eq!(
        hub.operations
            .lock()
            .get("conv-generation")
            .map(|entry| entry.token),
        Some(newer_token)
    );
}

#[tokio::test]
async fn public_run_rpc_requires_owner_and_blocks_registry_mutation() {
    let (_home, hub) = fixture_hub("churn", 0);
    stored_conversation(&hub, "conv-external-run", "external-session", _home.path());
    let created = hub
        .handle_rpc(
            "hub/conv/create_run",
            json!({"convId": "conv-external-run"}),
        )
        .await
        .unwrap();
    let run_id = created["runId"].as_str().unwrap();
    let owner_token = created["ownerToken"].as_str().unwrap();

    let remove_error = hub.remove_agent("fixture").await.unwrap_err();
    assert!(matches!(
        remove_error,
        super::HubError::Conflict(ref id) if id == "conv-external-run"
    ));
    let wrong_owner = hub
        .handle_rpc(
            "hub/conv/finalize_run",
            json!({
                "convId": "conv-external-run",
                "runId": run_id,
                "ownerToken": Uuid::new_v4().to_string(),
                "status": "completed"
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        wrong_owner,
        super::HubError::Conflict(ref id) if id == "conv-external-run"
    ));

    let finalized = hub
        .handle_rpc(
            "hub/conv/finalize_run",
            json!({
                "convId": "conv-external-run",
                "runId": run_id,
                "ownerToken": owner_token,
                "status": "completed"
            }),
        )
        .await
        .unwrap();
    assert_eq!(finalized, json!(true));
    assert!(!hub.operations.lock().contains_key("conv-external-run"));
    hub.remove_agent("fixture").await.unwrap();
}

#[tokio::test]
async fn prompt_worker_reports_conflict_when_its_finalization_cas_loses() {
    let (home, hub) = fixture_hub("prompt-block", 0);
    let conv = stored_conversation(&hub, "conv-cas-loss", "session-one", home.path());
    mark_live_and_bound(&hub, &conv);

    let prompt_hub = Arc::clone(&hub);
    let prompt_task = tokio::spawn(async move {
        prompt_hub
            .send_prompt(prompt("conv-cas-loss", "cas-loss"))
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
    assert!(
        hub.store()
            .finalize_run_cas(
                &run_id,
                &conv.id,
                crate::store::RunStatus::Completed,
                Some("external-finalizer"),
            )
            .unwrap()
    );
    let cancel = hub.cancel(&conv.id).await.unwrap();
    assert_eq!(cancel.run_id.as_deref(), Some(run_id.as_str()));
    assert!(!cancel.requested);
    assert!(
        !home.path().join("cancels").exists(),
        "a terminal persisted run must not emit session/cancel"
    );
    fs::write(home.path().join("prompt-release"), "").unwrap();
    let error = prompt_task.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        super::HubError::Conflict(ref id) if id == "conv-cas-loss"
    ));
}
