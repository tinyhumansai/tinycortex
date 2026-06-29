//! Unit tests for the circuit breaker (`super`).

use super::*;

/// `record_success` must only report a recovery transition (`true`) when it
/// actually clears a tripped breaker — the signal `get_or_init_connection` uses
/// to log recovery exactly once instead of on every successful call.
#[test]
fn record_success_announces_only_on_trip_to_healthy_transition() {
    let cb = CircuitBreaker::new();

    // Untripped breaker: a success is steady-state, not a transition.
    assert!(!cb.record_success());

    // Trip the breaker by crossing the failure threshold.
    let mut tripped = false;
    for _ in 0..CB_THRESHOLD {
        tripped = cb.record_failure();
    }
    assert!(tripped, "breaker should trip at CB_THRESHOLD failures");

    // First success after a trip is the recovery edge → announce once.
    assert!(cb.record_success());
    // Subsequent successes are steady-state → stay silent.
    assert!(!cb.record_success());
}

/// A freshly-tripped breaker reports open until the cooldown elapses.
#[test]
fn tripped_breaker_is_open_until_cooldown() {
    let cb = CircuitBreaker::new();
    for _ in 0..CB_THRESHOLD {
        cb.record_failure();
    }
    assert!(
        cb.is_open(),
        "breaker must be open immediately after tripping"
    );
    assert!(cb.record_success());
    assert!(!cb.is_open(), "a success clears the breaker");
}
