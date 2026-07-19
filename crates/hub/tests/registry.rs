//! T7 — Registry unit tests (BDD Feature 1).

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, PermissionPolicy, ProxyEndpointConfig, ProxyTransport,
    Registry,
};
use acp_hub::error::HubError;

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

#[test]
fn referenced_proxy_cannot_be_removed() {
    let mut reg = Registry::default();
    reg.register_proxy(
        "guarded-proxy".into(),
        ProxyEndpointConfig {
            transport: ProxyTransport::Stdio {
                command: "proxy".into(),
                args: vec![],
                env: Default::default(),
            },
        },
    )
    .unwrap();
    reg.register_agent(
        "proxy-user".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "agent".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec!["guarded-proxy".into()],
            permission_policy: PermissionPolicy::Reject,
            client_capabilities: Default::default(),
        },
    )
    .unwrap();

    let err = reg.remove_proxy("guarded-proxy").unwrap_err();
    assert!(
        err.to_string()
            .contains("referenced by agent(s): proxy-user")
    );
    assert!(reg.proxies.contains_key("guarded-proxy"));
}

#[test]
fn failed_agent_replacement_does_not_mutate_registry() {
    let mut reg = Registry::default();
    reg.register_agent(
        "stable".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "stable-command".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec![],
            permission_policy: PermissionPolicy::Reject,
            client_capabilities: Default::default(),
        },
    )
    .unwrap();
    let before = reg.clone();

    let result = reg.register_agent(
        "stable".into(),
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "replacement".into(),
                args: vec![],
                env: Default::default(),
            },
            proxy_chain: vec!["missing-proxy".into()],
            permission_policy: PermissionPolicy::Reject,
            client_capabilities: Default::default(),
        },
    );
    assert!(result.is_err());
    assert_eq!(reg, before);
}

