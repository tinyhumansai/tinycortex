//! Raw archive coverage detection and incremental rebuild inputs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::memory::chunks::{
    count_raw_paths_ingested_with_prefix, filter_raw_paths_not_ingested,
    list_chunk_raw_ref_paths_with_prefix, mark_raw_paths_ingested,
};
use crate::memory::config::MemoryConfig;
use crate::memory::store::content::{raw_source_dir, sanitize_uid, slugify_source_id};
use crate::memory::tree::{
    fallback_summary, ingest_summary, Summariser, SummaryContext, SummaryIngestInput, SummaryInput,
    TreeKind,
};
use crate::memory::tree::{store::list_summaries_at_level, TreeFactory};

use super::audit::{append_audit_entry, RealCostAccumulator, SyncAuditEntry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFileRef {
    pub abs: PathBuf,
    pub rel: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawCoverage {
    pub total: usize,
    pub covered: usize,
    pub pending: Vec<RawFileRef>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RebuildOutcome {
    pub files_read: usize,
    pub batches: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub actual_charged_usd: Option<f64>,
}

pub fn raw_coverage(
    config: &MemoryConfig,
    tree_scope: &str,
    archive_source_id: &str,
) -> anyhow::Result<RawCoverage> {
    let content_root = config.workspace.join("memory_tree/content");
    let source_directory = raw_source_dir(&content_root, archive_source_id);
    if !source_directory.exists() {
        return Ok(RawCoverage::default());
    }

    let mut files = Vec::new();
    collect_raw_files(&source_directory, &mut files)?;
    files.sort();
    let references: Vec<_> = files
        .into_iter()
        .filter_map(|abs| {
            let rel = abs
                .strip_prefix(&content_root)
                .ok()?
                .to_str()?
                .replace(std::path::MAIN_SEPARATOR, "/");
            Some(RawFileRef { abs, rel })
        })
        .collect();
    let total = references.len();
    if total == 0 {
        return Ok(RawCoverage::default());
    }

    let relative_prefix = format!("raw/{}/", slugify_source_id(archive_source_id));
    if count_raw_paths_ingested_with_prefix(config, &relative_prefix)? == 0 {
        backfill_coverage_from_summaries(config, tree_scope, &references)?;
    }

    let relative_paths: Vec<_> = references
        .iter()
        .map(|reference| reference.rel.clone())
        .collect();
    let mut pending_paths: HashSet<_> = filter_raw_paths_not_ingested(config, &relative_paths)?
        .into_iter()
        .collect();
    let chunk_covered = list_chunk_raw_ref_paths_with_prefix(config, &relative_prefix)?;
    pending_paths.retain(|path| !chunk_covered.contains(path));
    let pending: Vec<_> = references
        .into_iter()
        .filter(|reference| pending_paths.contains(&reference.rel))
        .collect();
    tracing::debug!(
        tree_scope,
        archive_source_id,
        total,
        pending = pending.len(),
        "[memory_sync:rebuild] raw coverage computed"
    );
    Ok(RawCoverage {
        total,
        covered: total.saturating_sub(pending.len()),
        pending,
    })
}

pub fn needs_rebuild(config: &MemoryConfig, tree_scope: &str, archive_source_id: &str) -> bool {
    match raw_coverage(config, tree_scope, archive_source_id) {
        Ok(coverage) => !coverage.pending.is_empty(),
        Err(error) => {
            tracing::warn!(tree_scope, archive_source_id, %error, "[memory_sync:rebuild] coverage check failed");
            false
        }
    }
}

pub async fn rebuild_tree_from_raw(
    config: &MemoryConfig,
    tree_scope: &str,
    archive_source_id: &str,
    summariser: &dyn Summariser,
) -> anyhow::Result<RebuildOutcome> {
    rebuild_tree_from_raw_with_audit(
        config,
        tree_scope,
        archive_source_id,
        summariser,
        &format!("rebuild:{tree_scope}"),
        "rebuild",
    )
    .await
}

pub async fn rebuild_tree_from_raw_with_audit(
    config: &MemoryConfig,
    tree_scope: &str,
    archive_source_id: &str,
    summariser: &dyn Summariser,
    audit_source_id: &str,
    audit_source_kind: &str,
) -> anyhow::Result<RebuildOutcome> {
    let started = std::time::Instant::now();
    let coverage = raw_coverage(config, tree_scope, archive_source_id)?;
    if coverage.pending.is_empty() {
        return Ok(RebuildOutcome::default());
    }

    let mut inputs = Vec::new();
    for file in &coverage.pending {
        let body = match std::fs::read_to_string(&file.abs) {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(path = %file.abs.display(), %error, "[memory_sync:rebuild] unreadable raw file skipped");
                continue;
            }
        };
        let stem = file
            .abs
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_owned();
        let timestamp_ms = stem
            .split('_')
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let timestamp =
            chrono::DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_else(chrono::Utc::now);
        inputs.push(RebuildInput {
            summary: SummaryInput {
                id: stem.clone(),
                token_count: crate::memory::chunks::approx_token_count(&body).max(1),
                content: body,
                entities: Vec::new(),
                topics: Vec::new(),
                time_range_start: timestamp,
                time_range_end: timestamp,
                score: 0.5,
            },
            label: stem,
            relative_path: file.rel.clone(),
        });
    }
    if inputs.is_empty() {
        return Ok(RebuildOutcome {
            files_read: coverage.pending.len(),
            ..RebuildOutcome::default()
        });
    }

    let tree = TreeFactory::source(tree_scope).get_or_create(config)?;
    let batches = batch_inputs(inputs, config.tree.input_token_budget);
    let batch_count = batches.len();
    let files_read = batches.iter().map(Vec::len).sum();
    let mut cost = RealCostAccumulator::new();

    for (batch_index, batch) in batches.into_iter().enumerate() {
        let summary_inputs: Vec<_> = batch.iter().map(|item| item.summary.clone()).collect();
        let context = SummaryContext {
            tree_id: &tree.id,
            tree_kind: TreeKind::Source,
            target_level: 1,
            token_budget: config.tree.output_token_budget,
        };
        let call = match summariser
            .summarise_with_usage(&summary_inputs, &context)
            .await
        {
            Ok(call) if !call.output.content.trim().is_empty() => call,
            Ok(_) => {
                tracing::warn!(
                    batch_index,
                    "[memory_sync:rebuild] blank summary; using fallback"
                );
                crate::memory::tree::SummaryCall {
                    output: fallback_summary(&summary_inputs, context.token_budget),
                    ..Default::default()
                }
            }
            Err(error) => {
                tracing::warn!(batch_index, %error, "[memory_sync:rebuild] summariser failed; using fallback");
                crate::memory::tree::SummaryCall {
                    output: fallback_summary(&summary_inputs, context.token_budget),
                    ..Default::default()
                }
            }
        };
        let output = call.output;
        let batch_input_tokens: u64 = summary_inputs
            .iter()
            .map(|input| input.token_count as u64)
            .sum();
        cost.add_batch(
            batch_input_tokens,
            output.token_count as u64,
            call.input_tokens,
            call.output_tokens,
            call.charged_amount_usd,
        );
        let time_range_start = summary_inputs
            .iter()
            .map(|input| input.time_range_start)
            .min()
            .unwrap_or_else(chrono::Utc::now);
        let time_range_end = summary_inputs
            .iter()
            .map(|input| input.time_range_end)
            .max()
            .unwrap_or_else(chrono::Utc::now);
        ingest_summary(
            config,
            &tree,
            SummaryIngestInput {
                content: output.content,
                token_count: output.token_count,
                entities: output.entities,
                topics: output.topics,
                time_range_start,
                time_range_end,
                score: 0.5,
                child_labels: batch.iter().map(|input| input.label.clone()).collect(),
                child_basenames: Vec::new(),
            },
            summariser,
        )
        .await?;
        let covered: Vec<_> = batch
            .iter()
            .map(|input| input.relative_path.clone())
            .collect();
        mark_raw_paths_ingested(config, &covered)?;
        tracing::info!(
            tree_scope,
            batch_index,
            files = covered.len(),
            "[memory_sync:rebuild] batch ingested and covered"
        );
    }

    let input_tokens = cost.audit_input_tokens();
    let output_tokens = cost.audit_output_tokens();
    let estimated_cost_usd = cost.estimated_cost();
    let actual_charged_usd = cost.actual_charged_usd();
    append_audit_entry(
        config,
        &SyncAuditEntry {
            timestamp: chrono::Utc::now(),
            source_id: audit_source_id.into(),
            source_kind: audit_source_kind.into(),
            scope: tree_scope.into(),
            items_fetched: files_read as u32,
            batches: batch_count as u32,
            input_tokens,
            output_tokens,
            estimated_cost_usd,
            composio_actions_called: 0,
            composio_cost_usd: 0.0,
            actual_charged_usd,
            duration_ms: started.elapsed().as_millis() as u64,
            success: true,
            error: None,
        },
    )?;
    Ok(RebuildOutcome {
        files_read,
        batches: batch_count,
        input_tokens,
        output_tokens,
        estimated_cost_usd,
        actual_charged_usd,
    })
}

#[derive(Clone)]
struct RebuildInput {
    summary: SummaryInput,
    label: String,
    relative_path: String,
}

fn batch_inputs(inputs: Vec<RebuildInput>, token_budget: u32) -> Vec<Vec<RebuildInput>> {
    let budget = token_budget.max(1) as u64;
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_tokens = 0u64;
    for input in inputs {
        let tokens = input.summary.token_count as u64;
        if !current.is_empty() && current_tokens.saturating_add(tokens) > budget {
            batches.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        current_tokens = current_tokens.saturating_add(tokens);
        current.push(input);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn collect_raw_files(directory: &Path, output: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_raw_files(&path, output)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("md")
            && path.file_name().and_then(|name| name.to_str()) != Some("_source.md")
        {
            output.push(path);
        }
    }
    Ok(())
}

fn backfill_coverage_from_summaries(
    config: &MemoryConfig,
    tree_scope: &str,
    files: &[RawFileRef],
) -> anyhow::Result<u64> {
    let tree = TreeFactory::source(tree_scope).get_or_create(config)?;
    if tree.max_level == 0 {
        return Ok(0);
    }
    let summaries = list_summaries_at_level(config, &tree.id, 1)?;
    let mut kind_uids: HashSet<(&'static str, String)> = HashSet::new();
    let mut stems = HashSet::new();
    for summary in summaries {
        for label in summary.child_ids {
            if let Some(uid) = label.strip_prefix("commit:") {
                kind_uids.insert(("commits", sanitize_uid(uid)));
            } else if let Some(uid) = label.strip_prefix("issue:") {
                kind_uids.insert(("issues", sanitize_uid(uid)));
            } else if let Some(uid) = label.strip_prefix("pr:") {
                kind_uids.insert(("prs", sanitize_uid(uid)));
            } else {
                stems.insert(label);
            }
        }
    }
    let covered: Vec<_> = files
        .iter()
        .filter(|reference| {
            let path = Path::new(&reference.rel);
            let kind = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str());
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            stems.contains(stem)
                || kind_uids.iter().any(|(expected_kind, uid)| {
                    kind == Some(*expected_kind)
                        && stem
                            .split_once('_')
                            .is_some_and(|(_, file_uid)| file_uid == uid)
                })
        })
        .map(|reference| reference.rel.clone())
        .collect();
    if covered.is_empty() {
        Ok(0)
    } else {
        mark_raw_paths_ingested(config, &covered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_is_incremental_and_ignores_source_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let config = MemoryConfig::new(temp.path());
        let root = config
            .workspace
            .join("memory_tree/content/raw/github-com-org-repo/issues");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("100_one.md"), "one").unwrap();
        std::fs::write(root.join("200_two.md"), "two").unwrap();
        std::fs::write(root.parent().unwrap().join("_source.md"), "metadata").unwrap();

        let first = raw_coverage(&config, "github:org/repo", "github.com/org/repo").unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.pending.len(), 2);
        mark_raw_paths_ingested(&config, &[first.pending[0].rel.clone()]).unwrap();
        let second = raw_coverage(&config, "github:org/repo", "github.com/org/repo").unwrap();
        assert_eq!(second.covered, 1);
        assert_eq!(second.pending.len(), 1);
        assert!(needs_rebuild(
            &config,
            "github:org/repo",
            "github.com/org/repo"
        ));
    }

    #[tokio::test]
    async fn rebuild_ingests_l1_summaries_marks_coverage_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = MemoryConfig::new(temp.path());
        config.tree.input_token_budget = 2;
        let root = config
            .workspace
            .join("memory_tree/content/raw/github-com-org-repo/issues");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("100_one.md"), "first body").unwrap();
        std::fs::write(root.join("200_two.md"), "second body").unwrap();

        let first = rebuild_tree_from_raw(
            &config,
            "github:org/repo",
            "github.com/org/repo",
            &crate::memory::tree::ConcatSummariser,
        )
        .await
        .unwrap();
        assert_eq!(first.files_read, 2);
        assert_eq!(first.batches, 2);
        assert!(!needs_rebuild(
            &config,
            "github:org/repo",
            "github.com/org/repo"
        ));
        assert_eq!(crate::memory::chunks::count_chunks(&config).unwrap(), 0);
        let tree = TreeFactory::source("github:org/repo")
            .get_or_create(&config)
            .unwrap();
        assert_eq!(
            list_summaries_at_level(&config, &tree.id, 1).unwrap().len(),
            2
        );

        let second = rebuild_tree_from_raw(
            &config,
            "github:org/repo",
            "github.com/org/repo",
            &crate::memory::tree::ConcatSummariser,
        )
        .await
        .unwrap();
        assert_eq!(second, RebuildOutcome::default());
        assert_eq!(
            list_summaries_at_level(&config, &tree.id, 1).unwrap().len(),
            2
        );
    }
}
