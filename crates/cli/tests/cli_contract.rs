use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use acp_hub::store::{MessageSource, NewMessage, Store};
use serde_json::{Value, json};

fn acp_hub() -> Command {
    Command::new(env!("CARGO_BIN_EXE_acp-hub"))
}

fn assert_output_hides(output: &Output, secrets: &[&str], context: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for (stream_name, stream) in [("stdout", stdout.as_ref()), ("stderr", stderr.as_ref())] {
        for secret in secrets {
            assert!(
                !stream.contains(secret),
                "{context} disclosed {secret:?} on {stream_name}: {stream}"
            );
        }
        assert!(
            !stream.contains("alice@") && !stream.contains("/private"),
            "{context} disclosed URL credentials/path on {stream_name}: {stream}"
        );
    }
}

fn captured_child_streams(child: &mut Child) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .expect("captures child stdout")
        .read_to_string(&mut stdout)
        .expect("reads child stdout");
    child
        .stderr
        .take()
        .expect("captures child stderr")
        .read_to_string(&mut stderr)
        .expect("reads child stderr");
    (stdout, stderr)
}

fn wait_for_file(child: &mut Child, path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if child.try_wait().expect("polls child").is_some() {
            let (stdout, stderr) = captured_child_streams(child);
            panic!(
                "child exited before {} appeared\nstdout: {stdout}\nstderr: {stderr}",
                path.display()
            );
        }
        if Instant::now() >= deadline {
            child.kill().expect("stops timed-out child");
            child.wait().expect("waits for timed-out child");
            let (stdout, stderr) = captured_child_streams(child);
            panic!(
                "timed out waiting for {}\nstdout: {stdout}\nstderr: {stderr}",
                path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_daemon_cleanup(home: &Path) {
    let transient = ["daemon.json", "daemon.id", "daemon.sock"];
    let lock = home.join("daemon.lock");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = transient
            .iter()
            .filter(|name| home.join(name).exists())
            .copied()
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            match fs::remove_file(&lock) {
                Ok(()) => return,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
                Err(err) if Instant::now() >= deadline => {
                    panic!(
                        "daemon released state but still holds {}: {err}",
                        lock.display()
                    );
                }
                Err(_) => {}
            }
        } else {
            assert!(
                Instant::now() < deadline,
                "daemon artifacts did not clear: {remaining:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn top_level_help_exposes_the_canonical_commands() {
    let output = acp_hub().arg("--help").output().expect("runs acp-hub");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help is utf-8");
    for command in ["agent", "proxy", "conv", "send", "cancel", "search", "mcp"] {
        assert!(
            stdout
                .lines()
                .any(|line| line.trim_start().starts_with(command)),
            "missing top-level command {command}:\n{stdout}"
        );
    }
    assert!(!stdout.contains("conv send"));
    assert!(!stdout.contains("conv search"));
}

#[test]
fn session_discovery_has_no_invented_import_flag() {
    let output = acp_hub()
        .args(["agent", "sessions", "--help"])
        .output()
        .expect("runs sessions help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help is utf-8");
    assert!(!stdout.contains("--import"));
}

#[test]
fn search_help_exposes_pagination() {
    let output = acp_hub()
        .args(["search", "--help"])
        .output()
        .expect("runs search help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help is utf-8");
    assert!(stdout.contains("--limit"));
    assert!(stdout.contains("--offset"));
}

#[test]
fn unsafe_search_limits_fail_without_creating_daemon_artifacts() {
    let home = tempfile::tempdir().expect("creates isolated ACP Hub home");
    let output = acp_hub()
        .arg("--home")
        .arg(home.path())
        .args(["search", "needle", "--limit", "0"])
        .output()
        .expect("runs search validation");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("error is utf-8");
    assert!(stderr.contains("limit must be between 1 and 200"));
    for artifact in ["daemon.json", "daemon.id", "daemon.lock", "daemon.sock"] {
        assert!(
            !home.path().join(artifact).exists(),
            "invalid CLI input created {artifact}"
        );
    }
}

#[test]
fn send_rejects_non_advancing_message_page_cursor_without_looping() {
    const RPC_FIXTURE: &str = r#"
const fs = require("fs");
const net = require("net");
const endpoint = process.argv[2];
const ready = process.argv[3];
const capture = process.argv[4];
const server = net.createServer(socket => {
  let buffered = "";
  socket.setEncoding("utf8");
  socket.on("data", chunk => {
    buffered += chunk;
    for (;;) {
      const newline = buffered.indexOf("\n");
      if (newline < 0) break;
      const line = buffered.slice(0, newline);
      buffered = buffered.slice(newline + 1);
      if (!line.trim()) continue;
      const request = JSON.parse(line);
      let result;
      if (request.method === "hub/conv/send") {
        result = {
          convId: "cursor-conv",
          runId: "cursor-run",
          stopReason: "end_turn",
          promptSeq: 41
        };
      } else if (request.method === "hub/conv/messages_page") {
        fs.writeFileSync(capture, JSON.stringify(request.params));
        const run1 = {
          id: "visible-once",
          conv_id: "cursor-conv",
          run_id: "cursor-run",
          role: "assistant",
          kind: "message",
          body_text: "visible once",
          seq: 42
        };
        const run2 = {
          id: "must-not-leak",
          conv_id: "cursor-conv",
          run_id: "later-run",
          role: "assistant",
          kind: "message",
          body_text: "concurrent second turn",
          seq: 43
        };
        const items = request.params.runId === "cursor-run" ? [run1] : [run1, run2];
        result = { items, nextCursor: "stuck-cursor", nextOffset: 0, total: items.length };
      } else {
        socket.write(JSON.stringify({
          jsonrpc: "2.0",
          id: request.id,
          error: { code: -32601, message: "method not found" }
        }) + "\n");
        continue;
      }
      socket.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    }
  });
});
server.listen(endpoint, () => fs.writeFileSync(ready, "ready"));
"#;

    let home = tempfile::tempdir().expect("creates isolated ACP Hub home");
    let fixture_path = home.path().join("non-advancing-rpc.cjs");
    let ready_path = home.path().join("rpc-ready");
    let capture_path = home.path().join("messages-page-params.json");
    fs::write(&fixture_path, RPC_FIXTURE).expect("writes fake daemon fixture");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    #[cfg(windows)]
    let endpoint = format!(r"\\.\pipe\acp-hub-non-advancing-{unique}");
    #[cfg(unix)]
    let endpoint = home
        .path()
        .join(format!("non-advancing-{unique}.sock"))
        .to_string_lossy()
        .into_owned();

    let mut server = Command::new("node")
        .arg(&fixture_path)
        .arg(&endpoint)
        .arg(&ready_path)
        .arg(&capture_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts fake daemon");
    wait_for_file(&mut server, &ready_path);
    fs::write(
        home.path().join("daemon.json"),
        serde_json::to_vec(&json!({
            "pid": server.id(),
            "endpoint": endpoint,
            "daemon_id": "non-advancing-fixture",
            "started_at": "2026-01-01T00:00:00Z"
        }))
        .expect("serializes daemon metadata"),
    )
    .expect("writes daemon metadata");

    let mut send = acp_hub()
        .arg("--home")
        .arg(home.path())
        .args(["send", "cursor-conv", "--text", "hello", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts CLI send");
    let deadline = Instant::now() + Duration::from_secs(5);
    while send.try_wait().expect("polls CLI send").is_none() {
        if Instant::now() >= deadline {
            send.kill().expect("stops looping CLI send");
            send.wait().expect("waits for looping CLI send");
            server.kill().expect("stops fake daemon");
            server.wait().expect("waits for fake daemon");
            panic!("CLI send looped on a non-advancing nextCursor");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = send.wait_with_output().expect("collects CLI send output");
    server.kill().expect("stops fake daemon");
    server.wait().expect("waits for fake daemon");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("CLI error is UTF-8");
    assert!(
        stderr.contains("message page cursor did not advance"),
        "unexpected safe error: {stderr}"
    );
    assert!(!stderr.contains(&endpoint));
    let captured: Value = serde_json::from_slice(
        &fs::read(&capture_path).expect("messages page request was captured"),
    )
    .expect("captured page params are JSON");
    assert_eq!(
        captured.get("afterSeq"),
        Some(&json!(41)),
        "CLI must page after the promptSeq returned by hub/conv/send: {captured}"
    );
    assert_eq!(
        captured.get("runId"),
        Some(&json!("cursor-run")),
        "CLI must scope pages to the runId returned by hub/conv/send: {captured}"
    );
    assert_eq!(
        captured.get("cursor"),
        Some(&json!("stuck-cursor")),
        "CLI must pass the opaque continuation returned by the preceding page: {captured}"
    );
    let records = String::from_utf8(output.stdout)
        .expect("CLI output is UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("update is JSON"))
        .collect::<Vec<_>>();
    assert_eq!(
        records.len(),
        1,
        "message update was duplicated: {records:?}"
    );
    assert_eq!(records[0]["type"], json!("update"));
    assert_eq!(records[0]["message"]["seq"], json!(42));
}

#[test]
fn registry_process_output_redacts_every_configured_secret() {
    let home = tempfile::tempdir().expect("creates isolated ACP Hub home");
    let run = |args: &[&str]| {
        let mut command = acp_hub();
        command
            .arg("--home")
            .arg(home.path())
            .env("ACP_HUB_IDLE_TIMEOUT", "1")
            .args(args);
        command.output().expect("runs acp-hub")
    };
    let secrets = [
        "url-password-91",
        "query-token-82",
        "authorization-secret-73",
        "cookie-secret-64",
        "arg-secret-55",
        "arg-secret-46",
        "proxy-arg-secret-37",
        "proxy-env-secret-28",
    ];
    let registered = run(&[
        "agent",
        "add",
        "secret-endpoint",
        "--type",
        "http",
        "--url",
        "https://alice:url-password-91@example.invalid/private?token=query-token-82",
        "--header",
        "Authorization=Bearer authorization-secret-73",
        "--header",
        "Cookie=session=cookie-secret-64",
    ]);
    assert!(
        registered.status.success(),
        "{}",
        String::from_utf8_lossy(&registered.stderr)
    );
    assert_output_hides(&registered, &secrets, "agent add");

    let args_registered = run(&[
        "agent",
        "add",
        "arg-endpoint",
        "--command",
        "fixture-command",
        "--args",
        "arg-secret-55",
        "arg-secret-46",
    ]);
    assert!(
        args_registered.status.success(),
        "{}",
        String::from_utf8_lossy(&args_registered.stderr)
    );
    assert_output_hides(&args_registered, &secrets, "stdio agent add");

    let proxy_registered = run(&[
        "proxy",
        "add",
        "secret-proxy",
        "--command",
        "fixture-proxy-command",
        "--args",
        "proxy-arg-secret-37",
        "--env",
        "PROXY_TOKEN=proxy-env-secret-28",
    ]);
    assert!(
        proxy_registered.status.success(),
        "{}",
        String::from_utf8_lossy(&proxy_registered.stderr)
    );
    assert_output_hides(&proxy_registered, &secrets, "proxy add");
    let proxies = run(&["proxy", "list", "--json"]);
    assert!(proxies.status.success());
    assert_output_hides(&proxies, &secrets, "proxy list");
    let proxies_json: Value =
        serde_json::from_slice(&proxies.stdout).expect("proxy list output is JSON");
    assert!(
        proxies_json
            .get("secret-proxy")
            .is_some_and(Value::is_object),
        "keyword-containing proxy id or config was replaced: {proxies_json}"
    );
    assert_eq!(
        proxies_json.pointer("/secret-proxy/transport/args"),
        Some(&json!(["<redacted>"]))
    );
    assert_eq!(
        proxies_json.pointer("/secret-proxy/transport/env/PROXY_TOKEN"),
        Some(&json!("<redacted>"))
    );

    for (context, args, pointer) in [
        (
            "stdio agent list",
            &["agent", "list", "--json"][..],
            "/arg-endpoint/transport/args",
        ),
        (
            "stdio agent inspect",
            &["agent", "inspect", "arg-endpoint", "--json"][..],
            "/config/transport/args",
        ),
    ] {
        let output = run(args);
        assert!(output.status.success());
        assert_output_hides(&output, &secrets, context);
        let value: Value = serde_json::from_slice(&output.stdout).expect("registry output is JSON");
        let redacted_args = value
            .pointer(pointer)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{context} omitted stdio argument shape: {value}"));
        assert_eq!(redacted_args, &vec![json!("<redacted>"); 2]);
    }

    for (context, args) in [
        ("agent list", &["agent", "list", "--json"][..]),
        (
            "agent inspect",
            &["agent", "inspect", "secret-endpoint", "--json"][..],
        ),
    ] {
        let output = run(args);
        assert!(output.status.success());
        assert_output_hides(&output, &secrets, context);
        let value: Value =
            serde_json::from_slice(&output.stdout).expect("safe registry output is JSON");
        if context == "agent list" {
            assert!(
                value.get("secret-endpoint").is_some(),
                "keyword-containing agent id was replaced: {value}"
            );
        } else {
            assert_eq!(
                value.get("agentId"),
                Some(&json!("secret-endpoint")),
                "agent inspect did not preserve the endpoint id: {value}"
            );
        }
    }

    let rejected = run(&[
        "agent",
        "add",
        "rejected-secret-endpoint",
        "--type",
        "http",
        "--url",
        "https://alice:url-password-91@example.invalid/private?token=query-token-82",
        "--header",
        "Authorization=Bearer authorization-secret-73",
        "--header",
        "Cookie=session=cookie-secret-64",
        "--permission-policy",
        "not-a-policy",
    ]);
    assert!(!rejected.status.success());
    assert_output_hides(&rejected, &secrets, "failed agent add");

    let missing = run(&["agent", "inspect", "missing-agent", "--json"]);
    assert!(!missing.status.success());
    assert_output_hides(&missing, &secrets, "failed agent inspect");

    wait_for_daemon_cleanup(home.path());
}
#[test]
fn send_json_follows_byte_budget_pages_and_emits_final_record_last() {
    const AGENT_FIXTURE: &str = r#"
const fs = require("fs");
const readline = require("readline");
const rl = readline.createInterface({ input: process.stdin });
const respond = (id, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n");
rl.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    respond(message.id, {
      protocolVersion: message.params.protocolVersion,
      agentCapabilities: { loadSession: true, sessionCapabilities: {} },
      authMethods: []
    });
  } else if (message.method === "session/new") {
    respond(message.id, { sessionId: "byte-page-session" });
  } else if (message.method === "session/load") {
    respond(message.id, {});
  } else if (message.method === "session/prompt") {
    fs.writeFileSync(process.argv[2], "ready");
    const finish = () => {
      if (fs.existsSync(process.argv[3])) {
        respond(message.id, { stopReason: "end_turn" });
      } else {
        setTimeout(finish, 5);
      }
    };
    finish();
  }
});
"#;

    let home = tempfile::tempdir().expect("creates isolated ACP Hub home");
    let fixture_path = home.path().join("byte-page-agent.cjs");
    let ready_path = home.path().join("prompt-ready");
    let release_path = home.path().join("prompt-release");
    fs::write(&fixture_path, AGENT_FIXTURE).expect("writes synthetic ACP agent");

    let run = |args: &[&str]| {
        acp_hub()
            .arg("--home")
            .arg(home.path())
            .env("ACP_HUB_IDLE_TIMEOUT", "1")
            .args(args)
            .output()
            .expect("runs acp-hub")
    };
    let fixture_arg = fixture_path.to_str().expect("fixture path is UTF-8");
    let ready_arg = ready_path.to_str().expect("ready path is UTF-8");
    let release_arg = release_path.to_str().expect("release path is UTF-8");
    let registered = run(&[
        "agent",
        "add",
        "byte-page-agent",
        "--command",
        "node",
        "--args",
        fixture_arg,
        ready_arg,
        release_arg,
    ]);
    assert!(
        registered.status.success(),
        "agent registration failed: {}",
        String::from_utf8_lossy(&registered.stderr)
    );

    let home_arg = home.path().to_str().expect("home path is UTF-8");
    let created = run(&[
        "conv",
        "create",
        "byte-page-agent",
        "--cwd",
        home_arg,
        "--json",
    ]);
    assert!(
        created.status.success(),
        "conversation creation failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value =
        serde_json::from_slice(&created.stdout).expect("conversation response is JSON");
    let conv_id = created["convId"]
        .as_str()
        .expect("conversation id")
        .to_string();

    let mut send = acp_hub();
    let mut child = send
        .arg("--home")
        .arg(home.path())
        .env("ACP_HUB_IDLE_TIMEOUT", "1")
        .args(["send", &conv_id, "--text", "page everything", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts send process");
    wait_for_file(&mut child, &ready_path);

    let body = "x".repeat(2 * 1024 * 1024);
    let store = Store::open(home.path()).expect("opens projection store");
    let run_id = store
        .messages(&conv_id, false)
        .expect("reads active prompt")
        .into_iter()
        .rev()
        .find(|message| message.role == "user")
        .and_then(|message| message.run_id)
        .expect("active prompt carries its run id");
    let expected_sequences = (0..3)
        .map(|index| {
            store
                .append_message(&NewMessage {
                    id: format!("byte-page-{index}"),
                    conv_id: conv_id.clone(),
                    run_id: Some(run_id.clone()),
                    source: MessageSource::LocalTurn,
                    role: "assistant".to_string(),
                    kind: Some("message".to_string()),
                    content_json: json!({"type": "text", "text": body}),
                    body_text: body.clone(),
                })
                .expect("inserts run-scoped byte-budget fixture")
        })
        .collect::<Vec<_>>();
    assert_eq!(expected_sequences.len(), 3);
    drop(store);
    fs::write(&release_path, b"release").expect("releases synthetic prompt");

    let output = child.wait_with_output().expect("waits for send process");
    assert!(
        output.status.success(),
        "send failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = String::from_utf8(output.stdout)
        .expect("send output is UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("send record is JSON"))
        .collect::<Vec<_>>();
    let emitted_sequences = records
        .iter()
        .filter(|record| record["type"] == "update")
        .map(|record| record["message"]["seq"].as_i64().expect("message sequence"))
        .collect::<Vec<_>>();
    assert_eq!(
        emitted_sequences, expected_sequences,
        "every byte-budget page must be emitted once in sequence order"
    );
    assert_eq!(
        records.last().and_then(|record| record["type"].as_str()),
        Some("final"),
        "the terminal record must follow every message update"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["type"] == "final")
            .count(),
        1
    );

    wait_for_daemon_cleanup(home.path());
}
