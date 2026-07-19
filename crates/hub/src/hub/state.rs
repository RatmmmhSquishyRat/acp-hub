use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;

use crate::acp::AgentHandle;
use crate::callbacks::HubCtx;
use crate::daemon::ActivityTracker;
use crate::endpoint::{FileFingerprint, Registry};
use crate::error::HubError;
use crate::runtime::RuntimeCache;
use crate::store::Store;
use parking_lot::{Mutex as SyncMutex, RwLock};
use tokio::sync::Mutex;
#[cfg(test)]
use tokio::sync::oneshot;
use uuid::Uuid;
#[cfg(test)]
pub(super) type AsyncTestGate = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[derive(Debug, Clone)]
pub(super) struct PromptOperation {
    pub(super) run_id: String,
    pub(super) agent_session_id: String,
    pub(super) cancel_requested: bool,
}

#[derive(Debug, Clone)]
pub(super) enum OperationKind {
    Prompt(PromptOperation),
    ExternalRun { run_id: String },
    Refresh,
    Delete,
    SetParam,
    SetMode,
    Close,
}

#[derive(Debug, Clone)]
pub(super) struct OperationEntry {
    pub(super) token: Uuid,
    pub(super) agent_id: String,
    pub(super) kind: OperationKind,
}

pub(super) type OperationMap = HashMap<String, OperationEntry>;

#[derive(Debug, Clone)]
pub(super) struct SessionIdentityEntry {
    token: Uuid,
    conv_id: String,
}

pub(super) type SessionIdentityKey = (String, String);
pub(super) type SessionIdentityMap = HashMap<SessionIdentityKey, SessionIdentityEntry>;

pub(super) struct SessionIdentityLease {
    key: SessionIdentityKey,
    token: Uuid,
    identities: Arc<SyncMutex<SessionIdentityMap>>,
}

impl SessionIdentityLease {
    pub(super) fn acquire(
        identities: Arc<SyncMutex<SessionIdentityMap>>,
        agent_id: &str,
        agent_session_id: &str,
        conv_id: &str,
    ) -> Result<Self, HubError> {
        let key = (agent_id.to_string(), agent_session_id.to_string());
        let token = Uuid::new_v4();
        {
            let mut active = identities.lock();
            if let Some(entry) = active.get(&key) {
                return Err(HubError::Conflict(entry.conv_id.clone()));
            }
            active.insert(
                key.clone(),
                SessionIdentityEntry {
                    token,
                    conv_id: conv_id.to_string(),
                },
            );
        }
        Ok(Self {
            key,
            token,
            identities,
        })
    }
}

impl Drop for SessionIdentityLease {
    fn drop(&mut self) {
        let mut active = self.identities.lock();
        if active
            .get(&self.key)
            .is_some_and(|entry| entry.token == self.token)
        {
            active.remove(&self.key);
        }
    }
}

pub(super) struct OperationLease {
    pub(super) conv_id: String,
    pub(super) token: Uuid,
    pub(super) operations: Arc<SyncMutex<OperationMap>>,
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        release_operation(&self.operations, &self.conv_id, self.token);
    }
}

fn release_operation(operations: &SyncMutex<OperationMap>, conv_id: &str, token: Uuid) {
    let mut operations = operations.lock();
    if operations
        .get(conv_id)
        .is_some_and(|entry| entry.token == token)
    {
        operations.remove(conv_id);
    }
}

pub(super) struct ReplayLockEntry {
    pub(super) lock: Arc<Mutex<()>>,
    pub(super) users: usize,
}

pub(super) type ReplayLockMap = HashMap<String, ReplayLockEntry>;

pub(super) struct ReplayPruneGuard {
    pub(super) conv_id: String,
    pub(super) replay_lock: Arc<Mutex<()>>,
    replay_locks: Arc<SyncMutex<ReplayLockMap>>,
}

impl ReplayPruneGuard {
    pub(super) fn acquire(conv_id: &str, replay_locks: Arc<SyncMutex<ReplayLockMap>>) -> Self {
        let replay_lock = {
            let mut locks = replay_locks.lock();
            let entry = locks
                .entry(conv_id.to_string())
                .or_insert_with(|| ReplayLockEntry {
                    lock: Arc::new(Mutex::new(())),
                    users: 0,
                });
            entry.users += 1;
            Arc::clone(&entry.lock)
        };
        Self {
            conv_id: conv_id.to_string(),
            replay_lock,
            replay_locks,
        }
    }
}

impl Drop for ReplayPruneGuard {
    fn drop(&mut self) {
        let mut locks = self.replay_locks.lock();
        let remove = locks
            .get_mut(&self.conv_id)
            .filter(|entry| Arc::ptr_eq(&entry.lock, &self.replay_lock))
            .is_some_and(|entry| {
                debug_assert!(entry.users > 0);
                entry.users -= 1;
                entry.users == 0
            });
        if remove {
            locks.remove(&self.conv_id);
        }
    }
}

