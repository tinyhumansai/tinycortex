//! Append + cascade-seal for summary trees — ported from OpenHuman's
//! `memory_tree/tree/bucket_seal.rs`, simplified to the reduced foundation.
//!
//! `append_leaf` pushes a persisted chunk into the L0 buffer of a tree. Seal
//! gates differ by level:
//!
//! - **L0 (leaves → L1)**: seal when `token_sum >= input_token_budget`.
//! - **L≥1 (summaries → next level)**: seal when `item_ids.len() >=
//!   summary_fanout`. Counting siblings keeps the tree's fan-in stable
//!   regardless of summariser quality.
//!
//! When a buffer seals, its items move into the new summary's `child_ids`, the
//! buffer clears, and the new summary id is queued at the next level. The
//! cascade continues upward until a buffer fails its gate.
//!
//! ## Differences from OpenHuman
//!
//! The LLM is abstracted behind [`Summariser`]; on a summariser error or blank
//! output the seal falls back to the deterministic [`fallback_summary`]. The
//! content-staging / git-mirror, async-queue follow-up enqueue, event-bus
//! progress events, seal-time embedding, and per-document subtree paths are not
//! ported (deferred). Summary bodies are stored inline in `mem_tree_summaries`.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::score::extract::EntityExtractor;
use crate::memory::score::resolver::canonicalise;
use crate::memory::score::store::index_summary_entity_ids_tx;
use crate::memory::tree::hydrate::hydrate_inputs;
use crate::memory::tree::registry::new_summary_id;
use crate::memory::tree::store::{self, Buffer, SummaryNode, Tree};
use crate::memory::tree::summarise::{fallback_summary, Summariser, SummaryContext, SummaryInput};

/// Hard cap on cascade depth — guards against runaway loops if token accounting
/// ever slips.
const MAX_CASCADE_DEPTH: u32 = 32;

/// How a sealed summary node's `entities` and `topics` fields get populated.
#[derive(Clone)]
pub enum LabelStrategy {
    /// Run the extractor on the new summary's content; canonicalise the result
    /// into `entities` (canonical_ids) and `topics` (labels).
    ExtractFromContent(Arc<dyn EntityExtractor>),
    /// Dedup-merge each input's `entities` and `topics` into the parent.
    UnionFromChildren,
    /// Leave both fields empty regardless of inputs.
    Empty,
}

impl std::fmt::Debug for LabelStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExtractFromContent(ex) => write!(f, "ExtractFromContent({})", ex.name()),
            Self::UnionFromChildren => f.write_str("UnionFromChildren"),
            Self::Empty => f.write_str("Empty"),
        }
    }
}

/// Resolve `entities` and `topics` for a freshly-summarised node.
async fn resolve_labels(
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

/// A single leaf being appended to an L0 buffer.
#[derive(Clone, Debug)]
pub struct LeafRef {
    /// Persisted chunk id this leaf points at; used as the buffer `item_id` and
    /// deduped on append.
    pub chunk_id: String,
    /// Chunk token count; added to the L0 buffer's `token_sum` to drive the
    /// token-budget seal gate.
    pub token_count: u32,
    /// Chunk timestamp; folded into the buffer's `oldest_at` and the sealed
    /// summary's time range.
    pub timestamp: DateTime<Utc>,
    /// Raw chunk text, hydrated as a summariser input at seal time.
    pub content: String,
    /// Canonical entity ids carried up to the parent under
    /// [`LabelStrategy::UnionFromChildren`].
    pub entities: Vec<String>,
    /// Topic labels carried up to the parent under
    /// [`LabelStrategy::UnionFromChildren`].
    pub topics: Vec<String>,
    /// Chunk relevance score; the sealed summary takes the max over its inputs
    /// (clamped to `>= 0.0`).
    pub score: f32,
}

/// Append a leaf to `tree`, sealing buffers as they fill. Returns the ids of
/// any summaries that sealed during this call.
pub async fn append_leaf(
    config: &MemoryConfig,
    tree: &Tree,
    leaf: &LeafRef,
    summariser: &dyn Summariser,
    strategy: &LabelStrategy,
) -> Result<Vec<String>> {
    append_to_buffer(
        config,
        &tree.id,
        0,
        &leaf.chunk_id,
        leaf.token_count as i64,
        leaf.timestamp,
    )?;
    cascade_all_from(config, tree, 0, false, summariser, strategy).await
}

/// Queue-oriented variant of [`append_leaf`]: only stage the leaf in the L0
/// buffer and report whether the caller should enqueue a follow-up seal job.
pub fn append_leaf_deferred(config: &MemoryConfig, tree: &Tree, leaf: &LeafRef) -> Result<bool> {
    append_to_buffer(
        config,
        &tree.id,
        0,
        &leaf.chunk_id,
        leaf.token_count as i64,
        leaf.timestamp,
    )?;
    let buf = store::get_buffer(config, &tree.id, 0)?;
    Ok(should_seal(config, &buf))
}

/// Transactionally append a single item to `(tree_id, level)`'s buffer.
/// Idempotent on `(tree_id, level, item_id)`.
pub fn append_to_buffer(
    config: &MemoryConfig,
    tree_id: &str,
    level: u32,
    item_id: &str,
    token_delta: i64,
    item_ts: DateTime<Utc>,
) -> Result<()> {
    with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let mut buf = store::get_buffer_conn(&tx, tree_id, level)?;
        if buf.item_ids.iter().any(|existing| existing == item_id) {
            return Ok(()); // retry after a failed cascade — no double count
        }
        buf.item_ids.push(item_id.to_string());
        buf.token_sum = buf.token_sum.saturating_add(token_delta);
        buf.oldest_at = match buf.oldest_at {
            Some(existing) => Some(existing.min(item_ts)),
            None => Some(item_ts),
        };
        store::upsert_buffer_tx(&tx, &buf)?;
        tx.commit()?;
        Ok(())
    })
}

