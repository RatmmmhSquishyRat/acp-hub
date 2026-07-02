//! SDK transport adapters (design 1/3/4).
//!
//! Translates the registry's transport descriptions into SDK `ConnectTo`
//! components: `AcpAgent` for stdio, `HttpClient` for http/ws, and (via the
//! conductor) stdio `AcpAgent` for proxy components.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::{EnvVariable, McpServer, McpServerStdio};
use agent_client_protocol::{AcpAgent, Client, Conductor, DynConnectTo};
use agent_client_protocol_conductor::ProxiesAndAgent;
use agent_client_protocol_http::HttpClient;

use crate::endpoint::{AgentTransport, ProxyTransport};
use crate::error::HubError;

/// Build a type-erased agent component (`ConnectTo<Client>`) for a transport.
pub fn agent_component(transport: &AgentTransport) -> Result<DynConnectTo<Client>, HubError> {
    Ok(match transport {
        AgentTransport::Stdio { command, args, env } => {
            let server = McpServerStdio::new("agent", command.clone())
                .args(args.clone())
                .env(env_to_vars(env));
            DynConnectTo::new(AcpAgent::new(McpServer::Stdio(server)))
        }
        AgentTransport::Http { url, headers } => DynConnectTo::new(http_client(url, headers)?),
        AgentTransport::WebSocket { url, headers } => DynConnectTo::new(http_client(url, headers)?),
    })
}

/// Build a type-erased proxy component (`ConnectTo<Conductor>`) for a transport.
///
/// Stdio-only for this SDK revision; other transports return
/// [`HubError::UnsupportedProxyTransport`].
pub fn proxy_component(transport: &ProxyTransport) -> Result<DynConnectTo<Conductor>, HubError> {
    match transport {
        ProxyTransport::Stdio { command, args, env } => {
            let server = McpServerStdio::new("proxy", command.clone())
                .args(args.clone())
                .env(env_to_vars(env));
            Ok(DynConnectTo::new(AcpAgent::new(McpServer::Stdio(server))))
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

fn http_client(url: &str, headers: &BTreeMap<String, String>) -> Result<HttpClient, HubError> {
    let client = if headers.is_empty() {
        reqwest::Client::new()
    } else {
        let mut map = reqwest::header::HeaderMap::new();
        for (k, v) in headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| HubError::other(format!("invalid header name {k:?}: {e}")))?;
            let value = reqwest::header::HeaderValue::from_str(v)
                .map_err(|e| HubError::other(format!("invalid header value: {e}")))?;
            map.append(name, value);
        }
        reqwest::Client::builder()
            .default_headers(map)
            .build()
            .map_err(|e| HubError::other(format!("reqwest build: {e}")))?
    };
    HttpClient::with_client(url, client).map_err(|e| HubError::other(format!("http client: {e}")))
}

fn env_to_vars(env: &BTreeMap<String, String>) -> Vec<EnvVariable> {
    env.iter()
        .map(|(k, v)| EnvVariable::new(k.clone(), v.clone()))
        .collect()
}
