//! Per-database circuit breaker for repeated connection-init failures.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

pub(crate) const CB_THRESHOLD: u32 = 3;
pub(crate) const CB_COOLDOWN: Duration = Duration::from_secs(30);

pub(super) struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    tripped: AtomicBool,
    last_trip: Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            tripped: AtomicBool::new(false),
            last_trip: Mutex::new(None),
        }
    }

    pub fn record_success(&self) -> bool {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        *self.last_trip.lock() = None;
        self.tripped.swap(false, Ordering::Relaxed)
    }

    pub fn record_failure(&self) -> bool {
        let count = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if count >= CB_THRESHOLD && !self.tripped.swap(true, Ordering::Relaxed) {
            *self.last_trip.lock() = Some(Instant::now());
            return true;
        }
        if self.tripped.load(Ordering::Relaxed) {
            *self.last_trip.lock() = Some(Instant::now());
        }
        false
    }

    pub fn is_open(&self) -> bool {
        if !self.tripped.load(Ordering::Relaxed) {
            return false;
        }
        matches!(*self.last_trip.lock(), Some(tripped) if tripped.elapsed() < CB_COOLDOWN)
    }
}
