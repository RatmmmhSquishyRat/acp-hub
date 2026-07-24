//! Regression coverage for callback-capture failures that arrive out of band
//! from the ACP request which caused them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use acp_hub::acp::{AgentCommand, AgentHandle, spawn_agent_connection};
use acp_hub::callbacks::HubCtx;
use acp_hub::daemon::ActivityTracker;
use acp_hub::endpoint::{
    AgentEndpointConfig, AgentTransport, ClientCapabilityConfig, PermissionPolicy, Registry,
};
use acp_hub::hub::{CoreHub, CreateConversationParams, SendPromptParams};
use acp_hub::store::{ConvStatus, MessageSource, NewConversation, NewMessage, RunStatus, Store};
use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionId,
    SessionNotification, SessionResumeCapabilities, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectTo, DynConnectTo};

const AGENT_ID: &str = "capture-failure-probe";
const SESSION_ID: &str = "capture-failure-session";
const OVERSIZED_UPDATE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default)]
struct CaptureFailureProbe {
    load_attempts: Arc<AtomicUsize>,
    resume_attempts: Arc<AtomicUsize>,
    prompt_attempts: Arc<AtomicUsize>,
}

impl ConnectTo<Client> for CaptureFailureProbe {
    async fn connect_to(
        self,
        client: impl ConnectTo<Agent>,
    ) -> Result<(), agent_client_protocol::Error> {
        let load_attempts = Arc::clone(&self.load_attempts);
        let resume_attempts = Arc::clone(&self.resume_attempts);
        let prompt_attempts = Arc::clone(&self.prompt_attempts);

        Agent
            .builder()
            .name(AGENT_ID)
            .on_receive_request(
                async |request: InitializeRequest, responder, _cx| {
                    responder.respond(
                        InitializeResponse::new(request.protocol_version).agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(true)
                                .prompt_capabilities(PromptCapabilities::new())
                                .session_capabilities(
                                    SessionCapabilities::new()
                                        .resume(SessionResumeCapabilities::new()),
                                ),
                        ),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: LoadSessionRequest, responder, cx| {
                    let attempt = load_attempts.fetch_add(1, Ordering::SeqCst);
                    send_probe_update(&cx, &request.session_id, "load", attempt)?;
                    wait_for_callback().await;
                    responder.respond(LoadSessionResponse::new())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: ResumeSessionRequest, responder, cx| {
                    let attempt = resume_attempts.fetch_add(1, Ordering::SeqCst);
                    send_probe_update(&cx, &request.session_id, "resume", attempt)?;
                    wait_for_callback().await;
                    responder.respond(ResumeSessionResponse::new())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, cx| {
                    let attempt = prompt_attempts.fetch_add(1, Ordering::SeqCst);
                    send_probe_update(&cx, &request.session_id, "prompt", attempt)?;
                    wait_for_callback().await;
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(client)
            .await
    }
}

fn send_probe_update(
    cx: &agent_client_protocol::ConnectionTo<Client>,
    session_id: &SessionId,
    operation: &str,
    attempt: usize,
) -> Result<(), agent_client_protocol::Error> {
    if attempt == 0 {
        cx.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(format!("transient {operation} update")),
            ))),
        ))?;
        cx.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("x".repeat(OVERSIZED_UPDATE_BYTES)),
            ))),
        ))
    } else {
        cx.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(format!("clean {operation} update")),
            ))),
        ))
    }
}

async fn wait_for_callback() {
    // Notification handlers run independently of the request future. Delay the
    // nominal response so the callback's out-of-band error is recorded first.
    tokio::time::sleep(Duration::from_millis(100)).await;
}

