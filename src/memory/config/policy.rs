//! Declarative scoring, retrieval, and queue policies.

use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_MAX_DEFER_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// Runtime bounds shared by retrieval entry points.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalLimits {
    /// Default number of returned hits.
    pub default_limit: usize,
    /// Default number of entity-search matches.
    pub search_default_limit: usize,
    /// Default number of structural time-window cover nodes.
    pub cover_default_limit: usize,
    /// Hard cap on caller-supplied result limits.
    pub max_limit: usize,
    /// Candidate occurrences loaded for graph routing.
    pub occurrence_lookup_limit: usize,
    /// Maximum graph traversal depth.
    pub max_graph_hops: u32,
    /// Default graph traversal depth.
    pub default_graph_hops: u32,
    /// Entity-index candidates considered by topic retrieval.
    pub topic_lookup_limit: usize,
    /// Page size for complete time-window chunk scans.
    pub window_chunk_page_size: usize,
    /// Maximum raw leaves hydrated in one public fetch.
    pub fetch_batch_limit: usize,
    /// Freshness signal half-life in days.
    pub freshness_half_life_days: f64,
}

impl Default for RetrievalLimits {
    fn default() -> Self {
        Self {
            default_limit: 10,
            search_default_limit: 5,
            cover_default_limit: 200,
            max_limit: 100,
            occurrence_lookup_limit: 500,
            max_graph_hops: 4,
            default_graph_hops: 2,
            topic_lookup_limit: 200,
            window_chunk_page_size: 5_000,
            fetch_batch_limit: 20,
            freshness_half_life_days: 7.0,
        }
    }
}

impl RetrievalLimits {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.default_limit > 0 && self.search_default_limit > 0 && self.cover_default_limit > 0,
            "retrieval default limits must be positive"
        );
        anyhow::ensure!(
            self.max_limit >= self.default_limit,
            "retrieval.limits.max_limit must cover the default"
        );
        anyhow::ensure!(
            self.occurrence_lookup_limit > 0
                && self.topic_lookup_limit > 0
                && self.window_chunk_page_size > 0
                && self.fetch_batch_limit > 0,
            "retrieval candidate and page limits must be positive"
        );
        anyhow::ensure!(
            self.default_graph_hops > 0 && self.default_graph_hops <= self.max_graph_hops,
            "retrieval graph hop bounds are invalid"
        );
        anyhow::ensure!(
            self.freshness_half_life_days.is_finite() && self.freshness_half_life_days > 0.0,
            "retrieval freshness half-life must be positive"
        );
        Ok(())
    }
}

/// Serializable scoring policy; extractor implementations are host-injected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoringPolicyConfig {
    /// Per-signal weights.
    pub weights: crate::memory::score::signals::SignalWeights,
    /// Final admission threshold.
    pub drop_threshold: f32,
    /// Cheap-score threshold for unconditional admission.
    pub definite_keep_threshold: f32,
    /// Cheap-score threshold for unconditional rejection.
    pub definite_drop_threshold: f32,
}

impl Default for ScoringPolicyConfig {
    fn default() -> Self {
        Self {
            weights: crate::memory::score::signals::SignalWeights::default(),
            drop_threshold: crate::memory::score::DEFAULT_DROP_THRESHOLD,
            definite_keep_threshold: crate::memory::score::DEFAULT_DEFINITE_KEEP,
            definite_drop_threshold: crate::memory::score::DEFAULT_DEFINITE_DROP,
        }
    }
}

impl ScoringPolicyConfig {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.drop_threshold.is_finite() && (0.0..=1.0).contains(&self.drop_threshold),
            "scoring.drop_threshold must be between zero and one"
        );
        anyhow::ensure!(
            self.definite_drop_threshold.is_finite()
                && self.definite_keep_threshold.is_finite()
                && self.definite_drop_threshold <= self.definite_keep_threshold,
            "scoring definite thresholds are invalid"
        );
        Ok(())
    }
}

/// Queue locking, retry backoff, and LLM concurrency policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    /// Lease duration for claimed jobs.
    pub lock_duration_ms: i64,
    /// Initial retry delay.
    pub retry_base_ms: i64,
    /// Maximum retry delay.
    pub retry_cap_ms: i64,
    /// Default number of attempts per job.
    pub max_attempts: u32,
    /// Concurrent LLM jobs permitted per process.
    pub llm_permits: usize,
    /// Maximum age of a repeatedly deferred job.
    pub max_defer_age_ms: i64,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            lock_duration_ms: 5 * 60 * 1_000,
            retry_base_ms: 60 * 1_000,
            retry_cap_ms: 60 * 60 * 1_000,
            max_attempts: 5,
            llm_permits: 1,
            max_defer_age_ms: DEFAULT_MAX_DEFER_AGE_MS,
        }
    }
}

impl QueueConfig {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.lock_duration_ms > 0
                && self.retry_base_ms > 0
                && self.retry_cap_ms >= self.retry_base_ms,
            "queue timing policy is invalid"
        );
        anyhow::ensure!(
            self.max_attempts > 0 && self.llm_permits > 0 && self.max_defer_age_ms > 0,
            "queue limits must be positive"
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
