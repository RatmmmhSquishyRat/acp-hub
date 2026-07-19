use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::{CoreHub, SendPromptParams};
use crate::daemon::ActivityTracker;
use crate::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, ProxyEndpointConfig,
    ProxyTransport, Registry,
};
use crate::store::{NewConversation, Store};
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use serde_json::{Value, json};

const REGISTRY_SECRETS: &[&str] = &[
    "stdio-argument-secret",
    "stdio-environment-secret",
    "http-userinfo-secret",
    "http-query-secret",
    "http-header-secret",
    "uppercase-scheme-query-secret",
    "websocket-userinfo-secret",
    "websocket-query-secret",
    "websocket-header-secret",
    "malformed-path-secret",
    "arbitrary-scheme-secret",
    "newline-path-secret",
    "tab-path-secret",
    "space-path-secret",
    "proxy-argument-secret",
    "proxy-environment-secret",
];

pub(super) fn registry_with_secrets() -> Registry {
    let mut agents = BTreeMap::new();
    agents.insert(
        "http".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url:
                    "https://reader:http-userinfo-secret@example.test/acp?opaque=http-query-secret"
                        .to_string(),
                headers: BTreeMap::from([(
                    "X-Ordinary-Metadata".to_string(),
                    "http-header-secret".to_string(),
                )]),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "uppercase-scheme".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "HTTPS://[2001:DB8::1]:8443/acp?token=uppercase-scheme-query-secret"
                    .to_string(),
                headers: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "malformed".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "https:malformed-path-secret".to_string(),
                headers: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "newline-authority".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "https://\n/newline-path-secret".to_string(),
                headers: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "tab-authority".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "https://\t/tab-path-secret".to_string(),
                headers: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "space-authority".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "https:// /space-path-secret".to_string(),
                headers: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "unsupported".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "gopher://arbitrary-scheme-secret.example/acp".to_string(),
                headers: BTreeMap::new(),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );
    agents.insert(
        "stdio".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "agent-runner".to_string(),
                args: vec!["--opaque".to_string(), "stdio-argument-secret".to_string()],
                env: BTreeMap::from([(
                    "ORDINARY_SETTING".to_string(),
                    "stdio-environment-secret".to_string(),
                )]),
            },
            proxy_chain: vec!["capture".to_string()],
            permission_policy: Default::default(),
            client_capabilities: ClientCapabilityConfig {
                terminal: true,
                ..Default::default()
            },
        },
    );
    agents.insert(
        "websocket".to_string(),
        AgentEndpointConfig {
            transport: AgentTransport::WebSocket {
                url: "wss://reader:websocket-userinfo-secret@example.test/acp?opaque=websocket-query-secret"
                    .to_string(),
                headers: BTreeMap::from([(
                    "X-Trace-Context".to_string(),
                    "websocket-header-secret".to_string(),
                )]),
            },
            proxy_chain: Vec::new(),
            permission_policy: Default::default(),
            client_capabilities: Default::default(),
        },
    );

    Registry {
        agents,
        proxies: BTreeMap::from([(
            "capture".to_string(),
            ProxyEndpointConfig {
                transport: ProxyTransport::Stdio {
                    command: "proxy-runner".to_string(),
                    args: vec!["--opaque".to_string(), "proxy-argument-secret".to_string()],
                    env: BTreeMap::from([(
                        "ORDINARY_PROXY_SETTING".to_string(),
                        "proxy-environment-secret".to_string(),
                    )]),
                },
            },
        )]),
    }
}

pub(super) fn assert_registry_read_is_secret_free(value: &Value) {
    let serialized = serde_json::to_string(value).unwrap();
    for secret in REGISTRY_SECRETS {
        assert!(
            !serialized.contains(secret),
            "ordinary registry read leaked {secret}: {serialized}"
        );
    }
}

