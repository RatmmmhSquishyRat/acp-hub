use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::state::{CoreHub, OperationLease, OperationMap};
use super::types::AgentInspection;
use crate::acp::{AgentCommand, AgentHandle, spawn_agent_connection_with_flow};
use crate::conductor;
use crate::endpoint::{
    AgentEndpointConfig, EndpointConfigRef, ProxyEndpointConfig, PublicEndpointConfig, Registry,
    public_endpoint_config,
};
use crate::error::HubError;
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

fn affected_agent_ids(current: &Registry, next: &Registry) -> Vec<String> {
    let changed_proxies: HashSet<&str> = current
        .proxies
        .keys()
        .chain(next.proxies.keys())
        .filter(|proxy_id| current.proxies.get(*proxy_id) != next.proxies.get(*proxy_id))
        .map(String::as_str)
        .collect();
    let mut affected: Vec<String> = current
        .agents
        .keys()
        .chain(next.agents.keys())
        .filter(|agent_id| {
            let before = current.agents.get(*agent_id);
            let after = next.agents.get(*agent_id);
            before != after
                || before
                    .into_iter()
                    .chain(after)
                    .flat_map(|agent| &agent.proxy_chain)
                    .any(|proxy_id| changed_proxies.contains(proxy_id.as_str()))
        })
        .cloned()
        .collect();
    affected.sort_unstable();
    affected.dedup();
    affected
}

pub(super) fn reject_active_agents(
    operations: &OperationMap,
    agent_ids: &[String],
) -> Result<(), HubError> {
    if let Some((conv_id, _)) = operations
        .iter()
        .find(|(_, entry)| agent_ids.iter().any(|agent_id| agent_id == &entry.agent_id))
    {
        return Err(HubError::Conflict(conv_id.clone()));
    }
    Ok(())
}

impl CoreHub {
    /// Register or replace an agent endpoint and persist `agents.json`.
    pub async fn register_agent(
        &self,
        agent_id: impl Into<String>,
        config: AgentEndpointConfig,
    ) -> Result<(), HubError> {
        let agent_id = agent_id.into();
        self.mutate_registry(move |next| next.register_agent(agent_id, config))
            .await
    }

    /// Remove an agent endpoint and persist `agents.json`.
    pub async fn remove_agent(&self, agent_id: &str) -> Result<(), HubError> {
        let agent_id = agent_id.to_string();
        self.mutate_registry(move |next| next.remove_agent(&agent_id))
            .await
    }

    /// Register or replace a proxy endpoint and persist `agents.json`.
    pub async fn register_proxy(
        &self,
        proxy_id: impl Into<String>,
        config: ProxyEndpointConfig,
    ) -> Result<(), HubError> {
        let proxy_id = proxy_id.into();
        self.mutate_registry(move |next| next.register_proxy(proxy_id, config))
            .await
    }

    /// Remove a proxy endpoint and persist `agents.json`.
    pub async fn remove_proxy(&self, proxy_id: &str) -> Result<(), HubError> {
        let proxy_id = proxy_id.to_string();
        self.mutate_registry(move |next| next.remove_proxy(&proxy_id))
            .await
    }

    /// List registered agents through the secret-safe public DTO.
    pub fn list_agents(&self) -> BTreeMap<String, PublicEndpointConfig> {
        self.registry
            .read()
            .agents
            .iter()
            .map(|(id, config)| {
                (
                    id.clone(),
                    public_endpoint_config(EndpointConfigRef::Agent(config)),
                )
            })
            .collect()
    }

    /// List registered proxies through the secret-safe public DTO.
    pub fn list_proxies(&self) -> BTreeMap<String, PublicEndpointConfig> {
        self.registry
            .read()
            .proxies
            .iter()
            .map(|(id, config)| {
                (
                    id.clone(),
                    public_endpoint_config(EndpointConfigRef::Proxy(config)),
                )
            })
            .collect()
    }

    /// Inspect a registered agent endpoint without opening a new ACP connection.
    pub fn inspect_agent(&self, agent_id: &str) -> Result<AgentInspection, HubError> {
        let raw_config = self.agent_config(agent_id)?;
        let config = public_endpoint_config(EndpointConfigRef::Agent(&raw_config));
        let cache = self.store().agent_cache(agent_id)?;
        let cache_populated = cache.is_some();
        let (agent_info, capabilities) = match cache {
            Some((agent_info, capabilities)) => (
                Some(serde_json::from_str(&agent_info)?),
                Some(serde_json::from_str(&capabilities)?),
            ),
            None => (None, None),
        };
        Ok(AgentInspection {
            agent_id: agent_id.to_string(),
            config,
            agent_info,
            capabilities,
            cache_populated,
        })
    }

