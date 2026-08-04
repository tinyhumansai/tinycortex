//! Unit tests for [`super::MemoryHealth`].
//!
//! These pin the two properties the host's memory adapter depends on: the
//! variant set is closed and small enough for a lossless `match` into the
//! kernel's generic `DriverHealth`, and the wire form carries a stable
//! `status` discriminant plus a `reason`.

use super::*;
use serde_json::json;

#[test]
fn ready_has_no_reason_and_is_usable() {
    let health = MemoryHealth::Ready;
    assert_eq!(health.as_str(), "ready");
    assert_eq!(health.reason(), None);
    assert!(health.is_usable());
    assert_eq!(health.to_string(), "ready");
}

#[test]
fn degraded_carries_a_reason_and_is_still_usable() {
    let health = MemoryHealth::degraded("vector index rebuilding");
    assert_eq!(health.as_str(), "degraded");
    assert_eq!(health.reason(), Some("vector index rebuilding"));
    // A degraded driver is still the bound driver.
    assert!(health.is_usable());
    assert_eq!(health.to_string(), "degraded: vector index rebuilding");
}

#[test]
fn down_carries_a_reason_and_is_not_usable() {
    let health = MemoryHealth::down("connection refused");
    assert_eq!(health.as_str(), "down");
    assert_eq!(health.reason(), Some("connection refused"));
    assert!(!health.is_usable());
    assert_eq!(health.to_string(), "down: connection refused");
}

#[test]
fn health_serializes_with_a_stable_status_discriminant() {
    assert_eq!(
        serde_json::to_value(MemoryHealth::Ready).unwrap(),
        json!({ "status": "ready" })
    );
    assert_eq!(
        serde_json::to_value(MemoryHealth::degraded("slow")).unwrap(),
        json!({ "status": "degraded", "reason": "slow" })
    );
    assert_eq!(
        serde_json::to_value(MemoryHealth::down("gone")).unwrap(),
        json!({ "status": "down", "reason": "gone" })
    );
}

#[test]
fn health_round_trips_through_serde() {
    for health in [
        MemoryHealth::Ready,
        MemoryHealth::degraded("reindexing"),
        MemoryHealth::down("auth expired"),
    ] {
        let encoded = serde_json::to_string(&health).unwrap();
        let decoded: MemoryHealth = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, health);
    }
}

#[test]
fn health_constructors_match_their_variants() {
    assert_eq!(
        MemoryHealth::degraded("x"),
        MemoryHealth::Degraded {
            reason: "x".to_string()
        }
    );
    assert_eq!(
        MemoryHealth::down("y"),
        MemoryHealth::Down {
            reason: "y".to_string()
        }
    );
}

#[test]
fn degraded_without_a_reason_is_rejected_on_the_wire() {
    // `reason` is mandatory: a degraded/down driver that explains nothing is
    // useless in status output, so the contract refuses to decode it.
    assert!(serde_json::from_value::<MemoryHealth>(json!({ "status": "degraded" })).is_err());
    assert!(serde_json::from_value::<MemoryHealth>(json!({ "status": "down" })).is_err());
}
