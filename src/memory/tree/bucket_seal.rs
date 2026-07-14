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

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::score::store::index_summary_entity_ids_tx;
use crate::memory::store::content::{
    slugify_source_id, stage_summary_with_layout, SummaryComposeInput, SummaryDiskLayout,
    SummaryTreeKind,
};
use crate::memory::tree::hydrate::hydrate_inputs;
use crate::memory::tree::label_resolver::resolve_labels;
use crate::memory::tree::registry::new_summary_id;
use crate::memory::tree::store::{self, Buffer, SummaryNode, Tree};
use crate::memory::tree::summarise::{fallback_summary, Summariser, SummaryContext};
pub(crate) use crate::memory::tree::types::NoopSealObserver;
pub use crate::memory::tree::types::{LabelStrategy, LeafRef, SealObserver, SealServices};

/// Hard cap on cascade depth — guards against runaway loops if token accounting
/// ever slips.
const MAX_CASCADE_DEPTH: u32 = 32;

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
        let status: String = tx
            .query_row(
                "SELECT status FROM mem_tree_trees WHERE id = ?1",
                rusqlite::params![tree_id],
                |row| row.get(0),
            )
            .context("Failed to read tree status before append")?;
        anyhow::ensure!(status == "active", "tree '{tree_id}' is archived");
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
    cascade_all_from_with_services(
        config,
        tree,
        start_level,
        force,
        &SealServices {
            summariser,
            embedder: None,
            observer: &NoopSealObserver,
        },
        strategy,
        false,
    )
    .await
}

pub async fn cascade_all_from_with_services(
    config: &MemoryConfig,
    tree: &Tree,
    start_level: u32,
    force: bool,
    services: &SealServices<'_>,
    strategy: &LabelStrategy,
    enqueue_follow_ups: bool,
) -> Result<Vec<String>> {
    let mut sealed_ids: Vec<String> = Vec::new();
    let mut first_iteration = true;

    for level in (start_level..).take(MAX_CASCADE_DEPTH as usize) {
        let buf = store::get_buffer(config, &tree.id, level)?;
        let forced = first_iteration && force;
        first_iteration = false;

        if !forced && !should_seal(config, &buf) {
            break;
        }
        if buf.is_empty() {
            break;
        }

        let summary_id = seal_one_level_with_services(
            config,
            tree,
            &buf,
            services,
            strategy,
            enqueue_follow_ups,
        )
        .await?;
        sealed_ids.push(summary_id);
    }

    // Flavoured trees carry a first-class compiled root artifact; refresh it
    // whenever a seal in this cascade may have moved the root. `tree` here is a
    // pre-seal snapshot, so `compile_flavoured_root` re-reads the live row to
    // pick up the new `root_id`. Best-effort: a stale artifact must never fail
    // an otherwise-successful seal.
    if !sealed_ids.is_empty() && tree.kind == crate::memory::tree::TreeKind::Flavoured {
        if let Err(err) = crate::memory::tree::flavoured::compile_flavoured_root(config, &tree.id) {
            log::warn!(
                "[memory_tree:flavoured] compile root failed tree_id={}: {err:#}",
                tree.id
            );
        }
    }

    Ok(sealed_ids)
}

