//! UX-CORE wait — Store-poll attach until run terminal (shared CLI / MCP / tests).
//!
//! CLI uses [`wait_run_via_client_with_emit`] so **each poll** can print new view
//! lines while the run is still in-flight (G3 / V3 incremental attach).
//! MCP may batch via [`wait_run_via_client`] and return messages with the final.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::state::CoreHub;
use super::types::{MessagesPageParams, WaitRunParams, WaitRunResult};
use crate::error::HubError;
use crate::store::{
    MergeLimits, MessagePageQuery, MessageRow, ViewMessage, merge_transcript_with,
};

impl CoreHub {
    /// Poll Store until terminal; no mid-poll callback (tests / MCP-style batch).
    pub async fn wait_run(&self, params: WaitRunParams) -> Result<WaitRunResult, HubError> {
        self.wait_run_with_emit(params, |_| {}).await
    }

    /// Poll Store until terminal, invoking `on_new` for **each poll's new view lines**
    /// while the run is still open (and once more for late post-finalize rows).
    pub async fn wait_run_with_emit<F>(
        &self,
        params: WaitRunParams,
        mut on_new: F,
    ) -> Result<WaitRunResult, HubError>
    where
        F: FnMut(&[ViewMessage]),
    {
        self.ensure_conversation(&params.conv_id)?;
        let started = Instant::now();
        let mut after_seq = params.since_seq.unwrap_or(0);
        let mut info = self
            .store()
            .resolve_wait_run(&params.conv_id, params.run_id.as_deref())?;
        let run_id = info.run_id.clone();
        let mut seen_view: HashSet<i64> = HashSet::new();
        let mut all_emitted: Vec<ViewMessage> = Vec::new();

        if info.is_terminal() {
            let messages =
                self.page_and_emit_views(&params.conv_id, &run_id, &mut after_seq, &mut seen_view, &mut on_new)?;
            return Ok(WaitRunResult {
                conv_id: params.conv_id,
                run: info,
                messages,
            });
        }

        loop {
            if let Some(limit_secs) = params.timeout_secs
                && started.elapsed() >= Duration::from_secs(limit_secs)
            {
                return Err(HubError::wait_timeout(&params.conv_id, limit_secs));
            }

            // Incremental: emit merge of **this poll's new rows only**.
            let delta = self.page_run_rows(&params.conv_id, &run_id, after_seq)?;
            if !delta.is_empty() {
                for row in &delta {
                    after_seq = after_seq.max(row.seq);
                }
                let view = merge_transcript_with(&delta, MergeLimits::send_run());
                let fresh = take_unseen(&view.items, &mut seen_view);
                if !fresh.is_empty() {
                    on_new(&fresh);
                    all_emitted.extend(fresh);
                }
            }

            info = match self.store().get_run(&run_id)? {
                Some(row) if row.conv_id == params.conv_id => row,
                Some(_) | None => return Err(HubError::run_not_found(&run_id)),
            };

            if info.is_terminal() {
                // Late capture after finalize — still emit as they appear.
                let late = self.page_run_rows(&params.conv_id, &run_id, after_seq)?;
                if !late.is_empty() {
                    for row in &late {
                        after_seq = after_seq.max(row.seq);
                    }
                    let view = merge_transcript_with(&late, MergeLimits::send_run());
                    let fresh = take_unseen(&view.items, &mut seen_view);
                    if !fresh.is_empty() {
                        on_new(&fresh);
                        all_emitted.extend(fresh);
                    }
                }
                return Ok(WaitRunResult {
                    conv_id: params.conv_id,
                    run: info,
                    messages: all_emitted,
                });
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn page_run_rows(
        &self,
        conv_id: &str,
        run_id: &str,
        after_seq: i64,
    ) -> Result<Vec<MessageRow>, HubError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seq = after_seq;
        loop {
            let page = self.store().messages_page_query(MessagePageQuery {
                conv_id,
                include_audit: false,
                run_id: Some(run_id),
                after_seq: Some(seq),
                cursor: cursor.as_deref(),
                limit: 200,
                offset: 0,
            })?;
            for row in page.items {
                seq = seq.max(row.seq);
                out.push(row);
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    fn page_and_emit_views<F>(
        &self,
        conv_id: &str,
        run_id: &str,
        after_seq: &mut i64,
        seen: &mut HashSet<i64>,
        on_new: &mut F,
    ) -> Result<Vec<ViewMessage>, HubError>
    where
        F: FnMut(&[ViewMessage]),
    {
        let rows = self.page_run_rows(conv_id, run_id, *after_seq)?;
        for row in &rows {
            *after_seq = (*after_seq).max(row.seq);
        }
        let view = merge_transcript_with(&rows, MergeLimits::send_run());
        let fresh = take_unseen(&view.items, seen);
        if !fresh.is_empty() {
            on_new(&fresh);
        }
        Ok(fresh)
    }
}

/// Client-side wait (no mid-poll callback) — MCP batch path.
pub async fn wait_run_via_client(
    client: &super::client::HubClient,
    params: WaitRunParams,
) -> Result<WaitRunResult, HubError> {
    wait_run_via_client_with_emit(client, params, |_| {}).await
}

/// Client-side wait with **per-poll** `on_new` for incremental CLI attach (V3).
pub async fn wait_run_via_client_with_emit<F>(
    client: &super::client::HubClient,
    params: WaitRunParams,
    mut on_new: F,
) -> Result<WaitRunResult, HubError>
where
    F: FnMut(&[ViewMessage]),
{
    let started = Instant::now();
    let mut after_seq = params.since_seq.unwrap_or(0);
    let mut info = client
        .get_run(params.conv_id.clone(), params.run_id.clone())
        .await?;
    let run_id = info.run_id.clone();
    let mut seen_view: HashSet<i64> = HashSet::new();
    let mut all_emitted: Vec<ViewMessage> = Vec::new();

    if info.is_terminal() {
        let rows = page_all_run_messages(client, &params.conv_id, &run_id, after_seq).await?;
        for row in &rows {
            after_seq = after_seq.max(row.seq);
        }
        let view = merge_transcript_with(&rows, MergeLimits::send_run());
        let fresh = take_unseen(&view.items, &mut seen_view);
        if !fresh.is_empty() {
            on_new(&fresh);
            all_emitted.extend(fresh);
        }
        return Ok(WaitRunResult {
            conv_id: params.conv_id,
            run: info,
            messages: all_emitted,
        });
    }

    loop {
        if let Some(limit_secs) = params.timeout_secs
            && started.elapsed() >= Duration::from_secs(limit_secs)
        {
            return Err(HubError::wait_timeout(&params.conv_id, limit_secs));
        }

        let delta = page_all_run_messages(client, &params.conv_id, &run_id, after_seq).await?;
        if !delta.is_empty() {
            for row in &delta {
                after_seq = after_seq.max(row.seq);
            }
            let view = merge_transcript_with(&delta, MergeLimits::send_run());
            let fresh = take_unseen(&view.items, &mut seen_view);
            if !fresh.is_empty() {
                on_new(&fresh);
                all_emitted.extend(fresh);
            }
        }

        info = match client
            .get_run(params.conv_id.clone(), Some(run_id.clone()))
            .await
        {
            Ok(row) => row,
            Err(HubError::RunNotFound { .. }) => {
                return Err(HubError::run_not_found(run_id));
            }
            Err(other) => return Err(other),
        };

        if info.is_terminal() {
            let late = page_all_run_messages(client, &params.conv_id, &run_id, after_seq).await?;
            if !late.is_empty() {
                for row in &late {
                    after_seq = after_seq.max(row.seq);
                }
                let view = merge_transcript_with(&late, MergeLimits::send_run());
                let fresh = take_unseen(&view.items, &mut seen_view);
                if !fresh.is_empty() {
                    on_new(&fresh);
                    all_emitted.extend(fresh);
                }
            }
            return Ok(WaitRunResult {
                conv_id: params.conv_id,
                run: info,
                messages: all_emitted,
            });
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn take_unseen(items: &[ViewMessage], seen: &mut HashSet<i64>) -> Vec<ViewMessage> {
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.seq) {
            out.push(item.clone());
        }
    }
    out
}

async fn page_all_run_messages(
    client: &super::client::HubClient,
    conv_id: &str,
    run_id: &str,
    after_seq: i64,
) -> Result<Vec<MessageRow>, HubError> {
    use serde_json::Value;

    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seq = after_seq;
    loop {
        let page = client
            .messages_page(MessagesPageParams {
                conv_id: conv_id.to_string(),
                include_audit: false,
                run_id: Some(run_id.to_string()),
                after_seq: Some(seq),
                cursor: cursor.clone(),
                limit: 200,
                offset: 0,
            })
            .await?;
        if let Some(items) = page.get("items").and_then(Value::as_array) {
            for item in items {
                if let Ok(row) = serde_json::from_value::<MessageRow>(item.clone()) {
                    seq = seq.max(row.seq);
                    out.push(row);
                } else if let Some(s) = item.get("seq").and_then(Value::as_i64) {
                    seq = seq.max(s);
                }
            }
        }
        cursor = page
            .get("nextCursor")
            .or_else(|| page.get("next_cursor"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::ActivityTracker;
    use crate::endpoint::Registry;
    use crate::store::{MessageSource, NewConversation, NewMessage, RunStatus, Store};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    fn open_conv(store: &Store, id: &str) {
        store
            .create_conversation(&NewConversation {
                id: id.into(),
                agent_id: "a".into(),
                agent_session_id: "s".into(),
                cwd: Some("/tmp".into()),
                additional_directories: vec![],
                title: None,
            })
            .unwrap();
    }

    fn hub_with(store: Store) -> CoreHub {
        let home = tempdir().unwrap();
        CoreHub::new(
            home.path(),
            Registry {
                agents: Default::default(),
                proxies: Default::default(),
            },
            store,
            Arc::new(ActivityTracker::new()),
        )
    }

    #[tokio::test]
    async fn wait_run_sees_cancel_finalize_as_terminal() {
        let store = Store::open_memory().unwrap();
        open_conv(&store, "c-wait-cancel");
        store.create_run("run-cancel", "c-wait-cancel").unwrap();
        let hub = Arc::new(hub_with(store));

        let wait_hub = Arc::clone(&hub);
        let wait_task = tokio::spawn(async move {
            wait_hub
                .wait_run(WaitRunParams {
                    conv_id: "c-wait-cancel".into(),
                    run_id: Some("run-cancel".into()),
                    since_seq: None,
                    timeout_secs: Some(5),
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            hub.store()
                .finalize_run_cas(
                    "run-cancel",
                    "c-wait-cancel",
                    RunStatus::Cancelled,
                    Some("cancelled"),
                )
                .unwrap()
        );

        let result = wait_task.await.unwrap().expect("wait must finish");
        assert_eq!(result.run.status, "cancelled");
        assert_eq!(result.run.stop_reason.as_deref(), Some("cancelled"));
        assert!(result.run.is_terminal());
    }

    #[tokio::test]
    async fn wait_run_unknown_id_fails_immediately() {
        let store = Store::open_memory().unwrap();
        open_conv(&store, "c-missing");
        let hub = hub_with(store);
        let err = hub
            .wait_run(WaitRunParams {
                conv_id: "c-missing".into(),
                run_id: Some("run-nope".into()),
                since_seq: None,
                timeout_secs: Some(30),
            })
            .await
            .unwrap_err();
        assert_eq!(err.phase1_code(), Some("run_not_found"));
    }

    /// V3 / G3: on_new fires for messages while run is still running — not only at terminal.
    #[tokio::test]
    async fn wait_run_emits_views_before_terminal() {
        let store = Store::open_memory().unwrap();
        open_conv(&store, "c-incr");
        store.create_run("run-incr", "c-incr").unwrap();
        let hub = Arc::new(hub_with(store));

        let mid_emits = Arc::new(AtomicUsize::new(0));
        let mid_emits2 = Arc::clone(&mid_emits);
        let wait_hub = Arc::clone(&hub);
        let wait_task = tokio::spawn(async move {
            wait_hub
                .wait_run_with_emit(
                    WaitRunParams {
                        conv_id: "c-incr".into(),
                        run_id: Some("run-incr".into()),
                        since_seq: None,
                        timeout_secs: Some(5),
                    },
                    move |views| {
                        if !views.is_empty() {
                            mid_emits2.fetch_add(1, Ordering::SeqCst);
                        }
                    },
                )
                .await
        });

        // While still running, append an assistant message that wait must stream.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            hub.store().run_status("run-incr").unwrap(),
            Some(RunStatus::Running)
        );
        hub.store()
            .append_message(&NewMessage {
                id: "m-mid".into(),
                conv_id: "c-incr".into(),
                run_id: Some("run-incr".into()),
                source: MessageSource::LocalTurn,
                role: "assistant".into(),
                kind: Some("message".into()),
                content_json: serde_json::json!({"text": "mid-stream body"}),
                body_text: "mid-stream body".into(),
            })
            .unwrap();

        // Give poll loop time to see the message while still running.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let emits_while_running = mid_emits.load(Ordering::SeqCst);
        assert!(
            emits_while_running >= 1,
            "must emit at least once before terminal (got {emits_while_running})"
        );

        assert!(
            hub.store()
                .finalize_run_cas(
                    "run-incr",
                    "c-incr",
                    RunStatus::Completed,
                    Some("end_turn"),
                )
                .unwrap()
        );

        let result = wait_task.await.unwrap().expect("wait completes");
        assert_eq!(result.run.status, "completed");
        assert!(
            result
                .messages
                .iter()
                .any(|m| m.body_text.contains("mid-stream body")),
            "result must include mid-stream body: {:?}",
            result.messages
        );
    }
}
