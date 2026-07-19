//! ACP Client driver: connection lifecycle, capability negotiation, session
//! management, and turn execution.
//!
//! Each agent endpoint gets one long-lived connection task. The Hub sends
//! commands via a tokio channel; the task processes them using
//! `ConnectionTo<Agent>`. Cancelling a turn occurs through the cloned
//! `ConnectionTo<Agent>` in [`AgentHandle`] — `send_notification(CancelNotification)`
//! reaches the agent even while the loop is blocked on `send_prompt`.
//!
//! The notification handler (`HubCtx::handle_notification`) captures every
//! `session/update` into the store.  Callback handlers answer agent-to-client
//! requests (permission, fs, terminal).  Both share `Arc<HubCtx>`.

use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthenticateRequest, CancelNotification, ClientCapabilities,
    CloseSessionRequest, ContentBlock, CreateTerminalRequest, CreateTerminalResponse,
    DeleteSessionRequest, FileSystemCapabilities, InitializeRequest, KillTerminalRequest,
    KillTerminalResponse, ListSessionsRequest, LoadSessionRequest, LogoutRequest,
    NewSessionRequest, PromptRequest, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionRequest, SessionInfo, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Dispatch, DynConnectTo, Handled};

use crate::bounded_transport::InboundFlowControl;
use crate::callbacks::HubCtx;
use crate::endpoint::{AgentEndpointConfig, ClientCapabilityConfig};
use crate::error::{AuthMethodSummary, HubError};

// ---- Commands -------------------------------------------------------------

/// A command sent from the Hub's RPC layer to a connection task.
#[allow(clippy::large_enum_variant)]
pub enum AgentCommand {
    CreateSession {
        conv_id: String,
        agent_id: String,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        mcp_servers: Vec<agent_client_protocol::schema::v1::McpServer>,
        reply: tokio::sync::oneshot::Sender<Result<SessionCreated, HubError>>,
    },
    LoadSession {
        conv_id: String,
        agent_id: String,
        agent_session_id: String,
        cwd: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<SessionCreated, HubError>>,
    },
    ResumeSession {
        conv_id: String,
        agent_id: String,
        agent_session_id: String,
        cwd: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<SessionCreated, HubError>>,
    },
    SendPrompt {
        conv_id: String,
        agent_session_id: String,
        prompt: Vec<ContentBlock>,
        params: Vec<(String, String)>,
        mode_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<PromptDone, HubError>>,
    },
    // NOTE: Cancel is NOT sent through the command channel (the loop is
    // blocked during SendPrompt). Instead, the Hub sends CancelNotification
    // directly via `AgentHandle.cx.send_notification(…)`.
    CloseSession {
        conv_id: String,
        agent_session_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), HubError>>,
    },
    DeleteSession {
        conv_id: String,
        agent_session_id: String,
        local_only: bool,
        reply: tokio::sync::oneshot::Sender<Result<(), HubError>>,
    },
    ListSessions {
        cwd: Option<PathBuf>,
        reply: tokio::sync::oneshot::Sender<Result<ListSessionsResult, HubError>>,
    },
    SetConfig {
        agent_session_id: String,
        config_id: String,
        value: String,
        reply: tokio::sync::oneshot::Sender<Result<(), HubError>>,
    },
    SetMode {
        agent_session_id: String,
        mode_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), HubError>>,
    },
    Authenticate {
        method_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), HubError>>,
    },
    Logout {
        reply: tokio::sync::oneshot::Sender<Result<(), HubError>>,
    },
}

#[derive(Debug, Clone)]
pub struct SessionCreated {
    pub agent_session_id: String,
    pub modes: Option<serde_json::Value>,
    pub config_options: Option<serde_json::Value>,
    pub capabilities: AgentCapabilities,
}

#[derive(Debug, Clone)]
pub struct PromptDone {
    pub stop_reason: StopReason,
    pub run_id: String,
}

#[derive(Debug, Clone)]
pub struct ListSessionsResult {
    pub sessions: Vec<agent_client_protocol::schema::v1::SessionInfo>,
}

