#[test]
fn update_before_new_session_response_flushes_after_parent_is_created() {
    let (ctx, home) = context();
    let mut notifications = ctx.subscribe_notifications();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    let update = SessionNotification::new(
        SessionId::new("new-session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "early update",
        )))),
    );

    ctx.handle_notification("agent-a", "connection-a", update)
        .expect("queue pre-bind update");
    assert!(matches!(
        notifications.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));

    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-a".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "new-session".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .expect("create parent conversation");
    ctx.bind_session("new-session", binding("agent-a", "conv-a", &home))
        .expect("bind and flush update");

    let event = notifications.try_recv().expect("streamed notification");
    assert_eq!(event.method, "hub/conv/update");
    assert_eq!(event.params["conversationId"], "conv-a");
    let messages = ctx.store().messages("conv-a", false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("early update"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn notifications_arriving_during_bind_drain_preserve_protocol_order() {
    fn message(session_id: &str, text: &str) -> SessionNotification {
        SessionNotification::new(
            SessionId::new(session_id),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(text),
            ))),
        )
    }

    let (ctx, home) = context();
    let mut notifications = ctx.subscribe_notifications();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-order".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-order".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    ctx.handle_notification("agent-a", "connection-a", message("session-order", "older"))
        .expect("queue older pre-bind update");
    let gate = CallbackTestGate::new();
    *ctx.bind_drain_gate.lock() = Some(gate.clone());
    let bind_ctx = Arc::clone(&ctx);
    let bind_home = home.clone();
    let bind = thread::spawn(move || {
        bind_ctx.bind_session(
            "session-order",
            binding("agent-a", "conv-order", &bind_home),
        )
    });

    gate.reached.wait();
    ctx.handle_notification("agent-a", "connection-a", message("session-order", "newer"))
        .expect("queue newer update behind the drain");
    gate.resume.wait();
    bind.join().expect("binding thread").expect("bind session");

    let messages = ctx.store().messages("conv-order", false).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages[0].body_text.contains("older"));
    assert!(messages[1].body_text.contains("newer"));
    let first = notifications.try_recv().expect("older broadcast");
    let second = notifications.try_recv().expect("newer broadcast");
    assert_eq!(first.params["update"]["content"]["text"], "older");
    assert_eq!(second.params["update"]["content"]["text"], "newer");

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn bind_drain_failure_restores_pending_state_and_keeps_successful_budget() {
    fn message(text: &str) -> SessionNotification {
        SessionNotification::new(
            SessionId::new("session-drain-failure"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(text),
            ))),
        )
    }

    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-drain-failure".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-drain-failure".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    ctx.handle_notification("agent-a", "connection-a", message("persisted first"))
        .unwrap();
    ctx.handle_notification("agent-a", "connection-a", message("still pending"))
        .unwrap();
    let key = SessionKey::new("agent-a", "session-drain-failure");
    ctx.capture_budgets.lock().insert(
        key.clone(),
        CaptureBudget {
            updates: MAX_CAPTURE_UPDATES_PER_TURN - 1,
            bytes: 0,
        },
    );

    let error = ctx
        .bind_session(
            "session-drain-failure",
            binding("agent-a", "conv-drain-failure", &home),
        )
        .expect_err("second drained update must exceed the capture budget");
    assert!(error.to_string().contains("capture budget exceeded"));
    assert!(!ctx.is_session_bound("agent-a", "session-drain-failure"));
    let messages = ctx.store().messages("conv-drain-failure", false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("persisted first"));
    let pending = ctx.pending_notifications.lock();
    let queue = pending.sessions.get(&key).expect("failed update requeued");
    assert_eq!(queue.len(), 1);
    assert_eq!(pending.count, 1);
    assert_eq!(pending.bytes, queue[0].bytes);
    assert!(!pending.draining.contains(&key));
    drop(pending);
    assert_eq!(
        ctx.capture_budgets
            .lock()
            .get(&key)
            .expect("successful drain budget retained")
            .updates,
        MAX_CAPTURE_UPDATES_PER_TURN
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn stale_generation_pending_update_is_discarded_before_new_bind() {
    let (ctx, home) = context();
    let mut notifications = ctx.subscribe_notifications();
    ctx.configure_agent("agent-a", "connection-old", config(false, false))
        .unwrap();
    ctx.handle_notification(
        "agent-a",
        "connection-old",
        SessionNotification::new(
            SessionId::new("session-generation"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("stale"),
            ))),
        ),
    )
    .expect("queue old-generation update");
    ctx.configure_agent("agent-a", "connection-new", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-generation".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-generation".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();

    ctx.bind_session(
        "session-generation",
        binding("agent-a", "conv-generation", &home),
    )
    .expect("stale queued update must not poison the new generation");
    assert!(
        ctx.store()
            .messages("conv-generation", false)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        notifications.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    let pending = ctx.pending_notifications.lock();
    assert_eq!(pending.count, 0);
    assert_eq!(pending.bytes, 0);
    assert!(
        !pending
            .sessions
            .contains_key(&SessionKey::new("agent-a", "session-generation"))
    );
    drop(pending);

    ctx.handle_notification(
        "agent-a",
        "connection-new",
        SessionNotification::new(
            SessionId::new("session-generation"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("current"),
            ))),
        ),
    )
    .expect("current generation remains capturable");
    let messages = ctx.store().messages("conv-generation", false).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("current"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn replacing_connection_purges_all_old_generation_pending_quota() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-old", config(false, false))
        .unwrap();
    for index in 0..MAX_PENDING_SESSIONS {
        ctx.handle_notification(
            "agent-a",
            "connection-old",
            SessionNotification::new(
                SessionId::new(format!("old-session-{index}")),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("old generation"),
                ))),
            ),
        )
        .expect("fill old-generation pending session quota");
    }
    let old_count = ctx.pending_notifications.lock().count;
    ctx.configure_agent("agent-a", "connection-old", config(false, false))
        .unwrap();
    assert_eq!(
        ctx.pending_notifications.lock().count,
        old_count,
        "reconfiguring the current connection must preserve its pending updates"
    );

    ctx.configure_agent("agent-a", "connection-new", config(false, false))
        .unwrap();
    {
        let pending = ctx.pending_notifications.lock();
        assert!(pending.sessions.is_empty());
        assert_eq!(pending.count, 0);
        assert_eq!(pending.bytes, 0);
    }
    ctx.handle_notification(
        "agent-a",
        "connection-new",
        SessionNotification::new(
            SessionId::new("new-session"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("new generation"),
            ))),
        ),
    )
    .expect("purged quota must admit a new-generation pre-bind update");

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn replacing_connection_revokes_all_bound_state_and_reaps_terminal() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-old", config(false, true))
        .unwrap();
    ctx.bind_session("bound-session", binding("agent-a", "conv-a", &home))
        .unwrap();
    ctx.set_current_run("agent-a", "bound-session", "run-old");
    ctx.set_loading("agent-a", "bound-session", true);
    ctx.begin_capture_operation(
        "agent-a",
        "connection-old",
        "bound-session",
        "session/prompt",
    )
    .unwrap();
    ctx.handle_notification(
        "agent-a",
        "connection-old",
        SessionNotification::new(
            SessionId::new("unbound-session"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("pending old generation"),
            ))),
        ),
    )
    .unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "callbacks::state_tests::terminal_long_lived_child_fixture",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn old-generation terminal child");
    let reaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    ctx.terminals.lock().insert(
        "old-terminal".into(),
        TerminalHandle {
            owner: SessionKey::new("agent-a", "bound-session"),
            child: Some(child),
            process_tree: None,
            readers: Vec::new(),
            output: Arc::new(Mutex::new(TerminalOutput::default())),
            exit_status: None,
            _activity: None,
            reaped: Some(Arc::clone(&reaped)),
        },
    );

    ctx.configure_agent("agent-a", "connection-old", config(false, true))
        .unwrap();
    assert!(ctx.is_session_bound("agent-a", "bound-session"));
    assert!(ctx.terminals.lock().contains_key("old-terminal"));
    assert!(!reaped.load(std::sync::atomic::Ordering::SeqCst));

    ctx.configure_agent("agent-a", "connection-new", config(false, true))
        .unwrap();

    let bound_key = SessionKey::new("agent-a", "bound-session");
    assert!(!ctx.is_session_bound("agent-a", "bound-session"));
    assert!(!ctx.current_run.read().contains_key(&bound_key));
    assert!(!ctx.loading_sessions.read().contains(&bound_key));
    assert!(!ctx.capture_budgets.lock().contains_key(&bound_key));
    assert!(
        !ctx.capture_failures
            .lock()
            .keys()
            .any(|key| key.session.agent_id == "agent-a")
    );
    assert!(
        !ctx.pending_notifications
            .lock()
            .sessions
            .keys()
            .any(|key| key.agent_id == "agent-a")
    );
    assert!(ctx.terminals.lock().is_empty());
    assert!(
        reaped.load(std::sync::atomic::Ordering::SeqCst),
        "replacement must kill and reap old-generation terminal children"
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn connection_replacement_cannot_race_after_capture_validation() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-old", config(false, false))
        .unwrap();
    let gate = CallbackTestGate::new();
    *ctx.capture_after_connection_gate.lock() = Some(gate.clone());
    let capture_ctx = Arc::clone(&ctx);
    let capture = thread::spawn(move || {
        capture_ctx.handle_notification(
            "agent-a",
            "connection-old",
            SessionNotification::new(
                SessionId::new("racing-session"),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new("old generation"),
                ))),
            ),
        )
    });

    gate.reached.wait();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let configure_ctx = Arc::clone(&ctx);
    let configure = thread::spawn(move || {
        started_tx.send(()).unwrap();
        configure_ctx
            .configure_agent("agent-a", "connection-new", config(false, false))
            .unwrap();
        done_tx.send(()).unwrap();
    });
    started_rx.recv().unwrap();
    let completed_before_capture = done_rx
        .recv_timeout(std::time::Duration::from_millis(50))
        .is_ok();
    gate.resume.wait();
    capture
        .join()
        .expect("capture thread")
        .expect("old capture completes before replacement");
    if !completed_before_capture {
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("replacement completes after capture");
    }
    configure.join().expect("configure thread");

    assert!(
        !completed_before_capture,
        "replacement must wait for the validated capture generation lease"
    );
    let pending = ctx.pending_notifications.lock();
    assert_eq!(pending.count, 0);
    assert_eq!(pending.bytes, 0);
    assert!(pending.sessions.is_empty());
    drop(pending);

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn unbound_notification_sessions_have_a_global_quota() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    for index in 0..MAX_PENDING_SESSIONS {
        let update = SessionNotification::new(
            SessionId::new(format!("session-{index}")),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("queued"),
            ))),
        );
        ctx.handle_notification("agent-a", "connection-a", update)
            .unwrap();
    }
    let overflow = SessionNotification::new(
        SessionId::new("overflow"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "queued",
        )))),
    );
    assert!(
        ctx.handle_notification("agent-a", "connection-a", overflow)
            .unwrap_err()
            .to_string()
            .contains("too many unbound sessions")
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn bound_session_updates_enforce_single_and_turn_budgets() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-budget".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-budget".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    ctx.bind_session("session-budget", binding("agent-a", "conv-budget", &home))
        .unwrap();

    let oversized = SessionNotification::new(
        SessionId::new("session-budget"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    assert!(
        ctx.handle_notification("agent-a", "connection-a", oversized)
            .unwrap_err()
            .to_string()
            .contains("session update exceeds")
    );

    ctx.capture_budgets.lock().insert(
        SessionKey::new("agent-a", "session-budget"),
        CaptureBudget {
            updates: MAX_CAPTURE_UPDATES_PER_TURN,
            bytes: 0,
        },
    );
    let small = SessionNotification::new(
        SessionId::new("session-budget"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "small",
        )))),
    );
    assert!(
        ctx.handle_notification("agent-a", "connection-a", small)
            .unwrap_err()
            .to_string()
            .contains("capture budget exceeded")
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn capture_failure_ledger_keeps_first_error_and_clears_at_session_boundaries() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-ledger".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-ledger".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    let session_binding = binding("agent-a", "conv-ledger", &home);
    ctx.bind_session("session-ledger", session_binding.clone())
        .unwrap();
    ctx.set_loading("agent-a", "session-ledger", true);
    ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
        .unwrap();

    let oversized = SessionNotification::new(
        SessionId::new("session-ledger"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", oversized)
        .unwrap_err();
    ctx.capture_budgets.lock().insert(
        SessionKey::new("agent-a", "session-ledger"),
        CaptureBudget {
            updates: MAX_CAPTURE_UPDATES_PER_TURN,
            bytes: 0,
        },
    );
    let budget_overflow = SessionNotification::new(
        SessionId::new("session-ledger"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "small",
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", budget_overflow)
        .unwrap_err();

    let first = ctx
        .take_capture_failure("agent-a", "connection-a", "session-ledger")
        .expect("active operation must retain its capture failure")
        .to_string();
    assert!(first.contains("session update exceeds"));
    assert!(!first.contains("capture budget exceeded"));
    assert!(
        ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
            .is_none()
    );

    ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
        .unwrap();
    let stale = SessionNotification::new(
        SessionId::new("session-ledger"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    ctx.handle_notification("agent-a", "connection-a", stale)
        .unwrap_err();
    ctx.bind_session("session-ledger", session_binding.clone())
        .unwrap();
    assert!(
        ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
            .is_none(),
        "binding must clear stale capture correlation"
    );

    ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
        .unwrap();
    ctx.unbind_session("agent-a", "session-ledger");
    assert!(
        ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
            .is_none(),
        "unbinding must clear capture correlation"
    );

    ctx.bind_session("session-ledger", session_binding).unwrap();
    ctx.begin_capture_operation("agent-a", "connection-a", "session-ledger", "session/load")
        .unwrap();
    ctx.revoke_agent("agent-a").unwrap();
    assert!(
        ctx.take_capture_failure("agent-a", "connection-a", "session-ledger")
            .is_none(),
        "revoking an endpoint must clear capture correlation"
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn stale_begin_after_current_begin_is_rejected_without_poisoning_current_ledger() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-old", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-generation-ledger".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-generation-ledger".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    ctx.bind_session(
        "session-generation-ledger",
        binding("agent-a", "conv-generation-ledger", &home),
    )
    .unwrap();
    ctx.configure_agent("agent-a", "connection-new", config(false, false))
        .unwrap();
    let begun = Arc::new(std::sync::Barrier::new(2));
    let begin_ctx = Arc::clone(&ctx);
    let begin_barrier = Arc::clone(&begun);
    let current_begin = thread::spawn(move || {
        begin_ctx
            .begin_capture_operation(
                "agent-a",
                "connection-new",
                "session-generation-ledger",
                "session/prompt",
            )
            .unwrap();
        begin_barrier.wait();
    });
    begun.wait();
    let stale_begin_error = ctx
        .begin_capture_operation(
            "agent-a",
            "connection-old",
            "session-generation-ledger",
            "session/prompt",
        )
        .expect_err("stale command loop must not overwrite the current ledger");
    assert!(stale_begin_error.to_string().contains("stale connection"));
    current_begin.join().expect("current begin thread");
    let stale = SessionNotification::new(
        SessionId::new("session-generation-ledger"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "stale",
        )))),
    );
    let stale_error = ctx
        .handle_notification("agent-a", "connection-old", stale)
        .expect_err("old generation must be rejected");
    assert!(stale_error.to_string().contains("stale connection"));
    assert!(
        ctx.take_capture_failure("agent-a", "connection-old", "session-generation-ledger")
            .is_none(),
        "old generation must not consume the current operation ledger"
    );

    let oversized = SessionNotification::new(
        SessionId::new("session-generation-ledger"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    ctx.handle_notification("agent-a", "connection-new", oversized)
        .expect_err("current generation oversized update");
    let current = ctx
        .take_capture_failure("agent-a", "connection-new", "session-generation-ledger")
        .expect("current generation retains its capture failure")
        .to_string();
    assert!(current.contains("session update exceeds"));
    assert!(!current.contains("stale connection"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[tokio::test(flavor = "current_thread")]
async fn active_prompt_captures_failure_before_queued_replacement_swaps_generation() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-old", config(false, false))
        .unwrap();
    ctx.store()
        .create_conversation(&NewConversation {
            id: "conv-active-replacement".into(),
            agent_id: "agent-a".into(),
            agent_session_id: "session-active-replacement".into(),
            cwd: Some(home.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    ctx.bind_session(
        "session-active-replacement",
        binding("agent-a", "conv-active-replacement", &home),
    )
    .unwrap();
    let command = ctx
        .acquire_connection_lease("agent-a", "connection-old")
        .await
        .unwrap();
    ctx.begin_capture_operation(
        "agent-a",
        "connection-old",
        "session-active-replacement",
        "session/prompt",
    )
    .unwrap();

    let replacement_ctx = Arc::clone(&ctx);
    let replacement = tokio::spawn(async move {
        replacement_ctx
            .configure_agent_async("agent-a", "connection-new", config(false, false))
            .await;
    });
    while ctx
        .generation_gate("agent-a")
        .waiting_writers
        .load(Ordering::SeqCst)
        == 0
    {
        tokio::task::yield_now().await;
    }

    let callback = ctx
        .try_acquire_connection_lease("agent-a", "connection-old")
        .expect("active command admits its nested current-generation callback");
    let oversized = SessionNotification::new(
        SessionId::new("session-active-replacement"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "x".repeat(MAX_PENDING_SINGLE_NOTIFICATION_BYTES),
        )))),
    );
    ctx.handle_notification("agent-a", "connection-old", oversized)
        .expect_err("oversized active-generation update must fail capture");
    drop(callback);
    assert!(
        !replacement.is_finished(),
        "replacement must wait for the active command to finish"
    );
    let prompt_error = ctx
        .take_capture_failure("agent-a", "connection-old", "session-active-replacement")
        .expect("prompt response must observe the queued update failure");
    assert!(prompt_error.to_string().contains("session/prompt"));

    drop(command);
    replacement.await.expect("replacement task");
    let stale_command = match ctx
        .acquire_connection_lease("agent-a", "connection-old")
        .await
    {
        Ok(_) => panic!("queued old-generation command must be rejected after replacement"),
        Err(error) => error,
    };
    assert!(stale_command.to_string().contains("stale connection"));
    ctx.connection("agent-a", "connection-new")
        .expect("replacement generation installed");

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}