/// Level-aware seal gate. L0 gates on `token_sum`; L≥1 gates on sibling count.
/// Budgets are read from [`MemoryConfig::tree`], not hardcoded.
pub fn should_seal(config: &MemoryConfig, buf: &Buffer) -> bool {
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
/// The potentially slow summariser runs without a database lock. The commit
/// transaction therefore re-reads and consumes only the snapshotted prefix;
/// concurrent appends remain buffered, while a competing seal causes this
/// transaction to abort cleanly.
pub async fn seal_one_level_with_services(
    config: &MemoryConfig,
    tree: &Tree,
    buf: &Buffer,
    services: &SealServices<'_>,
    strategy: &LabelStrategy,
    enqueue_follow_ups: bool,
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
        input_token_budget: config.tree.input_token_budget,
        overhead_reserve_tokens: config.tree.summary_overhead_reserve_tokens,
        ask: tree.ask.as_deref(),
    };
    // Treat a blank summary the same as a hard error — fall back to the
    // deterministic concat so we never persist `content = ""`.
    services
        .observer
        .progress(tree, "summarising", level, Some(inputs.len() as u32));
    let output = match services.summariser.summarise(&inputs, &ctx).await {
        Ok(o) if !o.content.trim().is_empty() => o,
        _ => fallback_summary(&inputs, budget),
    };

    let (node_entities, node_topics) = resolve_labels(strategy, &inputs, &output.content).await?;

    services.observer.progress(tree, "embedding", level, None);
    let embedding = match services.embedder {
        Some(embedder) if !output.content.trim().is_empty() => {
            let input: String = output.content.chars().take(4_000).collect();
            Some(
                embedder
                    .embed(&input)
                    .await
                    .context("embed sealed summary")?,
            )
        }
        _ => None,
    };

    let now = Utc::now();
    let summary_id = new_summary_id(target_level);
    let node = SummaryNode {
        id: summary_id.clone(),
        tree_id: tree.id.clone(),
        tree_kind: tree.kind,
        level: target_level,
        parent_id: None,
        // Hydration can skip deleted inputs. Never claim missing children in
        // the persisted summary merely because their ids remained buffered.
        child_ids: inputs.iter().map(|input| input.id.clone()).collect(),
        content: output.content,
        token_count: output.token_count,
        entities: node_entities,
        topics: node_topics,
        time_range_start,
        time_range_end,
        score,
        sealed_at: now,
        deleted: false,
        embedding,
        doc_id: None,
        version_ms: None,
    };

    services
        .observer
        .progress(tree, "persisting", target_level, None);
    let summary_kind = match tree.kind {
        crate::memory::tree::TreeKind::Source => SummaryTreeKind::Source,
        crate::memory::tree::TreeKind::Topic => SummaryTreeKind::Topic,
        crate::memory::tree::TreeKind::Global => SummaryTreeKind::Global,
        crate::memory::tree::TreeKind::Flavoured => SummaryTreeKind::Flavoured,
    };
    let child_basenames = if target_level == 1 {
        Some(
            node.child_ids
                .iter()
                .map(|id| {
                    crate::memory::chunks::get_chunk_raw_refs(config, id)
                        .ok()
                        .flatten()
                        .and_then(|refs| refs.into_iter().next())
                        .map(|raw| {
                            raw.path
                                .strip_suffix(".md")
                                .unwrap_or(&raw.path)
                                .to_string()
                        })
                })
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    let layout = if target_level >= 1_000 {
        SummaryDiskLayout::Merge
    } else {
        SummaryDiskLayout::Standard
    };
    let staged = stage_summary_with_layout(
        &crate::memory::chunks::content_root(config),
        &SummaryComposeInput {
            summary_id: &node.id,
            tree_kind: summary_kind,
            tree_id: &node.tree_id,
            tree_scope: &tree.scope,
            level: node.level,
            child_ids: &node.child_ids,
            child_basenames: child_basenames.as_deref(),
            child_count: node.child_ids.len(),
            time_range_start: node.time_range_start,
            time_range_end: node.time_range_end,
            sealed_at: node.sealed_at,
            body: &node.content,
        },
        &slugify_source_id(&tree.scope),
        layout,
    )?;

    let signature = crate::memory::chunks::tree_active_signature(config);
    let tree_id = tree.id.clone();
    let summary_id_for_tx = summary_id.clone();
    let node_for_tx = node.clone();
    let staged_for_tx = staged.clone();
    with_connection(config, move |conn| {
        let tx = conn.unchecked_transaction()?;

        let (current_max, status): (u32, String) = tx
            .query_row(
                "SELECT max_level, status FROM mem_tree_trees WHERE id = ?1",
                rusqlite::params![&tree_id],
                |r| Ok((r.get::<_, i64>(0)?.max(0) as u32, r.get(1)?)),
            )
            .context("Failed to read current state for tree")?;
        anyhow::ensure!(status == "active", "tree '{tree_id}' is archived");

        store::insert_staged_summary_tx(&tx, &node_for_tx, Some(&staged_for_tx), &signature)?;
        index_summary_entity_ids_tx(
            &tx,
            &node_for_tx.entities,
            &node_for_tx.id,
            node_for_tx.score,
            now.timestamp_millis(),
            Some(&tree_id),
        )?;
        // Backlink children → new parent for single-row traversal.
        for child_id in &node_for_tx.child_ids {
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
        store::consume_snapshot_tx(&tx, buf)?;

        // Append the new summary to the parent buffer.
        let mut parent = store::get_buffer_conn(&tx, &tree_id, target_level)?;
        parent.item_ids.push(summary_id_for_tx.clone());
        parent.token_sum = parent
            .token_sum
            .saturating_add(node_for_tx.token_count as i64);
        parent.oldest_at = match parent.oldest_at {
            Some(existing) => Some(existing.min(time_range_start)),
            None => Some(time_range_start),
        };
        store::upsert_buffer_tx(&tx, &parent)?;

        if enqueue_follow_ups && should_seal(config, &parent) {
            let payload = crate::memory::queue::SealPayload {
                tree_id: tree_id.clone(),
                level: target_level,
                force_now_ms: None,
            };
            crate::memory::queue::store::enqueue_tx_with_default(
                &tx,
                &crate::memory::queue::NewJob::seal(&payload)?,
                config.queue.max_attempts,
            )?;
        }

        if target_level > current_max {
            store::update_tree_after_seal_tx(&tx, &tree_id, &summary_id_for_tx, target_level, now)?;
        } else {
            store::refresh_last_sealed_tx(&tx, &tree_id, now)?;
        }

        tx.commit()?;
        Ok(())
    })?;

    services
        .observer
        .summary_committed(tree, &node, &staged.content_path, "bucket_seal")?;

    Ok(summary_id)
}
pub use super::document_seal::{seal_document_subtree_with_services, MERGE_LEVEL_BASE};

#[cfg(test)]
#[path = "bucket_seal_label_tests.rs"]
mod label_tests;
#[cfg(test)]
#[path = "bucket_seal_tests.rs"]
mod tests;
