//! `cover_window` — the minimum-node cover of a time window.
//!
//! Given an explicit `[since_ms, until_ms]` window (and optional source
//! filter), return the **smallest set of nodes that covers every in-window
//! chunk** — a mix of summary nodes (where a whole subtree is in-window) and
//! raw leaf chunks (everything else). This is the read path a morning brief
//! uses for "last 24h" so it summarises only fresh content instead of the
//! all-time root.
//!
//! The cover is purely structural (ported from OpenHuman's
//! `memory_tree::retrieval::cover`). Two passes per source tree:
//! 1. **Eligible summaries** — every non-deleted summary whose time envelope is
//!    fully inside the window (`list_summaries_in_window`). Because seal sets a
//!    summary's envelope to the MIN/MAX of its children, "envelope ⊆ window" ⇔
//!    "all descendant leaves are in the window".
//! 2. **Frontier + raw fallback** — keep the topmost eligible summaries
//!    (`maximal` = eligible whose parent is not itself eligible); mark the
//!    chunks they transitively cover; emit any remaining in-window chunk raw.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::memory::chunks::{list_chunks, Chunk, ListChunksQuery, Metadata, SourceKind};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::store::{get_tree_by_scope, list_summaries_in_window};
use crate::memory::tree::{SummaryNode, TreeKind};

use super::types::{hydrated_chunk_hit, hydrated_summary_hit, QueryResponse, RetrievalHit};

/// Default cap on returned cover items when the caller passes `limit = 0`.
const DEFAULT_LIMIT: usize = 200;

/// Upper bound on in-window chunks scanned across all sources.
///
/// NOTE: this is a silent truncation with no signal distinct from the
/// `limit`-based one. [`QueryResponse::truncated`] is derived only from
/// `total > hits.len()` after the [`cover_window`] result-count cut. If the
/// window contains more in-window chunks than this cap, `list_chunks`
/// truncates before `cover_window` ever sees the rest, and the caller has no
/// way to distinguish "there were more chunks than the scan cap allows" from
/// "there were exactly this many, all covered by summaries".
const MAX_WINDOW_CHUNKS: usize = 5_000;

/// Derive the tree scope a chunk seals under: `path_scope` overrides
/// `source_id` for shared-directory document sources, mirroring the append
/// path's scope derivation.
fn chunk_tree_scope(metadata: &Metadata) -> String {
    metadata
        .path_scope
        .clone()
        .unwrap_or_else(|| metadata.source_id.clone())
}

/// Compute the minimum-node cover of `[since_ms, until_ms]`. Results are grouped
/// by source (`tree_scope`), ordered ascending by start time, then truncated to
/// `limit` (`DEFAULT_LIMIT` when 0).
///
/// # Gotcha: truncation is alphabetical by source, not by relevance
///
/// The pre-truncation sort key is `(tree_scope, time_range_start)` — sources
/// are grouped and ordered alphabetically by scope string, then
/// chronologically within each source. When `total > limit`,
/// [`Vec::truncate`] drops the tail of that ordering, which means entire
/// sources sorting late alphabetically (e.g. `zoom:` before `slack:` truncates
/// first) can be dropped wholesale rather than every source being
/// proportionally represented. There is no relevance-based ranking here — the
/// cover is a structural (not scored) result.
///
/// # Errors
///
/// Returns `Err` if `until_ms < since_ms`.
pub fn cover_window(
    config: &MemoryConfig,
    since_ms: i64,
    until_ms: i64,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
    limit: usize,
) -> Result<QueryResponse> {
    cover_window_scoped(
        config,
        since_ms,
        until_ms,
        source_id,
        source_kind,
        None,
        limit,
    )
}

/// Compute a cover while restricting memory-source chunks to `source_scope`.
/// The filter runs before the scan cap and result limit so disallowed sources
/// cannot crowd permitted sources out of the response.
pub fn cover_window_scoped(
    config: &MemoryConfig,
    since_ms: i64,
    until_ms: i64,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
    source_scope: Option<HashSet<String>>,
    limit: usize,
) -> Result<QueryResponse> {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    if until_ms < since_ms {
        return Err(anyhow::anyhow!(
            "cover_window: until_ms ({until_ms}) precedes since_ms ({since_ms})"
        ));
    }

    let mut hits = collect_cover(
        config,
        since_ms,
        until_ms,
        source_id,
        source_kind,
        source_scope,
    )?;

    // Group by source, then chronological ascending within each source.
    hits.sort_by(|a, b| {
        a.tree_scope
            .cmp(&b.tree_scope)
            .then(a.time_range_start.cmp(&b.time_range_start))
    });
    let total = hits.len();
    hits.truncate(limit);
    Ok(QueryResponse::new(hits, total))
}

