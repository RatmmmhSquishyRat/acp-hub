use super::support::{
    fixture_hub, fixture_hub_with_blocked_operation, mark_live_and_bound, stored_conversation,
    wait_for_marker,
};

#[tokio::test]
async fn new_session_snapshot_failure_rolls_back_every_local_publication() {
    let (home, hub) = fixture_hub("churn", 0);
    hub.store().fail_next_static_snapshot_for_test();
    let params = super::CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(home.path().to_path_buf()),
        agent_session_id: None,
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    };

    let error = hub.create_conversation(params.clone()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected static snapshot failure")
    );
    assert!(hub.store().list_conversations(None).unwrap().is_empty());
    assert!(!hub.ctx.is_session_bound("fixture", "new-session"));
    assert!(hub.operations.lock().is_empty());
    assert!(hub.session_identities.lock().is_empty());

    let retry = hub.create_conversation(params).await.unwrap();
    assert_eq!(retry.agent_session_id, "new-session");
    assert!(hub.store().conversation(&retry.conv_id).unwrap().is_some());
}

#[tokio::test]
async fn new_session_pending_capture_failure_removes_restored_queue_and_parent_row() {
    let (home, hub) = fixture_hub("new-pending-update", 0);
    hub.ctx.fail_next_bind_capture_for_test();
    let params = super::CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(home.path().to_path_buf()),
        agent_session_id: None,
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    };

    let error = hub.create_conversation(params.clone()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected pending capture failure")
    );
    assert!(hub.store().list_conversations(None).unwrap().is_empty());
    assert!(!hub.ctx.is_session_bound("fixture", "new-session"));
    assert!(hub.operations.lock().is_empty());
    assert!(hub.session_identities.lock().is_empty());

    let retry = hub.create_conversation(params).await.unwrap();
    let messages = hub.store().messages(&retry.conv_id, false).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "failed publication left a stale pending notification for the retry"
    );
    assert!(messages[0].body_text.contains("pending-new-session-update"));
}

#[tokio::test]
async fn new_session_identity_conflict_discards_the_failed_creators_quarantine() {
    let (home, hub) = fixture_hub("new-pending-update", 0);
    let identity = hub
        .reserve_session_identity("fixture", "new-session", "existing-conversation")
        .unwrap();
    let params = super::CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(home.path().to_path_buf()),
        agent_session_id: None,
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    };

    let error = hub.create_conversation(params.clone()).await.unwrap_err();
    assert!(matches!(error, crate::HubError::Conflict(_)));
    assert!(hub.store().list_conversations(None).unwrap().is_empty());
    assert!(!hub.ctx.is_session_bound("fixture", "new-session"));

    drop(identity);
    let retry = hub.create_conversation(params).await.unwrap();
    let messages = hub.store().messages(&retry.conv_id, false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("pending-new-session-update"));
}

#[tokio::test]
async fn new_session_row_failure_discards_prebind_notifications() {
    let (home, hub) = fixture_hub("new-pending-update", 0);
    hub.store().fail_next_create_conversation_for_test();
    let params = super::CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(home.path().to_path_buf()),
        agent_session_id: None,
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    };

    let error = hub.create_conversation(params.clone()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected conversation creation failure")
    );
    assert!(hub.store().list_conversations(None).unwrap().is_empty());
    assert!(!hub.ctx.is_session_bound("fixture", "new-session"));
    assert!(hub.session_identities.lock().is_empty());

    let retry = hub.create_conversation(params).await.unwrap();
    let messages = hub.store().messages(&retry.conv_id, false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("pending-new-session-update"));
}

#[tokio::test]
async fn duplicate_new_session_cannot_unbind_or_delete_the_existing_owner() {
    let (home, hub) = fixture_hub("new-pending-update", 0);
    let existing = stored_conversation(&hub, "existing-conversation", "new-session", home.path());
    mark_live_and_bound(&hub, &existing);
    let params = super::CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(home.path().to_path_buf()),
        agent_session_id: None,
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    };

    let error = hub.create_conversation(params).await.unwrap_err();
    assert!(matches!(
        error,
        crate::HubError::Conflict(ref conv_id) if conv_id == "existing-conversation"
    ));
    assert!(hub.ctx.is_session_bound("fixture", "new-session"));
    assert!(
        hub.store()
            .conversation("existing-conversation")
            .unwrap()
            .is_some()
    );
    assert!(
        hub.store()
            .messages("existing-conversation", false)
            .unwrap()
            .is_empty()
    );
    assert_eq!(hub.store().list_conversations(None).unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_new_session_is_rejected_before_a_second_agent_request() {
    let (home, hub) = fixture_hub_with_blocked_operation("operation-block", 0, "session/new");
    let params = super::CreateConversationParams {
        agent_id: "fixture".to_string(),
        cwd: Some(home.path().to_path_buf()),
        agent_session_id: None,
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    };
    let first_hub = std::sync::Arc::clone(&hub);
    let first_params = params.clone();
    let first = tokio::spawn(async move { first_hub.create_conversation(first_params).await });
    wait_for_marker(&home.path().join("operation-ready")).await;

    let second = hub.create_conversation(params).await.unwrap_err();
    assert!(matches!(
        second,
        crate::HubError::Conflict(ref conflict)
            if conflict.contains("session/new already in progress")
    ));

    std::fs::write(home.path().join("operation-release"), "").unwrap();
    let created = first.await.unwrap().unwrap();
    assert_eq!(created.agent_session_id, "new-session");
    assert_eq!(hub.store().list_conversations(None).unwrap().len(), 1);
}
