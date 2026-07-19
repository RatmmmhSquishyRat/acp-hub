use super::*;

const TERMINAL_RETIRE_CLEANUP_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub(super) struct AgentConnection {
    pub(super) connection_id: String,
    pub(super) config: AgentEndpointConfig,
}

#[derive(Default)]
pub(super) struct GenerationGate {
    commands: Arc<tokio::sync::RwLock<()>>,
    callbacks: Arc<tokio::sync::RwLock<()>>,
    pub(super) waiting_writers: AtomicUsize,
    active_commands: AtomicUsize,
}

impl GenerationGate {
    fn writer_intent(self: &Arc<Self>) -> GenerationWriteIntent {
        self.waiting_writers.fetch_add(1, Ordering::SeqCst);
        GenerationWriteIntent {
            gate: Arc::clone(self),
        }
    }
}

struct GenerationWriteIntent {
    gate: Arc<GenerationGate>,
}

impl Drop for GenerationWriteIntent {
    fn drop(&mut self) {
        self.gate.waiting_writers.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) struct AgentConnectionLease {
    _guard: tokio::sync::OwnedRwLockReadGuard<()>,
    gate: Arc<GenerationGate>,
}

impl Drop for AgentConnectionLease {
    fn drop(&mut self) {
        self.gate.active_commands.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) type InboundConnectionLease = tokio::sync::OwnedRwLockReadGuard<()>;

pub(crate) struct AgentGenerationWriter {
    _intent: GenerationWriteIntent,
    _commands: tokio::sync::OwnedRwLockWriteGuard<()>,
    _callbacks: tokio::sync::OwnedRwLockWriteGuard<()>,
}

impl HubCtx {
    pub fn new(store: Store) -> Arc<Self> {
        let (notifications, _) = broadcast::channel(1024);
        Arc::new(Self {
            store,
            sessions: RwLock::default(),
            current_run: RwLock::default(),
            loading_sessions: RwLock::default(),
            pending_notifications: Mutex::default(),
            session_creation_captures: Mutex::default(),
            capture_budgets: Mutex::default(),
            capture_failures: Mutex::default(),
            agent_connections: RwLock::default(),
            generation_gates: Mutex::default(),
            activity: RwLock::default(),
            notifications,
            terminals: Mutex::default(),
            #[cfg(test)]
            bind_drain_gate: Mutex::default(),
            #[cfg(test)]
            terminal_spawn_gate: Mutex::default(),
            #[cfg(all(test, windows))]
            terminal_job_assignment_gate: Mutex::default(),
            #[cfg(test)]
            terminal_kill_error_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            capture_after_connection_gate: Mutex::default(),
            #[cfg(test)]
            bind_capture_failure_once: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn set_activity_tracker(&self, activity: Arc<ActivityTracker>) {
        *self.activity.write() = Some(activity);
    }

    pub(super) fn generation_gate(&self, agent_id: &str) -> Arc<GenerationGate> {
        Arc::clone(
            self.generation_gates
                .lock()
                .entry(agent_id.into())
                .or_default(),
        )
    }

    pub(crate) async fn acquire_connection_lease(
        &self,
        agent_id: &str,
        connection_id: &str,
    ) -> Result<AgentConnectionLease, HubError> {
        let gate = self.generation_gate(agent_id);
        let guard = Arc::clone(&gate.commands).read_owned().await;
        self.connection(agent_id, connection_id)?;
        gate.active_commands.fetch_add(1, Ordering::SeqCst);
        Ok(AgentConnectionLease {
            _guard: guard,
            gate,
        })
    }

    pub(crate) fn try_acquire_connection_lease(
        &self,
        agent_id: &str,
        connection_id: &str,
    ) -> Result<InboundConnectionLease, HubError> {
        let gate = self.generation_gate(agent_id);
        if gate.waiting_writers.load(Ordering::SeqCst) > 0
            && gate.active_commands.load(Ordering::SeqCst) == 0
        {
            return Err(HubError::other(format!(
                "agent {agent_id:?} connection replacement is pending"
            )));
        }
        let lease = Arc::clone(&gate.callbacks).try_read_owned().map_err(|_| {
            HubError::other(format!(
                "agent {agent_id:?} connection replacement is active"
            ))
        })?;
        self.connection(agent_id, connection_id)?;
        Ok(lease)
    }

    pub(crate) async fn agent_generation_writer(&self, agent_id: &str) -> AgentGenerationWriter {
        let gate = self.generation_gate(agent_id);
        let intent = gate.writer_intent();
        let commands = Arc::clone(&gate.commands).write_owned().await;
        let callbacks = Arc::clone(&gate.callbacks).write_owned().await;
        AgentGenerationWriter {
            _intent: intent,
            _commands: commands,
            _callbacks: callbacks,
        }
    }

    pub(super) fn try_agent_generation_writer(
        &self,
        agent_id: &str,
    ) -> Result<AgentGenerationWriter, HubError> {
        let gate = self.generation_gate(agent_id);
        let intent = gate.writer_intent();
        let commands = Arc::clone(&gate.commands)
            .try_write_owned()
            .map_err(|_| HubError::other(format!("agent {agent_id:?} connection is busy")))?;
        let callbacks = Arc::clone(&gate.callbacks)
            .try_write_owned()
            .map_err(|_| HubError::other(format!("agent {agent_id:?} callbacks are busy")))?;
        Ok(AgentGenerationWriter {
            _intent: intent,
            _commands: commands,
            _callbacks: callbacks,
        })
    }

    pub fn configure_agent(
        &self,
        agent_id: &str,
        connection_id: &str,
        config: AgentEndpointConfig,
    ) -> Result<(), HubError> {
        let _generation = self.try_agent_generation_writer(agent_id)?;
        self.configure_agent_locked(agent_id, connection_id, config);
        Ok(())
    }

    pub(crate) async fn configure_agent_async(
        &self,
        agent_id: &str,
        connection_id: &str,
        config: AgentEndpointConfig,
    ) {
        let _generation = self.agent_generation_writer(agent_id).await;
        self.configure_agent_locked(agent_id, connection_id, config);
    }

    pub(super) fn configure_agent_locked(
        &self,
        agent_id: &str,
        connection_id: &str,
        config: AgentEndpointConfig,
    ) {
        let mut connections = self.agent_connections.write();
        let replaced = connections
            .get(agent_id)
            .is_some_and(|connection| connection.connection_id != connection_id);
        if replaced {
            self.remove_agent_state(agent_id);
        }
        connections.insert(
            agent_id.into(),
            AgentConnection {
                connection_id: connection_id.into(),
                config,
            },
        );
    }

    pub fn revoke_agent(&self, agent_id: &str) -> Result<(), HubError> {
        let _generation = self.try_agent_generation_writer(agent_id)?;
        self.revoke_agent_locked(agent_id);
        Ok(())
    }

    pub(crate) fn revoke_agent_locked(&self, agent_id: &str) {
        self.agent_connections.write().remove(agent_id);
        self.remove_agent_state(agent_id);
    }

    pub(crate) async fn revoke_connection(&self, agent_id: &str, connection_id: &str) {
        if self
            .agent_connections
            .read()
            .get(agent_id)
            .is_none_or(|connection| connection.connection_id != connection_id)
        {
            return;
        }
        let _generation = self.agent_generation_writer(agent_id).await;
        let removed = {
            let mut connections = self.agent_connections.write();
            if connections
                .get(agent_id)
                .is_some_and(|connection| connection.connection_id == connection_id)
            {
                connections.remove(agent_id);
                true
            } else {
                false
            }
        };
        if removed {
            self.remove_agent_state(agent_id);
        }
    }

    pub(super) fn connection(
        &self,
        agent_id: &str,
        connection_id: &str,
    ) -> Result<AgentConnection, HubError> {
        self.agent_connections
            .read()
            .get(agent_id)
            .filter(|connection| connection.connection_id == connection_id)
            .cloned()
            .ok_or_else(|| HubError::other(format!("stale connection for agent {agent_id:?}")))
    }

    pub(super) fn remove_agent_state(&self, agent_id: &str) {
        self.remove_session_creation_capture(agent_id);
        self.sessions
            .write()
            .retain(|key, _| key.agent_id != agent_id);
        self.current_run
            .write()
            .retain(|key, _| key.agent_id != agent_id);
        self.loading_sessions
            .write()
            .retain(|key| key.agent_id != agent_id);
        self.capture_budgets
            .lock()
            .retain(|key, _| key.agent_id != agent_id);
        self.capture_failures
            .lock()
            .retain(|key, _| key.session.agent_id != agent_id);
        {
            let mut pending = self.pending_notifications.lock();
            let keys = pending
                .sessions
                .keys()
                .filter(|key| key.agent_id == agent_id)
                .cloned()
                .collect::<Vec<_>>();
            for key in keys {
                if let Some(queue) = pending.sessions.remove(&key) {
                    pending.count = pending.count.saturating_sub(queue.len());
                    pending.bytes = pending
                        .bytes
                        .saturating_sub(queue.iter().map(|entry| entry.bytes).sum());
                }
            }
            pending.draining.retain(|key| key.agent_id != agent_id);
        }
        self.retire_terminals_matching(|handle| handle.owner.agent_id == agent_id);
    }

    pub fn subscribe_notifications(&self) -> broadcast::Receiver<RpcRequest> {
        self.notifications.subscribe()
    }

    /// Return whether this endpoint-local session currently has a callback binding.
    pub fn is_session_bound(&self, agent_id: &str, session_id: &str) -> bool {
        self.sessions
            .read()
            .contains_key(&SessionKey::new(agent_id, session_id))
    }

    pub fn bind_session(&self, session_id: &str, binding: SessionBinding) -> Result<(), HubError> {
        let key = SessionKey::new(&binding.agent_id, session_id);
        {
            let mut sessions = self.sessions.write();
            let mut state = self.pending_notifications.lock();
            if !state.draining.insert(key.clone()) {
                return Err(HubError::other(format!(
                    "session {session_id:?} is already being bound"
                )));
            }
            sessions.remove(&key);
            state.sessions.entry(key.clone()).or_default();
        }
        self.capture_failures
            .lock()
            .retain(|failure_key, _| failure_key.session != key);
        self.capture_budgets.lock().entry(key.clone()).or_default();

        #[cfg(test)]
        {
            let gate = self.bind_drain_gate.lock().take();
            if let Some(gate) = gate {
                gate.wait();
            }
        }

        loop {
            let entry = {
                let mut state = self.pending_notifications.lock();
                let entry = state.sessions.get_mut(&key).and_then(VecDeque::pop_front);
                if let Some(entry) = &entry {
                    state.count = state.count.saturating_sub(1);
                    state.bytes = state.bytes.saturating_sub(entry.bytes);
                }
                entry
            };

            let Some(entry) = entry else {
                let mut sessions = self.sessions.write();
                let mut state = self.pending_notifications.lock();
                if !state.draining.contains(&key) {
                    return Err(HubError::other(format!(
                        "session {session_id:?} was revoked while binding"
                    )));
                }
                if state
                    .sessions
                    .get(&key)
                    .is_some_and(|queue| !queue.is_empty())
                {
                    continue;
                }
                state.sessions.remove(&key);
                state.draining.remove(&key);
                sessions.insert(key.clone(), binding);
                return Ok(());
            };

            let connections = self.agent_connections.read();
            if connections
                .get(&key.agent_id)
                .is_none_or(|connection| connection.connection_id != entry.connection_id)
            {
                continue;
            }

            let budget_before = self
                .capture_budgets
                .lock()
                .get(&key)
                .cloned()
                .unwrap_or_default();
            let capture_result = if self.take_bind_capture_failure_for_test() {
                Err(HubError::other("injected pending capture failure"))
            } else {
                self.capture_bound_notification(
                    &key.agent_id,
                    &key,
                    session_id,
                    &binding,
                    entry.notification.clone(),
                    entry.bytes,
                )
            };
            drop(connections);
            if let Err(error) = capture_result {
                self.record_capture_failure(&key, &entry.connection_id, &error);
                let restored = {
                    let mut state = self.pending_notifications.lock();
                    if state.draining.remove(&key) {
                        state
                            .sessions
                            .entry(key.clone())
                            .or_default()
                            .push_front(entry);
                        state.count = state.count.saturating_add(1);
                        state.bytes = state.bytes.saturating_add(
                            state
                                .sessions
                                .get(&key)
                                .and_then(VecDeque::front)
                                .map_or(0, |entry| entry.bytes),
                        );
                        true
                    } else {
                        false
                    }
                };
                if restored {
                    self.capture_budgets
                        .lock()
                        .insert(key.clone(), budget_before);
                }
                return Err(error);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_next_bind_capture_for_test(&self) {
        self.bind_capture_failure_once
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn take_bind_capture_failure_for_test(&self) -> bool {
        self.bind_capture_failure_once.swap(false, Ordering::AcqRel)
    }

    #[cfg(not(test))]
    fn take_bind_capture_failure_for_test(&self) -> bool {
        false
    }

    pub fn unbind_session(&self, agent_id: &str, session_id: &str) {
        self.unbind_session_matching(agent_id, session_id, None);
    }

    pub(crate) fn unbind_session_if_conversation(
        &self,
        agent_id: &str,
        session_id: &str,
        conv_id: &str,
    ) -> bool {
        self.unbind_session_matching(agent_id, session_id, Some(conv_id))
    }

    fn unbind_session_matching(
        &self,
        agent_id: &str,
        session_id: &str,
        expected_conv_id: Option<&str>,
    ) -> bool {
        let key = SessionKey::new(agent_id, session_id);
        {
            let mut sessions = self.sessions.write();
            if let Some(expected_conv_id) = expected_conv_id
                && sessions
                    .get(&key)
                    .is_some_and(|binding| binding.conv_id != expected_conv_id)
            {
                return false;
            }
            sessions.remove(&key);
        }
        self.current_run.write().remove(&key);
        self.loading_sessions.write().remove(&key);
        self.capture_budgets.lock().remove(&key);
        self.capture_failures
            .lock()
            .retain(|failure_key, _| failure_key.session != key);
        {
            let mut state = self.pending_notifications.lock();
            if let Some(pending) = state.sessions.remove(&key) {
                state.count = state.count.saturating_sub(pending.len());
                state.bytes = state
                    .bytes
                    .saturating_sub(pending.iter().map(|entry| entry.bytes).sum());
            }
            state.draining.remove(&key);
        }

        self.retire_terminals_matching(|handle| handle.owner == key);
        true
    }

    fn retire_terminals_matching(&self, mut matches: impl FnMut(&TerminalHandle) -> bool) {
        let retired = {
            let mut terminals = self.terminals.lock();
            let ids = terminals
                .iter()
                .filter_map(|(id, handle)| matches(handle).then_some(id.clone()))
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| terminals.remove(&id).map(|handle| (id, handle)))
                .collect::<Vec<_>>()
        };
        for (terminal_id, mut handle) in retired {
            let mut last_error = None;
            for attempt in 0..TERMINAL_RETIRE_CLEANUP_ATTEMPTS {
                match handle.cleanup() {
                    Ok(()) => {
                        last_error = None;
                        break;
                    }
                    Err(error) => {
                        last_error = Some(error);
                        if attempt + 1 < TERMINAL_RETIRE_CLEANUP_ATTEMPTS {
                            thread::yield_now();
                        }
                    }
                }
            }
            if let Some(error) = last_error {
                tracing::warn!(
                    terminal_id,
                    error = %error,
                    attempts = TERMINAL_RETIRE_CLEANUP_ATTEMPTS,
                    "terminal cleanup remained unsuccessful after bounded retirement attempts"
                );
            }
        }
    }

    pub fn set_current_run(&self, agent_id: &str, session_id: &str, run_id: &str) {
        let key = SessionKey::new(agent_id, session_id);
        self.current_run.write().insert(key.clone(), run_id.into());
        self.capture_budgets
            .lock()
            .insert(key, CaptureBudget::default());
    }

    pub fn clear_current_run(&self, agent_id: &str, session_id: &str) {
        self.current_run
            .write()
            .remove(&SessionKey::new(agent_id, session_id));
    }

    pub(crate) fn begin_capture_operation(
        &self,
        agent_id: &str,
        connection_id: &str,
        session_id: &str,
        operation: &'static str,
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
        let session = SessionKey::new(agent_id, session_id);
        let run_id = self.current_run.read().get(&session).cloned();
        self.capture_budgets
            .lock()
            .insert(session.clone(), CaptureBudget::default());
        self.capture_failures.lock().insert(
            CaptureFailureKey {
                session,
                connection_id: connection_id.into(),
            },
            CaptureFailure {
                operation,
                run_id,
                first_error: None,
            },
        );
        Ok(())
    }

    pub(crate) fn take_capture_failure(
        &self,
        agent_id: &str,
        connection_id: &str,
        session_id: &str,
    ) -> Option<HubError> {
        let failure = self.capture_failures.lock().remove(&CaptureFailureKey {
            session: SessionKey::new(agent_id, session_id),
            connection_id: connection_id.into(),
        })?;
        let source = failure.first_error?;
        let correlation = failure
            .run_id
            .map(|run_id| format!(" for run {run_id}"))
            .unwrap_or_default();
        Some(HubError::other(format!(
            "{}{} failed because a session update could not be captured: {source}",
            failure.operation, correlation
        )))
    }

    pub(super) fn record_capture_failure(
        &self,
        key: &SessionKey,
        connection_id: &str,
        error: &HubError,
    ) {
        let mut failures = self.capture_failures.lock();
        let failure_key = CaptureFailureKey {
            session: key.clone(),
            connection_id: connection_id.into(),
        };
        if let Some(failure) = failures.get_mut(&failure_key)
            && failure.first_error.is_none()
        {
            failure.first_error = Some(error.to_string());
        }
    }

    pub(super) fn binding(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<SessionBinding, HubError> {
        self.sessions
            .read()
            .get(&SessionKey::new(agent_id, session_id))
            .cloned()
            .ok_or_else(|| {
                HubError::other(format!(
                    "unknown session {session_id:?} for agent {agent_id:?}"
                ))
            })
    }

    pub(super) fn run_for_session(&self, agent_id: &str, session_id: &str) -> Option<String> {
        self.current_run
            .read()
            .get(&SessionKey::new(agent_id, session_id))
            .cloned()
    }

    /// Mark a session as currently in load-replay mode (Layer 1).
    pub fn set_loading(&self, agent_id: &str, session_id: &str, loading: bool) {
        let key = SessionKey::new(agent_id, session_id);
        if loading {
            self.loading_sessions.write().insert(key.clone());
            self.capture_budgets
                .lock()
                .insert(key, CaptureBudget::default());
        } else {
            self.loading_sessions.write().remove(&key);
        }
    }

    /// Check if a session is in load-replay mode.
    pub(super) fn is_loading(&self, agent_id: &str, session_id: &str) -> bool {
        self.loading_sessions
            .read()
            .contains(&SessionKey::new(agent_id, session_id))
    }
}
