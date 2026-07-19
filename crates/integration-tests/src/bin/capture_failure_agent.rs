use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionId,
    SessionNotification, SessionResumeCapabilities, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectTo, Stdio};

const AGENT_ID: &str = "capture-failure-probe";
const OVERSIZED_UPDATE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default)]
struct CaptureFailureAgent {
    load_attempts: Arc<AtomicUsize>,
    resume_attempts: Arc<AtomicUsize>,
    prompt_attempts: Arc<AtomicUsize>,
}

impl ConnectTo<Client> for CaptureFailureAgent {
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
                    send_probe_updates(&cx, &request.session_id, "load", attempt)?;
                    wait_for_callback().await;
                    responder.respond(LoadSessionResponse::new())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: ResumeSessionRequest, responder, cx| {
                    let attempt = resume_attempts.fetch_add(1, Ordering::SeqCst);
                    send_probe_updates(&cx, &request.session_id, "resume", attempt)?;
                    wait_for_callback().await;
                    responder.respond(ResumeSessionResponse::new())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, cx| {
                    let attempt = prompt_attempts.fetch_add(1, Ordering::SeqCst);
                    send_probe_updates(&cx, &request.session_id, "prompt", attempt)?;
                    wait_for_callback().await;
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(client)
            .await
    }
}

fn send_probe_updates(
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
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::main]
async fn main() {
    if let Err(error) = CaptureFailureAgent::default()
        .connect_to(Stdio::new())
        .await
    {
        eprintln!("capture failure ACP agent stopped: {error}");
        std::process::exit(1);
    }
}
