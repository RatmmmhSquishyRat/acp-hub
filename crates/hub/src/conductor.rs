//! Proxy-chain assembly via the SDK conductor (design 4, Spec 5).
//!
//! Resolves an agent endpoint plus its ordered `proxyChain` into a single
//! type-erased `ConnectTo<Client>` component. With no proxies the agent is
//! used directly; with proxies a `ConductorImpl` wraps the chain so the Hub
//! still speaks to one component while proxies preprocess outbound and
//! postprocess inbound traffic.

use agent_client_protocol::{Client, DynConnectTo};

use crate::endpoint::Registry;
use crate::error::HubError;
use crate::transport;

/// Build the effective component for an agent endpoint, applying its proxy
/// chain (if any).
pub fn build_endpoint_component(
    registry: &Registry,
    agent_id: &str,
) -> Result<DynConnectTo<Client>, HubError> {
    let agent_cfg = registry
        .agents
        .get(agent_id)
        .ok_or_else(|| HubError::not_found("agent", agent_id))?;
    let agent = transport::agent_component(&agent_cfg.transport)?;

    if agent_cfg.proxy_chain.is_empty() {
        return Ok(agent);
    }

    let proxy_cfgs = registry.proxy_chain(agent_id)?;
    let mut proxies = Vec::with_capacity(proxy_cfgs.len());
    for p in proxy_cfgs {
        proxies.push(transport::proxy_component(&p.transport)?);
    }
    Ok(transport::with_proxy_chain(agent, proxies))
}
