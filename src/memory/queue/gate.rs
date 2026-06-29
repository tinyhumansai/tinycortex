//! Global concurrency gate for LLM-bound jobs.
//!
//! OpenHuman delegated LLM concurrency to a process-wide single-slot semaphore
//! in `scheduler_gate` (shared across the queue worker, voice cleanup, triage,
//! and reflection). That module is not part of this crate's ported surface, and
//! the async runtime (`tokio`) is a dev-only dependency here, so this is a
//! self-contained, runtime-agnostic re-implementation built on `parking_lot`.
//!
//! [`acquire`] blocks until a permit is free and returns an RAII [`Permit`]
//! that returns the slot on drop. The deterministic [`try_acquire`] never
//! blocks — it is the seam tests use to assert the gate actually limits
//! concurrency. The worker holds a permit for the duration of an LLM-bound
//! handler (see [`JobKind::is_llm_bound`](crate::memory::queue::types::JobKind::is_llm_bound))
//! and releases it before settling the row.

use std::sync::Arc;

use parking_lot::{Condvar, Mutex};

/// Default number of concurrent LLM-bound jobs. One slot mirrors the upstream
/// single-slot semaphore (laptop-RAM safety for local models); raise it for
/// bandwidth-bound cloud backends.
pub const DEFAULT_LLM_PERMITS: usize = 1;

/// A counting gate. Cheap to clone (shared inner state).
#[derive(Clone)]
pub struct LlmGate {
    inner: Arc<Inner>,
}

struct Inner {
    available: Mutex<usize>,
    cond: Condvar,
}

/// RAII permit — returns its slot to the gate on drop.
pub struct Permit {
    inner: Arc<Inner>,
}

impl LlmGate {
    /// Build a gate with `permits` concurrent slots. `permits` of 0 is clamped
    /// to 1 so the gate can never deadlock the only worker.
    pub fn new(permits: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                available: Mutex::new(permits.max(1)),
                cond: Condvar::new(),
            }),
        }
    }

    /// Try to take a permit without blocking. Returns `None` when all slots are
    /// in use.
    pub fn try_acquire(&self) -> Option<Permit> {
        let mut avail = self.inner.available.lock();
        if *avail == 0 {
            return None;
        }
        *avail -= 1;
        Some(Permit {
            inner: self.inner.clone(),
        })
    }

    /// Block until a permit is free, then take it.
    pub fn acquire(&self) -> Permit {
        let mut avail = self.inner.available.lock();
        while *avail == 0 {
            self.inner.cond.wait(&mut avail);
        }
        *avail -= 1;
        Permit {
            inner: self.inner.clone(),
        }
    }

    /// Permits currently free (diagnostics / tests).
    pub fn available_permits(&self) -> usize {
        *self.inner.available.lock()
    }
}

impl Default for LlmGate {
    fn default() -> Self {
        Self::new(DEFAULT_LLM_PERMITS)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut avail = self.inner.available.lock();
        *avail += 1;
        self.inner.cond.notify_one();
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
