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
    RequestPermissionResponse, ResumeSessionRequest, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, DynConnectTo};

use crate::callbacks::HubCtx;
use crate::endpoint::{ClientCapabilityConfig, PermissionPolicy};
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
        additional_directories: Vec<PathBuf>,
        reply: tokio::sync::oneshot::Sender<Result<SessionCreated, HubError>>,
    },
    ResumeSession {
        conv_id: String,
        agent_id: String,
        agent_session_id: String,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
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
    __agent_id: String,
    cfg: ClientCapabilityConfig,
    permission_policy: PermissionPolicy,
    ctx: Arc<HubCtx>,
) -> tokio::sync::oneshot::Receiver<Result<AgentHandle, HubError>> {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(64);
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let handle_tx = Arc::new(parking_lot::Mutex::new(Some(handle_tx)));
    let ctx2 = Arc::clone(&ctx);
    let handle_tx_inner = Arc::clone(&handle_tx);

    tokio::spawn(async move {
        let result = Client
            .builder()
            .on_receive_notification(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |notif: SessionNotification, _cx| {
                        if let Err(e) = ctx.handle_notification(notif) {
                            tracing::warn!(?e, "notification handler");
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: RequestPermissionRequest, responder, _cx| {
                        let resp = ctx.handle_permission(&req);
                        if let Err(e) = responder.respond(resp) {
                            tracing::warn!(error = %e, "responder failed");
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: ReadTextFileRequest, responder, _cx| match ctx
                        .handle_read_text_file(&req)
                    {
                        Ok(resp) => responder.respond(resp),
                        Err(e) => responder.respond_with_internal_error(e),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: WriteTextFileRequest, responder, _cx| match ctx
                        .handle_write_text_file(&req)
                    {
                        Ok(resp) => responder.respond(resp),
                        Err(e) => responder.respond_with_internal_error(e),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: CreateTerminalRequest, responder, _cx| match ctx
                        .handle_terminal_create(&req)
                    {
                        Ok(resp) => responder.respond(resp),
                        Err(e) => responder.respond_with_internal_error(e),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: TerminalOutputRequest, responder, _cx| {
                        let resp = ctx.handle_terminal_output(&req);
                        if let Err(e) = responder.respond(resp) {
                            tracing::warn!(error = %e, "responder failed");
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: WaitForTerminalExitRequest, responder, _cx| {
                        let resp = ctx.handle_terminal_wait(&req);
                        if let Err(e) = responder.respond(resp) {
                            tracing::warn!(error = %e, "responder failed");
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: KillTerminalRequest, responder, _cx| {
                        let resp = ctx.handle_terminal_kill(&req);
                        if let Err(e) = responder.respond(resp) {
                            tracing::warn!(error = %e, "responder failed");
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: ReleaseTerminalRequest, responder, _cx| {
                        let resp = ctx.handle_terminal_release(&req);
                        if let Err(e) = responder.respond(resp) {
                            tracing::warn!(error = %e, "responder failed");
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(component, async move |cx| {
                let init = match cx
                    .send_request(
                        InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
                            .client_capabilities(build_client_caps(&cfg)),
                    )
                    .block_task()
                    .await
                {
                    Ok(init) => init,
                    Err(e) => {
                        if let Some(tx) = handle_tx_inner.lock().take() {
                            let _ = tx.send(Err(HubError::Other(format!(
                                "agent initialize failed: {e}"
                            ))));
                        }
                        return Err(e);
                    }
                };
                // #15: reject agents that negotiate a non-v1 protocol version.
                if init.protocol_version != agent_client_protocol::schema::ProtocolVersion::V1 {
                    if let Some(tx) = handle_tx_inner.lock().take() {
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

                if let Some(tx) = handle_tx_inner.lock().take() {
                    let _ = tx.send(Ok(AgentHandle {
                        cmd_tx: cmd_tx.clone(),
                        cx: cx.clone(),
                        capabilities: caps.clone(),
                        auth_methods: auth,
                    }));
                }

                run_command_loop(cx, cmd_rx, &caps, cfg, permission_policy, Arc::clone(&ctx2)).await
            })
            .await;

        if let Err(e) = result {
            if let Some(tx) = handle_tx.lock().take() {
                let _ = tx.send(Err(HubError::Other(format!("connection failed: {e}"))));
            }
        }
    });

    handle_rx
}

// ---- Command loop ---------------------------------------------------------

async fn run_command_loop(
    cx: ConnectionTo<Agent>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<AgentCommand>,
    caps: &AgentCapabilities,
    client_caps: ClientCapabilityConfig,
    permission_policy: PermissionPolicy,
    ctx: Arc<HubCtx>,
) -> Result<(), agent_client_protocol::Error> {
    use crate::callbacks::SessionBinding;
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            AgentCommand::CreateSession {
                conv_id,
                agent_id,
                cwd,
                additional_directories,
                mcp_servers,
                reply,
            } => {
                let r = create_session(
                    &cx,
                    caps,
                    &agent_id,
                    cwd.clone(),
                    additional_directories,
                    mcp_servers,
                )
                .await;
                if let Ok(ref created) = r {
                    ctx.bind_session(
                        &created.agent_session_id,
                        SessionBinding {
                            conv_id: conv_id.clone(),
                            agent_id: agent_id.clone(),
                            permission_policy,
                            fs: client_caps.fs.clone(),
                            cwd,
                            terminal_enabled: client_caps.terminal,
                        },
                    );
                }
                let _ = reply.send(r);
            }
            AgentCommand::LoadSession {
                conv_id,
                agent_id,
                agent_session_id,
                cwd,
                additional_directories,
                reply,
            } => {
                // Bind session BEFORE load so session/update notifications
                // during LoadSessionRequest are captured (Layer 1 messages).
                ctx.bind_session(
                    &agent_session_id,
                    SessionBinding {
                        conv_id: conv_id.clone(),
                        agent_id: agent_id.clone(),
                        permission_policy,
                        fs: client_caps.fs.clone(),
                        cwd: cwd.clone(),
                        terminal_enabled: client_caps.terminal,
                    },
                );
                ctx.set_loading(&agent_session_id, true);
                let r = load_session(
                    &cx,
                    caps,
                    &agent_id,
                    &agent_session_id,
                    cwd,
                    additional_directories,
                )
                .await;
                ctx.set_loading(&agent_session_id, false);
                let _ = reply.send(r);
            }
            AgentCommand::ResumeSession {
                conv_id,
                agent_id,
                agent_session_id,
                cwd,
                additional_directories,
                reply,
            } => {
                // Bind session BEFORE resume so notifications are captured.
                ctx.bind_session(
                    &agent_session_id,
                    SessionBinding {
                        conv_id: conv_id.clone(),
                        agent_id: agent_id.clone(),
                        permission_policy,
                        fs: client_caps.fs.clone(),
                        cwd: cwd.clone(),
                        terminal_enabled: client_caps.terminal,
                    },
                );
                ctx.set_loading(&agent_session_id, true);
                let r = resume_session(
                    &cx,
                    caps,
                    &agent_id,
                    &agent_session_id,
                    cwd,
                    additional_directories,
                )
                .await;
                ctx.set_loading(&agent_session_id, false);
                let _ = reply.send(r);
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
                let r = send_prompt(&cx, &agent_session_id, prompt, params, mode_id).await;
                let _ = reply.send(r);
            }
            AgentCommand::CloseSession {
                conv_id,
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
                        endpoint: endpoint_for_command(&ctx, &conv_id, &agent_session_id),
                        operation: "session/close".into(),
                        required_capability: "session_capabilities.close".into(),
                    })
                };
                if r.is_ok() {
                    ctx.unbind_session(&agent_session_id);
                }
                let _ = reply.send(r);
            }
            AgentCommand::DeleteSession {
                conv_id,
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
                        endpoint: endpoint_for_command(&ctx, &conv_id, &agent_session_id),
                        operation: "session/delete".into(),
                        required_capability: "session_capabilities.delete".into(),
                    })
                };
                ctx.unbind_session(&agent_session_id);
                let _ = reply.send(r);
            }
            AgentCommand::ListSessions { cwd, reply } => {
                // P2-2: cursor-based pagination — follow next_cursor until the
                // agent stops paging or the max-pages safety guard trips.
                const MAX_PAGES: usize = 100;
                let mut all: Vec<agent_client_protocol::schema::v1::SessionInfo> = Vec::new();
                let mut cursor: Option<String> = None;
                let mut pages = 0usize;
                let mut err: Option<HubError> = None;
                loop {
                    pages += 1;
                    let mut req = ListSessionsRequest::new();
                    if let Some(d) = &cwd {
                        req = req.cwd(d.clone());
                    }
                    if let Some(c) = &cursor {
                        req = req.cursor(c.clone());
                    }
                    match cx.send_request(req).block_task().await {
                        Ok(resp) => {
                            all.extend(resp.sessions);
                            match resp.next_cursor {
                                Some(c) if !c.is_empty() => cursor = Some(c),
                                // Absent/empty cursor => no more pages.
                                _ => {
                                    cursor = None;
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            err = Some(HubError::Acp(e));
                            break;
                        }
                    }
                    if pages >= MAX_PAGES {
                        break;
                    }
                }
                if err.is_none() && cursor.is_some() {
                    tracing::warn!(
                        pages,
                        "session/list pagination hit max-pages guard; results may be incomplete"
                    );
                }
                let _ = reply.send(match err {
                    // First-page failure: propagate the error (preserves prior
                    // contract — callers see the underlying ACP error).
                    Some(e) if all.is_empty() => Err(e),
                    // Partial fetch then a later-page error: keep what we have.
                    Some(e) => {
                        tracing::warn!(?e, "session/list pagination error after partial fetch");
                        Ok(ListSessionsResult { sessions: all })
                    }
                    None => Ok(ListSessionsResult { sessions: all }),
                });
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

// ---- Session ops ----------------------------------------------------------

fn endpoint_for_command(ctx: &HubCtx, conv_id: &str, agent_session_id: &str) -> String {
    ctx.store()
        .conversation(conv_id)
        .ok()
        .flatten()
        .map(|conv| conv.agent_id)
        .unwrap_or_else(|| {
            format!("unknown endpoint for conversation {conv_id} session {agent_session_id}")
        })
}

fn validate_new_session_options(
    caps: &AgentCapabilities,
    agent_id: &str,
    additional: &[PathBuf],
    mcp_servers: &[agent_client_protocol::schema::v1::McpServer],
) -> Result<(), HubError> {
    validate_session_additional_directories(caps, agent_id, "session/new", additional)?;

    for server in mcp_servers {
        let required_capability = match server {
            agent_client_protocol::schema::v1::McpServer::Http(_) => {
                (!caps.mcp_capabilities.http).then_some("mcp_capabilities.http")
            }
            agent_client_protocol::schema::v1::McpServer::Sse(_) => {
                (!caps.mcp_capabilities.sse).then_some("mcp_capabilities.sse")
            }
            agent_client_protocol::schema::v1::McpServer::Stdio(_) => None,
            _ => Some("mcp_capabilities for requested mcp_servers"),
        };
        if let Some(required_capability) = required_capability {
            return Err(HubError::UnsupportedCapability {
                endpoint: agent_id.into(),
                operation: "session/new".into(),
                required_capability: required_capability.into(),
            });
        }
    }

    Ok(())
}

fn validate_session_additional_directories(
    caps: &AgentCapabilities,
    agent_id: &str,
    operation: &'static str,
    additional: &[PathBuf],
) -> Result<(), HubError> {
    if !additional.is_empty() && caps.session_capabilities.additional_directories.is_none() {
        return Err(HubError::UnsupportedCapability {
            endpoint: agent_id.into(),
            operation: operation.into(),
            required_capability: "session_capabilities.additional_directories".into(),
        });
    }
    Ok(())
}

async fn create_session(
    cx: &ConnectionTo<Agent>,
    caps: &AgentCapabilities,
    agent_id: &str,
    cwd: PathBuf,
    additional: Vec<PathBuf>,
    mcp_servers: Vec<agent_client_protocol::schema::v1::McpServer>,
) -> Result<SessionCreated, HubError> {
    validate_new_session_options(caps, agent_id, &additional, &mcp_servers)?;
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
    additional: Vec<PathBuf>,
) -> Result<SessionCreated, HubError> {
    if !caps.load_session {
        return Err(HubError::UnsupportedCapability {
            endpoint: agent_id.into(),
            operation: "session/load".into(),
            required_capability: "load_session".into(),
        });
    }
    validate_session_additional_directories(caps, agent_id, "session/load", &additional)?;
    let req = LoadSessionRequest::new(
        agent_client_protocol::schema::v1::SessionId::new(agent_session_id),
        cwd,
    )
    .additional_directories(additional);
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
    additional: Vec<PathBuf>,
) -> Result<SessionCreated, HubError> {
    if caps.session_capabilities.resume.is_none() {
        return Err(HubError::UnsupportedCapability {
            endpoint: agent_id.into(),
            operation: "session/resume".into(),
            required_capability: "session_capabilities.resume".into(),
        });
    }
    validate_session_additional_directories(caps, agent_id, "session/resume", &additional)?;
    let req = ResumeSessionRequest::new(
        agent_client_protocol::schema::v1::SessionId::new(agent_session_id),
        cwd,
    )
    .additional_directories(additional);
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
    })
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
    use agent_client_protocol::schema::v1::{
        McpCapabilities, McpServer, McpServerHttp, McpServerSse, McpServerStdio,
        SessionAdditionalDirectoriesCapabilities, SessionCapabilities,
    };

    fn assert_unsupported_session_new(
        result: Result<(), HubError>,
        endpoint: &str,
        required_capability: &'static str,
    ) {
        match result.unwrap_err() {
            HubError::UnsupportedCapability {
                endpoint: actual_endpoint,
                operation,
                required_capability: actual_capability,
            } => {
                assert_eq!(actual_endpoint, endpoint);
                assert_eq!(operation, "session/new");
                assert_eq!(actual_capability, required_capability);
            }
            other => panic!("expected UnsupportedCapability, got {other:?}"),
        }
    }

    fn assert_unsupported_operation(
        result: Result<(), HubError>,
        endpoint: &str,
        expected_operation: &'static str,
        required_capability: &'static str,
    ) {
        match result.unwrap_err() {
            HubError::UnsupportedCapability {
                endpoint: actual_endpoint,
                operation,
                required_capability: actual_capability,
            } => {
                assert_eq!(actual_endpoint, endpoint);
                assert_eq!(operation, expected_operation);
                assert_eq!(actual_capability, required_capability);
            }
            other => panic!("expected UnsupportedCapability, got {other:?}"),
        }
    }

    #[test]
    fn new_session_rejects_additional_directories_without_capability() {
        assert_unsupported_session_new(
            validate_new_session_options(
                &AgentCapabilities::new(),
                "agent-a",
                &[PathBuf::from("/workspace/extra")],
                &[],
            ),
            "agent-a",
            "session_capabilities.additional_directories",
        );
    }

    #[test]
    fn new_session_allows_additional_directories_with_capability() {
        let caps = AgentCapabilities::new().session_capabilities(
            SessionCapabilities::new()
                .additional_directories(SessionAdditionalDirectoriesCapabilities::new()),
        );

        validate_new_session_options(&caps, "agent-a", &[PathBuf::from("/workspace/extra")], &[])
            .unwrap();
    }

    #[test]
    fn load_session_rejects_additional_directories_without_capability() {
        assert_unsupported_operation(
            validate_session_additional_directories(
                &AgentCapabilities::new(),
                "agent-a",
                "session/load",
                &[PathBuf::from("/workspace/extra")],
            ),
            "agent-a",
            "session/load",
            "session_capabilities.additional_directories",
        );
    }

    #[test]
    fn resume_session_allows_additional_directories_with_capability() {
        let caps = AgentCapabilities::new().session_capabilities(
            SessionCapabilities::new()
                .additional_directories(SessionAdditionalDirectoriesCapabilities::new()),
        );

        validate_session_additional_directories(
            &caps,
            "agent-a",
            "session/resume",
            &[PathBuf::from("/workspace/extra")],
        )
        .unwrap();
    }

    #[test]
    fn new_session_rejects_unsupported_mcp_transports() {
        assert_unsupported_session_new(
            validate_new_session_options(
                &AgentCapabilities::new(),
                "agent-a",
                &[],
                &[McpServer::Http(McpServerHttp::new(
                    "remote",
                    "https://mcp.example/acp",
                ))],
            ),
            "agent-a",
            "mcp_capabilities.http",
        );

        assert_unsupported_session_new(
            validate_new_session_options(
                &AgentCapabilities::new(),
                "agent-a",
                &[],
                &[McpServer::Sse(McpServerSse::new(
                    "remote",
                    "https://mcp.example/sse",
                ))],
            ),
            "agent-a",
            "mcp_capabilities.sse",
        );
    }

    #[test]
    fn new_session_allows_supported_mcp_transports() {
        let caps =
            AgentCapabilities::new().mcp_capabilities(McpCapabilities::new().http(true).sse(true));

        validate_new_session_options(
            &caps,
            "agent-a",
            &[],
            &[
                McpServer::Stdio(McpServerStdio::new("local", "local-mcp")),
                McpServer::Http(McpServerHttp::new("http", "https://mcp.example/acp")),
                McpServer::Sse(McpServerSse::new("sse", "https://mcp.example/sse")),
            ],
        )
        .unwrap();
    }
}
