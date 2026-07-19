use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use super::conversation::{ReplayMethod, wrap_load_failure};
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
    /// discovered sessions into the projection (FAQ: "全量记录静态资源snapshot").
    pub async fn list_agent_sessions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<agent_client_protocol::schema::v1::SessionInfo>, HubError> {
        let handle = self.agent_handle(agent_id).await?;
        let result = self
            .request_agent(agent_id, &handle, |reply| AgentCommand::ListSessions {
                cwd: None,
                reply,
            })
            .await?;
        // Auto-import each discovered session into the projection.
        for info in &result.sessions {
            let sid = info.session_id.to_string();
            let title = info.title.as_deref();
            let cwd = info.cwd.to_str().ok_or_else(|| {
                HubError::other(format!(
                    "agent session {sid} returned a cwd that is not valid UTF-8"
                ))
            })?;
            let dirs: Vec<String> = info
                .additional_directories
                .iter()
                .map(|d| {
                    d.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                        HubError::other(format!(
                            "agent session {sid} returned an additional directory that is not valid UTF-8"
                        ))
                    })
                })
                .collect::<Result<_, _>>()?;
            let conv_id =
                self.store()
                    .upsert_agent_session(agent_id, &sid, title, Some(cwd), &dirs)?;
            // Load messages via session/load (Layer 1) if supported.
            if handle.capabilities.load_session {
                let conv = self
                    .store()
                    .conversation(&conv_id)?
                    .ok_or_else(|| HubError::not_found("conversation", &conv_id))?;
                self.refresh_session_projection_external(
                    &conv,
                    info.cwd.clone(),
                    ReplayMethod::Load,
                )
                .await
                .map_err(|source| {
                    wrap_load_failure(agent_id.to_string(), conv.id.clone(), sid.clone(), source)
                })?;
            }
        }
        Ok(result.sessions)
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
        let agent_config = registry
            .agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| HubError::not_found("agent", agent_id))?;
        let endpoint = conductor::build_endpoint_component(&registry, agent_id)?;
        let rx = spawn_agent_connection_with_flow(
            endpoint.component,
            agent_id.to_string(),
            agent_config,
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
        let disk_fingerprint = Registry::fingerprint(&self.home)?;
        let expected_fingerprint = *self.registry_fingerprint.read();
        if disk_fingerprint != expected_fingerprint {
            return Err(HubError::InvalidRegistry(
                "agents.json changed outside the running daemon; restart the daemon to load the external edit before applying registry mutations"
                    .to_string(),
            ));
        }

        let current = self.registry.read().clone();
        let mut next = current.clone();
        mutate(&mut next)?;
        let affected = affected_agent_ids(&current, &next);
        let mut generation_writers = Vec::with_capacity(affected.len());
        for agent_id in &affected {
            generation_writers.push(self.ctx.agent_generation_writer(agent_id).await);
        }
        let mut handles = self.handles.lock().await;
        let mut inits = self.handle_inits.lock().await;
        let _operations = self.lock_agents_idle(&affected)?;

        next.save(&self.home)?;
        let saved_fingerprint = Registry::fingerprint(&self.home)?;
        *self.registry.write() = next;
        *self.registry_fingerprint.write() = saved_fingerprint;
        for agent_id in affected {
            self.ctx.revoke_agent_locked(&agent_id);
            handles.remove(&agent_id);
            inits.remove(&agent_id);
        }
        drop(generation_writers);
        Ok(())
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