fn agent_config() -> AgentEndpointConfig {
    AgentEndpointConfig {
        transport: AgentTransport::Stdio {
            command: "unused".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
        proxy_chain: Vec::new(),
        permission_policy: PermissionPolicy::Reject,
        client_capabilities: ClientCapabilityConfig::default(),
    }
}

fn stdio_agent_config() -> AgentEndpointConfig {
    let mut config = agent_config();
    config.transport = AgentTransport::Stdio {
        command: env!("CARGO_BIN_EXE_capture_failure_agent").into(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    config
}

fn core_setup() -> (tempfile::TempDir, CoreHub) {
    let temp = tempfile::tempdir().expect("temporary CoreHub home");
    let store = Store::open(temp.path()).expect("open test store");
    store
        .create_conversation(&NewConversation {
            id: "conv-capture-failure".into(),
            agent_id: AGENT_ID.into(),
            agent_session_id: SESSION_ID.into(),
            cwd: Some(temp.path().to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .expect("create test conversation");
    store
        .append_message(&NewMessage {
            id: "old-projection".into(),
            conv_id: "conv-capture-failure".into(),
            run_id: None,
            source: MessageSource::LoadReplay,
            role: "assistant".into(),
            kind: None,
            content_json: serde_json::json!({"type": "text", "text": "old projection"}),
            body_text: "old projection".into(),
        })
        .expect("seed old projection");
    let mut registry = Registry::default();
    registry
        .register_agent(AGENT_ID.into(), stdio_agent_config())
        .expect("register stdio capture probe");
    let hub = CoreHub::new(
        temp.path(),
        registry,
        store,
        Arc::new(ActivityTracker::new()),
    );
    (temp, hub)
}

fn existing_conversation_params(cwd: &Path) -> CreateConversationParams {
    CreateConversationParams {
        agent_id: AGENT_ID.into(),
        cwd: Some(cwd.to_path_buf()),
        agent_session_id: Some(SESSION_ID.into()),
        mcp_servers: Vec::new(),
        additional_directories: Vec::new(),
    }
}

async fn setup() -> (tempfile::TempDir, Arc<HubCtx>, AgentHandle) {
    let temp = tempfile::tempdir().expect("temporary Hub home");
    let store = Store::open(temp.path()).expect("open test store");
    store
        .create_conversation(&NewConversation {
            id: "conv-capture-failure".into(),
            agent_id: AGENT_ID.into(),
            agent_session_id: SESSION_ID.into(),
            cwd: Some(temp.path().to_string_lossy().into_owned()),
            additional_directories: Vec::new(),
            title: None,
        })
        .expect("create test conversation");
    let ctx = HubCtx::new(store);
    let component: DynConnectTo<Client> = DynConnectTo::new(CaptureFailureProbe::default());
    let handle = tokio::time::timeout(
        Duration::from_secs(10),
        spawn_agent_connection(component, AGENT_ID.into(), agent_config(), Arc::clone(&ctx)),
    )
    .await
    .expect("probe initialization timeout")
    .expect("probe handle channel")
    .expect("initialize probe");
    (temp, ctx, handle)
}

async fn load(
    handle: &AgentHandle,
    method: &'static str,
    cwd: &Path,
) -> Result<(), acp_hub::HubError> {
    let (reply, response) = tokio::sync::oneshot::channel();
    let command = match method {
        "load" => AgentCommand::LoadSession {
            conv_id: "conv-capture-failure".into(),
            agent_id: AGENT_ID.into(),
            agent_session_id: SESSION_ID.into(),
            cwd: cwd.to_path_buf(),
            reply,
        },
        "resume" => AgentCommand::ResumeSession {
            conv_id: "conv-capture-failure".into(),
            agent_id: AGENT_ID.into(),
            agent_session_id: SESSION_ID.into(),
            cwd: cwd.to_path_buf(),
            reply,
        },
        _ => unreachable!("unsupported replay method"),
    };
    handle
        .cmd_tx
        .send(command)
        .await
        .expect("send replay command");
    response.await.expect("replay command response").map(|_| ())
}

async fn prompt(handle: &AgentHandle) -> Result<(), acp_hub::HubError> {
    let (reply, response) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(AgentCommand::SendPrompt {
            conv_id: "conv-capture-failure".into(),
            agent_session_id: SESSION_ID.into(),
            prompt: vec![ContentBlock::Text(TextContent::new("probe capture"))],
            params: Vec::new(),
            mode_id: None,
            reply,
        })
        .await
        .expect("send prompt command");
    response.await.expect("prompt command response").map(|_| ())
}

fn assert_capture_failure(result: Result<(), acp_hub::HubError>, operation: &str) {
    let error = result.expect_err("capture failure must override the successful ACP response");
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    assert!(
        message.contains(operation) && message.contains("session update exceeds"),
        "unexpected capture failure: {message}"
    );
}

#[tokio::test]
async fn load_capture_failure_overrides_success_and_does_not_poison_next_load() {
    let (temp, _ctx, handle) = setup().await;

    assert_capture_failure(load(&handle, "load", temp.path()).await, "load");
    load(&handle, "load", temp.path())
        .await
        .expect("later clean load must succeed");
}

#[tokio::test]
async fn resume_capture_failure_overrides_success_and_does_not_poison_next_resume() {
    let (temp, _ctx, handle) = setup().await;

    assert_capture_failure(load(&handle, "resume", temp.path()).await, "resume");
    load(&handle, "resume", temp.path())
        .await
        .expect("later clean resume must succeed");
}

#[tokio::test]
async fn prompt_capture_failure_overrides_end_turn_and_does_not_poison_next_prompt() {
    let (temp, ctx, handle) = setup().await;
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "conv-capture-failure",
        AGENT_ID,
        SESSION_ID,
        PathBuf::from(temp.path()),
    )
    .expect("bind prompt session");

    ctx.store()
        .create_run("run-failed-capture", "conv-capture-failure")
        .expect("create failed-capture run");
    ctx.set_current_run(AGENT_ID, SESSION_ID, "run-failed-capture");
    let failed = prompt(&handle).await;
    ctx.clear_current_run(AGENT_ID, SESSION_ID);
    ctx.store()
        .finalize_run_cas(
            "run-failed-capture",
            "conv-capture-failure",
            RunStatus::Failed,
            None,
        )
        .expect("finalize failed-capture run");
    assert_capture_failure(failed, "prompt");
    assert_eq!(
        ctx.store().run_status("run-failed-capture").unwrap(),
        Some(RunStatus::Failed)
    );

    ctx.store()
        .create_run("run-clean-capture", "conv-capture-failure")
        .expect("create clean-capture run");
    ctx.set_current_run(AGENT_ID, SESSION_ID, "run-clean-capture");
    prompt(&handle)
        .await
        .expect("later clean prompt must succeed");
    ctx.clear_current_run(AGENT_ID, SESSION_ID);
    ctx.store()
        .finalize_run_cas(
            "run-clean-capture",
            "conv-capture-failure",
            RunStatus::Completed,
            Some("EndTurn"),
        )
        .expect("finalize clean-capture run");
    assert_eq!(
        ctx.store().run_status("run-clean-capture").unwrap(),
        Some(RunStatus::Completed)
    );
}

#[tokio::test]
async fn core_load_capture_failure_rolls_back_replay_and_preserves_old_projection() {
    let (temp, hub) = core_setup();

    let failed = hub
        .create_conversation(existing_conversation_params(temp.path()))
        .await
        .map(|_| ());
    assert_capture_failure(failed, "load");
    let messages = hub
        .store()
        .messages("conv-capture-failure", false)
        .expect("messages after failed refresh");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body_text, "old projection");
    assert!(
        messages
            .iter()
            .all(|message| !message.body_text.contains("transient load update")),
        "failed replay rows must be rolled back"
    );

    hub.create_conversation(existing_conversation_params(temp.path()))
        .await
        .expect("later clean load must succeed");
    let messages = hub
        .store()
        .messages("conv-capture-failure", false)
        .expect("messages after clean refresh");
    assert_eq!(messages.len(), 1);
    assert!(messages[0].body_text.contains("clean load update"));
}

#[tokio::test]
async fn core_prompt_capture_failure_marks_run_failed_before_later_clean_completion() {
    let (temp, hub) = core_setup();
    let initial_failure = hub
        .create_conversation(existing_conversation_params(temp.path()))
        .await
        .map(|_| ());
    assert_capture_failure(initial_failure, "load");
    hub.create_conversation(existing_conversation_params(temp.path()))
        .await
        .expect("clean load establishes a live session");

    let failed = hub
        .send_prompt(SendPromptParams {
            conv_id: "conv-capture-failure".into(),
            prompt: vec![ContentBlock::Text(TextContent::new(
                "failing public prompt",
            ))],
            params: Vec::new(),
            mode_id: None,
        })
        .await
        .map(|_| ());
    assert_capture_failure(failed, "prompt");
    let messages = hub
        .store()
        .messages("conv-capture-failure", true)
        .expect("messages after failed prompt");
    let failed_run_id = messages
        .iter()
        .find(|message| {
            message.role == "user" && message.body_text.contains("failing public prompt")
        })
        .and_then(|message| message.run_id.clone())
        .expect("failed prompt must retain its stored run correlation");
    assert_eq!(
        hub.store().run_status(&failed_run_id).unwrap(),
        Some(RunStatus::Failed)
    );
    assert_eq!(
        hub.store()
            .conversation("conv-capture-failure")
            .unwrap()
            .expect("conversation after failed prompt")
            .status,
        ConvStatus::Failed,
        "failed run must surface as conversation failed (not idle)"
    );

    let clean = hub
        .send_prompt(SendPromptParams {
            conv_id: "conv-capture-failure".into(),
            prompt: vec![ContentBlock::Text(TextContent::new("clean public prompt"))],
            params: Vec::new(),
            mode_id: None,
        })
        .await
        .expect("later clean prompt must succeed");
    assert_eq!(
        hub.store().run_status(&clean.run_id).unwrap(),
        Some(RunStatus::Completed)
    );
}
