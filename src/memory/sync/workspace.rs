//! Local workspace source synchronization through crate-owned readers.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::sources::{reader_for, MemorySourceEntry, SourceReader};
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    LocalDocument, SyncContext, SyncEvent, SyncOutcome, SyncPipeline, SyncPipelineKind, SyncStage,
};

pub struct WorkspaceSourcePipeline {
    id: String,
    source: MemorySourceEntry,
    reader: Option<Box<dyn SourceReader>>,
}

impl WorkspaceSourcePipeline {
    pub fn new(source: MemorySourceEntry) -> anyhow::Result<Self> {
        source.validate().map_err(anyhow::Error::msg)?;
        let reader = reader_for(&source.kind);
        Ok(Self {
            id: format!("workspace:{}:{}", source.kind.as_str(), source.id),
            source,
            reader,
        })
    }

    fn source_id(&self, item_id: &str) -> String {
        format!("mem_src:{}:{item_id}", self.source.id)
    }

    async fn event(&self, context: &SyncContext, stage: SyncStage, message: Option<String>) {
        let _ = context
            .events
            .emit(SyncEvent {
                source_id: self.id.clone(),
                toolkit: self.source.kind.as_str().into(),
                connection_id: Some(self.source.id.clone()),
                stage,
                message,
            })
            .await;
    }
}

