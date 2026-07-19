use std::future::Future;
use std::io;

use serde::Serialize;

use super::*;

pub(super) async fn collect_session_pages<F, Fut>(
    agent_id: &str,
    mut fetch: F,
) -> Result<ListSessionsResult, HubError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<SessionInfo>, Option<String>), HubError>>,
{
    collect_session_pages_with_limits(agent_id, &mut fetch, SessionListLimits::DEFAULT).await
}

const MAX_SESSION_LIST_PAGES: usize = 256;
const MAX_SESSION_LIST_RECEIVED_SESSIONS: usize = 20_000;
const MAX_SESSION_LIST_CURSOR_BYTES: usize = 8 * 1024;
const MAX_SESSION_LIST_SERIALIZED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct SessionListLimits {
    pub(super) pages: usize,
    pub(super) sessions: usize,
    pub(super) cursor_bytes: usize,
    pub(super) serialized_bytes: usize,
}

impl SessionListLimits {
    const DEFAULT: Self = Self {
        pages: MAX_SESSION_LIST_PAGES,
        sessions: MAX_SESSION_LIST_RECEIVED_SESSIONS,
        cursor_bytes: MAX_SESSION_LIST_CURSOR_BYTES,
        serialized_bytes: MAX_SESSION_LIST_SERIALIZED_BYTES,
    };
}

struct CanonicalByteCounter {
    bytes: usize,
    limit: usize,
    exceeded: bool,
}

impl io::Write for CanonicalByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self.bytes.saturating_add(bytes.len());
        if next > self.limit {
            self.exceeded = true;
            return Err(io::Error::other(
                "session/list serialized byte limit exceeded",
            ));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn charge_session_page<T: Serialize>(
    value: &T,
    serialized_bytes: &mut usize,
    limit: usize,
) -> Result<(), HubError> {
    let mut counter = CanonicalByteCounter {
        bytes: *serialized_bytes,
        limit,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer(&mut counter, value) {
        if counter.exceeded {
            return Err(HubError::ResourceLimit {
                resource: "session_list_serialized_bytes",
                limit,
            });
        }
        return Err(error.into());
    }
    *serialized_bytes = counter.bytes;
    Ok(())
}

pub(super) async fn collect_session_pages_with_limits<F, Fut>(
    agent_id: &str,
    fetch: &mut F,
    limits: SessionListLimits,
) -> Result<ListSessionsResult, HubError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<SessionInfo>, Option<String>), HubError>>,
{
    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut seen_sessions = std::collections::HashSet::new();
    let mut received_sessions = 0usize;
    let mut serialized_bytes = 0usize;

    for page_index in 0..limits.pages {
        let (page, next_cursor) = fetch(cursor.clone()).await?;
        charge_session_page(
            &(&page, &next_cursor),
            &mut serialized_bytes,
            limits.serialized_bytes,
        )?;
        received_sessions = received_sessions.saturating_add(page.len());
        if received_sessions > limits.sessions {
            return Err(HubError::ResourceLimit {
                resource: "session_list_sessions",
                limit: limits.sessions,
            });
        }
        for session in page {
            if seen_sessions.insert(session.session_id.to_string()) {
                sessions.push(session);
            }
        }
        let Some(next) = next_cursor else {
            return Ok(ListSessionsResult { sessions });
        };
        if next.len() > limits.cursor_bytes {
            return Err(HubError::ResourceLimit {
                resource: "session_list_cursor_bytes",
                limit: limits.cursor_bytes,
            });
        }
        if !seen_cursors.insert(next.clone()) {
            return Err(HubError::other(format!(
                "agent {agent_id:?} repeated session/list cursor {next:?}"
            )));
        }
        if page_index + 1 == limits.pages {
            return Err(HubError::ResourceLimit {
                resource: "session_list_pages",
                limit: limits.pages,
            });
        }
        cursor = Some(next);
    }

    Err(HubError::ResourceLimit {
        resource: "session_list_pages",
        limit: limits.pages,
    })
}
