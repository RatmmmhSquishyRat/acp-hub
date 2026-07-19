#[test]
fn session_creation_quarantine_shares_the_global_pending_session_quota() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.configure_agent("agent-b", "connection-b", config(false, false))
        .unwrap();
    let mut creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();

    for index in 0..(MAX_PENDING_SESSIONS / 2) {
        let update = SessionNotification::new(
            SessionId::new(format!("creation-{index}")),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("quarantined"),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", update)
            .unwrap();
    }
    for index in 0..(MAX_PENDING_SESSIONS / 2) {
        let update = SessionNotification::new(
            SessionId::new(format!("ordinary-{index}")),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("pending"),
            ))),
        );
        ctx.handle_notification("agent-b", "connection-b", update)
            .unwrap();
    }
    let overflow = SessionNotification::new(
        SessionId::new("combined-overflow"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "must fail",
        )))),
    );
    assert!(
        ctx.handle_notification("agent-b", "connection-b", overflow)
            .unwrap_err()
            .to_string()
            .contains("too many unbound sessions")
    );
    creation.reject("creation-0").unwrap();

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn session_creation_quarantine_failure_poisons_publication() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    let mut creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();
    let oversized = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", oversized)
        .unwrap_err();
    assert!(
        creation
            .publish("new-session")
            .unwrap_err()
            .to_string()
            .contains("session/new update capture failed")
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn session_creation_quarantine_replays_other_bound_session_updates() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    for (conv_id, session_id) in [
        ("conv-existing", "existing-session"),
        ("conv-new", "new-session"),
    ] {
        ctx.store()
            .create_conversation(&NewConversation {
                id: conv_id.to_string(),
                agent_id: "agent-a".to_string(),
                agent_session_id: session_id.to_string(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
    }
    ctx.bind_session(
        "existing-session",
        binding("agent-a", "conv-existing", &home),
    )
    .unwrap();
    let mut creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();
    for (session_id, text) in [
        ("existing-session", "existing update"),
        ("new-session", "new update"),
    ] {
        ctx.handle_notification(
            "agent-a",
            "connection-a",
            SessionNotification::new(
                SessionId::new(session_id),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text),
                ))),
            ),
        )
        .unwrap();
    }
    creation.publish("new-session").unwrap();
    ctx.bind_session("new-session", binding("agent-a", "conv-new", &home))
        .unwrap();

    let existing = ctx.store().messages("conv-existing", false).unwrap();
    let new = ctx.store().messages("conv-new", false).unwrap();
    assert_eq!(existing.len(), 1);
    assert!(existing[0].body_text.contains("existing update"));
    assert_eq!(new.len(), 1);
    assert!(new[0].body_text.contains("new update"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn abandoned_session_creation_replays_bound_updates_and_discards_unknown_updates() {
    for explicit_reject in [false, true] {
        let (ctx, home) = context();
        ctx.configure_agent("agent-a", "connection-a", config(false, false))
            .unwrap();
        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-existing".to_string(),
                agent_id: "agent-a".to_string(),
                agent_session_id: "existing-session".to_string(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.bind_session(
            "existing-session",
            binding("agent-a", "conv-existing", &home),
        )
        .unwrap();

        let mut creation = ctx
            .begin_session_creation_capture("agent-a", "connection-a")
            .unwrap();
        for (session_id, text) in [
            ("existing-session", "must survive"),
            ("unknown-session", "must be discarded"),
        ] {
            ctx.handle_notification(
                "agent-a",
                "connection-a",
                SessionNotification::new(
                    SessionId::new(session_id),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(text),
                    ))),
                ),
            )
            .unwrap();
        }
        if explicit_reject {
            creation.reject("returned-session").unwrap();
        } else {
            drop(creation);
        }

        let existing = ctx.store().messages("conv-existing", false).unwrap();
        assert_eq!(existing.len(), 1);
        assert!(existing[0].body_text.contains("must survive"));

        ctx.store()
            .create_conversation(&NewConversation {
                id: "conv-unknown".to_string(),
                agent_id: "agent-a".to_string(),
                agent_session_id: "unknown-session".to_string(),
                cwd: Some(home.to_string_lossy().into_owned()),
                additional_directories: Vec::new(),
                title: None,
            })
            .unwrap();
        ctx.bind_session(
            "unknown-session",
            binding("agent-a", "conv-unknown", &home),
        )
        .unwrap();
        assert!(
            ctx.store()
                .messages("conv-unknown", false)
                .unwrap()
                .is_empty()
        );

        drop(ctx);
        let _ = fs::remove_dir_all(home);
    }
}

