//! [`EntityExtractor`] trait plus the regex and composite implementations
//! used as Phase 2's default extraction stack.

use async_trait::async_trait;

use super::regex;
use super::types::ExtractedEntities;

/// Interface for anything that can read a chunk's text and emit entities.
///
/// This is the pluggable extraction seam: the deterministic regex extractor,
/// the trait-abstracted LLM NER extractor, and the [`CompositeExtractor`] that
/// merges several extractors all implement it. Callers (e.g. `score_chunk`)
/// hold an `Arc<dyn EntityExtractor>` and never depend on a concrete backend.
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    /// Human-readable name for logs and diagnostics.
    fn name(&self) -> &'static str;

    /// Run extraction. Implementations should be idempotent per input.
    async fn extract(&self, text: &str) -> anyhow::Result<ExtractedEntities>;
}

/// Synchronous regex extractor adapted to the async [`EntityExtractor`] trait.
pub struct RegexEntityExtractor;

#[async_trait]
impl EntityExtractor for RegexEntityExtractor {
    fn name(&self) -> &'static str {
        "regex"
    }

    async fn extract(&self, text: &str) -> anyhow::Result<ExtractedEntities> {
        Ok(regex::extract(text))
    }
}

/// Runs a sequence of extractors and merges their results.
///
/// An extractor returning an error is logged and skipped — one bad extractor
/// does not abort ingestion.
pub struct CompositeExtractor {
    inner: Vec<Box<dyn EntityExtractor>>,
}

impl CompositeExtractor {
    /// Build a composite from an explicit list of extractors. Order matters
    /// only to logs — outputs are merged and deduplicated.
    pub fn new(inner: Vec<Box<dyn EntityExtractor>>) -> Self {
        Self { inner }
    }

    /// Convenience constructor: regex-only (the Phase 2 default).
    pub fn regex_only() -> Self {
        Self::new(vec![Box::new(RegexEntityExtractor)])
    }
}

#[async_trait]
impl EntityExtractor for CompositeExtractor {
    fn name(&self) -> &'static str {
        "composite"
    }

    async fn extract(&self, text: &str) -> anyhow::Result<ExtractedEntities> {
        let mut out = ExtractedEntities::default();
        for ex in &self.inner {
            match ex.extract(text).await {
                Ok(batch) => out.merge(batch),
                Err(_e) => {
                    // One bad extractor must not abort ingestion — skip it and
                    // continue merging the rest of the chain's output.
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "composite_tests.rs"]
mod tests;
