//! P1 — SDK proof: prove the official rust-sdk connection/session/prompt loop
//! works in-process against the deterministic `Testy` agent using the
//! `ActiveSession::read_update` API (never `read_to_string`, which drops
//! non-text updates — see plan P1/P4).
//!
//! This test pins the exact API surface the rest of the crate builds on.

use std::path::PathBuf;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{InitializeRequest, StopReason};
use agent_client_protocol::{Client, SessionMessage};
use agent_client_protocol_test::testy::{Testy, TestyCommand};

#[tokio::test]
async fn sdk_probe_echo_roundtrip_via_read_update() {
    // `Testy` implements `ConnectTo<Client>`, so it is the transport for
    // `Client.builder().connect_with`. A single in-process connection runs the
    // whole initialize → session → prompt → read_update loop.
    Client
        .builder()
        .connect_with(Testy::new(), async |cx| {
            // Initialize — negotiate ACP v1.
            let init = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            // Testy advertises load_session and full prompt capabilities.
            assert!(init.agent_capabilities.load_session);
            assert!(init.agent_capabilities.prompt_capabilities.image);

            // Build a session and drive it synchronously, draining every
            // update through `read_update` until the terminal stop reason.
            cx.build_session(PathBuf::from("/tmp"))
                .block_task()
                .run_until(async |mut session| {
                    session.send_prompt(
                        TestyCommand::Echo {
                            message: "hub-probe".to_string(),
                        }
                        .to_prompt(),
                    )?;

                    let mut updates = 0u32;
                    loop {
                        match session.read_update().await? {
                            SessionMessage::StopReason(reason) => {
                                assert_eq!(reason, StopReason::EndTurn);
                                break;
                            }
                            SessionMessage::SessionMessage(_) => updates += 1,
                            _ => updates += 1,
                        }
                    }

                    assert!(updates > 0, "expected at least one session update");
                    Ok(())
                })
                .await
        })
        .await
        .expect("sdk probe roundtrip should succeed");
}
