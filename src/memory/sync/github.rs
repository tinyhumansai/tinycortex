//! GitHub repository synchronization through a host-provided network reader.

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::sources::MemorySourceEntry;
use crate::memory::store::content::{write_raw_items, RawItem, RawKind};

use super::rebuild::rebuild_tree_from_raw_with_audit;
use super::traits::{
    SyncContext, SyncEvent, SyncOutcome, SyncPipeline, SyncPipelineKind, SyncStage,
};

pub struct GithubRepoSyncPipeline {
    id: String,
    source: MemorySourceEntry,
}

impl GithubRepoSyncPipeline {
    pub fn new(source: MemorySourceEntry) -> anyhow::Result<Self> {
        source.validate().map_err(anyhow::Error::msg)?;
        if source.kind != crate::memory::sources::SourceKind::GithubRepo {
            anyhow::bail!("GitHub repo pipeline requires github_repo source");
        }
        Ok(Self {
            id: format!("workspace:github_repo:{}", source.id),
            source,
        })
    }

    async fn event(&self, context: &SyncContext, stage: SyncStage, message: Option<String>) {
        let _ = context
            .events
            .emit(SyncEvent {
                source_id: self.id.clone(),
                toolkit: "github_repo".into(),
                connection_id: Some(self.source.id.clone()),
                stage,
                message,
            })
            .await;
    }
}

#[async_trait]
impl SyncPipeline for GithubRepoSyncPipeline {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Workspace
    }

    async fn init(&self, _: &MemoryConfig, _: &SyncContext) -> anyhow::Result<()> {
        Ok(())
    }

    async fn tick(
        &self,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> anyhow::Result<SyncOutcome> {
        let reader = context
            .external_sources
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("GitHub repo pipeline requires an external reader"))?;
        let summariser = context
            .summariser
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("GitHub repo pipeline requires a summariser"))?;
        let url = self
            .source
            .url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("GitHub repo source missing url"))?;
        let (owner, repo) = parse_repo(url)?;
        let tree_scope = format!("github:{owner}/{repo}");
        let archive_source_id = format!("github.com/{owner}/{repo}");

        self.event(context, SyncStage::Fetching, None).await;
        let items = reader.list_items(&self.source).await?;
        let content_root = config.workspace.join("memory_tree/content");
        let mut archived = 0u32;
        for item in &items {
            let Some((kind, uid)) = raw_coordinates(&item.id) else {
                tracing::warn!(item_id = %item.id, "[memory_sync:github] unsupported item id skipped");
                continue;
            };
            let content = match reader.read_item(&self.source, &item.id).await {
                Ok(content) => content,
                Err(error) => {
                    tracing::warn!(item_id = %item.id, %error, "[memory_sync:github] item read failed");
                    continue;
                }
            };
            write_raw_items(
                &content_root,
                &archive_source_id,
                &[RawItem {
                    uid: &uid,
                    created_at_ms: item.updated_at_ms.unwrap_or(0),
                    markdown: &content.body,
                    kind,
                }],
            )?;
            archived = archived.saturating_add(1);
        }

        self.event(
            context,
            SyncStage::Ingesting,
            Some(format!("{archived} items archived")),
        )
        .await;
        let rebuilt = rebuild_tree_from_raw_with_audit(
            config,
            &tree_scope,
            &archive_source_id,
            summariser.as_ref(),
            &self.source.id,
            self.source.kind.as_str(),
        )
        .await?;
        self.event(context, SyncStage::Completed, None).await;
        Ok(SyncOutcome {
            records_ingested: archived,
            more_pending: false,
            note: Some(format!(
                "{archived} items archived, {} summaries rebuilt",
                rebuilt.batches
            )),
            ..SyncOutcome::default()
        })
    }
}

fn parse_repo(url: &str) -> anyhow::Result<(String, String)> {
    let path = url
        .trim()
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(url.trim().trim_end_matches('/'));
    let path = path
        .strip_prefix("https://github.com/")
        .or_else(|| path.strip_prefix("http://github.com/"))
        .or_else(|| path.strip_prefix("git@github.com:"))
        .ok_or_else(|| anyhow::anyhow!("unsupported GitHub repository URL"))?;
    let mut parts = path.split('/');
    let owner = parts.next().filter(|part| !part.is_empty());
    let repo = parts.next().filter(|part| !part.is_empty());
    if parts.next().is_some() {
        anyhow::bail!("GitHub repository URL must identify one repository");
    }
    Ok((
        owner
            .ok_or_else(|| anyhow::anyhow!("GitHub URL missing owner"))?
            .into(),
        repo.ok_or_else(|| anyhow::anyhow!("GitHub URL missing repository"))?
            .into(),
    ))
}

fn raw_coordinates(item_id: &str) -> Option<(RawKind, String)> {
    let (prefix, uid) = item_id.split_once(':')?;
    let kind = match prefix {
        "commit" => RawKind::Commit,
        "issue" => RawKind::Issue,
        "pr" => RawKind::PullRequest,
        _ => return None,
    };
    (!uid.is_empty()).then(|| (kind, uid.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_repo_urls() {
        assert_eq!(
            parse_repo("https://github.com/tinyhumansai/openhuman.git").unwrap(),
            ("tinyhumansai".into(), "openhuman".into())
        );
        assert_eq!(
            parse_repo("git@github.com:tinyhumansai/openhuman").unwrap(),
            ("tinyhumansai".into(), "openhuman".into())
        );
    }

    #[test]
    fn maps_raw_item_coordinates() {
        assert_eq!(
            raw_coordinates("issue:42"),
            Some((RawKind::Issue, "42".into()))
        );
        assert_eq!(raw_coordinates("unknown:42"), None);
    }
}