/// Build the cover. Chunk-driven: pull the in-window chunk set first (chunks
/// exist before their tree is sealed), group by source, then look up each
/// source's tree for eligible summaries.
fn collect_cover(
    config: &MemoryConfig,
    since_ms: i64,
    until_ms: i64,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
    source_scope: Option<HashSet<String>>,
) -> Result<Vec<RetrievalHit>> {
    let chunks = list_chunks(
        config,
        &ListChunksQuery {
            source_id: source_id.map(|s| s.to_string()),
            source_kind,
            since_ms: Some(since_ms),
            until_ms: Some(until_ms),
            limit: Some(MAX_WINDOW_CHUNKS),
            exclude_dropped: true,
            source_scope,
            ..Default::default()
        },
    )?;

    let mut by_source: HashMap<String, Vec<Chunk>> = HashMap::new();
    for chunk in chunks {
        let scope = chunk_tree_scope(&chunk.metadata);
        by_source.entry(scope).or_default().push(chunk);
    }

    // An exact `source_id` filter means `chunks` is a strict subset of its
    // (possibly shared) tree, so shared-tree summaries must be restricted to the
    // requested leaves.
    let exact_source = source_id.is_some();
    let mut hits: Vec<RetrievalHit> = Vec::new();
    for (source, src_chunks) in by_source {
        cover_one_source(
            config,
            &source,
            since_ms,
            until_ms,
            src_chunks,
            exact_source,
            &mut hits,
        )?;
    }
    Ok(hits)
}

/// Minimum cover for one source: frontier summaries (when the source has a
/// sealed tree) plus every in-window chunk they don't already cover, raw.
fn cover_one_source(
    config: &MemoryConfig,
    source: &str,
    since_ms: i64,
    until_ms: i64,
    chunks: Vec<Chunk>,
    exact_source: bool,
    out: &mut Vec<RetrievalHit>,
) -> Result<()> {
    let tree = get_tree_by_scope(config, TreeKind::Source, source)?;
    let (tree_id, eligible) = match &tree {
        Some(t) => (
            t.id.clone(),
            list_summaries_in_window(config, &t.id, since_ms, until_ms)?,
        ),
        None => (String::new(), Vec::new()),
    };
    // Latest-wins for versioned document sources.
    let (eligible, suppressed_chunk_ids) = filter_superseded_doc_versions(eligible);
    let present: HashSet<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
    let plan = plan_cover(&eligible, exact_source.then_some(&present));

    let by_id: HashMap<&str, &SummaryNode> = eligible.iter().map(|s| (s.id.as_str(), s)).collect();
    for id in &plan.maximal_ids {
        if let Some(node) = by_id.get(id.as_str()) {
            out.push(hydrated_summary_hit(config, node, source));
        }
    }

    for chunk in &chunks {
        if plan.covered_chunk_ids.contains(&chunk.id) || suppressed_chunk_ids.contains(&chunk.id) {
            continue;
        }
        out.push(hydrated_chunk_hit(config, chunk, &tree_id, source, 0.0));
    }
    Ok(())
}

/// The structural result of the cover for one tree's eligible summaries.
pub(crate) struct CoverPlan {
    /// Topmost eligible summary ids (eligible nodes whose parent is not
    /// eligible). These stand in for their whole subtree.
    pub(crate) maximal_ids: Vec<String>,
    /// Leaf chunk ids transitively covered by the `maximal` summaries.
    pub(crate) covered_chunk_ids: HashSet<String>,
}

