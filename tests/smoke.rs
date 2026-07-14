use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use tinycortex::memory::chunks::get_chunk;
use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::ingest::canonicalize::chat::{ChatBatch, ChatMessage};
use tinycortex::memory::ingest::{ingest_chat, QueueJobSink};
use tinycortex::memory::queue::{
    drain_until_idle, AppendDecision, AppendTarget, ExtractDecision, NodeRef, QueueDelegates,
    ReembedProgress, SealDocumentPayload, SealPayload, StaleBuffer,
};
use tinycortex::memory::retrieval::query_source;
use tinycortex::memory::score::embed::InertEmbedder;
use tinycortex::memory::score::ScoringConfig;
use tinycortex::memory::tree::store::get_tree;
use tinycortex::memory::tree::{append_leaf_deferred, ConcatSummariser, LeafRef, TreeFactory};
use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

struct EngineDelegates;

#[async_trait]
impl QueueDelegates for EngineDelegates {
    async fn extract_chunk(
        &self,
        config: &MemoryConfig,
        chunk_id: &str,
    ) -> anyhow::Result<Option<ExtractDecision>> {
        Ok(get_chunk(config, chunk_id)?.map(|chunk| ExtractDecision {
            kept: true,
            uses_document_subtree: false,
            tree_scope: chunk.metadata.source_id,
        }))
    }

    async fn append_node(
        &self,
        config: &MemoryConfig,
        node: &NodeRef,
        target: &AppendTarget,
    ) -> anyhow::Result<Option<AppendDecision>> {
        let NodeRef::Leaf { chunk_id } = node else {
            return Ok(None);
        };
        let Some(chunk) = get_chunk(config, chunk_id)? else {
            return Ok(None);
        };
        let source_id = match target {
            AppendTarget::Source { source_id } => source_id,
            AppendTarget::Topic { .. } => return Ok(None),
        };
        let factory = TreeFactory::source(source_id.as_str());
        let tree = factory.get_or_create(config)?;
        append_leaf_deferred(
            config,
            &tree,
            &LeafRef {
                chunk_id: chunk.id,
                token_count: chunk.token_count,
                timestamp: chunk.metadata.timestamp,
                content: chunk.content,
                entities: vec![],
                topics: vec![],
                score: 1.0,
            },
        )?;
        Ok(Some(AppendDecision {
            tree_id: tree.id,
            should_seal: true,
        }))
    }

    async fn seal_level(
        &self,
        config: &MemoryConfig,
        payload: &SealPayload,
    ) -> anyhow::Result<Option<SealPayload>> {
        let tree = get_tree(config, &payload.tree_id)?.expect("queued tree exists");
        TreeFactory::from_tree(&tree)
            .seal_now(config, &ConcatSummariser::new())
            .await?;
        Ok(None)
    }

    async fn list_stale_buffers(
        &self,
        _config: &MemoryConfig,
        _max_age_secs: i64,
    ) -> anyhow::Result<Vec<StaleBuffer>> {
        Ok(vec![])
    }

    async fn seal_document(
        &self,
        _config: &MemoryConfig,
        _payload: &SealDocumentPayload,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn reembed_batch(
        &self,
        _config: &MemoryConfig,
        _signature: &str,
    ) -> anyhow::Result<ReembedProgress> {
        Ok(ReembedProgress::Covered)
    }

    fn active_signature(&self, _config: &MemoryConfig) -> String {
        "inert".into()
    }

    fn has_uncovered_reembed_work(
        &self,
        _config: &MemoryConfig,
        _signature: &str,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }
}

#[tokio::test]
async fn stores_and_finds_memory() {
    let store = InMemoryMemoryStore::new();
    store
        .insert(MemoryInput::new(
            "default",
            "TinyCortex starts as a Rust memory core",
        ))
        .await
        .expect("insert memory");

    let hits = store
        .search(MemoryQuery::text("Rust memory"))
        .await
        .expect("search memory");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.namespace, "default");
}

#[tokio::test]
async fn ingests_drains_seals_and_retrieves_real_engine_data() {
    let workspace = TempDir::new().unwrap();
    let config = MemoryConfig::new(workspace.path());
    let batch = ChatBatch {
        platform: "slack".into(),
        channel_label: "#launch".into(),
        messages: vec![ChatMessage {
            author: "alice".into(),
            timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            text: "The Phoenix launch is Friday after the staging review and runbook sign-off."
                .into(),
            source_ref: Some("slack://launch/1".into()),
        }],
    };

    let ingested = ingest_chat(
        &config,
        "slack:#launch",
        "alice",
        vec![],
        batch,
        &QueueJobSink,
        &ScoringConfig::from_memory_config(&config),
    )
    .await
    .unwrap();
    assert_eq!(ingested.chunks_written, 1);
    assert_eq!(ingested.extract_jobs_enqueued, 1);

    drain_until_idle(&config, &EngineDelegates).await.unwrap();
    let response = query_source(
        &config,
        Some("slack:#launch"),
        None,
        None,
        None,
        &InertEmbedder,
        10,
    )
    .await
    .unwrap();
    assert_eq!(response.hits.len(), 1);
    assert!(response.hits[0].content.contains("Phoenix launch"));
}
