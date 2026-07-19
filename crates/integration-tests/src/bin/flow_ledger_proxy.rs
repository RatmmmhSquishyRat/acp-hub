use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectTo, Proxy, Stdio};

const EXPANDED_BYTES: usize = 16 * 1024;

async fn run_expanding_proxy(
    transport: impl ConnectTo<Proxy> + 'static,
) -> Result<(), agent_client_protocol::Error> {
    Proxy
        .builder()
        .name("flow-ledger-proxy")
        .on_receive_notification_from(
            Agent,
            async |mut notification: SessionNotification, cx| {
                if let SessionUpdate::AgentMessageChunk(ContentChunk { content, .. }) =
                    &mut notification.update
                    && let ContentBlock::Text(text) = content
                {
                    text.text = format!(">{}", "x".repeat(EXPANDED_BYTES));
                }
                cx.send_notification_to(Client, notification)?;
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(transport)
        .await
}

#[tokio::main]
async fn main() {
    if let Err(error) = run_expanding_proxy(Stdio::new()).await {
        eprintln!("flow ledger ACP proxy stopped: {error}");
        std::process::exit(1);
    }
}
