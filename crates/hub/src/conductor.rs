//! Proxy-chain assembly via the SDK conductor (design 4, Spec 5).
//!
//! Resolves an agent endpoint plus its ordered `proxyChain` into a single
//! type-erased `ConnectTo<Client>` component. With no proxies the agent is
//! used directly; with proxies a `ConductorImpl` wraps the chain so the Hub
//! still speaks to one component while proxies preprocess outbound and
//! postprocess inbound traffic.

use agent_client_protocol::{Client, DynConnectTo};

use crate::bounded_transport::InboundFlowControl;
use crate::endpoint::Registry;
use crate::error::HubError;
use crate::transport;

pub(crate) struct EndpointComponent {
    pub(crate) component: DynConnectTo<Client>,
    pub(crate) flows: Vec<InboundFlowControl>,
}

/// Build the effective component for an agent endpoint, applying its proxy
/// chain (if any).
pub(crate) fn build_endpoint_component(
    registry: &Registry,
    agent_id: &str,
) -> Result<EndpointComponent, HubError> {
    let proxy_cfgs = registry.proxy_chain(agent_id)?;
    let agent_cfg = registry
        .agents
        .get(agent_id)
        .ok_or_else(|| HubError::not_found("agent", agent_id))?;
    let (agent, flow) = transport::agent_component(&agent_cfg.transport)?;

    if proxy_cfgs.is_empty() {
        return Ok(EndpointComponent {
            component: agent,
            flows: vec![flow],
        });
    }

    let mut proxies = Vec::with_capacity(proxy_cfgs.len());
    let mut flows = vec![flow];
    for p in proxy_cfgs {
        let (proxy, proxy_flow) = transport::proxy_component(&p.transport)?;
        proxies.push(proxy);
        flows.push(proxy_flow);
    }
    for flow in &flows {
        flow.allow_logical_ack();
    }
    Ok(EndpointComponent {
        component: transport::with_proxy_chain(agent, proxies),
        flows,
    })
}

#[cfg(test)]
mod tests {
    use crate::endpoint::{
        AgentEndpointConfig, AgentTransport, PermissionPolicy, ProxyEndpointConfig, ProxyTransport,
        Registry,
    };
    use crate::error::HubError;

    use super::build_endpoint_component;

    #[test]
    fn duplicate_chain_validation_precedes_agent_component_construction() {
        let mut registry = Registry::default();
        for proxy_id in ["guard", "audit"] {
            registry
                .proxies
                .insert(proxy_id.into(), proxy_config(proxy_id));
        }
        registry.agents.insert(
            "invalid-agent".into(),
            invalid_agent_config(vec!["guard".into(), "audit".into(), "guard".into()]),
        );

        let err = match build_endpoint_component(&registry, "invalid-agent") {
            Ok(_) => panic!("expected InvalidRegistry"),
            Err(err) => err,
        };
        assert!(matches!(err, HubError::InvalidRegistry(_)));
    }

    #[test]
    fn overlong_chain_validation_precedes_agent_component_construction() {
        let proxy_ids: Vec<String> = (0..17).map(|index| format!("proxy-{index:02}")).collect();
        let mut registry = Registry::default();
        for proxy_id in &proxy_ids {
            registry
                .proxies
                .insert(proxy_id.clone(), proxy_config(proxy_id));
        }
        registry
            .agents
            .insert("invalid-agent".into(), invalid_agent_config(proxy_ids));

        let err = match build_endpoint_component(&registry, "invalid-agent") {
            Ok(_) => panic!("expected InvalidRegistry"),
            Err(err) => err,
        };
        assert!(matches!(err, HubError::InvalidRegistry(_)));
    }

    fn invalid_agent_config(proxy_chain: Vec<String>) -> AgentEndpointConfig {
        AgentEndpointConfig {
            transport: AgentTransport::Http {
                url: "not a valid URL".into(),
                headers: Default::default(),
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
}
