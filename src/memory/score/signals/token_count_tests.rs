use super::*;

#[test]
fn tiny_is_zero() {
    assert_eq!(score(0), 0.0);
    assert_eq!(score(5), 0.0);
    assert_eq!(score(9), 0.0);
}

#[test]
fn ramp_up_linear() {
    // score(MIN) = 0, score(RAMP_LOW) = 1.0
    assert!((score(TOKEN_MIN) - 0.0).abs() < 1e-4);
    assert!((score(TOKEN_RAMP_LOW) - 1.0).abs() < 1e-4);
    // midpoint ~0.5
    let mid = TOKEN_MIN + (TOKEN_RAMP_LOW - TOKEN_MIN) / 2;
    assert!((score(mid) - 0.5).abs() < 0.05);
}

#[test]
fn plateau_is_one() {
    assert_eq!(score(200), 1.0);
    assert_eq!(score(1000), 1.0);
    assert_eq!(score(TOKEN_RAMP_HIGH), 1.0);
}

#[test]
fn ramp_down_to_half() {
    assert!((score(TOKEN_MAX) - 0.5).abs() < 1e-4);
    assert_eq!(score(TOKEN_MAX + 10_000), 0.5);
}

#[test]
fn monotonic_in_bands() {
    // Strictly increasing on the up-ramp
    assert!(score(TOKEN_MIN + 1) < score(TOKEN_RAMP_LOW - 1));
    // Strictly decreasing on the down-ramp
    assert!(score(TOKEN_RAMP_HIGH + 1) > score(TOKEN_MAX - 1));
}
