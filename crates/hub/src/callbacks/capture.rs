use super::*;

pub(super) const MAX_PENDING_SESSIONS: usize = 64;
const MAX_PENDING_NOTIFICATIONS: usize = 1_024;
const MAX_PENDING_NOTIFICATION_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_PENDING_SINGLE_NOTIFICATION_BYTES: usize = 256 * 1024;
pub(super) const MAX_CAPTURE_UPDATES_PER_TURN: usize = 4_096;
const MAX_CAPTURE_BYTES_PER_TURN: usize = 16 * 1024 * 1024;

impl HubCtx {
    // ---- notification capture ----------------------------------------------

    pub fn handle_notification(
        &self,
        agent_id: &str,
        connection_id: &str,
        notif: SessionNotification,
    ) -> Result<(), HubError> {
        let key = SessionKey::new(agent_id, notif.session_id.to_string().as_str());
        let result = self.capture_notification(agent_id, connection_id, notif);
        if let Err(error) = &result {
            self.record_capture_failure(&key, connection_id, error);
        }
        result
    }

    pub(super) fn capture_notification(
        &self,
        agent_id: &str,
        connection_id: &str,
        notif: SessionNotification,
    ) -> Result<(), HubError> {
        let connections = self.agent_connections.read();
        if connections
            .get(agent_id)
            .is_none_or(|connection| connection.connection_id != connection_id)
        {
            return Err(HubError::other(format!(
                "stale connection for agent {agent_id:?}"
            )));
        }
        #[cfg(test)]
        {
            let gate = self.capture_after_connection_gate.lock().take();
            if let Some(gate) = gate {
                gate.wait();
            }
        }
        let sid = notif.session_id.to_string();
        let key = SessionKey::new(agent_id, &sid);
        let bytes = serde_json::to_vec(&notif)?.len();
        if bytes > MAX_PENDING_SINGLE_NOTIFICATION_BYTES {
            return Err(HubError::other(format!(
                "session update exceeds the {MAX_PENDING_SINGLE_NOTIFICATION_BYTES}-byte limit"
            )));
        }
        let binding = {
            let sessions = self.sessions.read();
            match sessions.get(&key) {
                Some(binding) => binding.clone(),
                None => {
                    const MAX_PENDING_PER_SESSION: usize = 256;
                    let mut pending = self.pending_notifications.lock();
                    let is_new_session = !pending.sessions.contains_key(&key);
                    if is_new_session && pending.sessions.len() >= MAX_PENDING_SESSIONS {
                        return Err(HubError::other(
                            "too many unbound sessions have pending updates",
                        ));
                    }
                    if pending.count >= MAX_PENDING_NOTIFICATIONS
                        || pending.bytes.saturating_add(bytes) > MAX_PENDING_NOTIFICATION_BYTES
                    {
                        return Err(HubError::other("pending pre-bind update quota exceeded"));
                    }
                    let queue = pending.sessions.entry(key).or_default();
                    if queue.len() >= MAX_PENDING_PER_SESSION {
                        return Err(HubError::other(format!(
                            "too many updates arrived before session {sid:?} was bound"
                        )));
                    }
                    queue.push_back(PendingNotification {
                        connection_id: connection_id.into(),
                        notification: notif,
                        bytes,
                    });
                    pending.count += 1;
                    pending.bytes += bytes;
                    return Ok(());
                }
            }
        };
        let result = self.capture_bound_notification(agent_id, &key, &sid, &binding, notif, bytes);
        drop(connections);
        result
    }

