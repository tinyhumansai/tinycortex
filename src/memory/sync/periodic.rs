//! Host-neutral periodic synchronization cadence policy.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::memory::sources::{MemorySourceEntry, SourceKind};
use crate::memory::sync::audit::SyncAuditEntry;

pub const DEFAULT_SYNC_INTERVAL_SECS: u64 = 24 * 60 * 60;

pub fn effective_interval_secs(configured: Option<u64>) -> Option<u64> {
    match configured {
        Some(0) => None,
        Some(seconds) => Some(seconds),
        None => Some(DEFAULT_SYNC_INTERVAL_SECS),
    }
}

pub fn due_workspace_sources(
    sources: &[MemorySourceEntry],
    audit: &[SyncAuditEntry],
    configured_interval_secs: Option<u64>,
    now: DateTime<Utc>,
) -> Vec<MemorySourceEntry> {
    let Some(interval) = effective_interval_secs(configured_interval_secs) else {
        return Vec::new();
    };
    let successes = last_success_by_source(audit);
    sources
        .iter()
        .filter(|source| source.enabled && is_periodic_workspace_kind(&source.kind))
        .filter(|source| {
            successes
                .get(&source.id)
                .is_none_or(|last| elapsed_since(*last, now) >= Duration::from_secs(interval))
        })
        .cloned()
        .collect()
}

fn is_periodic_workspace_kind(kind: &SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::GithubRepo | SourceKind::Folder | SourceKind::RssFeed | SourceKind::WebPage
    )
}

fn last_success_by_source(audit: &[SyncAuditEntry]) -> HashMap<String, DateTime<Utc>> {
    let mut result = HashMap::new();
    for entry in audit.iter().filter(|entry| entry.success) {
        if !matches!(
            entry.source_kind.as_str(),
            "github_repo" | "folder" | "rss_feed" | "web_page"
        ) {
            continue;
        }
        result
            .entry(entry.source_id.clone())
            .and_modify(|current: &mut DateTime<Utc>| *current = (*current).max(entry.timestamp))
            .or_insert(entry.timestamp);
    }
    result
}

fn elapsed_since(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> Duration {
    Duration::from_secs((now - timestamp).num_seconds().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, kind: SourceKind, enabled: bool) -> MemorySourceEntry {
        MemorySourceEntry {
            id: id.into(),
            kind,
            label: id.into(),
            enabled,
            toolkit: None,
            connection_id: None,
            path: Some("/tmp".into()),
            glob: None,
            url: Some("https://example.com".into()),
            branch: None,
            paths: Vec::new(),
            max_commits: None,
            max_issues: None,
            max_prs: None,
            query: None,
            since_days: None,
            max_items: None,
            selector: None,
            max_tokens_per_sync: None,
            max_cost_per_sync_usd: None,
            sync_depth_days: None,
        }
    }

    fn audit(id: &str, timestamp: DateTime<Utc>, success: bool) -> SyncAuditEntry {
        SyncAuditEntry {
            timestamp,
            source_id: id.into(),
            source_kind: "folder".into(),
            scope: id.into(),
            items_fetched: 0,
            batches: 0,
            input_tokens: 0,
            output_tokens: 0,
            estimated_cost_usd: 0.0,
            composio_actions_called: 0,
            composio_cost_usd: 0.0,
            actual_charged_usd: None,
            duration_ms: 0,
            success,
            error: None,
        }
    }

    #[test]
    fn cadence_handles_manual_minimum_and_persisted_success() {
        assert_eq!(effective_interval_secs(Some(0)), None);
        assert_eq!(effective_interval_secs(Some(60)), Some(60));
        let now = Utc::now();
        let sources = vec![
            source("new", SourceKind::Folder, true),
            source("recent", SourceKind::Folder, true),
            source("old", SourceKind::Folder, true),
            source("disabled", SourceKind::Folder, false),
            source("conversation", SourceKind::Conversation, true),
        ];
        let history = vec![
            audit("recent", now - chrono::Duration::hours(1), true),
            audit("old", now - chrono::Duration::hours(25), true),
            audit("recent", now - chrono::Duration::hours(30), true),
        ];
        let due = due_workspace_sources(&sources, &history, None, now);
        assert_eq!(
            due.iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "old"]
        );
        assert!(due_workspace_sources(&sources, &history, Some(0), now).is_empty());
    }
}
