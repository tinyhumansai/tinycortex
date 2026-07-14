//! Direct ingestion of pre-built L1 summaries.

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::memory::chunks::{tree_active_signature, with_connection};
use crate::memory::config::MemoryConfig;
use crate::memory::score::store::index_summary_entity_ids_tx;
use crate::memory::store::content::{
    slugify_source_id, stage_summary, SummaryComposeInput, SummaryTreeKind,
};
use crate::memory::tree::bucket_seal::cascade_all_from;
use crate::memory::tree::registry::new_summary_id;
use crate::memory::tree::store::{self, SummaryNode, Tree};
use crate::memory::tree::summarise::Summariser;
use crate::memory::tree::TreeFactory;

#[derive(Clone, Debug)]
pub struct SummaryIngestInput {
    pub content: String,
    pub token_count: u32,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub time_range_start: DateTime<Utc>,
    pub time_range_end: DateTime<Utc>,
    pub score: f32,
    pub child_labels: Vec<String>,
    pub child_basenames: Vec<Option<String>>,
}

#[derive(Clone, Debug)]
pub struct SummaryIngestOutcome {
    pub summary_id: String,
    pub content_path: String,
    pub sealed_ids: Vec<String>,
}

pub async fn ingest_summary(
    config: &MemoryConfig,
    tree: &Tree,
    input: SummaryIngestInput,
    summariser: &dyn Summariser,
) -> Result<SummaryIngestOutcome> {
    const TARGET_LEVEL: u32 = 1;
    let summary_id = new_summary_id(TARGET_LEVEL);
    let sealed_at = Utc::now();
    let node = SummaryNode {
        id: summary_id.clone(),
        tree_id: tree.id.clone(),
        tree_kind: tree.kind,
        level: TARGET_LEVEL,
        parent_id: None,
        child_ids: input.child_labels,
        content: input.content,
        token_count: input.token_count,
        entities: input.entities,
        topics: input.topics,
        time_range_start: input.time_range_start,
        time_range_end: input.time_range_end,
        score: input.score,
        sealed_at,
        deleted: false,
        embedding: None,
        doc_id: None,
        version_ms: None,
    };
    let content_root = crate::memory::chunks::content_root(config);
    let summary_tree_kind = match tree.kind {
        crate::memory::tree::TreeKind::Source => SummaryTreeKind::Source,
        crate::memory::tree::TreeKind::Topic => SummaryTreeKind::Topic,
        crate::memory::tree::TreeKind::Global => SummaryTreeKind::Global,
        crate::memory::tree::TreeKind::Flavoured => SummaryTreeKind::Flavoured,
    };
    let staged = stage_summary(
        &content_root,
        &SummaryComposeInput {
            summary_id: &summary_id,
            tree_kind: summary_tree_kind,
            tree_id: &tree.id,
            tree_scope: &tree.scope,
            level: TARGET_LEVEL,
            child_ids: &node.child_ids,
            child_basenames: if input.child_basenames.is_empty() {
                None
            } else {
                Some(&input.child_basenames)
            },
            child_count: node.child_ids.len(),
            time_range_start: node.time_range_start,
            time_range_end: node.time_range_end,
            sealed_at,
            body: &node.content,
        },
        &slugify_source_id(&tree.scope),
    )?;
    let signature = tree_active_signature(config);
    with_connection(config, |connection| {
        let transaction = connection.unchecked_transaction()?;
        let current_max: i64 = transaction.query_row(
            "SELECT max_level FROM mem_tree_trees WHERE id = ?1",
            [&tree.id],
            |row| row.get(0),
        )?;
        store::insert_staged_summary_tx(&transaction, &node, Some(&staged), &signature)?;
        index_summary_entity_ids_tx(
            &transaction,
            &node.entities,
            &node.id,
            node.score,
            sealed_at.timestamp_millis(),
            Some(&tree.id),
        )?;
        let mut buffer = store::get_buffer_conn(&transaction, &tree.id, TARGET_LEVEL)?;
        if !buffer.item_ids.contains(&summary_id) {
            buffer.item_ids.push(summary_id.clone());
            buffer.token_sum = buffer.token_sum.saturating_add(node.token_count as i64);
            buffer.oldest_at = Some(buffer.oldest_at.map_or(node.time_range_start, |existing| {
                existing.min(node.time_range_start)
            }));
            store::upsert_buffer_tx(&transaction, &buffer)?;
        }
        if TARGET_LEVEL > current_max.max(0) as u32 {
            store::update_tree_after_seal_tx(
                &transaction,
                &tree.id,
                &summary_id,
                TARGET_LEVEL,
                sealed_at,
            )?;
        }
        transaction.commit()?;
        Ok(())
    })?;

    let strategy = TreeFactory::from_tree(tree).label_strategy();
    let sealed_ids =
        cascade_all_from(config, tree, TARGET_LEVEL, false, summariser, &strategy).await?;
    #[cfg(feature = "sync")]
    tracing::info!(summary_id, tree_id = %tree.id, cascaded = sealed_ids.len(), "[memory_tree:direct_ingest] summary ingested");
    Ok(SummaryIngestOutcome {
        summary_id,
        content_path: staged.content_path,
        sealed_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tree::{store::get_buffer, ConcatSummariser};

    #[tokio::test]
    async fn direct_summary_lands_at_l1_without_creating_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let config = MemoryConfig::new(temp.path());
        let tree = TreeFactory::source("github:org/repo")
            .get_or_create(&config)
            .unwrap();
        let now = Utc::now();
        let outcome = ingest_summary(
            &config,
            &tree,
            SummaryIngestInput {
                content: "summary".into(),
                token_count: 2,
                entities: Vec::new(),
                topics: Vec::new(),
                time_range_start: now,
                time_range_end: now,
                score: 0.5,
                child_labels: vec!["100_issue-1".into()],
                child_basenames: Vec::new(),
            },
            &ConcatSummariser,
        )
        .await
        .unwrap();
        assert_eq!(crate::memory::chunks::count_chunks(&config).unwrap(), 0);
        let summary = store::get_summary(&config, &outcome.summary_id)
            .unwrap()
            .unwrap();
        assert_eq!(summary.level, 1);
        assert_eq!(summary.child_ids, vec!["100_issue-1"]);
        assert_eq!(get_buffer(&config, &tree.id, 1).unwrap().item_ids.len(), 1);
    }
}
