use super::*;

#[test]
fn retrieval_limits_validate_defaults_and_reject_invalid_bounds() {
    let mut limits = RetrievalLimits::default();
    limits.validate().unwrap();
    limits.default_limit = 0;
    assert!(limits.validate().is_err());
    limits = RetrievalLimits::default();
    limits.max_limit = limits.default_limit - 1;
    assert!(limits.validate().is_err());
    limits = RetrievalLimits::default();
    limits.default_graph_hops = limits.max_graph_hops + 1;
    assert!(limits.validate().is_err());
    limits = RetrievalLimits::default();
    limits.freshness_half_life_days = f64::NAN;
    assert!(limits.validate().is_err());
}

#[test]
fn scoring_policy_validates_threshold_ranges_and_order() {
    let mut policy = ScoringPolicyConfig::default();
    policy.validate().unwrap();
    policy.drop_threshold = 1.1;
    assert!(policy.validate().is_err());
    policy = ScoringPolicyConfig::default();
    policy.definite_drop_threshold = 0.9;
    policy.definite_keep_threshold = 0.1;
    assert!(policy.validate().is_err());
}

#[test]
fn queue_policy_validates_timings_and_limits() {
    let mut queue = QueueConfig::default();
    queue.validate().unwrap();
    queue.retry_cap_ms = queue.retry_base_ms - 1;
    assert!(queue.validate().is_err());
    queue = QueueConfig::default();
    queue.max_attempts = 0;
    assert!(queue.validate().is_err());
    queue = QueueConfig::default();
    queue.llm_permits = 0;
    assert!(queue.validate().is_err());
    queue = QueueConfig::default();
    queue.max_defer_age_ms = 0;
    assert!(queue.validate().is_err());
}
