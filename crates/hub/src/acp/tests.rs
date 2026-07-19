use super::*;
use crate::endpoint::FsConfig;
use agent_client_protocol::schema::v1::{
    AudioContent, EmbeddedResource, EmbeddedResourceResource, ImageContent, PromptCapabilities,
    SessionId, SessionInfo, TextResourceContents,
};
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
fn prompt_content_requires_each_advertised_capability() {
    let caps = AgentCapabilities::new().prompt_capabilities(PromptCapabilities::new());
    let cases = [
        (
            ContentBlock::Image(ImageContent::new("", "image/png")),
            "prompt_capabilities.image",
        ),
        (
            ContentBlock::Audio(AudioContent::new("", "audio/wav")),
            "prompt_capabilities.audio",
        ),
        (
            ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                    "",
                    "file:///context.txt",
                )),
            )),
            "prompt_capabilities.embedded_context",
        ),
    ];

    for (block, required_capability) in cases {
        let error = validate_prompt_capabilities("fixture-agent", &caps, &[block]).unwrap_err();
        assert!(matches!(
            error,
            HubError::UnsupportedCapability {
                endpoint,
                operation: "session/prompt",
                required_capability: actual,
            } if endpoint == "fixture-agent" && actual == required_capability
        ));
    }
}

#[test]
fn prompt_content_accepts_baseline_and_advertised_capabilities() {
    let caps = AgentCapabilities::new().prompt_capabilities(
        PromptCapabilities::new()
            .image(true)
            .audio(true)
            .embedded_context(true),
    );
    let prompt = serde_json::from_value::<Vec<ContentBlock>>(serde_json::json!([
        {"type": "text", "text": "hello"},
        {"type": "resource_link", "uri": "file:///context.txt", "name": "context"},
        {"type": "image", "data": "", "mimeType": "image/png"},
        {"type": "audio", "data": "", "mimeType": "audio/wav"},
        {
            "type": "resource",
            "resource": {"uri": "file:///context.txt", "text": "context"}
        }
    ]))
    .unwrap();

    validate_prompt_capabilities("fixture-agent", &caps, &prompt).unwrap();
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

#[tokio::test]
async fn session_page_collector_deduplicates_first_occurrence() {
    let pages = Arc::new(Mutex::new(VecDeque::from([
        (
            vec![SessionInfo::new(
                SessionId::new("session-a"),
                PathBuf::from("/first"),
            )],
            Some("next".to_string()),
        ),
        (
            vec![SessionInfo::new(
                SessionId::new("session-a"),
                PathBuf::from("/duplicate"),
            )],
            None,
        ),
    ])));
    let mut fetch = {
        let pages = Arc::clone(&pages);
        move |_| {
            let page = pages.lock().pop_front().expect("requested expected page");
            std::future::ready(Ok(page))
        }
    };

    let result = collect_session_pages_with_limits(
        "dedup-agent",
        &mut fetch,
        SessionListLimits {
            pages: 2,
            sessions: 2,
            cursor_bytes: 16,
            serialized_bytes: 4096,
        },
    )
    .await
    .unwrap();

    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].cwd, PathBuf::from("/first"));
}

#[tokio::test]
async fn session_page_collector_charges_duplicates_before_deduplication() {
    let mut fetch = |_| {
        std::future::ready(Ok((
            vec![
                SessionInfo::new(SessionId::new("same"), PathBuf::from("/first")),
                SessionInfo::new(SessionId::new("same"), PathBuf::from("/duplicate")),
            ],
            None,
        )))
    };

    let error = collect_session_pages_with_limits(
        "bounded-agent",
        &mut fetch,
        SessionListLimits {
            pages: 1,
            sessions: 1,
            cursor_bytes: 16,
            serialized_bytes: 4096,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        HubError::ResourceLimit {
            resource: "session_list_sessions",
            limit: 1,
        }
    ));
}

#[tokio::test]
async fn session_page_collector_enforces_page_cursor_and_serialized_budgets() {
    let mut page_fetch = |_| std::future::ready(Ok((Vec::new(), Some("another-page".to_string()))));
    let page_error = collect_session_pages_with_limits(
        "page-agent",
        &mut page_fetch,
        SessionListLimits {
            pages: 1,
            sessions: 1,
            cursor_bytes: 32,
            serialized_bytes: 4096,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        page_error,
        HubError::ResourceLimit {
            resource: "session_list_pages",
            limit: 1,
        }
    ));

    let mut cursor_fetch =
        |_| std::future::ready(Ok((Vec::new(), Some("cursor-too-long".to_string()))));
    let cursor_error = collect_session_pages_with_limits(
        "cursor-agent",
        &mut cursor_fetch,
        SessionListLimits {
            pages: 2,
            sessions: 1,
            cursor_bytes: 4,
            serialized_bytes: 4096,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        cursor_error,
        HubError::ResourceLimit {
            resource: "session_list_cursor_bytes",
            limit: 4,
        }
    ));

    let mut serialized_fetch = |_| {
        std::future::ready(Ok((
            vec![SessionInfo::new(
                SessionId::new("serialized"),
                PathBuf::from("/workspace"),
            )],
            None,
        )))
    };
    let serialized_error = collect_session_pages_with_limits(
        "serialized-agent",
        &mut serialized_fetch,
        SessionListLimits {
            pages: 1,
            sessions: 1,
            cursor_bytes: 4,
            serialized_bytes: 1,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        serialized_error,
        HubError::ResourceLimit {
            resource: "session_list_serialized_bytes",
            limit: 1,
        }
    ));
}
