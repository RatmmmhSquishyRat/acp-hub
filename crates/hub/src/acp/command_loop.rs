use super::*;

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

pub(super) async fn run_command_loop(
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
                if let Err(error) = validate_prompt_capabilities(connection_agent_id, caps, &prompt)
                {
                    let _ = reply.send(Err(error));
                    continue;
                }
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
pub(super) fn merge_capture_failure<T>(
    request_result: Result<T, HubError>,
    capture_failure: Option<HubError>,
) -> Result<T, HubError> {
    match request_result {
        Err(request_error) => Err(request_error),
        Ok(response) => capture_failure.map_or(Ok(response), Err),
    }
}

// ---- Session ops ----------------------------------------------------------
