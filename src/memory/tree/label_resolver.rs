//! Entity/topic label resolution for newly sealed summaries.

use anyhow::{Context, Result};
use std::collections::BTreeSet;

use super::summarise::SummaryInput;
use super::types::LabelStrategy;
use crate::memory::score::resolver::canonicalise;

/// Resolve `entities` and `topics` for a freshly-summarised node.
pub(super) async fn resolve_labels(
    strategy: &LabelStrategy,
    inputs: &[SummaryInput],
    summary_content: &str,
) -> Result<(Vec<String>, Vec<String>)> {
    match strategy {
        LabelStrategy::ExtractFromContent(extractor) => {
            let extracted = extractor
                .extract(summary_content)
                .await
                .context("seal-time extractor failed")?;
            let canonical = canonicalise(&extracted);
            let mut entities: Vec<String> = canonical
                .into_iter()
                .map(|c| c.canonical_id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            entities.sort();
            let mut topics: Vec<String> = extracted
                .topics
                .into_iter()
                .map(|t| t.label)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            topics.sort();
            Ok((entities, topics))
        }
        LabelStrategy::UnionFromChildren => {
            let mut entities: BTreeSet<String> = BTreeSet::new();
            let mut topics: BTreeSet<String> = BTreeSet::new();
            for inp in inputs {
                entities.extend(inp.entities.iter().cloned());
                topics.extend(inp.topics.iter().cloned());
            }
            Ok((entities.into_iter().collect(), topics.into_iter().collect()))
        }
        LabelStrategy::Empty => Ok((Vec::new(), Vec::new())),
    }
}
