//! SDK transport adapters (design 1/3/4).
//!
//! Translates registry descriptions into resource-bounded SDK `ConnectTo`
//! components. Framing limits are enforced before JSON deserialization for
//! stdio, HTTP/SSE, and WebSocket input.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use agent_client_protocol::{AcpAgent, Client, Conductor, DynConnectTo};
use agent_client_protocol_conductor::ProxiesAndAgent;

use crate::bounded_transport::{BoundedHttpAgent, BoundedStdioAgent, InboundFlowControl};
use crate::endpoint::{AgentTransport, ProxyTransport};
use crate::error::HubError;

/// Build a type-erased agent component and the flow-control handle consumed
/// by the Hub-side protocol handler.
pub(crate) fn agent_component(
    transport: &AgentTransport,
) -> Result<(DynConnectTo<Client>, InboundFlowControl), HubError> {
    let flow = InboundFlowControl::new();
    Ok(match transport {
        AgentTransport::Stdio { command, args, env } => {
            let server = McpServerStdio::new("agent", command.clone())
                .args(args.clone())
                .env(env_to_vars(env));
            (
                DynConnectTo::new(BoundedStdioAgent::with_flow(
                    AcpAgent::new(McpServer::Stdio(server)),
                    flow.clone(),
                )),
                flow,
            )
        }
        AgentTransport::Http { url, headers } | AgentTransport::WebSocket { url, headers } => (
            DynConnectTo::new(BoundedHttpAgent::with_flow(url, headers, flow.clone())?),
            flow,
        ),
    })
}

/// Build a type-erased proxy component and its flow-control handle.
///
/// Stdio-only for this SDK revision; other transports return
/// [`HubError::UnsupportedProxyTransport`].
pub(crate) fn proxy_component(
    transport: &ProxyTransport,
) -> Result<(DynConnectTo<Conductor>, InboundFlowControl), HubError> {
    let flow = InboundFlowControl::new();
    match transport {
        ProxyTransport::Stdio { command, args, env } => {
            let server = McpServerStdio::new("proxy", command.clone())
                .args(args.clone())
                .env(env_to_vars(env));
            Ok((
                DynConnectTo::new(BoundedStdioAgent::with_flow(
                    AcpAgent::new(McpServer::Stdio(server)),
                    flow.clone(),
                )),
                flow,
            ))
        }
    }
}

/// Assemble a conductor-backed component from an agent component and an
/// ordered list of proxy components. An empty proxy chain returns the agent
/// component directly (no conductor needed).
pub fn with_proxy_chain(
    agent: DynConnectTo<Client>,
    proxies: Vec<DynConnectTo<Conductor>>,
) -> DynConnectTo<Client> {
    if proxies.is_empty() {
        return agent;
    }
    let mut builder = ProxiesAndAgent::new(agent);
    for p in proxies {
        builder = builder.proxy(p);
    }
    DynConnectTo::new(agent_client_protocol_conductor::ConductorImpl::new_agent(
        "acp-hub-conductor",
        builder,
    ))
}

fn env_to_vars(env: &BTreeMap<String, String>) -> Vec<EnvVariable> {
    env.iter()
        .map(|(k, v)| EnvVariable::new(k.clone(), v.clone()))
        .collect()
}
