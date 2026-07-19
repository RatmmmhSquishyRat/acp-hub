use super::*;

pub(super) const MAX_PENDING_SESSIONS: usize = 64;
pub(super) const MAX_PENDING_NOTIFICATIONS: usize = 1_024;
pub(super) const MAX_PENDING_NOTIFICATION_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_PENDING_SINGLE_NOTIFICATION_BYTES: usize = 256 * 1024;
pub(super) const MAX_PENDING_PER_SESSION: usize = 256;
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
            self.record_session_creation_capture_failure(agent_id, connection_id, error);
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
        if self.capture_session_creation_notification(&key, connection_id, notif.clone(), bytes)? {
            return Ok(());
        }
        let binding = self.sessions.read().get(&key).cloned();
        let Some(binding) = binding else {
            let captures = self.session_creation_captures.lock();
            let mut pending = self.pending_notifications.lock();
            let mut distinct_keys = pending.sessions.keys().cloned().collect::<HashSet<_>>();
            distinct_keys.extend(
                captures
                    .agents
                    .values()
                    .flat_map(|capture| capture.entries.iter())
                    .map(|(entry_key, _)| entry_key.clone()),
            );
            let is_new_session = distinct_keys.insert(key.clone());
            if is_new_session && distinct_keys.len() > MAX_PENDING_SESSIONS {
                return Err(HubError::other(
                    "too many unbound sessions have pending updates",
                ));
            }
            if pending.count.saturating_add(captures.count) >= MAX_PENDING_NOTIFICATIONS
                || pending
                    .bytes
                    .saturating_add(captures.bytes)
                    .saturating_add(bytes)
                    > MAX_PENDING_NOTIFICATION_BYTES
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
        };
        let result = self.capture_bound_notification(agent_id, &key, &sid, &binding, notif, bytes);
        drop(connections);
        result
    }

    fn capture_session_creation_notification(
        &self,
        key: &SessionKey,
        connection_id: &str,
        notification: SessionNotification,
        bytes: usize,
    ) -> Result<bool, HubError> {
        let mut captures = self.session_creation_captures.lock();
        let Some(capture) = captures.agents.get(&key.agent_id) else {
            return Ok(false);
        };
        let pending = self.pending_notifications.lock();
        let per_session = capture
            .entries
            .iter()
            .filter(|(entry_key, _)| entry_key == key)
            .count()
            .saturating_add(pending.sessions.get(key).map_or(0, VecDeque::len));
        if per_session >= MAX_PENDING_PER_SESSION {
            return Err(HubError::other(format!(
                "too many updates arrived while session {:?} was being created",
                key.session_id
            )));
        }
        let mut distinct_keys = pending.sessions.keys().cloned().collect::<HashSet<_>>();
        distinct_keys.extend(
            captures
                .agents
                .values()
                .flat_map(|capture| capture.entries.iter())
                .map(|(entry_key, _)| entry_key.clone()),
        );
        let is_new_key = distinct_keys.insert(key.clone());
        if is_new_key && distinct_keys.len() > MAX_PENDING_SESSIONS {
            return Err(HubError::other(
                "too many sessions have updates pending publication",
            ));
        }
        if pending.count.saturating_add(captures.count) >= MAX_PENDING_NOTIFICATIONS
            || pending
                .bytes
                .saturating_add(captures.bytes)
                .saturating_add(bytes)
                > MAX_PENDING_NOTIFICATION_BYTES
        {
            return Err(HubError::other(
                "pending session creation update quota exceeded",
            ));
        }
        drop(pending);
        captures.count += 1;
        captures.bytes += bytes;
        captures
            .agents
            .get_mut(&key.agent_id)
            .expect("capture remains registered")
            .entries
            .push_back((
                key.clone(),
                PendingNotification {
                    connection_id: connection_id.to_string(),
                    notification,
                    bytes,
                },
            ));
        Ok(true)
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

impl HubCtx {
    pub(crate) fn begin_session_creation_capture(
        self: &Arc<Self>,
        agent_id: &str,
        connection_id: &str,
    ) -> Result<SessionCreationCaptureLease, HubError> {
        let token = Uuid::new_v4();
        let mut captures = self.session_creation_captures.lock();
        if captures.agents.contains_key(agent_id) {
            return Err(HubError::Conflict(format!(
                "{agent_id}:session/new already in progress"
            )));
        }
        captures.agents.insert(
            agent_id.to_string(),
            SessionCreationCapture {
                token,
                connection_id: connection_id.to_string(),
                entries: VecDeque::new(),
                first_error: None,
            },
        );
        Ok(SessionCreationCaptureLease {
            ctx: Arc::clone(self),
            agent_id: agent_id.to_string(),
            connection_id: connection_id.to_string(),
            token,
            finished: false,
        })
    }

    fn finish_session_creation_capture(
        &self,
        agent_id: &str,
        connection_id: &str,
        token: Uuid,
        session_id: Option<&str>,
        publish: bool,
        missing_ok: bool,
    ) -> Result<(), HubError> {
        let mut captures = self.session_creation_captures.lock();
        let Some(capture) = captures.agents.get(agent_id) else {
            return if missing_ok {
                Ok(())
            } else {
                Err(HubError::other(format!(
                    "session/new capture was revoked for agent {agent_id:?}"
                )))
            };
        };
        if capture.token != token || capture.connection_id != connection_id {
            return Err(HubError::other(format!(
                "stale session/new capture lease for agent {agent_id:?}"
            )));
        }
        let capture = captures
            .agents
            .remove(agent_id)
            .expect("capture was checked above");
        let capture_error = capture
            .first_error
            .map(|error| HubError::other(format!("session/new update capture failed: {error}")));
        let publish_matching = publish && capture_error.is_none();
        let capture_count = capture.entries.len();
        let capture_bytes = capture
            .entries
            .iter()
            .map(|(_, entry)| entry.bytes)
            .sum::<usize>();
        let mut matching = VecDeque::new();
        let mut replay = VecDeque::new();
        for (key, entry) in capture.entries {
            if session_id.is_some_and(|session_id| key.session_id == session_id) {
                if publish_matching {
                    matching.push_back((key, entry));
                }
            } else {
                replay.push_back((key, entry));
            }
        }
        let mut pending = self.pending_notifications.lock();
        captures.count = captures.count.saturating_sub(capture_count);
        captures.bytes = captures.bytes.saturating_sub(capture_bytes);
        for (key, entry) in matching {
            pending.count += 1;
            pending.bytes += entry.bytes;
            pending.sessions.entry(key).or_default().push_back(entry);
        }
        drop(pending);
        drop(captures);

        let mut replay_error = None;
        for (key, entry) in replay {
            let entry_connection_id = entry.connection_id.clone();
            if let Err(error) = self.replay_session_creation_notification(&key, entry) {
                self.record_capture_failure(&key, &entry_connection_id, &error);
                replay_error.get_or_insert(error);
            }
        }
        capture_error.or(replay_error).map_or(Ok(()), Err)
    }

    fn replay_session_creation_notification(
        &self,
        key: &SessionKey,
        entry: PendingNotification,
    ) -> Result<(), HubError> {
        let sid = key.session_id.clone();
        let binding = self.sessions.read().get(key).cloned();
        if let Some(binding) = binding {
            return self.capture_bound_notification(
                &key.agent_id,
                key,
                &sid,
                &binding,
                entry.notification,
                entry.bytes,
            );
        }
        Ok(())
    }

    fn record_session_creation_capture_failure(
        &self,
        agent_id: &str,
        connection_id: &str,
        error: &HubError,
    ) {
        let mut captures = self.session_creation_captures.lock();
        if let Some(capture) = captures.agents.get_mut(agent_id)
            && capture.connection_id == connection_id
            && capture.first_error.is_none()
        {
            capture.first_error = Some(error.to_string());
        }
    }

    pub(super) fn remove_session_creation_capture(&self, agent_id: &str) {
        let mut captures = self.session_creation_captures.lock();
        if let Some(capture) = captures.agents.remove(agent_id) {
            captures.count = captures.count.saturating_sub(capture.entries.len());
            captures.bytes = captures.bytes.saturating_sub(
                capture
                    .entries
                    .iter()
                    .map(|(_, entry)| entry.bytes)
                    .sum::<usize>(),
            );
        }
    }
}

impl SessionCreationCaptureLease {
    pub(crate) fn publish(&mut self, session_id: &str) -> Result<(), HubError> {
        if self.finished {
            return Ok(());
        }
        let result = self.ctx.finish_session_creation_capture(
            &self.agent_id,
            &self.connection_id,
            self.token,
            Some(session_id),
            true,
            false,
        );
        self.finished = true;
        result
    }

    pub(crate) fn reject(&mut self, session_id: &str) -> Result<(), HubError> {
        if self.finished {
            return Ok(());
        }
        let result = self.ctx.finish_session_creation_capture(
            &self.agent_id,
            &self.connection_id,
            self.token,
            Some(session_id),
            false,
            false,
        );
        self.finished = true;
        result
    }
}

impl Drop for SessionCreationCaptureLease {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.ctx.finish_session_creation_capture(
                &self.agent_id,
                &self.connection_id,
                self.token,
                None,
                false,
                true,
            );
        }
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
