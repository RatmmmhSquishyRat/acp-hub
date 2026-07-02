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
use crate::endpoint::ClientCapabilityConfig;
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
    __agent_id: String,
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
                        let _ = responder.respond(resp);
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
                        Err(e) => responder.respond(ReadTextFileResponse::new(format!("{e}"))),
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
                        Err(_) => responder.respond(WriteTextFileResponse::new()),
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
                        Err(e) => responder.respond(CreateTerminalResponse::new(
                            agent_client_protocol::schema::v1::TerminalId::new(format!("err: {e}")),
                        )),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let ctx = Arc::clone(&ctx2);
                    async move |req: TerminalOutputRequest, responder, _cx| {
                        let resp = ctx.handle_terminal_output(&req);
                        let _ = responder.respond(resp);
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
                        let _ = responder.respond(resp);
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
                        let _ = responder.respond(resp);
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
                        let _ = responder.respond(resp);
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(component, async move |cx| {
                let init = match cx
                    .send_request(InitializeRequest::new(
                        agent_client_protocol::schema::ProtocolVersion::V1,
                    ))
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

                run_command_loop(cx, cmd_rx, &caps, Arc::clone(&ctx2)).await
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
    ctx: Arc<HubCtx>,
) -> Result<(), agent_client_protocol::Error> {
    use crate::callbacks::SessionBinding;
    use crate::endpoint::{FsConfig, PermissionPolicy};
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
                            permission_policy: PermissionPolicy::default(),
                            fs: FsConfig::default(),
                            cwd,
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
                reply,
            } => {
                // Bind session BEFORE load so session/update notifications
                // during LoadSessionRequest are captured (Layer 1 messages).
                ctx.bind_session(
                    &agent_session_id,
                    SessionBinding {
                        conv_id: conv_id.clone(),
                        agent_id: agent_id.clone(),
                        permission_policy: PermissionPolicy::default(),
                        fs: FsConfig::default(),
                        cwd: cwd.clone(),
                    },
                );
                ctx.set_loading(&agent_session_id, true);
                let r = load_session(&cx, caps, &agent_id, &agent_session_id, cwd).await;
                ctx.set_loading(&agent_session_id, false);
                let _ = reply.send(r);
            }
            AgentCommand::ResumeSession {
                conv_id,
                agent_id,
                agent_session_id,
                cwd,
                reply,
            } => {
                // Bind session BEFORE resume so notifications are captured.
                ctx.bind_session(
                    &agent_session_id,
                    SessionBinding {
                        conv_id: conv_id.clone(),
                        agent_id: agent_id.clone(),
                        permission_policy: PermissionPolicy::default(),
                        fs: FsConfig::default(),
                        cwd: cwd.clone(),
                    },
                );
                ctx.set_loading(&agent_session_id, true);
                let r = resume_session(&cx, caps, &agent_id, &agent_session_id, cwd).await;
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
                    ctx.unbind_session(&agent_session_id);
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
                ctx.unbind_session(&agent_session_id);
                let _ = reply.send(r);
            }
            AgentCommand::ListSessions { cwd, reply } => {
                let mut req = ListSessionsRequest::new();
                if let Some(d) = &cwd {
                    req = req.cwd(d.clone());
                }
                let result = cx
                    .send_request(req)
                    .block_task()
                    .await
                    .map_err(HubError::Acp);
                let _ = reply.send(result.map(|r| ListSessionsResult {
                    sessions: r.sessions,
                }));
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

fn _build_client_caps(cfg: &ClientCapabilityConfig) -> ClientCapabilities {
    let mut caps = ClientCapabilities::new();
    let fs = FileSystemCapabilities::new()
        .read_text_file(cfg.fs.read_text_file)
        .write_text_file(cfg.fs.write_text_file);
    caps = caps.fs(fs);
    caps.terminal(cfg.terminal)
}
