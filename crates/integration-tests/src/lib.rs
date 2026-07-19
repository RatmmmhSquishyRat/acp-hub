//! Integration-test crate for acp-hub.
//!
//! Lives outside the publishable packages so it can depend on the unpublished
//! `agent-client-protocol-test` harness without blocking crates.io publish.

use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, FsConfig, PermissionPolicy,
};
use acp_hub::{
    HubError,
    callbacks::{HubCtx, SessionBinding},
};
use std::path::PathBuf;

/// Full callback configuration for deterministic in-process Testy fixtures.
///
/// Permission boundary rejection is covered by core tests; protocol-surface
/// integration tests intentionally enable every callback so Testy can exercise
/// the complete stable ACP v1 client surface.
pub fn test_agent_config() -> AgentEndpointConfig {
    AgentEndpointConfig {
        transport: AgentTransport::Stdio {
            command: "in-process-testy".to_string(),
            args: Vec::new(),
            env: Default::default(),
        },
        proxy_chain: Vec::new(),
        permission_policy: PermissionPolicy::AutoAllow,
        client_capabilities: ClientCapabilityConfig {
            fs: FsConfig {
                read_text_file: true,
                write_text_file: true,
                allowed_roots: Vec::new(),
            },
            terminal: true,
        },
    }
}

/// Complete the CoreHub-side binding step after a direct driver
/// `CreateSession` command in integration tests.
pub fn bind_test_session(
    ctx: &HubCtx,
    conv_id: &str,
    agent_id: &str,
    session_id: &str,
    cwd: PathBuf,
) -> Result<(), HubError> {
    let config = test_agent_config();
    ctx.bind_session(
        session_id,
        SessionBinding {
            conv_id: conv_id.to_string(),
            agent_id: agent_id.to_string(),
            permission_policy: config.permission_policy,
            fs: config.client_capabilities.fs,
            cwd,
        },
    )
}
