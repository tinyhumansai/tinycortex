//! Public service and input types for bucket-seal trees.

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::store::{SummaryNode, Tree};
use super::summarise::Summariser;
use crate::memory::score::embed::Embedder;
use crate::memory::score::extract::EntityExtractor;

/// Product callbacks around a seal after durable state transitions.
pub trait SealObserver: Send + Sync {
    /// Report structural progress without summary content.
    fn progress(&self, _tree: &Tree, _step: &str, _level: u32, _item_count: Option<u32>) {}
    /// Notify a host mirror after a summary commits.
    fn summary_committed(
        &self,
        _tree: &Tree,
        _node: &SummaryNode,
        _content_path: &str,
        _reason: &str,
    ) -> Result<()> {
        Ok(())
    }
}

pub(crate) struct NoopSealObserver;
impl SealObserver for NoopSealObserver {}

/// Injected compute and product services for sealing.
pub struct SealServices<'a> {
    /// Service that produces a summary from hydrated child inputs.
    pub summariser: &'a dyn Summariser,
    /// Optional embedding service used to index committed summaries.
    pub embedder: Option<&'a dyn Embedder>,
    /// Observer notified around durable seal transitions.
    pub observer: &'a dyn SealObserver,
}

/// Strategy for populating a summary's entity and topic labels.
#[derive(Clone)]
pub enum LabelStrategy {
    /// Extract labels from the new summary content.
    ExtractFromContent(Arc<dyn EntityExtractor>),
    /// Union labels already carried by children.
    UnionFromChildren,
    /// Leave labels empty.
    Empty,
}

impl std::fmt::Debug for LabelStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExtractFromContent(extractor) => {
                write!(f, "ExtractFromContent({})", extractor.name())
            }
            Self::UnionFromChildren => f.write_str("UnionFromChildren"),
            Self::Empty => f.write_str("Empty"),
        }
    }
}

/// One persisted leaf being appended to an L0 buffer.
#[derive(Clone, Debug)]
pub struct LeafRef {
    /// Stable identifier of the persisted chunk.
    pub chunk_id: String,
    /// Token contribution used by the L0 seal gate.
    pub token_count: u32,
    /// Source timestamp used for buffer age and summary bounds.
    pub timestamp: DateTime<Utc>,
    /// Chunk content available to callers constructing leaf inputs.
    pub content: String,
    /// Canonical entity identifiers attached to the chunk.
    pub entities: Vec<String>,
    /// Topic labels attached to the chunk.
    pub topics: Vec<String>,
    /// Importance score carried by the leaf.
    pub score: f32,
}
