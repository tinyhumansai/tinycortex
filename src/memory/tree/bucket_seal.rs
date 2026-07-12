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
use futures::stream::{StreamExt, TryStreamExt};

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::score::embed::Embedder;
use crate::memory::score::extract::EntityExtractor;
use crate::memory::score::resolver::canonicalise;
use crate::memory::score::store::index_summary_entity_ids_tx;
use crate::memory::store::content::{
    slugify_source_id, stage_summary_with_layout, SummaryComposeInput, SummaryDiskLayout,
    SummaryTreeKind,
};
use crate::memory::tree::hydrate::hydrate_inputs;
use crate::memory::tree::registry::new_summary_id;
use crate::memory::tree::store::{self, Buffer, SummaryNode, Tree};
use crate::memory::tree::summarise::{fallback_summary, Summariser, SummaryContext, SummaryInput};

/// Hard cap on cascade depth — guards against runaway loops if token accounting
/// ever slips.
const MAX_CASCADE_DEPTH: u32 = 32;

/// Product callbacks around a seal. Engine state is already durable when
/// `summary_committed` runs; hosts use it for mirrors such as wiki-git.
pub trait SealObserver: Send + Sync {
    fn progress(&self, _tree: &Tree, _step: &str, _level: u32, _item_count: Option<u32>) {}
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

/// Injected compute and product notifications used by the crate-owned seal
/// pipeline. `embedder = None` deliberately persists a re-embeddable summary.
pub struct SealServices<'a> {
    pub summariser: &'a dyn Summariser,
    pub embedder: Option<&'a dyn Embedder>,
    pub observer: &'a dyn SealObserver,
}

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
        level += 1;
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

        let current_max: u32 = tx
            .query_row(
                "SELECT max_level FROM mem_tree_trees WHERE id = ?1",
                rusqlite::params![&tree_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n.max(0) as u32)
            .context("Failed to read current max_level for tree")?;

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
        store::clear_buffer_tx(&tx, &tree_id, level)?;

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
            crate::memory::queue::enqueue_tx(&tx, &crate::memory::queue::NewJob::seal(&payload)?)?;
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

/// Level offset reserved for cross-document merge nodes.
pub const MERGE_LEVEL_BASE: u32 = 1_000;
const DOC_SUBTREE_MAX_FANIN: usize = 32;
const DOC_SUBTREE_SEAL_CONCURRENCY: usize = 8;

/// Build one immutable document-version subtree and feed its root into the
/// shared cross-document merge tier.
pub async fn seal_document_subtree_with_services(
    config: &MemoryConfig,
    tree: &Tree,
    doc_id: &str,
    version_ms: Option<i64>,
    chunk_ids: &[String],
    services: &SealServices<'_>,
    strategy: &LabelStrategy,
) -> Result<String> {
    if chunk_ids.is_empty() {
        anyhow::bail!("seal_document_subtree: empty chunk set");
    }
    log::debug!(
        "[memory_tree:seal_document] enter chunks={} has_version={}",
        chunk_ids.len(),
        version_ms.is_some()
    );
    let mut level = 0;
    let mut current_ids = chunk_ids.to_vec();
    let doc_root = loop {
        let batches = if level == 0 {
            batch_leaves_by_token_budget(config, &current_ids)?
        } else {
            batch_by_count(&current_ids, DOC_SUBTREE_MAX_FANIN)
        };
        let batch_futures: Vec<_> = batches
            .iter()
            .map(|batch| {
                seal_explicit_children(
                    config, tree, level, batch, doc_id, version_ms, services, strategy,
                )
            })
            .collect();
        let nodes: Vec<SummaryNode> = futures::stream::iter(batch_futures)
            .buffered(DOC_SUBTREE_SEAL_CONCURRENCY)
            .try_collect()
            .await?;
        current_ids = nodes.iter().map(|node| node.id.clone()).collect();
        level += 1;
        if current_ids.len() <= 1 {
            break nodes
                .into_iter()
                .next()
                .context("document seal produced no root")?;
        }
    };

    append_to_buffer(
        config,
        &tree.id,
        MERGE_LEVEL_BASE,
        &doc_root.id,
        doc_root.token_count as i64,
        doc_root.time_range_start,
    )?;
    cascade_all_from_with_services(
        config,
        tree,
        MERGE_LEVEL_BASE,
        false,
        services,
        strategy,
        false,
    )
    .await?;
    log::debug!("[memory_tree:seal_document] complete levels={level}");
    Ok(doc_root.id)
}

fn batch_leaves_by_token_budget(
    config: &MemoryConfig,
    chunk_ids: &[String],
) -> Result<Vec<Vec<String>>> {
    let chunks = crate::memory::chunks::get_chunks_batch(config, chunk_ids)?;
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut tokens = 0_i64;
    for id in chunk_ids {
        let Some(chunk) = chunks.get(id) else {
            continue;
        };
        let next = chunk.token_count as i64;
        if !current.is_empty()
            && (tokens + next > config.tree.input_token_budget as i64
                || current.len() >= DOC_SUBTREE_MAX_FANIN)
        {
            batches.push(std::mem::take(&mut current));
            tokens = 0;
        }
        current.push(id.clone());
        tokens += next;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    if batches.is_empty() {
        anyhow::bail!("seal_document_subtree: no resolvable chunks");
    }
    Ok(batches)
}

fn batch_by_count(ids: &[String], max: usize) -> Vec<Vec<String>> {
    ids.chunks(max.max(1)).map(<[String]>::to_vec).collect()
}

async fn seal_explicit_children(
    config: &MemoryConfig,
    tree: &Tree,
    level: u32,
    child_ids: &[String],
    doc_id: &str,
    version_ms: Option<i64>,
    services: &SealServices<'_>,
    strategy: &LabelStrategy,
) -> Result<SummaryNode> {
    let target_level = level + 1;
    let inputs = hydrate_inputs(config, level, child_ids)?;
    if inputs.is_empty() {
        anyhow::bail!("document seal has no hydrated inputs at level {level}");
    }
    let time_range_start = inputs.iter().map(|i| i.time_range_start).min().unwrap();
    let time_range_end = inputs.iter().map(|i| i.time_range_end).max().unwrap();
    let score = inputs
        .iter()
        .map(|i| i.score)
        .fold(f32::NEG_INFINITY, f32::max)
        .max(0.0);
    let output = if inputs.len() == 1 && inputs[0].token_count <= config.tree.output_token_budget {
        crate::memory::tree::SummaryOutput {
            content: inputs[0].content.clone(),
            token_count: inputs[0].token_count,
            ..Default::default()
        }
    } else {
        let context = SummaryContext {
            tree_id: &tree.id,
            tree_kind: tree.kind,
            target_level,
            token_budget: config.tree.output_token_budget,
        };
        match services.summariser.summarise(&inputs, &context).await {
            Ok(output) if !output.content.trim().is_empty() => output,
            _ => fallback_summary(&inputs, context.token_budget),
        }
    };
    let (entities, topics) = resolve_labels(strategy, &inputs, &output.content).await?;
    let embedding = match services.embedder {
        Some(embedder) if !output.content.trim().is_empty() => {
            let input: String = output.content.chars().take(4_000).collect();
            Some(
                embedder
                    .embed(&input)
                    .await
                    .context("embed document summary")?,
            )
        }
        _ => None,
    };
    let now = Utc::now();
    let node = SummaryNode {
        id: new_summary_id(target_level),
        tree_id: tree.id.clone(),
        tree_kind: tree.kind,
        level: target_level,
        parent_id: None,
        child_ids: child_ids.to_vec(),
        content: output.content,
        token_count: output.token_count,
        entities,
        topics,
        time_range_start,
        time_range_end,
        score,
        sealed_at: now,
        deleted: false,
        embedding,
        doc_id: Some(doc_id.to_string()),
        version_ms,
    };
    let summary_kind = match tree.kind {
        crate::memory::tree::TreeKind::Source => SummaryTreeKind::Source,
        crate::memory::tree::TreeKind::Topic => SummaryTreeKind::Topic,
        crate::memory::tree::TreeKind::Global => SummaryTreeKind::Global,
    };
    let doc_slug = slugify_source_id(doc_id);
    let staged = stage_summary_with_layout(
        &crate::memory::chunks::content_root(config),
        &SummaryComposeInput {
            summary_id: &node.id,
            tree_kind: summary_kind,
            tree_id: &node.tree_id,
            tree_scope: &tree.scope,
            level: node.level,
            child_ids: &node.child_ids,
            child_basenames: None,
            child_count: node.child_ids.len(),
            time_range_start: node.time_range_start,
            time_range_end: node.time_range_end,
            sealed_at: node.sealed_at,
            body: &node.content,
        },
        &slugify_source_id(&tree.scope),
        SummaryDiskLayout::DocSubtree {
            doc_slug: &doc_slug,
            version_ms,
        },
    )?;
    let signature = crate::memory::chunks::tree_active_signature(config);
    let node_for_tx = node.clone();
    let staged_for_tx = staged.clone();
    with_connection(config, move |connection| {
        let transaction = connection.unchecked_transaction()?;
        store::insert_staged_summary_tx(
            &transaction,
            &node_for_tx,
            Some(&staged_for_tx),
            &signature,
        )?;
        index_summary_entity_ids_tx(
            &transaction,
            &node_for_tx.entities,
            &node_for_tx.id,
            node_for_tx.score,
            now.timestamp_millis(),
            Some(&node_for_tx.tree_id),
        )?;
        for child_id in &node_for_tx.child_ids {
            if level == 0 {
                transaction.execute(
                    "UPDATE mem_tree_chunks SET parent_summary_id = ?1 WHERE id = ?2",
                    rusqlite::params![&node_for_tx.id, child_id],
                )?;
            } else {
                transaction.execute(
                    "UPDATE mem_tree_summaries SET parent_id = ?1 WHERE id = ?2 AND parent_id IS NULL",
                    rusqlite::params![&node_for_tx.id, child_id],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    })?;
    services.observer.summary_committed(
        tree,
        &node,
        &staged.content_path,
        "document_subtree_seal",
    )?;
    Ok(node)
}

#[cfg(test)]
#[path = "bucket_seal_label_tests.rs"]
mod label_tests;
#[cfg(test)]
#[path = "bucket_seal_tests.rs"]
mod tests;
