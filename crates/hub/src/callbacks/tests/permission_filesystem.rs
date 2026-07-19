#[test]
fn same_session_id_is_isolated_by_agent() {
    let (ctx, home) = context();
    ctx.bind_session("shared", binding("agent-a", "conv-a", &home))
        .unwrap();
    ctx.bind_session("shared", binding("agent-b", "conv-b", &home))
        .unwrap();

    ctx.set_current_run("agent-a", "shared", "run-a");
    ctx.set_loading("agent-b", "shared", true);

    assert_eq!(ctx.binding("agent-a", "shared").unwrap().conv_id, "conv-a");
    assert_eq!(ctx.binding("agent-b", "shared").unwrap().conv_id, "conv-b");
    assert_eq!(
        ctx.run_for_session("agent-a", "shared").as_deref(),
        Some("run-a")
    );
    assert_eq!(ctx.run_for_session("agent-b", "shared"), None);
    assert!(!ctx.is_loading("agent-a", "shared"));
    assert!(ctx.is_loading("agent-b", "shared"));

    ctx.unbind_session("agent-a", "shared");
    assert!(ctx.binding("agent-a", "shared").is_err());
    assert!(ctx.binding("agent-b", "shared").is_ok());

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn fs_capability_is_scoped_to_the_calling_agent() {
    let (ctx, home) = context();
    let file = home.join("visible.txt");
    fs::write(&file, "scoped").expect("write fixture");
    ctx.configure_agent("agent-a", "connection-a", config(true, false))
        .unwrap();
    ctx.configure_agent("agent-b", "connection-b", config(false, false))
        .unwrap();
    ctx.bind_session("shared", binding("agent-a", "conv-a", &home))
        .unwrap();
    ctx.bind_session("shared", binding("agent-b", "conv-b", &home))
        .unwrap();
    let request = ReadTextFileRequest::new(SessionId::new("shared"), &file);

    assert_eq!(
        ctx.handle_read_text_file("agent-a", "connection-a", &request)
            .unwrap()
            .content,
        "scoped"
    );
    assert!(
        ctx.handle_read_text_file("agent-b", "connection-b", &request)
            .unwrap_err()
            .to_string()
            .contains("not enabled")
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn filesystem_callbacks_write_and_read_inside_the_bound_root() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, false))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let file = home.join("callback.txt");

    ctx.handle_write_text_file(
        "agent-a",
        "connection-a",
        &WriteTextFileRequest::new(SessionId::new("session"), &file, "callback body"),
    )
    .unwrap();
    let response = ctx
        .handle_read_text_file(
            "agent-a",
            "connection-a",
            &ReadTextFileRequest::new(SessionId::new("session"), &file),
        )
        .unwrap();
    assert_eq!(response.content, "callback body");

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn replaced_connection_cannot_inherit_bound_session_permissions() {
    let (ctx, home) = context();
    let file = home.join("visible.txt");
    fs::write(&file, "scoped").unwrap();
    ctx.configure_agent("agent-a", "old-connection", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    ctx.configure_agent("agent-a", "new-connection", config(false, false))
        .unwrap();
    let request = ReadTextFileRequest::new(SessionId::new("session"), &file);

    for connection_id in ["old-connection", "new-connection"] {
        let error = ctx
            .handle_read_text_file("agent-a", connection_id, &request)
            .expect_err("replacement must revoke the old bound session");
        assert!(
            error.to_string().contains("unknown session"),
            "unexpected replacement error for {connection_id}: {error}"
        );
    }
    assert!(!ctx.is_session_bound("agent-a", "session"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn terminal_requires_advertised_agent_capability() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, false))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let request = CreateTerminalRequest::new(SessionId::new("session"), "unused");

    let error = ctx
        .handle_terminal_create("agent-a", "connection-a", &request)
        .expect_err("disabled terminal must be rejected");
    assert!(error.to_string().contains("not enabled"));

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn terminal_ids_are_scoped_to_the_bound_agent_and_session() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.configure_agent("agent-b", "connection-b", config(true, true))
        .unwrap();
    ctx.bind_session("shared", binding("agent-a", "conv-a", &home))
        .unwrap();
    ctx.bind_session("shared", binding("agent-b", "conv-b", &home))
        .unwrap();
    ctx.terminals.lock().insert(
        "terminal-a".into(),
        TerminalHandle {
            owner: SessionKey::new("agent-a", "shared"),
            child: None,
            process_tree: None,
            readers: Vec::new(),
            output: Arc::new(Mutex::new(TerminalOutput::default())),
            exit_status: None,
            _activity: None,
            reaped: None,
            cleanup_failures_remaining: 0,
        },
    );

    ctx.verify_terminal_owner("agent-a", "connection-a", "shared", "terminal-a")
        .expect("owner can access terminal");
    assert!(
        ctx.verify_terminal_owner("agent-b", "connection-b", "shared", "terminal-a")
            .unwrap_err()
            .to_string()
            .contains("another agent")
    );

    ctx.unbind_session("agent-a", "shared");
    assert!(
        ctx.verify_terminal_owner("agent-a", "connection-a", "shared", "terminal-a")
            .unwrap_err()
            .to_string()
            .contains("unknown terminal")
    );
    assert!(ctx.binding("agent-b", "shared").is_ok());

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}
