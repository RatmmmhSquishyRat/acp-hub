#[ignore = "spawned as a descendant that keeps terminal pipes open"]
#[test]
fn terminal_descendant_holds_pipe_fixture() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[ignore = "spawned as a terminal parent fixture"]
#[test]
// Intentionally orphan the descendant: the regression requires the terminal parent to exit
// while its descendant remains alive with inherited output pipes.
#[allow(clippy::zombie_processes)]
fn terminal_parent_spawns_pipe_holding_descendant_fixture() {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "callbacks::state_tests::terminal_descendant_holds_pipe_fixture",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn pipe-holding descendant");
    println!("acp-hub-descendant-pid={}", child.id());
}

#[test]
fn terminal_child_fixture() {
    println!("acp-hub-terminal-fixture");
}

#[ignore = "spawned as a long-lived terminal child fixture"]
#[test]
fn terminal_long_lived_child_fixture() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[ignore = "spawned as a short-lived pre-assignment descendant fixture"]
#[test]
fn terminal_short_lived_descendant_fixture() {
    std::thread::sleep(std::time::Duration::from_secs(2));
}

#[ignore = "spawned as an immediate descendant parent fixture"]
#[test]
// Intentionally keep the child handle unreaped: the terminal cleanup under test must kill
// this parent and its descendant as one Job/process tree.
#[allow(clippy::zombie_processes)]
fn terminal_immediate_descendant_parent_fixture() {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "callbacks::state_tests::terminal_short_lived_descendant_fixture",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn immediate descendant");
    let marker =
        std::env::var_os("ACP_HUB_TERMINAL_ASSIGNMENT_MARKER").expect("assignment marker path");
    fs::write(marker, child.id().to_string()).expect("write descendant marker");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_callbacks_capture_output_wait_and_release() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let executable = std::env::current_exe().unwrap();
    let request = CreateTerminalRequest::new(
        SessionId::new("session"),
        executable.to_string_lossy().into_owned(),
    )
    .args(vec![
        "--exact".to_string(),
        "callbacks::state_tests::terminal_child_fixture".to_string(),
        "--nocapture".to_string(),
    ])
    .cwd(home.clone())
    .output_byte_limit(64 * 1024);
    let created = ctx
        .handle_terminal_create("agent-a", "connection-a", &request)
        .unwrap();
    let terminal_id = created.terminal_id;

    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        ctx.handle_terminal_wait(
            "agent-a",
            "connection-a",
            &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let output = ctx
            .handle_terminal_output(
                "agent-a",
                "connection-a",
                &TerminalOutputRequest::new(SessionId::new("session"), terminal_id.clone()),
            )
            .unwrap();
        if output.output.contains("acp-hub-terminal-fixture") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "terminal output readers did not publish the fixture output"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    ctx.handle_terminal_release(
        "agent-a",
        "connection-a",
        &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
    )
    .unwrap();

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_terminal_kill_preserves_reaped_exit_status() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let executable = std::env::current_exe().unwrap();
    let request = CreateTerminalRequest::new(
        SessionId::new("session"),
        executable.to_string_lossy().into_owned(),
    )
    .args(vec![
        "--exact".to_string(),
        "callbacks::state_tests::terminal_child_fixture".to_string(),
        "--nocapture".to_string(),
    ])
    .cwd(home.clone());
    let terminal_id = ctx
        .handle_terminal_create("agent-a", "connection-a", &request)
        .unwrap()
        .terminal_id;
    let original = ctx
        .handle_terminal_wait(
            "agent-a",
            "connection-a",
            &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
        )
        .await
        .unwrap()
        .exit_status;

    let kill = KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone());
    ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
        .unwrap();
    ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
        .unwrap();

    let after_repeated_kill = ctx
        .handle_terminal_wait(
            "agent-a",
            "connection-a",
            &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
        )
        .await
        .expect("cached exit status must survive repeated kill calls")
        .exit_status;
    assert_eq!(after_repeated_kill.exit_code, original.exit_code);
    ctx.handle_terminal_release(
        "agent-a",
        "connection-a",
        &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
    )
    .unwrap();

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_kill_error_retains_child_for_retry() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let executable = std::env::current_exe().unwrap();
    let request = CreateTerminalRequest::new(
        SessionId::new("session"),
        executable.to_string_lossy().into_owned(),
    )
    .args(vec![
        "--ignored".to_string(),
        "--exact".to_string(),
        "callbacks::state_tests::terminal_long_lived_child_fixture".to_string(),
        "--nocapture".to_string(),
    ])
    .cwd(home.clone());
    let terminal_id = ctx
        .handle_terminal_create("agent-a", "connection-a", &request)
        .unwrap()
        .terminal_id;
    let terminal_key = terminal_id.to_string();
    let kill = KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone());
    ctx.terminal_kill_error_once
        .store(true, std::sync::atomic::Ordering::SeqCst);

    ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
        .expect_err("forced kill error");
    assert!(
        ctx.terminals
            .lock()
            .get(&terminal_key)
            .and_then(|handle| handle.child.as_ref())
            .is_some(),
        "a fallible kill must retain the child handle for retry"
    );

    ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
        .expect("retry terminal kill");
    ctx.handle_terminal_release(
        "agent-a",
        "connection-a",
        &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
    )
    .unwrap();

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_kill_reaps_descendants_before_joining_output_readers() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let request = CreateTerminalRequest::new(
        SessionId::new("session"),
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )
    .args(vec![
        "--ignored".to_string(),
        "--exact".to_string(),
        "callbacks::state_tests::terminal_parent_spawns_pipe_holding_descendant_fixture"
            .to_string(),
        "--nocapture".to_string(),
    ])
    .cwd(home.clone());
    let terminal_id = ctx
        .handle_terminal_create("agent-a", "connection-a", &request)
        .unwrap()
        .terminal_id;
    ctx.handle_terminal_wait(
        "agent-a",
        "connection-a",
        &WaitForTerminalExitRequest::new(SessionId::new("session"), terminal_id.clone()),
    )
    .await
    .expect("terminal parent exits while its descendant keeps the pipes open");

    let terminal_key = terminal_id.to_string();
    let kill = KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone());
    let kill_ctx = Arc::clone(&ctx);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            kill_ctx.handle_terminal_kill("agent-a", "connection-a", &kill)
        }),
    )
    .await
    .expect("process-tree cleanup must not wait for the descendant's sleep")
    .expect("kill task")
    .expect("kill terminal process tree");

    {
        let terminals = ctx.terminals.lock();
        let handle = terminals
            .get(&terminal_key)
            .expect("terminal remains cached");
        assert!(handle.child.is_none(), "terminal parent must be reaped");
        assert!(
            handle.process_tree.is_none(),
            "the process tree guard must be closed after termination"
        );
        assert!(
            handle.readers.is_empty(),
            "all terminal output readers must be joined before kill returns"
        );
    }
    ctx.handle_terminal_release(
        "agent-a",
        "connection-a",
        &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
    )
    .unwrap();

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[cfg(windows)]
#[test]
fn windows_terminal_is_suspended_until_job_assignment() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let marker = home.join("descendant-started");
    let gate = CallbackTestGate::new();
    *ctx.terminal_job_assignment_gate.lock() = Some(gate.clone());
    let request = CreateTerminalRequest::new(
        SessionId::new("session"),
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )
    .args(vec![
        "--ignored".to_string(),
        "--exact".to_string(),
        "callbacks::state_tests::terminal_immediate_descendant_parent_fixture".to_string(),
        "--nocapture".to_string(),
    ])
    .env(vec![EnvVariable::new(
        "ACP_HUB_TERMINAL_ASSIGNMENT_MARKER",
        marker.to_string_lossy().into_owned(),
    )])
    .cwd(home.clone());
    let create_ctx = Arc::clone(&ctx);
    let create = thread::spawn(move || {
        create_ctx.handle_terminal_create("agent-a", "connection-a", &request)
    });

    gate.reached.wait();
    let premature_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !marker.exists() && std::time::Instant::now() < premature_deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let ran_before_assignment = marker.exists();
    gate.resume.wait();
    let terminal_id = create
        .join()
        .expect("terminal create thread")
        .expect("create terminal after job assignment")
        .terminal_id;

    let started_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() && std::time::Instant::now() < started_deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        marker.exists(),
        "terminal must run after assignment and resume"
    );
    let terminal_key = terminal_id.to_string();
    ctx.handle_terminal_kill(
        "agent-a",
        "connection-a",
        &KillTerminalRequest::new(SessionId::new("session"), terminal_id.clone()),
    )
    .expect("kill assigned terminal tree");
    {
        let terminals = ctx.terminals.lock();
        let handle = terminals.get(&terminal_key).expect("cached terminal");
        assert!(handle.readers.is_empty(), "kill must join terminal readers");
        assert!(handle.process_tree.is_none(), "kill must close the Job");
    }
    ctx.handle_terminal_release(
        "agent-a",
        "connection-a",
        &ReleaseTerminalRequest::new(SessionId::new("session"), terminal_id),
    )
    .unwrap();
    assert!(
        !ran_before_assignment,
        "terminal spawned its descendant before Job assignment"
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn terminal_spawn_racing_unbind_is_reaped_without_consuming_quota() {
    let (ctx, home) = context();
    ctx.configure_agent("agent-a", "connection-a", config(true, true))
        .unwrap();
    ctx.bind_session("session", binding("agent-a", "conv-a", &home))
        .unwrap();
    let gate = TerminalSpawnTestGate::new();
    *ctx.terminal_spawn_gate.lock() = Some(gate.clone());
    let executable = std::env::current_exe().unwrap();
    let request = CreateTerminalRequest::new(
        SessionId::new("session"),
        executable.to_string_lossy().into_owned(),
    )
    .args(vec![
        "--ignored".to_string(),
        "--exact".to_string(),
        "callbacks::state_tests::terminal_long_lived_child_fixture".to_string(),
        "--nocapture".to_string(),
    ])
    .cwd(home.clone());
    let create_ctx = Arc::clone(&ctx);
    let create = thread::spawn(move || {
        create_ctx.handle_terminal_create("agent-a", "connection-a", &request)
    });

    gate.callback.reached.wait();
    let teardown_started = Arc::new(std::sync::Barrier::new(2));
    let teardown_ctx = Arc::clone(&ctx);
    let teardown_marker = Arc::clone(&teardown_started);
    let teardown = thread::spawn(move || {
        teardown_marker.wait();
        teardown_ctx.unbind_session("agent-a", "session");
    });
    teardown_started.wait();
    gate.callback.resume.wait();

    let _ = create.join().expect("terminal create thread");
    teardown.join().expect("session teardown thread");
    assert!(
        gate.reaped.load(std::sync::atomic::Ordering::SeqCst),
        "teardown must kill and reap the spawned child"
    );
    assert!(
        ctx.terminals.lock().is_empty(),
        "teardown must remove the terminal and release its quota slot"
    );

    drop(ctx);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn terminal_output_limit_keeps_utf8_tail() {
    let mut state = TerminalOutput {
        text: "prefix-你好-tail".into(),
        truncated: false,
    };

    truncate_from_start(&mut state, 8);

    assert!(state.truncated);
    assert!(state.text.is_char_boundary(0));
    assert!(state.text.len() <= 8);
    assert!(state.text.ends_with("-tail"));
}
