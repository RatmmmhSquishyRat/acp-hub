//! P5 store runtime validation: FTS5 search hit + non-destructive load replay.

use acp_hub::store::{MessageSource, NewConversation, NewMessage, Store};
use serde_json::json;

fn conv(store: &Store, id: &str, agent: &str) -> String {
    store
        .create_conversation(&NewConversation {
            id: id.to_string(),
            agent_id: agent.to_string(),
            agent_session_id: format!("{agent}-session"),
            cwd: Some("/tmp".into()),
            additional_directories: vec![],
            title: None,
        })
        .unwrap();
    id.to_string()
}

fn msg(store: &Store, conv: &str, role: &str, body: &str, n: usize) {
    store
        .append_message(&NewMessage {
            id: format!("{conv}-m{n}"),
            conv_id: conv.to_string(),
            run_id: None,
            source: MessageSource::LocalTurn,
            role: role.into(),
            kind: None,
            content_json: json!({ "text": body }),
            body_text: body.into(),
        })
        .unwrap();
}

#[test]
fn search_finds_appended_message() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "c1", "agent-a");
    msg(
        &store,
        &c,
        "assistant",
        "the hub-search-token lives here",
        1,
    );

    let page = store.search("hub-search-token", None, None, 10, 0).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].conv_id, "c1");
    assert_eq!(page.items[0].agent_id, "agent-a");
}

#[test]
fn load_replay_supersedes_but_keeps_audit_searchable() {
    let store = Store::open_memory().unwrap();
    let c = conv(&store, "c2", "agent-b");
    msg(&store, &c, "user", "original-captured-message", 1);
    msg(&store, &c, "assistant", "original-reply", 2);

    // Current projection before load = 2 messages.
    let cur = store.messages(&c, false).unwrap();
    assert_eq!(cur.len(), 2);

    // session/load replays 1 message.
    store
        .stage_load_replay(
            &c,
            "load-1",
            &[acp_hub::store::ReplayedMessage {
                id: "c2-l1".into(),
                role: "agent".into(),
                kind: None,
                content_json: json!({ "text": "replayed-reply" }),
                body_text: "replayed-reply".into(),
                message_key: Some("m-key".into()),
            }],
        )
        .unwrap();

    // Current projection now = 1 (the replayed row); audit still searchable.
    let cur = store.messages(&c, false).unwrap();
    assert_eq!(cur.len(), 1, "load replay should be the only current row");
    assert_eq!(cur[0].source, MessageSource::LoadReplay);

    // Audit search still finds the original captured message.
    let page = store
        .search("original-captured", None, None, 10, 0)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].source.as_deref().unwrap().ends_with(":audit"));
}
