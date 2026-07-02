//! Labeled-dataset format for retrieval evaluation.
//!
//! A [`Dataset`] is a corpus of [`Document`]s plus a set of [`QueryCase`]s, each
//! naming the document ids that count as relevant answers (binary relevance).
//! The format is plain JSON so datasets can be hand-authored, generated once and
//! frozen, or diffed in review. See `data/fixtures_v1.json` for the seed corpus.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

/// One retrievable document in the corpus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    /// Stable id referenced by [`QueryCase::relevant_ids`]. Unique within a set.
    pub id: String,
    /// Human-readable title (optional; surfaced in metadata for debugging).
    #[serde(default)]
    pub title: String,
    /// The document body that gets ingested and searched.
    pub text: String,
    /// Logical partition the document is stored under. Defaults to `"bench"`.
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

fn default_namespace() -> String {
    "bench".to_string()
}

/// One evaluation query with its ground-truth relevant document ids.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryCase {
    /// Stable id for this query (used in per-query result rows).
    pub id: String,
    /// The natural-language query text handed to the backend.
    pub query: String,
    /// Ids of documents that count as correct answers. Must be non-empty and
    /// each must reference an existing [`Document::id`].
    pub relevant_ids: Vec<String>,
    /// Restrict retrieval to a single namespace; `None` searches all.
    #[serde(default)]
    pub namespace: Option<String>,
}

impl QueryCase {
    /// The relevant ids as a set for metric computation.
    pub fn relevant_set(&self) -> HashSet<String> {
        self.relevant_ids.iter().cloned().collect()
    }
}

/// A complete labeled dataset: documents + queries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dataset {
    /// Short machine-readable name (e.g. `"fixtures_v1"`), echoed into reports.
    pub name: String,
    /// Free-form description of provenance and labeling method.
    #[serde(default)]
    pub description: String,
    /// The retrievable corpus.
    pub documents: Vec<Document>,
    /// The evaluation queries.
    pub queries: Vec<QueryCase>,
}

impl Dataset {
    /// Load and validate a dataset from a JSON file.
    pub fn from_json_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading dataset {}", path.display()))?;
        let dataset: Dataset = serde_json::from_str(&raw)
            .with_context(|| format!("parsing dataset {}", path.display()))?;
        dataset.validate()?;
        Ok(dataset)
    }

    /// Reject datasets that would produce meaningless metrics: duplicate document
    /// ids, empty relevant sets, or relevance labels that dangle off unknown ids.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut ids = HashSet::new();
        for doc in &self.documents {
            if !ids.insert(doc.id.as_str()) {
                bail!("duplicate document id: {}", doc.id);
            }
        }
        for query in &self.queries {
            if query.relevant_ids.is_empty() {
                bail!("query {} has no relevant_ids", query.id);
            }
            for rel in &query.relevant_ids {
                if !ids.contains(rel.as_str()) {
                    bail!("query {} references unknown document id {}", query.id, rel);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "dataset_tests.rs"]
mod tests;
