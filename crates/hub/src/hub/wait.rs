//! UX-CORE wait — Store-poll attach until run terminal (shared CLI / MCP / tests).

use std::time::{Duration, Instant};

use super::state::CoreHub;
use super::types::{MessagesPageParams, WaitRunParams, WaitRunResult};
use crate::error::HubError;
use crate::store::{MergeLimits, MessagePageQuery, merge_transcript_with};

impl CoreHub {
    /// Poll Store until the target run is terminal (UX-CORE §6.2).
    ///
    /// Does **not** send a new prompt. Missing / wrong run → `run_not_found` or
    /// `not_busy` immediately (no hang). Terminal includes `failed` (exit-success
    /// semantics for callers).
    pub async fn wait_run(&self, params: WaitRunParams) -> Result<WaitRunResult, HubError> {
        self.ensure_conversation(&params.conv_id)?;
        let started = Instant::now();
        let mut after_seq = params.since_seq.unwrap_or(0);
        let mut info = self
            .store()
            .resolve_wait_run(&params.conv_id, params.run_id.as_deref())?;
        let run_id = info.run_id.clone();

        // Already terminal: short path.
        if info.is_terminal() {
            let messages = self.collect_run_view(&params.conv_id, &run_id, after_seq)?;
            return Ok(WaitRunResult {
                conv_id: params.conv_id,
                run: info,
                messages,
            });
        }

        let mut all_rows = Vec::new();
        loop {
            if let Some(limit_secs) = params.timeout_secs
                && started.elapsed() >= Duration::from_secs(limit_secs)
            {
                return Err(HubError::wait_timeout(&params.conv_id, limit_secs));
            }

            // Page new messages for this run.
            let mut cursor: Option<String> = None;
            loop {
                let page = self.store().messages_page_query(MessagePageQuery {
                    conv_id: &params.conv_id,
                    include_audit: false,
                    run_id: Some(&run_id),
                    after_seq: Some(after_seq),
                    cursor: cursor.as_deref(),
                    limit: 200,
                    offset: 0,
                })?;
                for row in &page.items {
                    after_seq = after_seq.max(row.seq);
                    all_rows.push(row.clone());
                }
                cursor = page.next_cursor.clone();
                if cursor.is_none() {
                    break;
                }
            }

            info = match self.store().get_run(&run_id)? {
                Some(row) if row.conv_id == params.conv_id => row,
                Some(_) | None => return Err(HubError::run_not_found(&run_id)),
            };

            if info.is_terminal() {
                // One more page to catch late capture after finalize.
                let mut cursor: Option<String> = None;
                loop {
                    let page = self.store().messages_page_query(MessagePageQuery {
                        conv_id: &params.conv_id,
                        include_audit: false,
                        run_id: Some(&run_id),
                        after_seq: Some(after_seq),
                        cursor: cursor.as_deref(),
                        limit: 200,
                        offset: 0,
                    })?;
                    for row in &page.items {
                        after_seq = after_seq.max(row.seq);
                        all_rows.push(row.clone());
                    }
                    cursor = page.next_cursor.clone();
                    if cursor.is_none() {
                        break;
                    }
                }
                let view = merge_transcript_with(&all_rows, MergeLimits::send_run());
                return Ok(WaitRunResult {
                    conv_id: params.conv_id,
                    run: info,
                    messages: view.items,
                });
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn collect_run_view(
        &self,
        conv_id: &str,
        run_id: &str,
        after_seq: i64,
    ) -> Result<Vec<crate::store::ViewMessage>, HubError> {
        let mut all_rows = Vec::new();
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
            for row in &page.items {
                seq = seq.max(row.seq);
                all_rows.push(row.clone());
            }
            cursor = page.next_cursor.clone();
            if cursor.is_none() {
                break;
            }
        }
        Ok(merge_transcript_with(&all_rows, MergeLimits::send_run()).items)
    }
}

/// Client-side wait using `hub/conv/run` + `hub/conv/messages_page` (CLI / MCP).
pub async fn wait_run_via_client(
    client: &super::client::HubClient,
    params: WaitRunParams,
) -> Result<WaitRunResult, HubError> {
    use crate::store::{MergeLimits, MessageRow, merge_transcript_with};

    let started = Instant::now();
    let mut after_seq = params.since_seq.unwrap_or(0);
    let mut info = client
        .get_run(params.conv_id.clone(), params.run_id.clone())
        .await?;
    let run_id = info.run_id.clone();
    let mut collected: Vec<MessageRow> = Vec::new();

    if info.is_terminal() {
        let rows = page_all_run_messages(client, &params.conv_id, &run_id, after_seq).await?;
        let view = merge_transcript_with(&rows, MergeLimits::send_run());
        return Ok(WaitRunResult {
            conv_id: params.conv_id,
            run: info,
            messages: view.items,
        });
    }

    loop {
        if let Some(limit_secs) = params.timeout_secs
            && started.elapsed() >= Duration::from_secs(limit_secs)
        {
            return Err(HubError::wait_timeout(&params.conv_id, limit_secs));
        }

        let rows = page_all_run_messages(client, &params.conv_id, &run_id, after_seq).await?;
        for row in &rows {
            after_seq = after_seq.max(row.seq);
            collected.push(row.clone());
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
            for row in late {
                after_seq = after_seq.max(row.seq);
                collected.push(row);
            }
            let view = merge_transcript_with(&collected, MergeLimits::send_run());
            return Ok(WaitRunResult {
                conv_id: params.conv_id,
                run: info,
                messages: view.items,
            });
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn page_all_run_messages(
    client: &super::client::HubClient,
    conv_id: &str,
    run_id: &str,
    after_seq: i64,
) -> Result<Vec<crate::store::MessageRow>, HubError> {
    use crate::store::MessageRow;
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
    use crate::store::{NewConversation, RunStatus, Store};

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

    #[tokio::test]
    async fn wait_run_sees_cancel_finalize_as_terminal() {
        use crate::daemon::ActivityTracker;
        use crate::endpoint::Registry;
        use std::sync::Arc;
        use tempfile::tempdir;

        let home = tempdir().unwrap();
        let store = Store::open_memory().unwrap();
        open_conv(&store, "c-wait-cancel");
        store.create_run("run-cancel", "c-wait-cancel").unwrap();

        let hub = CoreHub::new(
            home.path(),
            Registry {
                agents: Default::default(),
                proxies: Default::default(),
            },
            store,
            Arc::new(ActivityTracker::new()),
        );

        let hub2 = Arc::new(hub);
        let wait_hub = Arc::clone(&hub2);
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

        // Mid-flight cancel finalize (simulates cancel path writing Store).
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            hub2.store()
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
        use crate::daemon::ActivityTracker;
        use crate::endpoint::Registry;
        use std::sync::Arc;
        use tempfile::tempdir;

        let home = tempdir().unwrap();
        let store = Store::open_memory().unwrap();
        open_conv(&store, "c-missing");
        let hub = CoreHub::new(
            home.path(),
            Registry {
                agents: Default::default(),
                proxies: Default::default(),
            },
            store,
            Arc::new(ActivityTracker::new()),
        );
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
}
