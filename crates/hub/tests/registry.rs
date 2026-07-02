//! T7 — Registry unit tests (BDD Feature 1).

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, PermissionPolicy, ProxyEndpointConfig, ProxyTransport,
    Registry,
};

#[test]
fn registry_add_and_remove() {
    let mut reg = Registry::default();
    reg.register_agent(
        "omp".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "omp".into(),
                args: vec!["acp".into()],
                env: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::default(),
            client_capabilities: Default::default(),
        },
    )
    .unwrap();
    assert!(reg.agents.contains_key("omp"));

    reg.remove_agent("omp").unwrap();
    assert!(!reg.agents.contains_key("omp"));
}

#[test]
fn registry_rejects_invalid_id() {
    let mut reg = Registry::default();
    let result = reg.register_agent(
        "bad id!".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "x".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::default(),
            client_capabilities: Default::default(),
        },
    );
    assert!(result.is_err());
}

#[test]
fn registry_json_roundtrip() {
    let mut reg = Registry::default();
    reg.register_agent(
        "test-agent".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "https://example.com/acp".into(),
                headers: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::AutoAllow,
            client_capabilities: Default::default(),
        },
    )
    .unwrap();
    reg.register_proxy(
        "test-proxy".into(),
        ProxyEndpointConfig {
            transport: ProxyTransport::Stdio {
                command: "proxy".into(),
                args: vec![],
                env: Default::default(),
            },
        },
    )
    .unwrap();

    let json = serde_json::to_string_pretty(&reg).unwrap();
    let parsed: Registry = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, reg);
}