#[test]
fn registry_save_replaces_atomically_without_shared_temp_file() {
    let home = std::env::temp_dir().join(format!(
        "acp-hub-registry-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&home).unwrap();

    let mut first = Registry::default();
    first
        .register_proxy(
            "first".into(),
            ProxyEndpointConfig {
                transport: ProxyTransport::Stdio {
                    command: "one".into(),
                    args: vec![],
                    env: Default::default(),
                },
            },
        )
        .unwrap();
    first.save(&home).unwrap();
    let first_fingerprint = Registry::fingerprint(&home).unwrap();

    let mut second = Registry::default();
    second
        .register_proxy(
            "other".into(),
            ProxyEndpointConfig {
                transport: ProxyTransport::Stdio {
                    command: "two".into(),
                    args: vec![],
                    env: Default::default(),
                },
            },
        )
        .unwrap();
    second.save(&home).unwrap();

    assert_eq!(Registry::load(&home).unwrap(), second);
    assert_ne!(Registry::fingerprint(&home).unwrap(), first_fingerprint);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(Registry::path(&home))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(std::fs::read_dir(&home).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn registry_load_rejects_duplicate_proxy_chain() {
    let home = std::env::temp_dir().join(format!(
        "acp-hub-registry-duplicate-chain-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&home).unwrap();

    let mut reg = Registry::default();
    reg.proxies.insert("guard".into(), proxy_config("guard"));
    reg.proxies.insert("audit".into(), proxy_config("audit"));
    reg.agents.insert(
        "duplicate-user".into(),
        agent_config(vec!["guard".into(), "audit".into(), "guard".into()]),
    );
    std::fs::write(
        Registry::path(&home),
        serde_json::to_vec_pretty(&reg).unwrap(),
    )
    .unwrap();

    let err = Registry::load(&home).unwrap_err();
    let HubError::InvalidRegistry(message) = err else {
        panic!("expected InvalidRegistry");
    };
    assert_eq!(
        message,
        r#"agent "duplicate-user" proxyChain contains duplicate proxy "guard""#
    );

    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn failed_duplicate_proxy_chain_registration_does_not_mutate_registry() {
    let mut reg = Registry::default();
    reg.proxies.insert("guard".into(), proxy_config("guard"));
    let before = reg.clone();

    let err = reg
        .register_agent(
            "duplicate-user".into(),
            agent_config(vec!["guard".into(), "guard".into()]),
        )
        .unwrap_err();
    assert!(matches!(err, HubError::InvalidRegistry(_)));
    assert_eq!(reg, before);
}

#[test]
fn proxy_chain_resolution_rejects_duplicate_ids() {
    let mut reg = Registry::default();
    reg.proxies.insert("guard".into(), proxy_config("guard"));
    reg.agents.insert(
        "duplicate-user".into(),
        agent_config(vec!["guard".into(), "guard".into()]),
    );

    let err = reg.proxy_chain("duplicate-user").unwrap_err();
    assert!(matches!(err, HubError::InvalidRegistry(_)));
}

fn agent_config(proxy_chain: Vec<String>) -> AgentEndpointConfig {
    AgentEndpointConfig {
        transport: AgentTransport::Stdio {
            command: "agent".into(),
            args: vec![],
            env: Default::default(),
        },
        proxy_chain,
        permission_policy: PermissionPolicy::Reject,
        client_capabilities: Default::default(),
    }
}

fn proxy_config(command: &str) -> ProxyEndpointConfig {
    ProxyEndpointConfig {
        transport: ProxyTransport::Stdio {
            command: command.into(),
            args: vec![],
            env: Default::default(),
        },
    }
}

#[test]
fn failed_overlong_proxy_chain_registration_does_not_mutate_registry() {
    let proxy_ids: Vec<String> = (0..17).map(|index| format!("proxy-{index:02}")).collect();
    let mut reg = Registry::default();
    for proxy_id in &proxy_ids {
        reg.proxies.insert(proxy_id.clone(), proxy_config(proxy_id));
    }
    let before = reg.clone();

    let err = reg
        .register_agent("too-many".into(), agent_config(proxy_ids))
        .unwrap_err();
    let HubError::InvalidRegistry(message) = err else {
        panic!("expected InvalidRegistry");
    };
    assert_eq!(
        message,
        r#"agent "too-many" proxyChain has 17 entries; maximum is 16"#
    );
    assert_eq!(reg, before);
}

#[test]
fn proxy_chain_resolution_rejects_overlong_chain() {
    let proxy_ids: Vec<String> = (0..17).map(|index| format!("proxy-{index:02}")).collect();
    let mut reg = Registry::default();
    for proxy_id in &proxy_ids {
        reg.proxies.insert(proxy_id.clone(), proxy_config(proxy_id));
    }
    reg.agents
        .insert("too-many".into(), agent_config(proxy_ids));

    let err = reg.proxy_chain("too-many").unwrap_err();
    let HubError::InvalidRegistry(message) = err else {
        panic!("expected InvalidRegistry");
    };
    assert_eq!(
        message,
        r#"agent "too-many" proxyChain has 17 entries; maximum is 16"#
    );
}

#[test]
fn maximum_proxy_chain_preserves_configured_order() {
    let proxy_ids: Vec<String> = [7, 0, 15, 3, 12, 1, 9, 5, 14, 2, 11, 6, 13, 4, 10, 8]
        .map(|index| format!("proxy-{index:02}"))
        .into();
    let mut reg = Registry::default();
    for proxy_id in &proxy_ids {
        reg.proxies.insert(proxy_id.clone(), proxy_config(proxy_id));
    }
    reg.register_agent("boundary-user".into(), agent_config(proxy_ids.clone()))
        .unwrap();

    reg.validate().unwrap();
    let resolved_commands: Vec<String> = reg
        .proxy_chain("boundary-user")
        .unwrap()
        .into_iter()
        .map(|proxy| match &proxy.transport {
            ProxyTransport::Stdio { command, .. } => command.clone(),
        })
        .collect();
    assert_eq!(resolved_commands, proxy_ids);
}
