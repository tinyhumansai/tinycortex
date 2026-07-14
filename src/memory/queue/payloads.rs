//! Strongly typed payloads persisted in queue job rows.

use serde::{Deserialize, Serialize};

/// Reference to either a leaf chunk or a sealed summary node.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeRef {
    /// Leaf chunk id.
    Leaf {
        /// Chunk row id.
        chunk_id: String,
    },
    /// Sealed summary id.
    Summary {
        /// Summary row id.
        summary_id: String,
    },
}

impl NodeRef {
    /// Stable kind-prefixed identity for queue deduplication.
    pub fn dedupe_fragment(&self) -> String {
        match self {
            Self::Leaf { chunk_id } => format!("leaf:{chunk_id}"),
            Self::Summary { summary_id } => format!("summary:{summary_id}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Extract/admit one chunk.
pub struct ExtractChunkPayload {
    /// Chunk row id.
    pub chunk_id: String,
}

impl ExtractChunkPayload {
    /// Stable queue deduplication key.
    pub fn dedupe_key(&self) -> String {
        format!("extract:{}", self.chunk_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// Destination for an append-buffer job.
pub enum AppendTarget {
    /// Source tree selected by source id.
    Source {
        /// Logical source id.
        source_id: String,
    },
    /// Topic tree selected by physical tree id.
    Topic {
        /// Physical topic tree id.
        tree_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Node and destination for one buffer append.
pub struct AppendBufferPayload {
    /// Leaf or summary to append.
    pub node: NodeRef,
    /// Destination buffer.
    pub target: AppendTarget,
}

impl AppendBufferPayload {
    /// Stable queue deduplication key.
    pub fn dedupe_key(&self) -> String {
        let node = self.node.dedupe_fragment();
        match &self.target {
            AppendTarget::Source { source_id } => format!("append:source:{source_id}:{node}"),
            AppendTarget::Topic { tree_id } => format!("append:topic:{tree_id}:{node}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Seal one buffer level.
pub struct SealPayload {
    /// Physical tree id.
    pub tree_id: String,
    /// Zero-based buffer level.
    pub level: u32,
    /// Presence forces sealing; value supplies the scheduling instant.
    pub force_now_ms: Option<i64>,
}

impl SealPayload {
    /// Stable per-tree-and-level deduplication key.
    pub fn dedupe_key(&self) -> String {
        format!("seal:{}:{}", self.tree_id, self.level)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// Scan and seal stale buffers.
pub struct FlushStalePayload {
    /// Optional maximum buffer age override.
    pub max_age_secs: Option<i64>,
}

impl FlushStalePayload {
    /// Stable key scoped to one three-hour scheduler block.
    pub fn dedupe_key(&self, date_iso: &str, hour_block: u32) -> String {
        format!("flush_stale:{date_iso}-h{hour_block}")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Re-embed rows missing the named vector signature.
pub struct ReembedBackfillPayload {
    /// Embedding provider/model/dimension signature.
    pub signature: String,
}

impl ReembedBackfillPayload {
    /// Stable per-signature deduplication key.
    pub fn dedupe_key(&self) -> String {
        format!("reembed_backfill:{}", self.signature)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Build and merge one versioned document subtree.
pub struct SealDocumentPayload {
    /// Connection-level tree scope.
    pub tree_scope: String,
    /// Stable document identity.
    pub doc_id: String,
    /// Source version in epoch milliseconds.
    pub version_ms: Option<i64>,
    /// Ordered leaf chunk ids for this version.
    pub chunk_ids: Vec<String>,
}

impl SealDocumentPayload {
    /// Stable per-document-version deduplication key.
    pub fn dedupe_key(&self) -> String {
        match self.version_ms {
            Some(version) => format!("seal_doc:{}@{version}", self.doc_id),
            None => format!("seal_doc:{}", self.doc_id),
        }
    }
}