#[async_trait]
impl SyncPipeline for WorkspaceSourcePipeline {
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
        if !self.source.enabled {
            return Ok(SyncOutcome {
                note: Some("source disabled".into()),
                ..SyncOutcome::default()
            });
        }
        self.event(context, SyncStage::Fetching, None).await;
        let state_toolkit = format!("workspace:{}", self.source.kind.as_str());
        let mut state =
            SyncState::load(context.state.as_ref(), &state_toolkit, &self.source.id).await?;
        let local_documents = context
            .local_documents
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("workspace pipeline requires a local document sink"))?;
        let items = match &self.reader {
            Some(reader) => reader
                .list_items(&self.source, config)
                .await
                .map_err(anyhow::Error::msg)?,
            None => {
                context
                    .external_sources
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "source kind requires an external reader: {}",
                            self.source.kind.as_str()
                        )
                    })?
                    .list_items(&self.source)
                    .await?
            }
        };
        let current_ids: HashSet<_> = items.iter().map(|item| item.id.clone()).collect();
        let mut versions = HashMap::with_capacity(items.len());
        let mut ingested = 0u32;

        for item in items {
            let version = item
                .updated_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into());
            versions.insert(item.id.clone(), version.clone());
            if item.updated_at_ms.is_some() && state.item_versions.get(&item.id) == Some(&version) {
                continue;
            }
            let source_id = self.source_id(&item.id);
            let content = match &self.reader {
                Some(reader) => reader
                    .read_item(&self.source, &item.id, config)
                    .await
                    .map_err(anyhow::Error::msg)?,
                None => {
                    context
                        .external_sources
                        .as_ref()
                        .expect("external reader checked before item loop")
                        .read_item(&self.source, &item.id)
                        .await?
                }
            };
            local_documents
                .upsert(LocalDocument {
                    source_id,
                    path_scope: None,
                    owner: "user".into(),
                    tags: vec!["memory_sources".into(), self.source.kind.as_str().into()],
                    title: content.title,
                    body: content.body,
                    modified_at: item
                        .updated_at_ms
                        .and_then(chrono::DateTime::from_timestamp_millis)
                        .unwrap_or_else(chrono::Utc::now),
                    source_ref: Some(format!("{}:{}", self.source.id, item.id)),
                })
                .await?;
            ingested = ingested.saturating_add(1);
        }

        let removed: Vec<_> = if self.source.kind == crate::memory::sources::SourceKind::Folder {
            state
                .item_versions
                .keys()
                .filter(|id| !current_ids.contains(*id))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        for item_id in &removed {
            local_documents.delete(&self.source_id(item_id)).await?;
        }
        state.item_versions = versions;
        state.last_sync_at_ms = Some(chrono::Utc::now().timestamp_millis() as u64);
        state.save(context.state.as_ref()).await?;
        self.event(
            context,
            SyncStage::Stored,
            Some(format!("{ingested} stored, {} removed", removed.len())),
        )
        .await;
        self.event(context, SyncStage::Completed, None).await;
        Ok(SyncOutcome {
            records_ingested: ingested,
            more_pending: false,
            actions_called: 0,
            provider_cost_usd: 0.0,
            note: (!removed.is_empty()).then(|| format!("{} removed", removed.len())),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::memory::sources::SourceKind;
    use crate::memory::sync::state::SyncStateStore;
    use crate::memory::sync::traits::{
        ExternalSourceReader, LocalDocumentSink, SkillDocSink, SkillDocument, SyncEventSink,
    };

    #[derive(Default)]
    struct Host {
        documents: Mutex<HashMap<String, LocalDocument>>,
        state: Mutex<HashMap<String, serde_json::Value>>,
        events: Mutex<Vec<SyncEvent>>,
        deletes: Mutex<Vec<String>>,
        external_items: Mutex<Vec<crate::memory::sources::SourceItem>>,
        external_bodies: Mutex<HashMap<String, crate::memory::sources::SourceContent>>,
    }

    #[async_trait]
    impl SkillDocSink for Host {
        async fn store(&self, _: SkillDocument) -> anyhow::Result<()> {
            Ok(())
        }
        async fn delete(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl LocalDocumentSink for Host {
        async fn upsert(&self, document: LocalDocument) -> anyhow::Result<()> {
            self.documents
                .lock()
                .unwrap()
                .insert(document.source_id.clone(), document);
            Ok(())
        }

        async fn delete(&self, source_id: &str) -> anyhow::Result<()> {
            self.documents.lock().unwrap().remove(source_id);
            self.deletes.lock().unwrap().push(source_id.into());
            Ok(())
        }
    }

    #[async_trait]
    impl SyncEventSink for Host {
        async fn emit(&self, event: SyncEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[async_trait]
    impl SyncStateStore for Host {
        async fn get(
            &self,
            namespace: &str,
            key: &str,
        ) -> anyhow::Result<Option<serde_json::Value>> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .get(&format!("{namespace}:{key}"))
                .cloned())
        }
        async fn set(
            &self,
            namespace: &str,
            key: &str,
            value: &serde_json::Value,
        ) -> anyhow::Result<()> {
            self.state
                .lock()
                .unwrap()
                .insert(format!("{namespace}:{key}"), value.clone());
            Ok(())
        }
    }

    #[async_trait]
    impl ExternalSourceReader for Host {
        async fn list_items(
            &self,
            _: &MemorySourceEntry,
        ) -> anyhow::Result<Vec<crate::memory::sources::SourceItem>> {
            Ok(self.external_items.lock().unwrap().clone())
        }

        async fn read_item(
            &self,
            _: &MemorySourceEntry,
            item_id: &str,
        ) -> anyhow::Result<crate::memory::sources::SourceContent> {
            self.external_bodies
                .lock()
                .unwrap()
                .get(item_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing external item {item_id}"))
        }
    }

    fn folder_source(path: &std::path::Path) -> MemorySourceEntry {
        MemorySourceEntry {
            id: "folder-1".into(),
            kind: SourceKind::Folder,
            label: "Notes".into(),
            enabled: true,
            toolkit: None,
            connection_id: None,
            path: Some(path.to_string_lossy().into_owned()),
            glob: Some("**/*.md".into()),
            url: None,
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

    #[tokio::test]
    async fn folder_pipeline_tracks_create_update_noop_and_remove() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let file = notes.join("daily.md");
        std::fs::write(&file, "first").unwrap();
        let config = MemoryConfig::new(temp.path().join("workspace"));
        let pipeline = WorkspaceSourcePipeline::new(folder_source(&notes)).unwrap();
        let host = Arc::new(Host::default());
        let context = SyncContext {
            events: host.clone(),
            documents: host.clone(),
            state: host.clone(),
            local_documents: Some(host.clone()),
            external_sources: None,
            summariser: None,
        };

        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            1
        );
        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            0
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file, "second").unwrap();
        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            1
        );
        assert_eq!(
            host.documents.lock().unwrap()["mem_src:folder-1:daily.md"].body,
            "second"
        );
        std::fs::remove_file(&file).unwrap();
        let removed = pipeline.tick(&config, &context).await.unwrap();
        assert_eq!(removed.records_ingested, 0);
        assert_eq!(removed.note.as_deref(), Some("1 removed"));
        assert!(host.documents.lock().unwrap().is_empty());
        assert_eq!(
            host.deletes.lock().unwrap().as_slice(),
            ["mem_src:folder-1:daily.md"]
        );
    }

    #[tokio::test]
    async fn external_pipeline_tracks_versions_without_destructive_absence() {
        use crate::memory::sources::{ContentType, SourceContent, SourceItem};

        let config = MemoryConfig::new(tempfile::tempdir().unwrap().path().join("workspace"));
        let mut source = folder_source(std::path::Path::new("unused"));
        source.id = "rss-1".into();
        source.kind = SourceKind::RssFeed;
        source.path = None;
        source.glob = None;
        source.url = Some("https://example.test/feed.xml".into());
        let pipeline = WorkspaceSourcePipeline::new(source).unwrap();
        let host = Arc::new(Host::default());
        host.external_items.lock().unwrap().push(SourceItem {
            id: "post-1".into(),
            title: "Post".into(),
            updated_at_ms: Some(1),
        });
        host.external_bodies.lock().unwrap().insert(
            "post-1".into(),
            SourceContent {
                id: "post-1".into(),
                title: "Post".into(),
                body: "first".into(),
                content_type: ContentType::Plaintext,
                metadata: serde_json::Value::Null,
            },
        );
        let context = SyncContext {
            events: host.clone(),
            documents: host.clone(),
            state: host.clone(),
            local_documents: Some(host.clone()),
            external_sources: Some(host.clone()),
            summariser: None,
        };

        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            1
        );
        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            0
        );
        host.external_items.lock().unwrap()[0].updated_at_ms = Some(2);
        host.external_bodies.lock().unwrap().remove("post-1");
        assert!(pipeline.tick(&config, &context).await.is_err());
        assert_eq!(
            host.documents.lock().unwrap()["mem_src:rss-1:post-1"].body,
            "first"
        );
        host.external_bodies.lock().unwrap().insert(
            "post-1".into(),
            SourceContent {
                id: "post-1".into(),
                title: "Post".into(),
                body: "second".into(),
                content_type: ContentType::Plaintext,
                metadata: serde_json::Value::Null,
            },
        );
        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            1
        );
        host.external_items.lock().unwrap()[0].updated_at_ms = None;
        host.external_bodies
            .lock()
            .unwrap()
            .get_mut("post-1")
            .unwrap()
            .body = "timestamp-less refresh".into();
        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            1
        );
        host.external_items.lock().unwrap().clear();
        assert_eq!(
            pipeline
                .tick(&config, &context)
                .await
                .unwrap()
                .records_ingested,
            0
        );
        assert!(host
            .documents
            .lock()
            .unwrap()
            .contains_key("mem_src:rss-1:post-1"));
    }
}
