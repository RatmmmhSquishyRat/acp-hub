use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_hub::HubError;
use acp_hub::acp::spawn_agent_connection;
use acp_hub::callbacks::HubCtx;
use acp_hub::store::Store;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CreateTerminalRequest, InitializeRequest, InitializeResponse,
    KillTerminalRequest, ReleaseTerminalRequest, SessionId, TerminalOutputRequest,
    WaitForTerminalExitRequest,
};
use agent_client_protocol::{Agent, Client, ConnectTo, DynConnectTo};

#[derive(Clone, Debug)]
struct SpawnedEndpointProbe {
    protocol_version: Option<ProtocolVersion>,
    started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<u32>>>>,
    exited: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

struct ChildGuard {
    child: Child,
    exited: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(exited) = self.exited.lock().expect("exit signal lock").take() {
            let _ = exited.send(());
        }
    }
}

impl ConnectTo<Client> for SpawnedEndpointProbe {
    async fn connect_to(
        self,
        client: impl ConnectTo<Agent>,
    ) -> Result<(), agent_client_protocol::Error> {
        let mut child = spawn_long_lived_child().map_err(|error| {
            agent_client_protocol::Error::new(
                -32603,
                format!("failed to spawn endpoint child probe: {error}"),
            )
        })?;
        if child
            .try_wait()
            .map_err(|error| {
                agent_client_protocol::Error::new(
                    -32603,
                    format!("failed to inspect endpoint child probe: {error}"),
                )
            })?
            .is_some()
        {
            return Err(agent_client_protocol::Error::new(
                -32603,
                "endpoint child probe exited before initialization",
            ));
        }
        if let Some(started) = self.started.lock().expect("start signal lock").take() {
            let _ = started.send(child.id());
        }
        let _child_guard = ChildGuard {
            child,
            exited: Arc::clone(&self.exited),
        };
        let protocol_version = self.protocol_version;

        Agent
            .builder()
            .name("spawned-endpoint-probe")
            .on_receive_request(
                async move |_request: InitializeRequest, responder, _cx| match protocol_version {
                    Some(protocol_version) => responder.respond(
                        InitializeResponse::new(protocol_version)
                            .agent_capabilities(AgentCapabilities::new()),
                    ),
                    None => std::future::pending().await,
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(client)
            .await
    }
}

fn endpoint_probe(
    protocol_version: Option<ProtocolVersion>,
) -> (
    DynConnectTo<Client>,
    tokio::sync::oneshot::Receiver<u32>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (exited_tx, exited_rx) = tokio::sync::oneshot::channel();
    let component = DynConnectTo::new(SpawnedEndpointProbe {
        protocol_version,
        started: Arc::new(Mutex::new(Some(started_tx))),
        exited: Arc::new(Mutex::new(Some(exited_tx))),
    });
    (component, started_rx, exited_rx)
}

#[cfg(windows)]
fn spawn_long_lived_child() -> std::io::Result<Child> {
    Command::new("ping.exe")
        .args(["-n", "60", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(not(windows))]
fn spawn_long_lived_child() -> std::io::Result<Child> {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(windows)]
fn terminal_command() -> (String, Vec<String>) {
    (
        "cmd.exe".to_string(),
        vec![
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            "echo session-a-terminal".to_string(),
        ],
    )
}

#[cfg(not(windows))]
fn terminal_command() -> (String, Vec<String>) {
    (
        "sh".to_string(),
        vec!["-c".to_string(), "printf session-a-terminal".to_string()],
    )
}

#[tokio::test]
async fn terminal_callbacks_reject_another_session_on_the_same_connection() {
    let temp = tempfile::tempdir().expect("temporary terminal root");
    let ctx = HubCtx::new(Store::open_memory().expect("in-memory store"));
    ctx.configure_agent(
        "shared-agent",
        "connection-1",
        acp_hub_integration_tests::test_agent_config(),
    )
    .unwrap();
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "conv-a",
        "shared-agent",
        "session-a",
        temp.path().to_path_buf(),
    )
    .expect("bind session A");
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "conv-b",
        "shared-agent",
        "session-b",
        temp.path().to_path_buf(),
    )
    .expect("bind session B");

    let (command, args) = terminal_command();
    let terminal_id = ctx
        .handle_terminal_create(
            "shared-agent",
            "connection-1",
            &CreateTerminalRequest::new(SessionId::new("session-a"), command)
                .args(args)
                .cwd(temp.path().to_path_buf()),
        )
        .expect("session A creates terminal")
        .terminal_id;

    let output_error = ctx
        .handle_terminal_output(
            "shared-agent",
            "connection-1",
            &TerminalOutputRequest::new(SessionId::new("session-b"), terminal_id.clone()),
        )
        .err();
    let wait_error = ctx
        .handle_terminal_wait(
            "shared-agent",
            "connection-1",
            &WaitForTerminalExitRequest::new(SessionId::new("session-b"), terminal_id.clone()),
        )
        .await
        .err();
    let kill_error = ctx
        .handle_terminal_kill(
            "shared-agent",
            "connection-1",
            &KillTerminalRequest::new(SessionId::new("session-b"), terminal_id.clone()),
        )
        .err();
    let release_error = ctx
        .handle_terminal_release(
            "shared-agent",
            "connection-1",
            &ReleaseTerminalRequest::new(SessionId::new("session-b"), terminal_id.clone()),
        )
        .err();

    for (operation, error) in [
        ("output", output_error),
        ("wait", wait_error),
        ("kill", kill_error),
        ("release", release_error),
    ] {
        let error = error
            .unwrap_or_else(|| panic!("session B unexpectedly completed terminal {operation}"));
        assert!(
            error.to_string().contains("another session"),
            "unexpected {operation} error: {error}"
        );
    }
    ctx.configure_agent(
        "shared-agent",
        "connection-2",
        acp_hub_integration_tests::test_agent_config(),
    )
    .unwrap();
    let stale_error = ctx
        .handle_terminal_output(
            "shared-agent",
            "connection-1",
            &TerminalOutputRequest::new(SessionId::new("session-a"), terminal_id.clone()),
        )
        .expect_err("the previous connection generation must not access the terminal");
    assert!(stale_error.to_string().contains("stale connection"));

    let replacement_error = ctx
        .handle_terminal_output(
            "shared-agent",
            "connection-2",
            &TerminalOutputRequest::new(SessionId::new("session-a"), terminal_id),
        )
        .expect_err("the replacement generation must not inherit the old terminal");
    assert!(replacement_error.to_string().contains("unknown terminal"));
    assert!(
        !ctx.is_session_bound("shared-agent", "session-a"),
        "replacement must revoke old-generation session bindings"
    );
}

#[tokio::test]
async fn dropping_the_last_handle_stops_the_connection_and_spawned_child() {
    let (component, started, exited) = endpoint_probe(Some(ProtocolVersion::V1));
    let ctx = HubCtx::new(Store::open_memory().expect("in-memory store"));
    let handle_rx = spawn_agent_connection(
        component,
        "lifecycle-probe".to_string(),
        acp_hub_integration_tests::test_agent_config(),
        ctx,
    );
    let handle = tokio::time::timeout(Duration::from_secs(5), handle_rx)
        .await
        .expect("initialize timeout")
        .expect("connection task ended before initialization")
        .expect("initialize response");
    let _child_id = tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("child start timeout")
        .expect("child start sender dropped");

    drop(handle);

    tokio::time::timeout(Duration::from_secs(2), exited)
        .await
        .expect("dropping the sole handle must stop the command loop and child")
        .expect("child exit sender dropped");
}

#[tokio::test]
async fn incompatible_initialize_response_rejects_the_handle_and_stops_the_connection() {
    let (component, started, exited) = endpoint_probe(Some(ProtocolVersion::V0));
    let ctx = HubCtx::new(Store::open_memory().expect("in-memory store"));
    let handle_rx = spawn_agent_connection(
        component,
        "incompatible-probe".to_string(),
        acp_hub_integration_tests::test_agent_config(),
        ctx,
    );
    let result = tokio::time::timeout(Duration::from_secs(5), handle_rx)
        .await
        .expect("initialize timeout")
        .expect("connection task ended without reporting initialization result");
    assert!(
        matches!(result, Err(HubError::UnsupportedProtocolVersion)),
        "an incompatible response must not produce a usable command handle"
    );
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("child start timeout")
        .expect("child start sender dropped");
    tokio::time::timeout(Duration::from_secs(2), exited)
        .await
        .expect("incompatible initialization must stop the connection and child")
        .expect("child exit sender dropped");
}

#[tokio::test]
async fn abandoned_initialize_receiver_cancels_connection_and_revokes_callbacks() {
    let (component, started, exited) = endpoint_probe(None);
    let temp = tempfile::tempdir().expect("temporary session root");
    let ctx = HubCtx::new(Store::open_memory().expect("in-memory store"));
    acp_hub_integration_tests::bind_test_session(
        &ctx,
        "abandoned-conversation",
        "abandoned-probe",
        "abandoned-session",
        temp.path().to_path_buf(),
    )
    .expect("bind callback state before connection setup");
    let mut handle_rx = spawn_agent_connection(
        component,
        "abandoned-probe".to_string(),
        acp_hub_integration_tests::test_agent_config(),
        Arc::clone(&ctx),
    );
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("child start timeout")
        .expect("child start sender dropped");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut handle_rx)
            .await
            .is_err(),
        "the controllable endpoint must leave initialize unanswered"
    );

    drop(handle_rx);

    tokio::time::timeout(Duration::from_secs(2), exited)
        .await
        .expect("abandoning initialization must stop the connection and child")
        .expect("child exit sender dropped");
    let cleanup_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while ctx.is_session_bound("abandoned-probe", "abandoned-session") {
        assert!(
            std::time::Instant::now() < cleanup_deadline,
            "connection cleanup did not revoke callback generation state"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