    pub(super) fn capture_bound_notification(
        &self,
        agent_id: &str,
        key: &SessionKey,
        sid: &str,
        binding: &SessionBinding,
        notif: SessionNotification,
        bytes: usize,
    ) -> Result<(), HubError> {
        {
            let mut budgets = self.capture_budgets.lock();
            let budget = budgets.entry(key.clone()).or_default();
            if budget.updates >= MAX_CAPTURE_UPDATES_PER_TURN
                || budget.bytes.saturating_add(bytes) > MAX_CAPTURE_BYTES_PER_TURN
            {
                return Err(HubError::other(format!(
                    "session update capture budget exceeded ({MAX_CAPTURE_UPDATES_PER_TURN} updates or {MAX_CAPTURE_BYTES_PER_TURN} bytes)"
                )));
            }
            budget.updates += 1;
            budget.bytes += bytes;
        }
        let conv_id = binding.conv_id.clone();
        let run_id = self.run_for_session(agent_id, sid);
        let source = if self.is_loading(agent_id, sid) {
            MessageSource::LoadReplay
        } else {
            MessageSource::LocalTurn
        };
        let update_json = serde_json::to_value(&notif.update)?;
        match notif.update {
            SessionUpdate::AgentMessageChunk(c) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                None,
                &c,
            ),
            SessionUpdate::UserMessageChunk(c) => {
                cap(&self.store, &conv_id, &run_id, source, "user", None, &c)
            }
            SessionUpdate::AgentThoughtChunk(c) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("thought"),
                &c,
            ),
            SessionUpdate::ToolCall(t) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("tool_call"),
                &t,
            ),
            SessionUpdate::ToolCallUpdate(u) => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("tool_call_update"),
                &u,
            ),
            SessionUpdate::Plan(p) => self
                .store
                .set_plan_snapshot(&conv_id, &serde_json::to_value(&p)?),
            SessionUpdate::AvailableCommandsUpdate(cmds) => self
                .store
                .set_available_commands_snapshot(&conv_id, &serde_json::to_value(&cmds)?),
            SessionUpdate::CurrentModeUpdate(m) => {
                let mut patch = serde_json::Map::new();
                patch.insert("currentMode".into(), serde_json::to_value(&m)?);
                self.store
                    .apply_session_info(&conv_id, None, None, Some(&patch))?;
                Ok(())
            }
            SessionUpdate::ConfigOptionUpdate(c) => {
                let v = serde_json::to_value(&c.config_options)?;
                self.store.set_config_snapshot(&conv_id, &v, None)?;
                Ok(())
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                let title: Option<String> = info.title.value().map(|s| s.to_string());
                let updated: Option<String> = info.updated_at.value().map(|s| s.to_string());
                let t = title.as_deref();
                let u = updated.as_deref();
                let meta: Option<&serde_json::Map<String, serde_json::Value>> = info.meta.as_ref();
                self.store.apply_session_info(&conv_id, t, u, meta)?;
                Ok(())
            }
            SessionUpdate::UsageUpdate(u) => {
                let cost = serde_json::to_value(&u.cost)?;
                self.store.upsert_usage_snapshot(
                    &conv_id,
                    u.used as i64,
                    u.size as i64,
                    Some(&cost),
                )?;
                Ok(())
            }
            other => cap(
                &self.store,
                &conv_id,
                &run_id,
                source,
                "assistant",
                Some("update"),
                &other,
            ),
        }?;
        let _ = self.notifications.send(RpcRequest::notification(
            "hub/conv/update",
            serde_json::json!({
                "agentId": agent_id,
                "sessionId": sid,
                "conversationId": conv_id,
                "runId": run_id,
                "source": source,
                "update": update_json,
            }),
        ));
        Ok(())
    }
}

fn cap(
    store: &Store,
    conv: &str,
    run: &Option<String>,
    source: MessageSource,
    role: &str,
    kind: Option<&str>,
    p: &impl serde::Serialize,
) -> Result<(), HubError> {
    let val = serde_json::to_value(p)?;
    let id = format!("msg-{}", Uuid::new_v4().simple());
    let body = search_body(&val);
    store
        .append_message(&NewMessage {
            id,
            conv_id: conv.into(),
            run_id: run.clone(),
            source,
            role: role.into(),
            kind: kind.map(str::to_string),
            content_json: val,
            body_text: body,
        })
        .map(|_| ())
}