/// Seal buffers starting at `start_level` and cascade upward. When `force` is
/// `true`, the buffer at `start_level` is sealed regardless of its token/fan-in
/// gate (used by time-based flush and the disconnect force-flush). Upper levels
/// are sealed only when they cross their gate.
pub async fn cascade_all_from(
    config: &MemoryConfig,
    tree: &Tree,
    start_level: u32,
    force: bool,
    summariser: &dyn Summariser,
    strategy: &LabelStrategy,
) -> Result<Vec<String>> {
    let mut sealed_ids: Vec<String> = Vec::new();
    let mut level: u32 = start_level;
    let mut first_iteration = true;

    for _ in 0..MAX_CASCADE_DEPTH {
        let buf = store::get_buffer(config, &tree.id, level)?;
        let forced = first_iteration && force;
        first_iteration = false;

        if !forced && !should_seal(config, &buf) {
            break;
        }
        if buf.is_empty() {
            break;
        }

        let summary_id = seal_one_level(config, tree, &buf, summariser, strategy).await?;
        sealed_ids.push(summary_id);
        level += 1;
    }
    Ok(sealed_ids)
}

/// Level-aware seal gate. L0 gates on `token_sum`; L≥1 gates on sibling count.
/// Budgets are read from [`MemoryConfig::tree`], not hardcoded.
pub(crate) fn should_seal(config: &MemoryConfig, buf: &Buffer) -> bool {
    if buf.is_empty() {
        return false;
    }
    if buf.level == 0 {
        buf.token_sum >= config.tree.input_token_budget as i64
    } else {
        (buf.item_ids.len() as u32) >= config.tree.summary_fanout
    }
}