/// Handle to an agent connection.
pub struct AgentHandle {
    pub cmd_tx: tokio::sync::mpsc::Sender<AgentCommand>,
    /// Cloned connection handle — used for cancel notifications sent
    /// directly (bypassing the blocked command loop).
    pub cx: ConnectionTo<Agent>,
    pub capabilities: AgentCapabilities,
    pub auth_methods: Vec<AuthMethodSummary>,
    pub(crate) connection_id: String,
}

// ---- Spawn ----------------------------------------------------------------

/// Spawn one long-lived connection task for an agent endpoint component.
/// Returns a oneshot receiver for the [`AgentHandle`].
pub fn spawn_agent_connection(
    component: DynConnectTo<Client>,
    agent_id: String,
    agent_config: AgentEndpointConfig,
    ctx: Arc<HubCtx>,
) -> tokio::sync::oneshot::Receiver<Result<AgentHandle, HubError>> {
    spawn_agent_connection_with_flow(component, agent_id, agent_config, ctx, Vec::new())
}

pub(crate) fn spawn_agent_connection_with_flow(
    component: DynConnectTo<Client>,
    agent_id: String,
    agent_config: AgentEndpointConfig,
    ctx: Arc<HubCtx>,
    flows: Vec<InboundFlowControl>,
) -> tokio::sync::oneshot::Receiver<Result<AgentHandle, HubError>> {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(64);
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let handle_tx = Arc::new(tokio::sync::Mutex::new(Some(handle_tx)));
    let ctx2 = Arc::clone(&ctx);
    let handle_tx_inner = Arc::clone(&handle_tx);
    let connection_id = format!("connection-{}", uuid::Uuid::new_v4().simple());

    tokio::spawn(async move {
        ctx2.configure_agent_async(&agent_id, &connection_id, agent_config.clone())
            .await;
        let cleanup_ctx = Arc::clone(&ctx2);
        let cleanup_agent_id = agent_id.clone();
        let cleanup_connection_id = connection_id.clone();
        let result = Client
            .builder()
            .on_receive_dispatch(
                {
                    let flows = flows.clone();
                    async move |dispatch: Dispatch, _cx| {
                        match &dispatch {
                            Dispatch::Request(_, _) => {}
                            Dispatch::Notification(notification) => {
                                for flow in &flows {
                                    flow.acknowledge_notification(
                                        notification.method(),
                                        notification.params(),
                                    )?;
                                }
                            }
                            Dispatch::Response(result, _) => {
                                for flow in &flows {
                                    flow.acknowledge_response(result)?;
                                }
                            }
                        }
                        Ok(Handled::No {
                            message: dispatch,
                            retry: false,
                        })
                    }
                },
                agent_client_protocol::on_receive_dispatch!(),
            )
            .on_receive_notification(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |notif: SessionNotification, _cx| {
                        let _generation = ctx
                            .try_acquire_connection_lease(&agent_id, &connection_id)
                            .map_err(HubError::into_acp_error)?;
                        ctx.handle_notification(&agent_id, &connection_id, notif)
                            .map_err(HubError::into_acp_error)?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: RequestPermissionRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_permission(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: ReadTextFileRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_read_text_file(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: WriteTextFileRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_write_text_file(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: CreateTerminalRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_terminal_create(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: TerminalOutputRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_terminal_output(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: WaitForTerminalExitRequest, responder, _cx| match ctx
                        .handle_terminal_wait(&agent_id, &connection_id, &req)
                        .await
                    {
                        Ok(resp) => responder.respond(resp),
                        Err(err) => responder.respond_with_error(err.into_acp_error()),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: KillTerminalRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_terminal_kill(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    let agent_id = agent_id.clone();
                    let connection_id = connection_id.clone();
                    async move |req: ReleaseTerminalRequest, responder, _cx| {
                        let _generation =
                            match ctx.try_acquire_connection_lease(&agent_id, &connection_id) {
                                Ok(lease) => lease,
                                Err(error) => {
                                    return responder.respond_with_error(error.into_acp_error());
                                }
                            };
                        match ctx.handle_terminal_release(&agent_id, &connection_id, &req) {
                            Ok(resp) => responder.respond(resp),
                            Err(err) => responder.respond_with_error(err.into_acp_error()),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(component, async move |cx| {
                let initialize =
                    InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
                        .client_capabilities(build_client_caps(&agent_config.client_capabilities));
                let initialize_result = {
                    let initialize = cx.send_request(initialize).block_task();
                    tokio::pin!(initialize);
                    let receiver_closed = async {
                        let mut sender = handle_tx_inner.lock().await;
                        if let Some(sender) = sender.as_mut() {
                            sender.closed().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    };
                    tokio::pin!(receiver_closed);
                    tokio::select! {
                        result = &mut initialize => Some(result),
                        () = &mut receiver_closed => None,
                    }
                };
                let Some(initialize_result) = initialize_result else {
                    return Err(agent_client_protocol::Error::internal_error()
                        .data("agent connection initialization receiver was dropped"));
                };
                let init = match initialize_result {
                    Ok(init) => init,
                    Err(e) => {
                        if let Some(tx) = handle_tx_inner.lock().await.take() {
                            let _ = tx.send(Err(HubError::Other(format!(
                                "agent initialize failed: {e}"
                            ))));
                        }
                        return Err(e);
                    }
                };
                if init.protocol_version != agent_client_protocol::schema::ProtocolVersion::V1 {
                    if let Some(tx) = handle_tx_inner.lock().await.take() {
                        let _ = tx.send(Err(HubError::UnsupportedProtocolVersion));
                    }
                    return Err(HubError::UnsupportedProtocolVersion.into_acp_error());
                }
                let caps = init.agent_capabilities;
                let auth: Vec<AuthMethodSummary> = init
                    .auth_methods
                    .iter()
                    .map(|m| AuthMethodSummary {
                        id: m.id().to_string(),
                        kind: "agent".to_string(),
                        display: Some(m.name().to_string()),
                    })
                    .collect();

                let handle_sender = handle_tx_inner.lock().await.take();
                let delivered = handle_sender.is_some_and(|tx| {
                    tx.send(Ok(AgentHandle {
                        cmd_tx,
                        cx: cx.clone(),
                        capabilities: caps.clone(),
                        auth_methods: auth,
                        connection_id: connection_id.clone(),
                    }))
                    .is_ok()
                });
                if !delivered {
                    return Err(agent_client_protocol::Error::internal_error()
                        .data("agent connection initialization receiver was dropped"));
                }

                run_command_loop(
                    cx,
                    cmd_rx,
                    &caps,
                    &agent_id,
                    &connection_id,
                    &agent_config,
                    Arc::clone(&ctx2),
                )
                .await
            })
            .await;

        cleanup_ctx
            .revoke_connection(&cleanup_agent_id, &cleanup_connection_id)
            .await;
        if let Err(e) = result
            && let Some(tx) = handle_tx.lock().await.take()
        {
            let _ = tx.send(Err(HubError::Other(format!("connection failed: {e}"))));
        }
    });

    handle_rx
}

// ---- Command loop ---------------------------------------------------------

mod capabilities;

pub(crate) use capabilities::validate_prompt_capabilities;

mod command_loop;

#[cfg(test)]
use command_loop::merge_capture_failure;
use command_loop::run_command_loop;

async fn create_session(
    cx: &ConnectionTo<Agent>,
    caps: &AgentCapabilities,
    __agent_id: &str,
    cwd: PathBuf,
    additional: Vec<PathBuf>,
    mcp_servers: Vec<agent_client_protocol::schema::v1::McpServer>,
) -> Result<SessionCreated, HubError> {
    let req = NewSessionRequest::new(cwd)
        .additional_directories(additional)
        .mcp_servers(mcp_servers);
    let resp = cx.send_request(req).block_task().await?;
    let sid = resp.session_id.to_string();
    Ok(SessionCreated {
        agent_session_id: sid,
        modes: serde_json::to_value(&resp.modes).ok(),
        config_options: serde_json::to_value(&resp.config_options).ok(),
        capabilities: caps.clone(),
    })
}

async fn load_session(
    cx: &ConnectionTo<Agent>,
    caps: &AgentCapabilities,
    agent_id: &str,
    agent_session_id: &str,
    cwd: PathBuf,
) -> Result<SessionCreated, HubError> {
    if !caps.load_session {
        return Err(HubError::UnsupportedCapability {
            endpoint: agent_id.into(),
            operation: "session/load",
            required_capability: "load_session",
        });
    }
    let req = LoadSessionRequest::new(
        agent_client_protocol::schema::v1::SessionId::new(agent_session_id),
        cwd,
    );
    let resp = cx.send_request(req).block_task().await?;
    let sid = agent_session_id.to_string();
    Ok(SessionCreated {
        agent_session_id: sid,
        modes: serde_json::to_value(&resp.modes).ok(),
        config_options: serde_json::to_value(&resp.config_options).ok(),
        capabilities: caps.clone(),
    })
}

async fn resume_session(
    cx: &ConnectionTo<Agent>,
    caps: &AgentCapabilities,
    agent_id: &str,
    agent_session_id: &str,
    cwd: PathBuf,
) -> Result<SessionCreated, HubError> {
    if caps.session_capabilities.resume.is_none() {
        return Err(HubError::UnsupportedCapability {
            endpoint: agent_id.into(),
            operation: "session/resume",
            required_capability: "session_capabilities.resume",
        });
    }
    let req = ResumeSessionRequest::new(
        agent_client_protocol::schema::v1::SessionId::new(agent_session_id),
        cwd,
    );
    let resp = cx.send_request(req).block_task().await?;
    let sid = agent_session_id.to_string();
    Ok(SessionCreated {
        agent_session_id: sid,
        modes: serde_json::to_value(&resp.modes).ok(),
        config_options: serde_json::to_value(&resp.config_options).ok(),
        capabilities: caps.clone(),
    })
}

async fn send_prompt(
    cx: &ConnectionTo<Agent>,
    session_id: &str,
    prompt: Vec<ContentBlock>,
    params: Vec<(String, String)>,
    mode_id: Option<String>,
) -> Result<PromptDone, HubError> {
    let run_id = format!("run-{}", uuid::Uuid::new_v4().simple());
    let sid = agent_client_protocol::schema::v1::SessionId::new(session_id);
    use agent_client_protocol::schema::v1::{SessionConfigId, SessionConfigValueId, SessionModeId};
    for (k, v) in &params {
        cx.send_request(SetSessionConfigOptionRequest::new(
            sid.clone(),
            SessionConfigId::new(k.as_str()),
            SessionConfigValueId::new(v.as_str()),
        ))
        .block_task()
        .await?;
    }
    if let Some(m) = &mode_id {
        cx.send_request(SetSessionModeRequest::new(
            sid.clone(),
            SessionModeId::new(m.as_str()),
        ))
        .block_task()
        .await?;
    }
    let resp = cx
        .send_request(PromptRequest::new(sid, prompt))
        .block_task()
        .await?;
    Ok(PromptDone {
        stop_reason: resp.stop_reason,
        run_id,
    })
}

fn ensure_agent_context(expected: &str, received: &str) -> Result<(), HubError> {
    if expected == received {
        Ok(())
    } else {
        Err(HubError::other(format!(
            "agent command for {received:?} was sent to connection {expected:?}"
        )))
    }
}

async fn list_all_sessions(
    cx: &ConnectionTo<Agent>,
    caps: &AgentCapabilities,
    agent_id: &str,
    cwd: Option<PathBuf>,
) -> Result<ListSessionsResult, HubError> {
    if caps.session_capabilities.list.is_none() {
        return Err(HubError::UnsupportedCapability {
            endpoint: agent_id.into(),
            operation: "session/list",
            required_capability: "session_capabilities.list",
        });
    }

    collect_session_pages(agent_id, |cursor| {
        let mut req = ListSessionsRequest::new();
        if let Some(dir) = &cwd {
            req = req.cwd(dir.clone());
        }
        if let Some(token) = cursor {
            req = req.cursor(token);
        }
        async move {
            let response = cx.send_request(req).block_task().await?;
            Ok((response.sessions, response.next_cursor))
        }
    })
    .await
}

mod session_list;

use session_list::collect_session_pages;
#[cfg(test)]
use session_list::{SessionListLimits, collect_session_pages_with_limits};

fn build_client_caps(cfg: &ClientCapabilityConfig) -> ClientCapabilities {
    let mut caps = ClientCapabilities::new();
    let fs = FileSystemCapabilities::new()
        .read_text_file(cfg.fs.read_text_file)
        .write_text_file(cfg.fs.write_text_file);
    caps = caps.fs(fs);
    caps.terminal(cfg.terminal)
}

#[cfg(test)]
#[path = "acp/tests.rs"]
mod tests;
