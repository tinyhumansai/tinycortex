//! Shared deterministic [`QueueDelegates`] implementation for the queue tests.
//!
//! Compiled only under `#[cfg(test)]`. Records call counts so tests can assert
//! the pipeline shape (extract → append → seal), and exposes knobs for the
//! admission decision, the seal cascade, the stale-buffer list, and the
//! re-embed progress sequence.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::handlers::{
    AppendDecision, ExtractDecision, QueueDelegates, ReembedProgress, StaleBuffer,
};
use crate::memory::queue::types::{AppendTarget, NodeRef, SealDocumentPayload, SealPayload};

/// Serializes tests that mutate the process-wide re-embed backfill flag.
pub(crate) static BACKFILL_FLAG_TEST_LOCK: Mutex<()> = parking_lot::const_mutex(());

/// Observable call counters, cloneable so a test can read them after `drain`.
#[derive(Default)]
pub(crate) struct Counts {
    pub extract: AtomicUsize,
    pub append: AtomicUsize,
    pub seal: AtomicUsize,
    pub flush: AtomicUsize,
    pub seal_document: AtomicUsize,
    pub reembed: AtomicUsize,
}

pub(crate) struct RecordingDelegates {
    pub counts: Arc<Counts>,
    /// `extract_chunk` decision; `None` simulates a missing chunk row.
    pub extract: Option<ExtractDecision>,
    /// `append_node` decision; `None` simulates a missing node / archived tree.
    pub append: Option<AppendDecision>,
    /// `seal_level` follow-up parent (one-shot; consumed on first seal).
    pub seal_parent: Mutex<Option<SealPayload>>,
    /// Buffers `list_stale_buffers` returns.
    pub stale: Vec<StaleBuffer>,
    /// `reembed_batch` outcomes, consumed front-to-back; empties to `Covered`.
    pub reembed: Mutex<VecDeque<ReembedProgress>>,
    /// `active_signature` value.
    pub signature: String,
    /// `has_uncovered_reembed_work` answer.
    pub uncovered: bool,
}

impl RecordingDelegates {
    /// A default that admits every chunk into `slack:#eng`, seals once with no
    /// cascade, has no stale buffers, and reports a covered embedding space.
    pub fn admitting() -> Self {
        Self {
            counts: Arc::new(Counts::default()),
            extract: Some(ExtractDecision {
                kept: true,
                uses_document_subtree: false,
                tree_scope: "slack:#eng".into(),
            }),
            append: Some(AppendDecision {
                tree_id: "tree:slack".into(),
                should_seal: true,
            }),
            seal_parent: Mutex::new(None),
            stale: Vec::new(),
            reembed: Mutex::new(VecDeque::new()),
            signature: "provider=test;model=x;dims=3".into(),
            uncovered: false,
        }
    }
}

#[async_trait]
impl QueueDelegates for RecordingDelegates {
    async fn extract_chunk(
        &self,
        _config: &MemoryConfig,
        _chunk_id: &str,
    ) -> Result<Option<ExtractDecision>> {
        self.counts.extract.fetch_add(1, Ordering::Relaxed);
        Ok(self.extract.clone())
    }

    async fn append_node(
        &self,
        _config: &MemoryConfig,
        _node: &NodeRef,
        _target: &AppendTarget,
    ) -> Result<Option<AppendDecision>> {
        self.counts.append.fetch_add(1, Ordering::Relaxed);
        Ok(self.append.clone())
    }

    async fn seal_level(
        &self,
        _config: &MemoryConfig,
        _payload: &SealPayload,
    ) -> Result<Option<SealPayload>> {
        self.counts.seal.fetch_add(1, Ordering::Relaxed);
        Ok(self.seal_parent.lock().take())
    }

    async fn list_stale_buffers(
        &self,
        _config: &MemoryConfig,
        _max_age_secs: i64,
    ) -> Result<Vec<StaleBuffer>> {
        self.counts.flush.fetch_add(1, Ordering::Relaxed);
        Ok(self.stale.clone())
    }

    async fn seal_document(
        &self,
        _config: &MemoryConfig,
        _payload: &SealDocumentPayload,
    ) -> Result<()> {
        self.counts.seal_document.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn reembed_batch(
        &self,
        _config: &MemoryConfig,
        _signature: &str,
    ) -> Result<ReembedProgress> {
        self.counts.reembed.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .reembed
            .lock()
            .pop_front()
            .unwrap_or(ReembedProgress::Covered))
    }

    fn active_signature(&self, _config: &MemoryConfig) -> String {
        self.signature.clone()
    }

    fn has_uncovered_reembed_work(&self, _config: &MemoryConfig, _signature: &str) -> Result<bool> {
        Ok(self.uncovered)
    }
}
