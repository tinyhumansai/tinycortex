//! Per-database circuit breaker for repeated connection-init failures.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

pub(crate) const CB_THRESHOLD: u32 = 3;
pub(crate) const CB_COOLDOWN: Duration = Duration::from_secs(30);

struct BreakerState {
    consecutive_failures: u32,
    tripped: bool,
    last_trip: Option<Instant>,
}

pub(super) struct CircuitBreaker {
    state: Mutex<BreakerState>,
}

impl CircuitBreaker {
    /// Create a closed breaker with no recorded initialization failures.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(BreakerState {
                consecutive_failures: 0,
                tripped: false,
                last_trip: None,
            }),
        }
    }

    /// Reset failure state after a successful initialization.
    ///
    /// Returns `true` when this call recovered a previously tripped breaker.
    pub fn record_success(&self) -> bool {
        let mut state = self.state.lock();
        let was_tripped = state.tripped;
        state.consecutive_failures = 0;
        state.tripped = false;
        state.last_trip = None;
        was_tripped
    }

    /// Record one initialization failure and trip at [`CB_THRESHOLD`].
    ///
    /// Returns `true` only for the failure that transitions the breaker from
    /// closed to tripped. Further failures refresh the cooldown timestamp.
    pub fn record_failure(&self) -> bool {
        let mut state = self.state.lock();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        let just_tripped = state.consecutive_failures >= CB_THRESHOLD && !state.tripped;
        if state.consecutive_failures >= CB_THRESHOLD {
            state.tripped = true;
            state.last_trip = Some(Instant::now());
        }
        just_tripped
    }

    /// Return whether the breaker is tripped and still within its cooldown.
    ///
    /// Once [`CB_COOLDOWN`] elapses this returns `false`, allowing one
    /// serialized initialization attempt; success resets the breaker and
    /// failure refreshes the cooldown.
    pub fn is_open(&self) -> bool {
        let state = self.state.lock();
        if !state.tripped {
            return false;
        }
        matches!(state.last_trip, Some(tripped) if tripped.elapsed() < CB_COOLDOWN)
    }
}
