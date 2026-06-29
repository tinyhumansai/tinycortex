use super::*;

#[test]
fn score_signals_default_to_zero() {
    let signals = ScoreSignals::default();
    assert_eq!(signals.token_count, 0.0);
    assert_eq!(signals.unique_words, 0.0);
    assert_eq!(signals.metadata_weight, 0.0);
    assert_eq!(signals.source_weight, 0.0);
    assert_eq!(signals.interaction, 0.0);
    assert_eq!(signals.entity_density, 0.0);
    assert_eq!(signals.llm_importance, 0.0);
}

#[test]
fn signal_weights_default_match_expected_priorities() {
    let weights = SignalWeights::default();
    assert_eq!(weights.token_count, 1.0);
    assert_eq!(weights.unique_words, 1.0);
    assert_eq!(weights.metadata_weight, 1.5);
    assert_eq!(weights.source_weight, 1.5);
    assert_eq!(weights.interaction, 3.0);
    assert_eq!(weights.entity_density, 1.0);
    assert_eq!(weights.llm_importance, 0.0);
}

#[test]
fn with_llm_enabled_only_changes_llm_weight() {
    let default = SignalWeights::default();
    let enabled = SignalWeights::with_llm_enabled();
    assert_eq!(enabled.token_count, default.token_count);
    assert_eq!(enabled.unique_words, default.unique_words);
    assert_eq!(enabled.metadata_weight, default.metadata_weight);
    assert_eq!(enabled.source_weight, default.source_weight);
    assert_eq!(enabled.interaction, default.interaction);
    assert_eq!(enabled.entity_density, default.entity_density);
    assert_eq!(enabled.llm_importance, 2.0);
}