pub(super) fn assert_operational_registry_fields(reads: &[Value]) {
    let agents = &reads[0];
    assert_eq!(
        agents["stdio"]["transport"]["command"],
        json!("agent-runner")
    );
    assert_eq!(
        agents["stdio"]["transport"]["args"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(agents["stdio"]["proxy_chain"], json!(["capture"]));
    assert_eq!(
        agents["stdio"]["client_capabilities"]["terminal"],
        json!(true)
    );
    assert_eq!(
        agents["http"]["transport"]["url"],
        json!("https://example.test/<redacted>")
    );
    assert_eq!(
        agents["http"]["transport"]["headers"]["X-Ordinary-Metadata"],
        json!("<redacted>")
    );
    assert_eq!(
        agents["uppercase-scheme"]["transport"]["url"],
        json!("https://[2001:db8::1]:8443/<redacted>")
    );
    assert_eq!(
        agents["websocket"]["transport"]["url"],
        json!("wss://example.test/<redacted>")
    );
    assert_eq!(
        agents["websocket"]["transport"]["headers"]["X-Trace-Context"],
        json!("<redacted>")
    );
    assert_eq!(
        agents["malformed"]["transport"]["url"],
        json!("<redacted-url>")
    );
    assert_eq!(
        agents["unsupported"]["transport"]["url"],
        json!("<redacted-url>")
    );
    for agent_id in ["newline-authority", "tab-authority", "space-authority"] {
        assert_eq!(
            agents[agent_id]["transport"]["url"],
            json!("<redacted-url>")
        );
    }

    let stdio_inspection = &reads[1];
    assert_eq!(stdio_inspection["agentInfo"]["name"], json!("cached-agent"));
    assert_eq!(stdio_inspection["capabilities"]["loadSession"], json!(true));
    assert_eq!(stdio_inspection["cachePopulated"], json!(true));

    let proxies = reads.last().unwrap();
    assert_eq!(
        proxies["capture"]["transport"]["command"],
        json!("proxy-runner")
    );
    assert_eq!(
        proxies["capture"]["transport"]["args"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

pub(super) async fn core_registry_reads(hub: &CoreHub) -> Vec<Value> {
    let mut reads = vec![hub.handle_rpc("hub/agent/list", Value::Null).await.unwrap()];
    for agent_id in [
        "stdio",
        "http",
        "websocket",
        "uppercase-scheme",
        "malformed",
        "unsupported",
        "newline-authority",
        "tab-authority",
        "space-authority",
    ] {
        reads.push(
            hub.handle_rpc("hub/agent/inspect", json!({ "agentId": agent_id }))
                .await
                .unwrap(),
        );
    }
    reads.push(hub.handle_rpc("hub/proxy/list", Value::Null).await.unwrap());
    reads
}
const OPERATION_FIXTURE: &str = r#"
const fs = require("fs");
const readline = require("readline");
const mode = process.argv[2];
const root = process.argv[3];
const cwd = process.argv[4];
const sessionCount = Number(process.argv[5] || "0");
const blockedMethod = process.argv[6] || "";
const rl = readline.createInterface({ input: process.stdin });
const respond = (id, result) => process.stdout.write(
  JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n"
);
const reject = (id, message) => process.stdout.write(
  JSON.stringify({ jsonrpc: "2.0", id, error: { code: -32000, message } }) + "\n"
);
const marker = (name, body = "") => fs.writeFileSync(`${root}/${name}`, body);
const append = (name, body) => fs.appendFileSync(`${root}/${name}`, `${body}\n`);
const waitFor = (name, done) => {
  if (fs.existsSync(`${root}/${name}`)) {
done();
  } else {
setTimeout(() => waitFor(name, done), 5);
  }
};
let replayLoadCount = 0;

rl.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
respond(message.id, {
  protocolVersion: message.params.protocolVersion,
  agentCapabilities: {
    loadSession: true,
    sessionCapabilities: { list: {}, close: {}, delete: {} }
  },
  authMethods: []
});
  } else if (message.method === "session/list") {
const sessions = sessionCount > 0
  ? Array.from({ length: sessionCount }, (_, index) => ({
      sessionId: `churn-session-${index}`,
      cwd
    }))
  : [{ sessionId: "refresh-session", cwd }];
respond(message.id, { sessions });
  } else if (message.method === "session/load") {
append("methods", `load:${message.params.sessionId}`);
if (mode === "refresh-block" && message.params.sessionId === "refresh-session") {
  marker("load-ready");
  waitFor("load-release", () => respond(message.id, {}));
} else if (mode === "refresh-error-block") {
  marker("load-ready");
  waitFor("load-release", () => reject(message.id, "load failed"));
} else if (mode === "replay-waiter") {
  replayLoadCount += 1;
  const attempt = replayLoadCount;
  marker(`load-${attempt}-ready`);
  waitFor(`load-${attempt}-release`, () => respond(message.id, {}));
} else {
  respond(message.id, {});
}
  } else if (message.method === "session/new") {
if (mode === "operation-block" && message.method === blockedMethod) {
  marker("operation-ready");
  waitFor("operation-release", () => respond(message.id, { sessionId: "new-session" }));
} else {
  respond(message.id, { sessionId: "new-session" });
}
  } else if (message.method === "session/prompt") {
append("methods", `prompt:${message.params.sessionId}`);
if (mode === "prompt-block" && message.params.sessionId === "session-one") {
  marker("prompt-ready");
  waitFor("prompt-release", () => respond(message.id, { stopReason: "end_turn" }));
} else {
  marker("second-prompt-reached");
  respond(message.id, { stopReason: "end_turn" });
}
  } else if (mode === "operation-block" && message.method === blockedMethod) {
marker("operation-ready");
waitFor("operation-release", () => respond(message.id, {}));
  } else if (message.method === "session/cancel") {
append("cancels", message.params.sessionId);
  } else if (message.id !== undefined) {
respond(message.id, {});
  }
});
"#;

pub(super) fn fixture_hub(mode: &str, session_count: usize) -> (tempfile::TempDir, Arc<CoreHub>) {
    fixture_hub_with_blocked_operation(mode, session_count, "")
}

pub(super) fn fixture_hub_with_blocked_operation(
    mode: &str,
    session_count: usize,
    blocked_method: &str,
) -> (tempfile::TempDir, Arc<CoreHub>) {
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("operation-fixture.cjs");
    fs::write(&script, OPERATION_FIXTURE).unwrap();
    let config = AgentEndpointConfig {
        transport: AgentTransport::Stdio {
            command: "node".to_string(),
            args: vec![
                script.to_string_lossy().into_owned(),
                mode.to_string(),
                home.path().to_string_lossy().into_owned(),
                home.path().to_string_lossy().into_owned(),
                session_count.to_string(),
                blocked_method.to_string(),
            ],
            env: BTreeMap::new(),
        },
        proxy_chain: Vec::new(),
        permission_policy: Default::default(),
        client_capabilities: Default::default(),
    };
    let registry = Registry {
        agents: BTreeMap::from([("fixture".to_string(), config)]),
        proxies: BTreeMap::new(),
    };
    let hub = Arc::new(CoreHub::new(
        home.path(),
        registry,
        Store::open_memory().unwrap(),
        Arc::new(ActivityTracker::new()),
    ));
    (home, hub)
}

pub(super) fn stored_conversation(
    hub: &CoreHub,
    conv_id: &str,
    session_id: &str,
    cwd: &Path,
) -> crate::store::ConversationRow {
    hub.store()
        .create_conversation(&NewConversation {
            id: conv_id.to_string(),
            agent_id: "fixture".to_string(),
            agent_session_id: session_id.to_string(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .unwrap();
    hub.store().conversation(conv_id).unwrap().unwrap()
}

pub(super) fn prompt(conv_id: &str, text: &str) -> SendPromptParams {
    SendPromptParams {
        conv_id: conv_id.to_string(),
        prompt: vec![ContentBlock::Text(TextContent::new(text))],
        params: Vec::new(),
        mode_id: None,
    }
}

pub(super) fn mark_live_and_bound(hub: &CoreHub, conv: &crate::store::ConversationRow) {
    let config = hub.agent_config(&conv.agent_id).unwrap();
    hub.bind_session(conv, &config).unwrap();
    hub.runtime.insert(
        &conv.id,
        crate::runtime::SessionState::Live,
        hub.runtime.next_generation(),
    );
}

pub(super) async fn wait_for_marker(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fixture marker did not appear: {}", path.display()));
}