/// Compute the frontier + covered-chunk set from a tree's eligible summaries.
///
/// A summary is **maximal** when its `parent_id` is not itself eligible.
/// Coverage descends `child_ids`: a child not in the eligible set is a leaf
/// chunk id. `restrict_to_present` guards the exact-source path — when `Some`, a
/// maximal summary is emitted only if every chunk it covers is in the present
/// set (so a shared-tree summary spanning sibling sources is dropped and its
/// in-filter chunks fall through to raw).
pub(crate) fn plan_cover(
    eligible: &[SummaryNode],
    restrict_to_present: Option<&HashSet<&str>>,
) -> CoverPlan {
    let eligible_ids: HashSet<&str> = eligible.iter().map(|s| s.id.as_str()).collect();
    let by_id: HashMap<&str, &SummaryNode> = eligible.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut maximal_ids: Vec<String> = Vec::new();
    let mut covered_chunk_ids: HashSet<String> = HashSet::new();
    for node in eligible.iter().filter(|s| match &s.parent_id {
        Some(parent) => !eligible_ids.contains(parent.as_str()),
        None => true,
    }) {
        let mut sub: HashSet<String> = HashSet::new();
        collect_descendant_chunks(node, &by_id, &mut sub);
        if let Some(present) = restrict_to_present {
            if !sub.iter().all(|c| present.contains(c.as_str())) {
                continue;
            }
        }
        maximal_ids.push(node.id.clone());
        covered_chunk_ids.extend(sub);
    }

    CoverPlan {
        maximal_ids,
        covered_chunk_ids,
    }
}

/// Walk a summary's subtree (within the eligible set) collecting leaf chunk ids.
fn collect_descendant_chunks(
    node: &SummaryNode,
    by_id: &HashMap<&str, &SummaryNode>,
    covered: &mut HashSet<String>,
) {
    for child in &node.child_ids {
        match by_id.get(child.as_str()) {
            Some(child_summary) => collect_descendant_chunks(child_summary, by_id, covered),
            None => {
                covered.insert(child.clone());
            }
        }
    }
}

/// Latest-wins for versioned document sources. Returns `eligible` with every
/// superseded revision's whole subtree removed, plus the chunk ids under those
/// dropped revisions so the raw fallback can't resurface stale page content.
pub(crate) fn filter_superseded_doc_versions(
    eligible: Vec<SummaryNode>,
) -> (Vec<SummaryNode>, HashSet<String>) {
    if !eligible.iter().any(|s| s.doc_id.is_some()) {
        return (eligible, HashSet::new());
    }

    let by_id: HashMap<&str, &SummaryNode> = eligible.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut max_version_by_doc: HashMap<&str, i64> = HashMap::new();
    for s in &eligible {
        if let Some(doc) = s.doc_id.as_deref() {
            let v = s.version_ms.unwrap_or(i64::MIN);
            max_version_by_doc
                .entry(doc)
                .and_modify(|m| {
                    if v > *m {
                        *m = v;
                    }
                })
                .or_insert(v);
        }
    }

    let mut winners_seen: HashSet<&str> = HashSet::new();
    let mut removed_summary_ids: HashSet<String> = HashSet::new();
    let mut suppressed_chunk_ids: HashSet<String> = HashSet::new();
    for s in &eligible {
        let Some(doc) = s.doc_id.as_deref() else {
            continue;
        };
        let v = s.version_ms.unwrap_or(i64::MIN);
        let max = max_version_by_doc.get(doc).copied().unwrap_or(i64::MIN);
        let loser = v < max || !winners_seen.insert(doc);
        if loser {
            removed_summary_ids.insert(s.id.clone());
            collect_subtree_ids(
                s,
                &by_id,
                &mut removed_summary_ids,
                &mut suppressed_chunk_ids,
            );
        }
    }

    let kept = eligible
        .into_iter()
        .filter(|s| !removed_summary_ids.contains(&s.id))
        .collect();
    (kept, suppressed_chunk_ids)
}

/// Walk a summary's subtree collecting both descendant summary ids and leaf
/// chunk ids. Used to evict a superseded document revision's whole subtree.
fn collect_subtree_ids(
    node: &SummaryNode,
    by_id: &HashMap<&str, &SummaryNode>,
    summaries: &mut HashSet<String>,
    chunks: &mut HashSet<String>,
) {
    for child in &node.child_ids {
        match by_id.get(child.as_str()) {
            Some(child_summary) => {
                summaries.insert(child.clone());
                collect_subtree_ids(child_summary, by_id, summaries, chunks);
            }
            None => {
                chunks.insert(child.clone());
            }
        }
    }
}

#[cfg(test)]
#[path = "cover_tests.rs"]
mod tests;
