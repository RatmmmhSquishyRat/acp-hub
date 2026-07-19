use std::fs;
use std::io::Read;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn receive_response(rx: &Receiver<String>, id: u64, transcript: &mut Vec<String>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = rx
            .recv_timeout(remaining)
            .unwrap_or_else(|err| panic!("timed out waiting for MCP response {id}: {err}"));
        transcript.push(line.clone());
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|err| panic!("invalid MCP JSON line {line:?}: {err}"));
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
}

fn write_message(child: &mut Child, value: Value) {
    let stdin = child.stdin.as_mut().expect("MCP stdin remains open");
    serde_json::to_writer(&mut *stdin, &value).expect("serializes MCP request");
    stdin.write_all(b"\n").expect("writes MCP request");
    stdin.flush().expect("flushes MCP request");
}

fn call_tool(
    child: &mut Child,
    rx: &Receiver<String>,
    transcript: &mut Vec<String>,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    write_message(
        child,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }),
    );
    receive_response(rx, id, transcript)
}

fn structured_content(response: &Value) -> &Value {
    response
        .pointer("/result/structuredContent")
        .unwrap_or_else(|| panic!("missing structured tool result: {response}"))
}

fn sorted_object_keys(value: &Value) -> Vec<String> {
    let mut keys = value
        .as_object()
        .expect("value is an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    keys
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
fn stdio_initializes_lists_tools_and_calls_the_daemon() {
    let home = tempfile::tempdir().expect("creates isolated ACP Hub home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_acp-hub"))
        .args([
            "--home",
            home.path().to_str().expect("UTF-8 temp path"),
            "mcp",
        ])
        .env("ACP_HUB_IDLE_TIMEOUT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts acp-hub mcp");

    let stdout = child.stdout.take().expect("captures MCP stdout");
    let (tx, rx) = mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut transcript = Vec::new();

    write_message(
        &mut child,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "acp-hub-test", "version": "0"}
            }
        }),
    );
    let initialized = receive_response(&rx, 1, &mut transcript);
    assert_eq!(
        initialized.pointer("/result/protocolVersion"),
        Some(&json!("2025-06-18"))
    );

    write_message(
        &mut child,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    );
    write_message(
        &mut child,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    let listed_tools = receive_response(&rx, 2, &mut transcript);
    let tools = listed_tools
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools/list returns tools");
    for required in [
        "list_agents",
        "register_agent",
        "inspect_agent",
        "remove_agent",
        "list_agent_sessions",
        "send_message",
        "cancel_conversation",
        "search",
    ] {
        assert!(
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(required)),
            "missing MCP tool {required}"
        );
    }
    assert!(tools.iter().all(|tool| tool.get("annotations").is_some()));
    for tool in tools {
        assert_eq!(
            tool.pointer("/inputSchema/additionalProperties"),
            Some(&json!(false)),
            "{} must publish a closed input schema: {}",
            tool.get("name")
                .and_then(Value::as_str)
                .unwrap_or("<unnamed>"),
            tool["inputSchema"]
        );
    }
    let register_schema = &tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("register_agent"))
        .expect("register agent tool")["inputSchema"];
    let register_schema_text =
        serde_json::to_string(register_schema).expect("serializes register schema");
    assert!(register_schema_text.contains("\"transport\""));
    assert!(!register_schema_text.contains("transport_type"));
    for discriminator in ["stdio", "http", "websocket"] {
        assert!(
            register_schema_text.contains(discriminator),
            "register schema does not expose {discriminator} discriminator: {register_schema_text}"
        );
    }

    let session_discovery = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("list_agent_sessions"))
        .expect("session discovery tool");
    assert_eq!(
        session_discovery.pointer("/annotations/readOnlyHint"),
        Some(&json!(false))
    );
    for name in ["send_message", "cancel_conversation"] {
        let tool = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            .expect("mutating MCP tool");
        assert_eq!(
            tool.pointer("/annotations/destructiveHint"),
            Some(&json!(true)),
            "{name} must require destructive-action handling"
        );
    }

    let contradictory = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        3,
        "register_agent",
        json!({
            "agent_id": "contradictory-agent",
            "transport": {
                "type": "stdio",
                "command": "must-not-register",
                "url": "https://mixed.invalid"
            }
        }),
    );
    assert_eq!(
        contradictory
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(true),
        "mixed transport fields must fail: {contradictory}"
    );
    let after_contradiction = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        4,
        "list_agents",
        json!({}),
    );
    assert!(
        structured_content(&after_contradiction)
            .get("contradictory-agent")
            .is_none(),
        "invalid registration mutated registry: {after_contradiction}"
    );

    for (id, agent_id, client_capabilities) in [
        (
            30,
            "misspelled-client-capability",
            json!({"termnial": true}),
        ),
        (
            31,
            "misspelled-fs-capability",
            json!({"fs": {"read_text_fil": true}}),
        ),
    ] {
        let rejected = call_tool(
            &mut child,
            &rx,
            &mut transcript,
            id,
            "register_agent",
            json!({
                "agent_id": agent_id,
                "transport": {"type": "stdio", "command": "must-not-register"},
                "client_capabilities": client_capabilities
            }),
        );
        assert_eq!(
            rejected.pointer("/result/isError").and_then(Value::as_bool),
            Some(true),
            "misspelled nested field was accepted: {rejected}"
        );
    }
    let after_nested_rejections = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        32,
        "list_agents",
        json!({}),
    );
    for rejected_id in ["misspelled-client-capability", "misspelled-fs-capability"] {
        assert!(
            structured_content(&after_nested_rejections)
                .get(rejected_id)
                .is_none(),
            "invalid nested config mutated registry: {after_nested_rejections}"
        );
    }

    let secrets = [
        "mcp-url-password-31",
        "mcp-query-token-42",
        "mcp-header-secret-53",
        "mcp-cookie-secret-54",
        "mcp-ws-password-65",
        "mcp-ws-query-76",
        "mcp-ws-header-secret-87",
        "mcp-arg-secret-64",
        "mcp-arg-secret-75",
        "mcp-env-secret-86",
        "mcp-env-secret-97",
        "mcp-proxy-arg-secret-18",
        "mcp-proxy-env-secret-29",
    ];
    let registered = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        5,
        "register_agent",
        json!({
            "agent_id": "monkey",
            "transport": {
                "type": "http",
                "url": "https://alice:mcp-url-password-31@example.invalid/private?token=mcp-query-token-42",
                "headers": {
                    "Authorization": "Bearer mcp-header-secret-53",
                    "Cookie": "session=mcp-cookie-secret-54"
                }
            }
        }),
    );
    assert_eq!(
        registered
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(false),
        "register failed: {registered}"
    );
    let websocket_registered = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        33,
        "register_agent",
        json!({
            "agent_id": "mcp-websocket-agent",
            "transport": {
                "type": "websocket",
                "url": "wss://bob:mcp-ws-password-65@socket.invalid/private?token=mcp-ws-query-76",
                "headers": {"X-Api-Key": "mcp-ws-header-secret-87"}
            }
        }),
    );
    assert_eq!(
        websocket_registered
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(false),
        "websocket registration failed: {websocket_registered}"
    );
    let proxy_registered = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        35,
        "register_proxy",
        json!({
            "proxy_id": "secret-proxy",
            "command": "fixture-proxy-command",
            "args": ["mcp-proxy-arg-secret-18"],
            "env": {"PROXY_TOKEN": "mcp-proxy-env-secret-29"}
        }),
    );
    assert_eq!(
        proxy_registered
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(false),
        "proxy registration failed: {proxy_registered}"
    );
    let proxies = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        36,
        "list_proxies",
        json!({}),
    );
    let listed_proxy = &structured_content(&proxies)["secret-proxy"];
    assert!(
        listed_proxy.is_object(),
        "keyword-containing proxy id or config was replaced: {proxies}"
    );
    assert_eq!(
        listed_proxy.pointer("/transport/type"),
        Some(&json!("stdio"))
    );
    assert_eq!(
        listed_proxy.pointer("/transport/args"),
        Some(&json!(["<redacted>"]))
    );
    assert_eq!(
        listed_proxy.pointer("/transport/env/PROXY_TOKEN"),
        Some(&json!("<redacted>"))
    );
    let proxy_removed = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        37,
        "remove_proxy",
        json!({"proxy_id": "secret-proxy"}),
    );
    assert_eq!(
        proxy_removed
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(false),
        "proxy removal failed: {proxy_removed}"
    );

    let agents = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        6,
        "list_agents",
        json!({}),
    );
    let listed = &structured_content(&agents)["monkey"];
    assert!(listed.is_object(), "registered agent missing: {agents}");
    assert_eq!(listed.pointer("/permission_policy"), Some(&json!("reject")));
    assert_eq!(
        listed.pointer("/client_capabilities/terminal"),
        Some(&json!(false))
    );
    assert_eq!(listed.pointer("/transport/type"), Some(&json!("http")));
    assert_eq!(
        listed.pointer("/transport/url"),
        Some(&json!("https://example.invalid/<redacted>"))
    );
    assert_eq!(
        sorted_object_keys(&listed["transport"]["headers"]),
        ["Authorization", "Cookie"]
    );
    assert!(
        listed["transport"]["headers"]
            .as_object()
            .expect("HTTP headers")
            .values()
            .all(|value| value == "<redacted>")
    );
    assert_eq!(listed.pointer("/proxy_chain"), Some(&json!([])));
    assert_eq!(
        listed.pointer("/client_capabilities/fs"),
        Some(&json!({
            "read_text_file": false,
            "write_text_file": false
        }))
    );
    assert!(
        listed
            .pointer("/client_capabilities/fs/allowed_roots")
            .is_none()
    );

    let listed_websocket = &structured_content(&agents)["mcp-websocket-agent"];
    assert_eq!(
        listed_websocket.pointer("/transport/type"),
        Some(&json!("websocket"))
    );
    assert_eq!(
        listed_websocket.pointer("/transport/url"),
        Some(&json!("wss://socket.invalid/<redacted>"))
    );
    assert_eq!(
        sorted_object_keys(&listed_websocket["transport"]["headers"]),
        ["X-Api-Key"]
    );
    assert_eq!(
        listed_websocket.pointer("/transport/headers/X-Api-Key"),
        Some(&json!("<redacted>"))
    );
    assert_eq!(
        listed_websocket.pointer("/permission_policy"),
        Some(&json!("reject"))
    );
    assert_eq!(
        listed_websocket.pointer("/client_capabilities/terminal"),
        Some(&json!(false))
    );
    let inspected = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        7,
        "inspect_agent",
        json!({"agent_id": "monkey"}),
    );
    assert_eq!(structured_content(&inspected)["agentId"], json!("monkey"));
    for response in [&agents, &inspected] {
        let encoded = serde_json::to_string(response).expect("serializes safe response");
        for secret in secrets {
            assert!(!encoded.contains(secret), "MCP response disclosed {secret}");
        }
        assert!(!encoded.contains("alice@") && !encoded.contains("/private"));
    }

    let removed = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        8,
        "remove_agent",
        json!({"agent_id": "monkey"}),
    );
    assert_eq!(
        removed.pointer("/result/isError").and_then(Value::as_bool),
        Some(false),
        "remove failed: {removed}"
    );
    let websocket_removed = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        34,
        "remove_agent",
        json!({"agent_id": "mcp-websocket-agent"}),
    );
    assert_eq!(
        websocket_removed
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(false),
        "websocket remove failed: {websocket_removed}"
    );
    let after_remove = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        9,
        "list_agents",
        json!({}),
    );
    assert!(
        structured_content(&after_remove).get("monkey").is_none(),
        "removed agent remains registered: {after_remove}"
    );
    assert!(
        structured_content(&after_remove)
            .get("mcp-websocket-agent")
            .is_none(),
        "removed websocket agent remains registered: {after_remove}"
    );

    let cancel_missing = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        10,
        "cancel_conversation",
        json!({"conv_id": "missing-conversation"}),
    );
    assert_eq!(
        cancel_missing.pointer("/error/data/kind"),
        Some(&json!("conversation")),
        "cancel should preserve resource error data: {cancel_missing}"
    );
    assert_eq!(
        cancel_missing.pointer("/error/data/id"),
        Some(&json!("missing-conversation"))
    );
    let sessions_missing = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        11,
        "list_agent_sessions",
        json!({"agent_id": "missing-agent"}),
    );
    assert_eq!(
        sessions_missing.pointer("/error/data/kind"),
        Some(&json!("agent")),
        "session discovery should preserve resource error data: {sessions_missing}"
    );

    const NO_LIST_AGENT: &str = r#"
