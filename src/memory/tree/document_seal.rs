//! Per-document immutable subtree sealing and merge-tier folding.

use anyhow::{Context, Result};
use chrono::Utc;
use futures::stream::{StreamExt, TryStreamExt};

use super::bucket_seal::{append_to_buffer, cascade_all_from_with_services};
use super::hydrate::hydrate_inputs;
use super::label_resolver::resolve_labels;
use super::registry::new_summary_id;
use super::store::{self, SummaryNode, Tree};
use super::summarise::{fallback_summary, SummaryContext};
use super::types::{LabelStrategy, SealServices};
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::score::store::index_summary_entity_ids_tx;
use crate::memory::store::content::{
    slugify_source_id, stage_summary_with_layout, SummaryComposeInput, SummaryDiskLayout,
    SummaryTreeKind,
};

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
    let root_id = doc_root.id.clone();
    let root_level = doc_root.level;
    with_connection(config, move |connection| {
        let transaction = connection.unchecked_transaction()?;
        let (current_root, current_max): (Option<String>, u32) = transaction.query_row(
            "SELECT root_id, max_level FROM mem_tree_trees WHERE id = ?1",
            [&tree.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if current_root.as_deref() != Some(root_id.as_str()) || root_level > current_max {
            store::update_tree_after_seal_tx(
                &transaction,
                &tree.id,
                &root_id,
                root_level,
                Utc::now(),
            )?;
        }
        transaction.commit()?;
        Ok(())
    })?;
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

#[allow(clippy::too_many_arguments)]
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
            input_token_budget: config.tree.input_token_budget,
            overhead_reserve_tokens: config.tree.summary_overhead_reserve_tokens,
            ask: tree.ask.as_deref(),
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
        crate::memory::tree::TreeKind::Flavoured => SummaryTreeKind::Flavoured,
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
