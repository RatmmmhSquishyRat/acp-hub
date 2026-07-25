//! PHASE4 doctor: agent_cache_empty check against real Store + Registry (no rewrite).

use std::process::Command;

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy, Registry,
};
use acp_hub::store::Store;
use serde_json::Value;

fn acp_hub() -> Command {
    Command::new(env!("CARGO_BIN_EXE_acp-hub"))
}

#[test]
fn doctor_reports_agent_cache_empty_with_probe_next_step() {
    let home = tempfile::tempdir().expect("home");
    let mut reg = Registry::default();
    reg.agents.insert(
        "lonely".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "true".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::AutoAllow,
            client_capabilities: ClientCapabilityConfig::default(),
        },
    );
    reg.save(home.path()).expect("save agents.json");

    // Store exists but cache row absent.
    let _store = Store::open(home.path()).expect("open store");
    assert!(
        _store.agent_cache("lonely").unwrap().is_none(),
        "precondition: empty cache"
    );

    let output = acp_hub()
        .arg("--home")
        .arg(home.path())
        .args(["doctor", "--json"])
        .output()
        .expect("doctor");
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    let checks = value["checks"].as_array().expect("checks array");
    let cache = checks
        .iter()
        .find(|c| c["id"] == "agent_cache_empty")
        .expect("agent_cache_empty check present");
    assert_eq!(cache["severity"], "info");
    assert_eq!(cache["agentId"], "lonely");
    let msg = cache["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("inspect") && msg.contains("--probe"),
        "next step must mention inspect --probe: {msg}"
    );

    // Populating cache clears the empty check.
    _store
        .upsert_agent_cache("lonely", "{}", r#"{"loadSession":true}"#)
        .unwrap();
    let output2 = acp_hub()
        .arg("--home")
        .arg(home.path())
        .args(["doctor", "--json"])
        .output()
        .expect("doctor after cache");
    assert!(output2.status.success());
    let value2: Value = serde_json::from_slice(&output2.stdout).unwrap();
    let still_empty = value2["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "agent_cache_empty" && c["agentId"] == "lonely");
    assert!(
        !still_empty,
        "cache populated → no agent_cache_empty for lonely"
    );
}
