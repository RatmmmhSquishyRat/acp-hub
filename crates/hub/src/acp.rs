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

use std::future::Future;
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
                                    flow.acknowledge_notification(notification.method())?;
                                }
                            }
                            Dispatch::Response(_, _) => {
                                if let Some(id) = dispatch.id() {
                                    for flow in &flows {
                                        flow.acknowledge_response(id.clone())?;
                                    }
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

fn reject_stale_command(cmd: AgentCommand, error: HubError) {
    match cmd {
        AgentCommand::CreateSession { reply, .. }
        | AgentCommand::LoadSession { reply, .. }
        | AgentCommand::ResumeSession { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        AgentCommand::SendPrompt { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        AgentCommand::CloseSession { reply, .. }
        | AgentCommand::DeleteSession { reply, .. }
        | AgentCommand::SetConfig { reply, .. }
        | AgentCommand::SetMode { reply, .. }
        | AgentCommand::Authenticate { reply, .. }
        | AgentCommand::Logout { reply } => {
            let _ = reply.send(Err(error));
        }
        AgentCommand::ListSessions { reply, .. } => {
            let _ = reply.send(Err(error));
        }
    }
}

async fn run_command_loop(
    cx: ConnectionTo<Agent>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<AgentCommand>,
    caps: &AgentCapabilities,
    connection_agent_id: &str,
    connection_id: &str,
    agent_config: &AgentEndpointConfig,
    ctx: Arc<HubCtx>,
) -> Result<(), agent_client_protocol::Error> {
    use crate::callbacks::SessionBinding;
    while let Some(cmd) = cmd_rx.recv().await {
        let _generation = match ctx
            .acquire_connection_lease(connection_agent_id, connection_id)
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                reject_stale_command(cmd, error);
                continue;
            }
        };
        match cmd {
            AgentCommand::CreateSession {
                conv_id: _,
                agent_id,
                cwd,
                additional_directories,
                mcp_servers,
                reply,
            } => {
                if let Err(err) = ensure_agent_context(connection_agent_id, &agent_id) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                let r = create_session(
                    &cx,
                    caps,
                    &agent_id,
                    cwd.clone(),
                    additional_directories,
                    mcp_servers,
                )
                .await;
                // Do not bind or flush pre-response session/update messages yet:
                // CoreHub must first create the conversation parent row using
                // the session id returned here. Its subsequent bind_session call
                // atomically exposes the binding and drains the pending updates.
                let _ = reply.send(r);
            }
            AgentCommand::LoadSession {
                conv_id,
                agent_id,
                agent_session_id,
                cwd,
                reply,
            } => {
                if let Err(err) = ensure_agent_context(connection_agent_id, &agent_id) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                // Bind session BEFORE load so session/update notifications
                // during LoadSessionRequest are captured (Layer 1 messages).
                if let Err(err) = ctx.bind_session(
                    &agent_session_id,
                    SessionBinding {
                        conv_id: conv_id.clone(),
                        agent_id: agent_id.clone(),
                        permission_policy: agent_config.permission_policy,
                        fs: agent_config.client_capabilities.fs.clone(),
                        cwd: cwd.clone(),
                    },
                ) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                if let Err(err) = ctx.begin_capture_operation(
                    connection_agent_id,
                    connection_id,
                    &agent_session_id,
                    "session/load",
                ) {
                    ctx.unbind_session(connection_agent_id, &agent_session_id);
                    let _ = reply.send(Err(err));
                    continue;
                }
                ctx.set_loading(connection_agent_id, &agent_session_id, true);
                let request_result =
                    load_session(&cx, caps, &agent_id, &agent_session_id, cwd).await;
                let capture_failure =
                    ctx.take_capture_failure(connection_agent_id, connection_id, &agent_session_id);
                ctx.set_loading(connection_agent_id, &agent_session_id, false);
                let result = merge_capture_failure(request_result, capture_failure);
                if result.is_err() {
                    ctx.unbind_session(connection_agent_id, &agent_session_id);
                }
                let _ = reply.send(result);
            }
            AgentCommand::ResumeSession {
                conv_id,
                agent_id,
                agent_session_id,
                cwd,
                reply,
            } => {
                if let Err(err) = ensure_agent_context(connection_agent_id, &agent_id) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                // Bind session BEFORE resume so notifications are captured.
                if let Err(err) = ctx.bind_session(
                    &agent_session_id,
                    SessionBinding {
                        conv_id: conv_id.clone(),
                        agent_id: agent_id.clone(),
                        permission_policy: agent_config.permission_policy,
                        fs: agent_config.client_capabilities.fs.clone(),
                        cwd: cwd.clone(),
                    },
                ) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                if let Err(err) = ctx.begin_capture_operation(
                    connection_agent_id,
                    connection_id,
                    &agent_session_id,
                    "session/resume",
                ) {
                    ctx.unbind_session(connection_agent_id, &agent_session_id);
                    let _ = reply.send(Err(err));
                    continue;
                }
                ctx.set_loading(connection_agent_id, &agent_session_id, true);
                let request_result =
                    resume_session(&cx, caps, &agent_id, &agent_session_id, cwd).await;
                let capture_failure =
                    ctx.take_capture_failure(connection_agent_id, connection_id, &agent_session_id);
                ctx.set_loading(connection_agent_id, &agent_session_id, false);
                let result = merge_capture_failure(request_result, capture_failure);
                if result.is_err() {
                    ctx.unbind_session(connection_agent_id, &agent_session_id);
                }
                let _ = reply.send(result);
            }
            AgentCommand::SendPrompt {
                conv_id: _,
                agent_session_id,
                prompt,
                params,
                mode_id,
                reply,
            } => {
                // Run lifecycle (create_run, set_current_run, finalize) is
                // managed by CoreHub BEFORE/AFTER this command. The driver
                // only sends the ACP prompt and returns the stop reason.
                if let Err(err) = ctx.begin_capture_operation(
                    connection_agent_id,
                    connection_id,
                    &agent_session_id,
                    "session/prompt",
                ) {
                    let _ = reply.send(Err(err));
                    continue;
                }
                let request_result =
                    send_prompt(&cx, &agent_session_id, prompt, params, mode_id).await;
                let capture_failure =
                    ctx.take_capture_failure(connection_agent_id, connection_id, &agent_session_id);
                let result = merge_capture_failure(request_result, capture_failure);
                let _ = reply.send(result);
            }
            AgentCommand::CloseSession {
                conv_id: _,
                agent_session_id,
                reply,
            } => {
                let r = if caps.session_capabilities.close.is_some() {
                    cx.send_request(CloseSessionRequest::new(
                        agent_client_protocol::schema::v1::SessionId::new(
                            agent_session_id.as_str(),
                        ),
                    ))
                    .block_task()
                    .await
                    .map(|_| ())
                    .map_err(HubError::Acp)
                } else {
                    Err(HubError::UnsupportedCapability {
                        endpoint: String::new(),
                        operation: "close",
                        required_capability: "session_capabilities.close",
                    })
                };
                if r.is_ok() {
                    ctx.unbind_session(connection_agent_id, &agent_session_id);
                }
                let _ = reply.send(r);
            }
            AgentCommand::DeleteSession {
                conv_id: _,
                agent_session_id,
                local_only,
                reply,
            } => {
                let r = if caps.session_capabilities.delete.is_some() {
                    cx.send_request(DeleteSessionRequest::new(
                        agent_client_protocol::schema::v1::SessionId::new(
                            agent_session_id.as_str(),
                        ),
                    ))
                    .block_task()
                    .await
                    .map(|_| ())
                    .map_err(HubError::Acp)
                } else if local_only {
                    Ok(())
                } else {
                    Err(HubError::UnsupportedCapability {
                        endpoint: String::new(),
                        operation: "delete",
                        required_capability: "session_capabilities.delete",
                    })
                };
                if r.is_ok() {
                    ctx.unbind_session(connection_agent_id, &agent_session_id);
                }
                let _ = reply.send(r);
            }
            AgentCommand::ListSessions { cwd, reply } => {
                let result = list_all_sessions(&cx, caps, connection_agent_id, cwd).await;
                let _ = reply.send(result);
            }
            AgentCommand::SetConfig {
                agent_session_id,
                config_id,
                value,
                reply,
            } => {
                use agent_client_protocol::schema::v1::{SessionConfigId, SessionConfigValueId};
                let r = cx
                    .send_request(SetSessionConfigOptionRequest::new(
                        agent_client_protocol::schema::v1::SessionId::new(
                            agent_session_id.as_str(),
                        ),
                        SessionConfigId::new(config_id.as_str()),
                        SessionConfigValueId::new(value.as_str()),
                    ))
                    .block_task()
                    .await
                    .map_err(HubError::Acp);
                let _ = reply.send(r.map(|_| ()));
            }
            AgentCommand::SetMode {
                agent_session_id,
                mode_id,
                reply,
            } => {
                use agent_client_protocol::schema::v1::SessionModeId;
                let r = cx
                    .send_request(SetSessionModeRequest::new(
                        agent_client_protocol::schema::v1::SessionId::new(
                            agent_session_id.as_str(),
                        ),
                        SessionModeId::new(mode_id.as_str()),
                    ))
                    .block_task()
                    .await
                    .map_err(HubError::Acp);
                let _ = reply.send(r.map(|_| ()));
            }
            AgentCommand::Authenticate { method_id, reply } => {
                let r = cx
                    .send_request(AuthenticateRequest::new(
                        agent_client_protocol::schema::v1::AuthMethodId::new(method_id.as_str()),
                    ))
                    .block_task()
                    .await
                    .map_err(HubError::Acp);
                let _ = reply.send(r.map(|_| ()));
            }
            AgentCommand::Logout { reply } => {
                let r = cx
                    .send_request(LogoutRequest::new())
                    .block_task()
                    .await
                    .map_err(HubError::Acp);
                let _ = reply.send(r.map(|_| ()));
            }
        }
    }
    Ok(())
}
fn merge_capture_failure<T>(
    request_result: Result<T, HubError>,
    capture_failure: Option<HubError>,
) -> Result<T, HubError> {
    match request_result {
        Err(request_error) => Err(request_error),
        Ok(response) => capture_failure.map_or(Ok(response), Err),
    }
}

// ---- Session ops ----------------------------------------------------------

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

async fn collect_session_pages<F, Fut>(
    agent_id: &str,
    mut fetch: F,
) -> Result<ListSessionsResult, HubError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<SessionInfo>, Option<String>), HubError>>,
{
    const MAX_PAGES: usize = 10_000;
    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = std::collections::HashSet::new();

    for _ in 0..MAX_PAGES {
        let (page, next_cursor) = fetch(cursor.clone()).await?;
        sessions.extend(page);
        let Some(next) = next_cursor else {
            return Ok(ListSessionsResult { sessions });
        };
        if !seen_cursors.insert(next.clone()) {
            return Err(HubError::other(format!(
                "agent {agent_id:?} repeated session/list cursor {next:?}"
            )));
        }
        cursor = Some(next);
    }

    Err(HubError::other(format!(
        "agent {agent_id:?} exceeded {MAX_PAGES} session/list pages"
    )))
}

fn build_client_caps(cfg: &ClientCapabilityConfig) -> ClientCapabilities {
    let mut caps = ClientCapabilities::new();
    let fs = FileSystemCapabilities::new()
        .read_text_file(cfg.fs.read_text_file)
        .write_text_file(cfg.fs.write_text_file);
    caps = caps.fs(fs);
    caps.terminal(cfg.terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::FsConfig;
    use agent_client_protocol::schema::v1::{SessionId, SessionInfo};
    use parking_lot::Mutex;
    use std::collections::VecDeque;

    #[test]
    fn client_capabilities_match_endpoint_configuration() {
        let cfg = ClientCapabilityConfig {
            fs: FsConfig {
                read_text_file: true,
                write_text_file: false,
                allowed_roots: vec![PathBuf::from("ignored-on-wire")],
            },
            terminal: true,
        };

        let caps = build_client_caps(&cfg);

        assert!(caps.fs.read_text_file);
        assert!(!caps.fs.write_text_file);
        assert!(caps.terminal);
    }

    #[test]
    fn rejects_commands_routed_to_the_wrong_connection() {
        let error = ensure_agent_context("agent-a", "agent-b").unwrap_err();
        assert!(error.to_string().contains("agent-b"));
        assert!(error.to_string().contains("agent-a"));
    }

    #[test]
    fn request_failure_remains_primary_when_capture_also_failed() {
        let result = merge_capture_failure::<()>(
            Err(HubError::other("primary request failure")),
            Some(HubError::other("secondary capture failure")),
        );

        let message = result.unwrap_err().to_string();
        assert!(message.contains("primary request failure"));
        assert!(!message.contains("secondary capture failure"));
    }

    #[tokio::test]
    async fn session_page_collector_follows_every_cursor() {
        let pages = Arc::new(Mutex::new(VecDeque::from([
            (
                None,
                vec![SessionInfo::new(
                    SessionId::new("session-a"),
                    PathBuf::from("/workspace"),
                )],
                Some("page-2".to_string()),
            ),
            (
                Some("page-2".to_string()),
                vec![SessionInfo::new(
                    SessionId::new("session-b"),
                    PathBuf::from("/workspace"),
                )],
                None,
            ),
        ])));

        let result = collect_session_pages("paged-agent", {
            let pages = Arc::clone(&pages);
            move |cursor| {
                let (expected, sessions, next) =
                    pages.lock().pop_front().expect("requested expected page");
                assert_eq!(cursor, expected);
                std::future::ready(Ok((sessions, next)))
            }
        })
        .await
        .unwrap();

        assert_eq!(result.sessions.len(), 2);
        assert!(pages.lock().is_empty());
    }

    #[tokio::test]
    async fn session_page_collector_rejects_repeated_cursor() {
        let calls = Arc::new(Mutex::new(0usize));
        let error = collect_session_pages("looping-agent", {
            let calls = Arc::clone(&calls);
            move |_| {
                *calls.lock() += 1;
                std::future::ready(Ok((Vec::new(), Some("same".to_string()))))
            }
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("repeated session/list cursor"));
        assert_eq!(*calls.lock(), 2);
    }
}