#[test]
fn first_session_creation_error_discards_already_quarantined_matching_updates() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-new".to_string(),
            agent_id: "agent-a".to_string(),
            agent_session_id: "new-session".to_string(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    let mut creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();
    let cached = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "must not publish",
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", cached)
        .unwrap();
    let oversized = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", oversized)
        .unwrap_err();
    let stale = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "stale",
        )))),
    );
    ctx.handle_notification("agent-a", "wrong-connection", stale)
        .unwrap_err();

    let error = creation.publish("new-session").unwrap_err().to_string();
    assert!(error.contains("session update exceeds"));
    assert!(!error.contains("stale connection"));
    ctx.bind_session("new-session", binding("agent-a", "conv-new", &home))
        .unwrap();
    assert!(
        ctx.store()
            .messages("conv-new", false)
            .unwrap()
            .is_empty()
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn stale_connection_error_does_not_poison_current_session_creation() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-new".to_string(),
            agent_id: "agent-a".to_string(),
            agent_session_id: "new-session".to_string(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    let mut creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();
    let stale = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "stale",
        )))),
    );
    ctx.handle_notification("agent-a", "wrong-connection", stale)
        .unwrap_err();
    let current = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "current",
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", current)
        .unwrap();

    creation.publish("new-session").unwrap();
    ctx.bind_session("new-session", binding("agent-a", "conv-new", &home))
        .unwrap();
    let messages = ctx.store().messages("conv-new", false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("current"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn session_creation_quarantine_shares_global_count_and_byte_quotas() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.configure_agent("agent-b", "connection-b", config(false, false))
        .unwrap();
    for index in 0..(MAX_PENDING_NOTIFICATIONS / 2) {
        let update = SessionNotification::new(
            SessionId::new(format!("ordinary-count-{}", index / MAX_PENDING_PER_SESSION)),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("pending"),
            ))),
        );
        ctx.handle_notification("agent-b", "connection-b", update)
            .unwrap();
    }
    let creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();
    for index in 0..(MAX_PENDING_NOTIFICATIONS / 2) {
        let update = SessionNotification::new(
            SessionId::new(format!("creation-count-{}", index / MAX_PENDING_PER_SESSION)),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("quarantined"),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", update)
            .unwrap();
    }
    let count_overflow = SessionNotification::new(
        SessionId::new("count-overflow"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "must fail",
        )))),
    );
    assert!(
        ctx.handle_notification("agent-a", "connection-a", count_overflow)
            .unwrap_err()
            .to_string()
            .contains("quota exceeded")
    );
    drop(creation);
    drop(ctx);
    let _ = fs::remove_dir_all(home);

    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.configure_agent("agent-b", "connection-b", config(false, false))
        .unwrap();
    let payload = "x".repeat(200 * 1024);
    let sample = SessionNotification::new(
        SessionId::new("creation-bytes"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            payload.clone(),
        )))),
    );
    let sample_bytes = serde_json::to_vec(&sample).unwrap().len();
    let admitted = MAX_PENDING_NOTIFICATION_BYTES / sample_bytes;
    assert!(admitted > 1);
    let creation = ctx
        .begin_session_creation_capture("agent-a", "connection-a")
        .unwrap();
    for _ in 0..(admitted / 2) {
        let update = SessionNotification::new(
            SessionId::new("creation-bytes"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(payload.clone()),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", update)
            .unwrap();
    }
    for _ in (admitted / 2)..admitted {
        let update = SessionNotification::new(
            SessionId::new("ordinary-bytes"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(payload.clone()),
            ))),
        );
        ctx.handle_notification("agent-b", "connection-b", update)
            .unwrap();
    }
    let byte_overflow = SessionNotification::new(
        SessionId::new("ordinary-bytes"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            payload,
        )))),
    );
    assert!(
        ctx.handle_notification("agent-b", "connection-b", byte_overflow)
            .unwrap_err()
            .to_string()
            .contains("quota exceeded")
    );
    drop(creation);

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}