const readline = require("readline");
const rl = readline.createInterface({ input: process.stdin });
const respond = (id, result) => process.stdout.write(JSON.stringify({jsonrpc: "2.0", id, result}) + "\n");
rl.on("line", line => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    respond(message.id, {
      protocolVersion: message.params.protocolVersion,
      agentCapabilities: {
        loadSession: false,
        sessionCapabilities: { delete: {} }
      },
      authMethods: []
    });
  } else if (message.method === "session/new") {
    respond(message.id, { sessionId: "strict-delete-session" });
  } else if (message.method === "session/delete") {
    respond(message.id, {});
  }
});
"#;
    let no_list_registered = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        12,
        "register_agent",
        json!({
            "agent_id": "no-list-agent",
            "transport": {
                "type": "stdio",
                "command": "node",
                "args": ["-e", NO_LIST_AGENT, "mcp-arg-secret-64", "mcp-arg-secret-75"],
                "env": {
                    "VISIBLE_ENV_ONE": "mcp-env-secret-86",
                    "VISIBLE_ENV_TWO": "mcp-env-secret-97"
                }
            }
        }),
    );
    assert_eq!(
        no_list_registered
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(false),
        "fixture registration failed: {no_list_registered}"
    );
    let agents_with_stdio = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        13,
        "list_agents",
        json!({}),
    );
    let stdio_args = structured_content(&agents_with_stdio)["no-list-agent"]["transport"]["args"]
        .as_array()
        .unwrap_or_else(|| panic!("MCP list omitted stdio argument shape: {agents_with_stdio}"));
    assert_eq!(stdio_args, &vec![json!("<redacted>"); 4]);
    let listed_stdio = &structured_content(&agents_with_stdio)["no-list-agent"];
    assert_eq!(
        listed_stdio.pointer("/transport/type"),
        Some(&json!("stdio"))
    );
    assert_eq!(
        listed_stdio.pointer("/transport/command"),
        Some(&json!("<redacted-command>"))
    );
    assert_eq!(
        sorted_object_keys(&listed_stdio["transport"]["env"]),
        ["VISIBLE_ENV_ONE", "VISIBLE_ENV_TWO"]
    );
    assert!(
        listed_stdio["transport"]["env"]
            .as_object()
            .expect("stdio environment")
            .values()
            .all(|value| value == "<redacted>")
    );
    assert_eq!(listed_stdio.pointer("/proxy_chain"), Some(&json!([])));
    assert_eq!(
        listed_stdio.pointer("/permission_policy"),
        Some(&json!("reject"))
    );
    assert_eq!(
        listed_stdio.pointer("/client_capabilities/terminal"),
        Some(&json!(false))
    );
    let encoded_stdio =
        serde_json::to_string(&agents_with_stdio).expect("serializes stdio registry response");
    assert!(!encoded_stdio.contains("mcp-arg-secret-64"));
    assert!(!encoded_stdio.contains("mcp-arg-secret-75"));
    let created = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        40,
        "create_conversation",
        json!({
            "agent_id": "no-list-agent",
            "cwd": home.path().to_str().expect("UTF-8 temp path")
        }),
    );
    assert_eq!(
        created.pointer("/result/isError").and_then(Value::as_bool),
        Some(false),
        "fixture conversation creation failed: {created}"
    );
    let protected_conv_id = structured_content(&created)["convId"]
        .as_str()
        .unwrap_or_else(|| panic!("create result omitted convId: {created}"))
        .to_string();
    let misspelled_delete = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        41,
        "delete_conversation",
        json!({"conv_id": protected_conv_id, "local_ony": true}),
    );
    assert_eq!(
        misspelled_delete
            .pointer("/result/isError")
            .and_then(Value::as_bool),
        Some(true),
        "misspelled local_only was accepted and could trigger remote deletion: {misspelled_delete}"
    );
    let after_misspelled_delete = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        42,
        "list_conversations",
        json!({"agent_id": "no-list-agent"}),
    );
    assert!(
        structured_content(&after_misspelled_delete)
            .as_array()
            .expect("conversation list")
            .iter()
            .any(|conversation| {
                conversation.get("id").and_then(Value::as_str) == Some(protected_conv_id.as_str())
            }),
        "invalid delete request mutated the registry: {after_misspelled_delete}"
    );
    let deleted = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        43,
        "delete_conversation",
        json!({"conv_id": protected_conv_id, "local_only": true}),
    );
    assert_eq!(
        deleted.pointer("/result/isError").and_then(Value::as_bool),
        Some(false),
        "local cleanup failed: {deleted}"
    );

    let unsupported = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        14,
        "list_agent_sessions",
        json!({"agent_id": "no-list-agent"}),
    );
    assert_eq!(
        unsupported.pointer("/error/data/reason"),
        Some(&json!("unsupported_capability")),
        "capability error lost its typed shape: {unsupported}"
    );
    assert_eq!(
        unsupported.pointer("/error/data/requiredCapability"),
        Some(&json!("session_capabilities.list"))
    );
    let _ = call_tool(
        &mut child,
        &rx,
        &mut transcript,
        15,
        "remove_agent",
        json!({"agent_id": "no-list-agent"}),
    );

    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("polls MCP process") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "MCP process did not exit after stdin closed"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(status.success(), "MCP process failed: {status}");
    stdout_reader.join().expect("joins MCP stdout reader");
    transcript.extend(rx.try_iter());

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("captures MCP stderr")
        .read_to_string(&mut stderr)
        .expect("reads MCP stderr");
    let stdout = transcript.join("\n");
    for secret in secrets {
        assert!(!stdout.contains(secret), "MCP stdout disclosed {secret}");
        assert!(!stderr.contains(secret), "MCP stderr disclosed {secret}");
    }

    wait_for_daemon_cleanup(home.path());
}