/// Seal `buf` at `level` into one summary at `level + 1`. Returns the new
/// summary id.
///
/// # Errors
/// Returns `Err` if `buf.item_ids` all fail to hydrate (e.g. their chunk/summary
/// rows were deleted) — the function refuses to persist a summary with zero
/// inputs. Also propagates any SQL failure from the seal transaction.
///
/// # NOTE: not atomic against concurrent appends
/// `buf` is a snapshot the caller already read; hydration and the summariser
/// call below run with **no lock held**, so an arbitrarily long await sits
/// between the snapshot and the transaction that commits it. Any
/// `append_to_buffer` call that lands on the same `(tree_id, level)` during
/// that window is silently lost: [`store::clear_buffer_tx`] wipes the whole
/// buffer row rather than removing only `buf.item_ids`. The same window also
/// lets two concurrent cascades seal the same buffer twice — the `parent_id IS
/// NULL` backlink guard below masks the resulting duplicate rather than
/// preventing it. See `TR-1` in
/// `docs/spec/audit/03-tree-archivist-conversations.md` for the fix (re-read
/// and set-difference inside the transaction instead of clearing).
pub(crate) async fn seal_one_level(
    config: &MemoryConfig,
    tree: &Tree,
    buf: &Buffer,
    summariser: &dyn Summariser,
    strategy: &LabelStrategy,
) -> Result<String> {
    let level = buf.level;
    let target_level = level + 1;

    let inputs = hydrate_inputs(config, level, &buf.item_ids)?;
    if inputs.is_empty() {
        anyhow::bail!(
            "refused to seal empty buffer tree_id={} level={}",
            tree.id,
            level
        );
    }

    let time_range_start = inputs
        .iter()
        .map(|i| i.time_range_start)
        .min()
        .unwrap_or_else(Utc::now);
    let time_range_end = inputs
        .iter()
        .map(|i| i.time_range_end)
        .max()
        .unwrap_or_else(Utc::now);
    let score = inputs
        .iter()
        .map(|i| i.score)
        .fold(f32::NEG_INFINITY, f32::max)
        .max(0.0);

    let budget = config.tree.output_token_budget;
    let ctx = SummaryContext {
        tree_id: &tree.id,
        tree_kind: tree.kind,
        target_level,
        token_budget: budget,
    };
    // Treat a blank summary the same as a hard error — fall back to the
    // deterministic concat so we never persist `content = ""`.
    let output = match summariser.summarise(&inputs, &ctx).await {
        Ok(o) if !o.content.trim().is_empty() => o,
        _ => fallback_summary(&inputs, budget),
    };

    let (node_entities, node_topics) = resolve_labels(strategy, &inputs, &output.content).await?;

    let now = Utc::now();
    let summary_id = new_summary_id(target_level);
    let node = SummaryNode {
        id: summary_id.clone(),
        tree_id: tree.id.clone(),
        tree_kind: tree.kind,
        level: target_level,
        parent_id: None,
        child_ids: buf.item_ids.clone(),
        content: output.content,
        token_count: output.token_count,
        entities: node_entities,
        topics: node_topics,
        time_range_start,
        time_range_end,
        score,
        sealed_at: now,
        deleted: false,
        embedding: None,
        doc_id: None,
        version_ms: None,
    };

    let signature = crate::memory::chunks::tree_active_signature(config);
    let tree_id = tree.id.clone();
    let summary_id_for_tx = summary_id.clone();
    with_connection(config, move |conn| {
        let tx = conn.unchecked_transaction()?;

        let current_max: u32 = tx
            .query_row(
                "SELECT max_level FROM mem_tree_trees WHERE id = ?1",
                rusqlite::params![&tree_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n.max(0) as u32)
            .context("Failed to read current max_level for tree")?;

        store::insert_summary_tx(&tx, &node, &signature)?;
        index_summary_entity_ids_tx(
            &tx,
            &node.entities,
            &node.id,
            node.score,
            now.timestamp_millis(),
            Some(&tree_id),
        )?;
        // Backlink children → new parent for single-row traversal.
        for child_id in &node.child_ids {
            if level == 0 {
                tx.execute(
                    "UPDATE mem_tree_chunks SET parent_summary_id = ?1
                       WHERE id = ?2 AND parent_summary_id IS NULL",
                    rusqlite::params![&summary_id_for_tx, child_id],
                )
                .context("Failed to backlink chunk to parent summary")?;
            } else {
                tx.execute(
                    "UPDATE mem_tree_summaries SET parent_id = ?1
                       WHERE id = ?2 AND parent_id IS NULL",
                    rusqlite::params![&summary_id_for_tx, child_id],
                )
                .context("Failed to backlink summary to parent summary")?;
            }
        }
        store::clear_buffer_tx(&tx, &tree_id, level)?;

        // Append the new summary to the parent buffer.
        let mut parent = store::get_buffer_conn(&tx, &tree_id, target_level)?;
        parent.item_ids.push(summary_id_for_tx.clone());
        parent.token_sum = parent.token_sum.saturating_add(node.token_count as i64);
        parent.oldest_at = match parent.oldest_at {
            Some(existing) => Some(existing.min(time_range_start)),
            None => Some(time_range_start),
        };
        store::upsert_buffer_tx(&tx, &parent)?;

        if target_level > current_max {
            store::update_tree_after_seal_tx(&tx, &tree_id, &summary_id_for_tx, target_level, now)?;
        } else {
            store::refresh_last_sealed_tx(&tx, &tree_id, now)?;
        }

        tx.commit()?;
        Ok(())
    })?;

    Ok(summary_id)
}

#[cfg(test)]
#[path = "bucket_seal_label_tests.rs"]
mod label_tests;
#[cfg(test)]
#[path = "bucket_seal_tests.rs"]
mod tests;