    /// Authenticate an agent using one of its advertised auth methods.
    pub async fn authenticate_agent(
        &self,
        agent_id: &str,
        method_id: &str,
    ) -> Result<(), HubError> {
        let handle = self.agent_handle(agent_id).await?;
        if !handle
            .auth_methods
            .iter()
            .any(|method| method.id == method_id)
        {
            return Err(HubError::not_found("auth method", method_id));
        }
        self.request_agent(agent_id, &handle, |reply| AgentCommand::Authenticate {
            method_id: method_id.to_string(),
            reply,
        })
        .await
    }

    /// Logout an agent through its live ACP connection.
    pub async fn logout_agent(&self, agent_id: &str) -> Result<(), HubError> {
        let handle = self.agent_handle(agent_id).await?;
        self.request_agent(agent_id, &handle, |reply| AgentCommand::Logout { reply })
            .await
    }

    /// List sessions known to the agent (ACP `session/list`) and auto-import
    /// Discover remote sessions (metadata-only upsert; **no** session/load).
    /// Returns Hub DTOs with interaction/space/in_hub_before (PHASE1-CONTRACT §3).
    pub async fn list_agent_sessions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<serde_json::Value>, HubError> {
        let handle = self.agent_handle(agent_id).await?;
        let result = self
            .request_agent(agent_id, &handle, |reply| AgentCommand::ListSessions {
                cwd: None,
                reply,
            })
            .await?;
        struct DiscoveredSession {
            sid: String,
            title: Option<String>,
            cwd: String,
            dirs: Vec<String>,
            meta: Option<serde_json::Value>,
        }
        let mut discovered: Vec<DiscoveredSession> = Vec::with_capacity(result.sessions.len());
        let mut seen = HashSet::new();
        for info in &result.sessions {
            let sid = info.session_id.to_string();
            if !seen.insert(sid.clone()) {
                continue;
            }
            let title = info.title.as_deref();
            if !info.cwd.is_absolute() {
                return Err(HubError::other(format!(
                    "agent session {sid} returned a relative cwd: {}",
                    info.cwd.display()
                )));
            }
            let cwd = info.cwd.to_str().ok_or_else(|| {
                HubError::other(format!(
                    "agent session {sid} returned a cwd that is not valid UTF-8"
                ))
            })?;
            let dirs: Vec<String> = info
                .additional_directories
                .iter()
                .map(|d| {
                    if !d.is_absolute() {
                        return Err(HubError::other(format!(
                            "agent session {sid} returned a relative additional directory: {}",
                            d.display()
                        )));
                    }
                    d.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                        HubError::other(format!(
                            "agent session {sid} returned an additional directory that is not valid UTF-8"
                        ))
                    })
                })
                .collect::<Result<_, _>>()?;
            let meta = info
                .meta
                .as_ref()
                .map(|m| serde_json::Value::Object(m.clone()));
            discovered.push(DiscoveredSession {
                sid,
                title: title.map(ToOwned::to_owned),
                cwd: cwd.to_string(),
                dirs,
                meta,
            });
        }

        let mut out = Vec::with_capacity(discovered.len());
        for item in discovered {
            let provisional_existing = self
                .store()
                .conversation_by_agent_session(agent_id, &item.sid)?;
            let provisional_conv_id = provisional_existing.as_ref().map_or_else(
                || format!("conv-{}", Uuid::new_v4().simple()),
                |row| row.id.clone(),
            );
            let _identity =
                self.reserve_session_identity(agent_id, &item.sid, &provisional_conv_id)?;
            // Metadata only — never session/load on discover (Phase 1).
            let upsert = self.store().upsert_agent_session_discover(
                agent_id,
                &item.sid,
                item.title.as_deref(),
                Some(&item.cwd),
                &item.dirs,
                item.meta.as_ref(),
            )?;
            let (_, space) = crate::store::parse_session_meta(item.meta.as_ref());
            out.push(serde_json::json!({
                "agent_session_id": item.sid,
                "sessionId": item.sid,
                "title": item.title,
                "interaction": upsert.interaction.as_str(),
                "space": space.as_str(),
                "in_hub_before": upsert.in_hub_before,
                "conv_id": upsert.conv_id,
                "origin": upsert.origin.as_str(),
            }));
        }
        Ok(out)
    }

    pub(super) async fn agent_handle(&self, agent_id: &str) -> Result<Arc<AgentHandle>, HubError> {
        if let Some(handle) = self.handles.lock().await.get(agent_id).cloned() {
            return Ok(handle);
        }
        let init_lock = {
            let mut inits = self.handle_inits.lock().await;
            Arc::clone(
                inits
                    .entry(agent_id.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _initializing = init_lock.lock().await;
        if let Some(handle) = self.handles.lock().await.get(agent_id).cloned() {
            return Ok(handle);
        }

        let registry = self.registry.read().clone();
        let registry_epoch = self.registry_epoch.load(Ordering::Acquire);
        let agent_config = registry
            .agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| HubError::not_found("agent", agent_id))?;
        let endpoint = conductor::build_endpoint_component(&registry, agent_id)?;
        let rx = spawn_agent_connection_with_flow(
            endpoint.component,
            agent_id.to_string(),
            agent_config.clone(),
            Arc::clone(&self.ctx),
            endpoint.flows,
        );
        let handle = Arc::new(
            tokio::time::timeout(Duration::from_secs(30), rx)
                .await
                .map_err(|_| {
                    HubError::other(format!(
                        "agent {agent_id} did not initialize within 30 seconds"
                    ))
                })?
                .map_err(|_| {
                    HubError::other(format!("agent {agent_id} connection task ended"))
                })??,
        );
        let capabilities = serde_json::to_string(&handle.capabilities)?;
        #[cfg(test)]
        let handle_publish_gate = { self.handle_publish_gate.lock().take() };
        #[cfg(test)]
        if let Some((reached, release)) = handle_publish_gate {
            let _ = reached.send(());
            let _ = release.await;
        }
        let still_current = self.registry_epoch.load(Ordering::Acquire) == registry_epoch
            && self
                .registry
                .read()
                .agents
                .get(agent_id)
                .is_some_and(|current| current == &agent_config);
        if !still_current {
            return Err(HubError::Conflict(agent_id.to_string()));
        }
        self.store()
            .upsert_agent_cache(agent_id, "{}", &capabilities)?;
        self.handles
            .lock()
            .await
            .insert(agent_id.to_string(), Arc::clone(&handle));

        Ok(handle)
    }
    pub(super) async fn enqueue_operation<T>(
        &self,
        handle: &Arc<AgentHandle>,
        operation: Arc<OperationLease>,
        command: impl FnOnce(oneshot::Sender<Result<T, HubError>>) -> AgentCommand,
    ) -> Result<T, HubError>
    where
        T: Send + 'static,
    {
        let agent_id = operation
            .operations
            .lock()
            .get(&operation.conv_id)
            .map(|entry| entry.agent_id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let permit = handle
            .cmd_tx
            .clone()
            .reserve_owned()
            .await
            .map_err(|_| HubError::other(format!("agent {agent_id} command loop is closed")))?;
        let (reply, response) = oneshot::channel();
        permit.send(command(reply));
        let worker = tokio::spawn(async move {
            let _operation = operation;
            match response.await {
                Ok(result) => result,
                Err(_) => Err(HubError::other(format!(
                    "agent {agent_id} command response dropped"
                ))),
            }
        });
        worker
            .await
            .map_err(|error| HubError::other(format!("operation worker failed: {error}")))?
    }

    async fn evict_handle_if_current(&self, agent_id: &str, handle: &Arc<AgentHandle>) {
        let mut handles = self.handles.lock().await;
        if handles
            .get(agent_id)
            .is_some_and(|current| Arc::ptr_eq(current, handle))
        {
            handles.remove(agent_id);
        }
    }

    async fn request_agent<T>(
        &self,
        agent_id: &str,
        handle: &Arc<AgentHandle>,
        f: impl FnOnce(oneshot::Sender<Result<T, HubError>>) -> AgentCommand,
    ) -> Result<T, HubError>
    where
        T: Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        if handle.cmd_tx.send(f(reply)).await.is_err() {
            self.evict_handle_if_current(agent_id, handle).await;
            return Err(HubError::other(format!(
                "agent {agent_id} command loop is closed"
            )));
        }
        match rx.await {
            Ok(result) => result,
            Err(_) => {
                self.evict_handle_if_current(agent_id, handle).await;
                Err(HubError::other(format!(
                    "agent {agent_id} command response dropped"
                )))
            }
        }
    }

    async fn mutate_registry(
        &self,
        mutate: impl FnOnce(&mut Registry) -> Result<(), HubError>,
    ) -> Result<(), HubError> {
        let _mutation = self.registry_mutation.lock().await;
        let current = self.registry.read().clone();
        let mut next = current.clone();
        mutate(&mut next)?;
        let affected = affected_agent_ids(&current, &next);

        let init_locks = {
            let mut inits = self.handle_inits.lock().await;
            affected
                .iter()
                .map(|agent_id| {
                    Arc::clone(
                        inits
                            .entry(agent_id.clone())
                            .or_insert_with(|| Arc::new(Mutex::new(()))),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut init_guards = Vec::with_capacity(init_locks.len());
        for init_lock in init_locks {
            init_guards.push(init_lock.lock_owned().await);
        }
        let mut generation_writers = Vec::with_capacity(affected.len());
        for agent_id in &affected {
            generation_writers.push(self.ctx.agent_generation_writer(agent_id).await);
        }
        let mut handles = self.handles.lock().await;
        let _operations = self.lock_agents_idle(&affected)?;

        let disk_fingerprint = Registry::fingerprint(&self.home)?;
        let expected_fingerprint = *self.registry_fingerprint.read();
        if disk_fingerprint != expected_fingerprint {
            return Err(HubError::InvalidRegistry(
                "agents.json changed outside the running daemon; restart the daemon to load the external edit before applying registry mutations"
                    .to_string(),
            ));
        }

        #[cfg(test)]
        let save_result = if self.registry_save_fail_once.swap(false, Ordering::AcqRel) {
            Err(HubError::other("injected registry save failure"))
        } else {
            next.save(&self.home)
        };
        #[cfg(not(test))]
        let save_result = next.save(&self.home);
        #[cfg(test)]
        let disk_registry = if self.registry_verify_fail_once.swap(false, Ordering::AcqRel) {
            Err(HubError::other("injected registry verification failure"))
        } else {
            Registry::load(&self.home)
        };
        #[cfg(not(test))]
        let disk_registry = Registry::load(&self.home);
        match (save_result, disk_registry) {
            (Ok(()), Ok(actual)) if actual == next => {}
            (Err(_), Ok(actual)) if actual == next => {
                // The atomic replace committed, but a post-replace hardening
                // step failed. Publish the committed image so memory and disk
                // cannot diverge and report the mutation as committed.
            }
            (Err(error), Ok(actual)) if actual == current => return Err(error),
            (Ok(()), Ok(actual)) | (Err(_), Ok(actual)) => {
                return Err(HubError::InvalidRegistry(format!(
                    "registry commit produced an unexpected disk image: {actual:?}"
                )));
            }
            (Err(save_error), Err(verification_error)) => {
                // `save` can report an error after atomic replacement (for
                // example while hardening the destination). Bypass that
                // hardening step only to identify the exact committed bytes;
                // the parsed image still receives full schema validation.
                let raw_disk_registry = std::fs::read_to_string(Registry::path(&self.home))
                    .map_err(HubError::from)
                    .and_then(|text| Registry::parse(&text));
                match raw_disk_registry {
                    Ok(actual) if actual == next => {}
                    Ok(actual) if actual == current => return Err(save_error),
                    Ok(actual) => {
                        return Err(HubError::InvalidRegistry(format!(
                            "registry commit produced an unexpected disk image: {actual:?}"
                        )));
                    }
                    Err(_) => {
                        return Err(HubError::InvalidRegistry(format!(
                            "registry save failed ({save_error}) and its commit state could not be verified ({verification_error})"
                        )));
                    }
                }
            }
            (Ok(()), Err(_)) => {
                // A successful save returns only after the atomic replace and
                // destination hardening. The just-serialized `next` image is
                // therefore committed even if the redundant reload fails;
                // finish publication instead of retaining old in-memory state.
            }
        }
        let mut cache_error = None;
        for agent_id in &affected {
            if let Err(error) = self.store().delete_agent_cache(agent_id)
                && cache_error.is_none()
            {
                cache_error = Some(error);
            }
        }
        // The disk image has already been parsed and matched above. If the
        // metadata/hash read now fails, publish the committed registry but use
        // a fail-closed sentinel so subsequent mutations require restart.
        let saved_fingerprint = Registry::fingerprint(&self.home).unwrap_or(None);
        *self.registry.write() = next;
        *self.registry_fingerprint.write() = saved_fingerprint;
        self.registry_epoch.fetch_add(1, Ordering::AcqRel);
        for agent_id in &affected {
            self.ctx.revoke_agent_locked(agent_id);
            handles.remove(agent_id);
        }
        drop(init_guards);
        drop(generation_writers);
        match cache_error {
            Some(error) => Err(HubError::other(format!(
                "registry mutation committed, but derived agent cache invalidation failed: {error}"
            ))),
            None => Ok(()),
        }
    }

    fn lock_agents_idle(
        &self,
        agent_ids: &[String],
    ) -> Result<parking_lot::MutexGuard<'_, OperationMap>, HubError> {
        let operations = self.operations.lock();
        reject_active_agents(&operations, agent_ids)?;
        Ok(operations)
    }

    pub(super) fn agent_config(&self, agent_id: &str) -> Result<AgentEndpointConfig, HubError> {
        self.registry
            .read()
            .agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| HubError::not_found("agent", agent_id))
    }
}
