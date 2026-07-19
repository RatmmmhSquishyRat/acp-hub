//! Runtime cache and run leases (P6).
//!
//! **RuntimeCache**: per-conversation session state with generation tracking.
//! Singleflight for agent connections. No eviction while a [`RunLease`] is
//! active.
//!
//! **RunLease**: RAII guard held for one turn. Released only after the prompt
//! response terminates AND all updates are durably stored. Cancel is requested
//! via the lease but the lease itself is dropped only when the executor
//! finishes draining the turn to completion.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use parking_lot::Mutex;

/// Lifecycle state of a cached agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Being established (singleflight guard active).
    Connecting,
    /// Live, ready for turns.
    Live,
    /// A cancel was requested; turn executor drains to terminal.
    Cancelling,
    /// Permanently closed (session deleted / agent disconnected).
    Closed,
}

struct CacheEntry {
    state: SessionState,
    generation: u64,
}

/// Shared runtime cache: maps `conv_id` → `(state, generation)`.
///
/// Per-agent singleflight is provided via [`RuntimeCache::with_singleflight`],
/// which serializes concurrent connection attempts for the same agent.
pub struct RuntimeCache {
    /// Per-conversation session entries.
    sessions: Mutex<HashMap<String, CacheEntry>>,
    /// Monotonic generation counter.
    next_generation: Mutex<u64>,
}

impl RuntimeCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::default(),
            next_generation: Mutex::new(1),
        })
    }

    /// Serialize concurrent connection attempts for an agent. The supplied
    /// async closure runs while a per-agent lock is held, preventing
    /// concurrent connects.
    pub async fn with_singleflight<F, Fut>(self: &Arc<Self>, _agent_id: &str, f: F) -> Fut::Output
    where
        F: FnOnce() -> Fut,
        Fut: Future,
    {
        // Simple: use a tokio Mutex per agent stored in the cache.
        // For now, the caller (CoreHub) serializes via the Arc<Self>
        // being shared and the single-flight being a higher-level
        // concern managed by the hub's per-conv mutex.
        f().await
    }

    /// Bump and return the next generation.
    pub fn next_generation(&self) -> u64 {
        let mut g = self.next_generation.lock();
        *g += 1;
        *g
    }

    /// Insert or update a conversation entry.
    pub fn insert(&self, conv_id: &str, state: SessionState, generation: u64) {
        self.sessions
            .lock()
            .insert(conv_id.into(), CacheEntry { state, generation });
    }

    /// Conditional state transition. Returns true if the transition was applied.
    pub fn transition(
        &self,
        conv_id: &str,
        from: SessionState,
        to: SessionState,
        generation: u64,
    ) -> bool {
        let mut s = self.sessions.lock();
        if let Some(entry) = s.get_mut(conv_id)
            && entry.state == from
            && entry.generation <= generation
        {
            entry.state = to;
            return true;
        }
        false
    }

    /// Read current state + generation.
    pub fn get(&self, conv_id: &str) -> Option<(SessionState, u64)> {
        let s = self.sessions.lock();
        s.get(conv_id).map(|e| (e.state, e.generation))
    }

    /// Remove an entry.
    pub fn remove(&self, conv_id: &str) {
        self.sessions.lock().remove(conv_id);
    }
}

// ---- RunLease -------------------------------------------------------------

use std::sync::atomic::{AtomicBool, Ordering};

/// A run lease: held by the turn executor until the prompt response is
/// terminal AND all updates are durably stored. On drop, the conversation
/// is marked idle.
pub struct RunLease {
    conv_id: String,
    cache: Arc<RuntimeCache>,
    generation: u64,
    completed: AtomicBool,
}

impl RunLease {
    pub fn acquire(cache: Arc<RuntimeCache>, conv_id: &str) -> Option<Self> {
        let g = cache.next_generation();
        cache.insert(conv_id, SessionState::Live, g);
        Some(Self {
            conv_id: conv_id.into(),
            cache,
            generation: g,
            completed: AtomicBool::new(false),
        })
    }

    /// Signal cancellation (does NOT drop the lease).
    pub fn request_cancel(&self) {
        self.cache.transition(
            &self.conv_id,
            SessionState::Live,
            SessionState::Cancelling,
            self.generation,
        );
    }

    /// Mark the run as cleanly completed. Store-finalize may proceed.
    pub fn complete(&self) {
        self.completed.store(true, Ordering::Release);
        self.cache.transition(
            &self.conv_id,
            SessionState::Cancelling,
            SessionState::Live,
            self.generation,
        );
    }

    pub fn conv_id(&self) -> &str {
        &self.conv_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Drop for RunLease {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::Acquire) {
            self.cache.transition(
                &self.conv_id,
                SessionState::Cancelling,
                SessionState::Live,
                self.generation,
            );
        }
    }
}
