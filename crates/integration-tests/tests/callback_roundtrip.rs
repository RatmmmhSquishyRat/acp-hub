//! End-to-end ACP callback round-trip coverage.
//!
//! Unlike the platform-specific callback paths in Testy, this fixture sends
//! permission, filesystem, and terminal requests that stay inside a fresh
//! temporary session root on every supported CI platform.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_hub::acp::{AgentCommand, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::store::{NewConversation, Store};
use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, CreateTerminalRequest, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind,
    PromptCapabilities, PromptRequest, PromptResponse, ReadTextFileRequest, ReleaseTerminalRequest,
    RequestPermissionRequest, SessionId, StopReason, TerminalOutputRequest, TextContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, WaitForTerminalExitRequest,
    WriteTextFileRequest,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, DynConnectTo, JsonRpcRequest};

const AGENT_ID: &str = "callback-probe";
const SESSION_ID: &str = "callback-session";

#[derive(Clone, Debug)]
struct CallbackProbe {
    cwd: PathBuf,
    evidence: Arc<Mutex<Option<CallbackEvidence>>>,
}

#[derive(Debug)]
struct CallbackEvidence {
    read_content: String,
    terminal_output: String,
    permission_completed: bool,
}

impl ConnectTo<Client> for CallbackProbe {
    async fn connect_to(
        self,
        client: impl ConnectTo<Agent>,
    ) -> Result<(), agent_client_protocol::Error> {
        let cwd = self.cwd;
        let evidence = Arc::clone(&self.evidence);
        Agent
            .builder()
            .name(AGENT_ID)
            .on_receive_request(
                async |request: InitializeRequest, responder, _cx| {
                    responder.respond(
                        InitializeResponse::new(request.protocol_version).agent_capabilities(
                            AgentCapabilities::new().prompt_capabilities(PromptCapabilities::new()),
                        ),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |_request: NewSessionRequest, responder, _cx| {
                    responder.respond(NewSessionResponse::new(SessionId::new(SESSION_ID)))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, cx| {
                    let cx_for_task = cx.clone();
                    let cwd = cwd.clone();
                    let evidence = Arc::clone(&evidence);
                    cx.spawn(async move {
                        match exercise_callbacks(&cx_for_task, &request.session_id, &cwd).await {
                            Ok(result) => {
                                *evidence.lock().expect("evidence lock") = Some(result);
                                responder.respond(PromptResponse::new(StopReason::EndTurn))
                            }
                            Err(error) => responder.respond_with_error(error),
                        }
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(client)
            .await
    }
}

async fn request<Req>(
    connection: &ConnectionTo<Client>,
    request: Req,
) -> Result<Req::Response, agent_client_protocol::Error>
where
    Req: JsonRpcRequest,
    Req::Response: Send,
{
    connection.send_request(request).block_task().await
}

async fn exercise_callbacks(
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
    cwd: &Path,
) -> Result<CallbackEvidence, agent_client_protocol::Error> {
    let permission = RequestPermissionRequest::new(
        session_id.clone(),
        ToolCallUpdate::new(
            "callback-permission",
            ToolCallUpdateFields::new()
                .title("ACP callback round-trip")
                .status(ToolCallStatus::Pending),
        ),
        vec![PermissionOption::new(
            "allow_once",
            "Allow once",
            PermissionOptionKind::AllowOnce,
        )],
    );
    let _permission_response = request(connection, permission).await?;

    let path = cwd.join("callback-roundtrip.txt");
    request(
        connection,
        WriteTextFileRequest::new(session_id.clone(), path.clone(), "callback round-trip\n"),
    )
    .await?;
    let read = request(
        connection,
        ReadTextFileRequest::new(session_id.clone(), path),
    )
    .await?;
    if read.content != "callback round-trip\n" {
        return Err(agent_client_protocol::Error::new(
            -32603,
            "filesystem callback returned unexpected content",
        ));
    }

    let (command, args) = callback_command();
    let terminal = request(
        connection,
        CreateTerminalRequest::new(session_id.clone(), command)
            .args(args)
            .cwd(cwd.to_path_buf())
            .output_byte_limit(4096),
    )
    .await?;
    let terminal_id = terminal.terminal_id;
    let wait = request(
        connection,
        WaitForTerminalExitRequest::new(session_id.clone(), terminal_id.clone()),
    )
    .await?;
    if wait.exit_status.exit_code != Some(0) {
        return Err(agent_client_protocol::Error::new(
            -32603,
            "terminal callback returned a non-zero exit code",
        ));
    }
    let output = request(
        connection,
        TerminalOutputRequest::new(session_id.clone(), terminal_id.clone()),
    )
    .await?;
    request(
        connection,
        ReleaseTerminalRequest::new(session_id.clone(), terminal_id),
    )
    .await?;
    if !output.output.contains("callback-terminal") {
        return Err(agent_client_protocol::Error::new(
            -32603,
            "terminal callback returned unexpected output",
        ));
    }

    Ok(CallbackEvidence {
        read_content: read.content,
        terminal_output: output.output,
        permission_completed: true,
    })
}

#[cfg(windows)]
fn callback_command() -> (String, Vec<String>) {
    (
        "cmd.exe".into(),
        vec![
            "/D".into(),
            "/S".into(),
            "/C".into(),
            "echo callback-terminal".into(),
        ],
    )
}

#[cfg(not(windows))]
fn callback_command() -> (String, Vec<String>) {
    (
        "sh".into(),
        vec!["-c".into(), "printf callback-terminal".into()],
    )
}

#[tokio::test]
async fn callbacks_round_trip_through_the_acp_connection() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_path_buf();
    let evidence = Arc::new(Mutex::new(None));
    let store = Store::open_memory().unwrap();
    store
        .create_conversation(&NewConversation {
            id: "conv-callback".into(),
            agent_id: AGENT_ID.into(),
            agent_session_id: "pending".into(),
            cwd: Some(cwd.display().to_string()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    let ctx = HubCtx::new(store);
    let component: DynConnectTo<Client> = DynConnectTo::new(CallbackProbe {
        cwd: cwd.clone(),
        evidence: Arc::clone(&evidence),
    });
    let handle = tokio::time::timeout(
        Duration::from_secs(10),
        spawn_agent_connection(
            component,
            AGENT_ID.into(),
            acp_hub_integration_tests::test_agent_config(),
            Arc::clone(&ctx),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();

    let (create_tx, create_rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::CreateSession {
            conv_id: "conv-callback".into(),
            agent_id: AGENT_ID.into(),
            cwd: cwd.clone(),
            additional_directories: vec![],
            mcp_servers: vec![],
            reply: create_tx,
        })
        .await
        .unwrap();
    let created = tokio::time::timeout(Duration::from_secs(10), create_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "conv-callback",
        AGENT_ID,
        &created.agent_session_id,
        cwd,
    )
    .unwrap();

    let (prompt_tx, prompt_rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "conv-callback".into(),
            agent_session_id: created.agent_session_id,
            prompt: vec![ContentBlock::Text(TextContent::new("exercise callbacks"))],
            params: vec![],
            mode_id: None,
            reply: prompt_tx,
        })
        .await
        .unwrap();
    let prompt = tokio::time::timeout(Duration::from_secs(15), prompt_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(prompt.stop_reason, StopReason::EndTurn);

    let evidence = evidence.lock().unwrap().take().expect("callback evidence");
    assert!(evidence.permission_completed);
    assert_eq!(evidence.read_content, "callback round-trip\n");
    assert!(evidence.terminal_output.contains("callback-terminal"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("callback-roundtrip.txt")).unwrap(),
        "callback round-trip\n"
    );
}