/// Daemon-internal Hub engine.
///
/// The projection store has one owner: [`HubCtx`]. CoreHub reaches it through
/// [`HubCtx::store`] so callback-captured updates and direct RPC reads/writes
/// always use the same SQLite connection.
pub struct CoreHub {
    pub(super) home: PathBuf,
    pub(super) registry: RwLock<Registry>,
    pub(super) runtime: Arc<RuntimeCache>,
    pub(super) ctx: Arc<HubCtx>,
    pub(super) handles: Mutex<HashMap<String, Arc<AgentHandle>>>,
    pub(super) handle_inits: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(super) replay_locks: Arc<SyncMutex<ReplayLockMap>>,
    pub(super) session_identities: Arc<SyncMutex<SessionIdentityMap>>,
    pub(super) operations: Arc<SyncMutex<OperationMap>>,
    #[cfg(test)]
    pub(super) cancel_snapshot_gate: SyncMutex<Option<AsyncTestGate>>,
    #[cfg(test)]
    pub(super) refresh_publish_gate: SyncMutex<Option<AsyncTestGate>>,
    #[cfg(test)]
    pub(super) handle_publish_gate: SyncMutex<Option<AsyncTestGate>>,
    #[cfg(test)]
    pub(super) registry_save_fail_once: AtomicBool,
    #[cfg(test)]
    pub(super) registry_verify_fail_once: AtomicBool,
    pub(super) registry_mutation: Mutex<()>,
    pub(super) registry_fingerprint: RwLock<Option<FileFingerprint>>,
    pub(super) registry_epoch: AtomicU64,
    pub(super) activity: Arc<ActivityTracker>,
}

impl CoreHub {
    /// Build a CoreHub from already-loaded registry and store state.
    pub fn new(
        home: impl AsRef<Path>,
        registry: Registry,
        store: Store,
        activity: Arc<ActivityTracker>,
    ) -> Self {
        let home = home.as_ref().to_path_buf();
        let registry_fingerprint = Registry::fingerprint(&home).unwrap_or(None);
        let ctx = HubCtx::new(store);
        ctx.set_activity_tracker(Arc::clone(&activity));
        Self {
            home,
            registry: RwLock::new(registry),
            runtime: RuntimeCache::new(),
            ctx,
            handles: Mutex::default(),
            handle_inits: Mutex::default(),
            replay_locks: Arc::new(SyncMutex::new(HashMap::new())),
            session_identities: Arc::new(SyncMutex::new(HashMap::new())),
            operations: Arc::new(SyncMutex::new(HashMap::new())),
            #[cfg(test)]
            cancel_snapshot_gate: SyncMutex::default(),
            #[cfg(test)]
            refresh_publish_gate: SyncMutex::default(),
            #[cfg(test)]
            handle_publish_gate: SyncMutex::default(),
            #[cfg(test)]
            registry_save_fail_once: AtomicBool::new(false),
            #[cfg(test)]
            registry_verify_fail_once: AtomicBool::new(false),
            registry_mutation: Mutex::default(),
            registry_fingerprint: RwLock::new(registry_fingerprint),
            registry_epoch: AtomicU64::new(0),
            activity,
        }
    }

    /// Load registry and store from `home`.
    pub fn open(home: impl AsRef<Path>) -> Result<Self, HubError> {
        let home = home.as_ref();
        let registry = Registry::load(home)?;
        let store = Store::open(home)?;
        Ok(Self::new(
            home,
            registry,
            store,
            Arc::new(ActivityTracker::new()),
        ))
    }

    /// Access the callback context used by agent connections.
    pub fn ctx(&self) -> Arc<HubCtx> {
        Arc::clone(&self.ctx)
    }

    /// Access the runtime cache.
    pub fn runtime(&self) -> Arc<RuntimeCache> {
        Arc::clone(&self.runtime)
    }

    /// Access the single projection store owned by the callback context.
    pub fn store(&self) -> &Store {
        self.ctx.store()
    }
}

impl CoreHub {
    pub(super) fn reserve_session_identity(
        &self,
        agent_id: &str,
        agent_session_id: &str,
        conv_id: &str,
    ) -> Result<SessionIdentityLease, HubError> {
        SessionIdentityLease::acquire(
            Arc::clone(&self.session_identities),
            agent_id,
            agent_session_id,
            conv_id,
        )
    }

    pub(super) fn reserve_operation(
        &self,
        conv_id: &str,
        agent_id: &str,
        kind: OperationKind,
    ) -> Result<OperationLease, HubError> {
        let mut operations = self.operations.lock();
        if operations.contains_key(conv_id) {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        if let OperationKind::Prompt(_) = &kind
            && operations.values().any(|entry| {
                entry.agent_id == agent_id && matches!(&entry.kind, OperationKind::Prompt(_))
            })
        {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        let token = Uuid::new_v4();
        operations.insert(
            conv_id.to_string(),
            OperationEntry {
                token,
                agent_id: agent_id.to_string(),
                kind,
            },
        );
        Ok(OperationLease {
            conv_id: conv_id.to_string(),
            token,
            operations: Arc::clone(&self.operations),
        })
    }

    pub(super) fn reserve_external_run(
        &self,
        conv_id: &str,
        agent_id: &str,
        run_id: &str,
    ) -> Result<Uuid, HubError> {
        let mut operations = self.operations.lock();
        if operations.contains_key(conv_id) {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        let token = Uuid::new_v4();
        operations.insert(
            conv_id.to_string(),
            OperationEntry {
                token,
                agent_id: agent_id.to_string(),
                kind: OperationKind::ExternalRun {
                    run_id: run_id.to_string(),
                },
            },
        );
        Ok(token)
    }

    pub(super) fn release_operation(&self, conv_id: &str, token: Uuid) {
        release_operation(&self.operations, conv_id, token);
    }
}
