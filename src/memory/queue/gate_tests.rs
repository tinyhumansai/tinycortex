use super::*;

#[test]
fn single_slot_gate_limits_to_one() {
    let gate = LlmGate::new(1);
    assert_eq!(gate.available_permits(), 1);
    let p = gate.try_acquire().expect("first permit available");
    assert_eq!(gate.available_permits(), 0);
    assert!(
        gate.try_acquire().is_none(),
        "no second permit while the first is held"
    );
    drop(p);
    assert_eq!(gate.available_permits(), 1);
    assert!(gate.try_acquire().is_some(), "permit freed after drop");
}

#[test]
fn multi_slot_gate_allows_n_then_blocks() {
    let gate = LlmGate::new(2);
    let _a = gate.try_acquire().unwrap();
    let _b = gate.try_acquire().unwrap();
    assert_eq!(gate.available_permits(), 0);
    assert!(gate.try_acquire().is_none());
}

#[test]
fn zero_permits_is_clamped_to_one() {
    let gate = LlmGate::new(0);
    assert_eq!(gate.available_permits(), 1);
    assert!(gate.try_acquire().is_some());
}

#[test]
fn acquire_returns_immediately_when_free() {
    let gate = LlmGate::new(1);
    let p = gate.acquire();
    assert_eq!(gate.available_permits(), 0);
    drop(p);
    assert_eq!(gate.available_permits(), 1);
}

#[test]
fn default_gate_is_single_slot() {
    let gate = LlmGate::default();
    assert_eq!(gate.available_permits(), DEFAULT_LLM_PERMITS);
}
