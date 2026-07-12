//! Host seams and pipeline contracts for live synchronization.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::memory::config::MemoryConfig;
use crate::memory::sources::{MemorySourceEntry, SourceContent, SourceItem};
use crate::memory::tree::Summariser;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPipelineKind {
    Composio,
    Workspace,
    Mcp,
}

impl SyncPipelineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Composio => "composio",
            Self::Workspace => "workspace",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStage {
    Requested,
    Fetching,
    Stored,
    Ingesting,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncEvent {
    pub source_id: String,
    pub toolkit: String,
    pub connection_id: Option<String>,
    pub stage: SyncStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[async_trait]
pub trait SyncEventSink: Send + Sync {
    async fn emit(&self, event: SyncEvent) -> anyhow::Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillDocument {
    pub namespace_skill_id: String,
    pub connection_id: String,
    pub document_id: String,
    pub title: String,
    pub content: String,
    pub toolkit: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[async_trait]
pub trait SkillDocSink: Send + Sync {
    async fn store(&self, document: SkillDocument) -> anyhow::Result<()>;
    async fn delete(&self, namespace_skill_id: &str, document_id: &str) -> anyhow::Result<()>;
}

#[derive(Clone, Debug)]
pub struct LocalDocument {
    pub source_id: String,
    pub path_scope: Option<String>,
    pub owner: String,
    pub tags: Vec<String>,
    pub title: String,
    pub body: String,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    pub source_ref: Option<String>,
}

#[async_trait]
pub trait LocalDocumentSink: Send + Sync {
    async fn upsert(&self, document: LocalDocument) -> anyhow::Result<()>;
    async fn delete(&self, source_id: &str) -> anyhow::Result<()>;
}

/// Host adapter for source kinds whose fetch transport is product-owned.
/// Tinycortex still owns lifecycle, deduplication, persistence, and events.
#[async_trait]
pub trait ExternalSourceReader: Send + Sync {
    async fn list_items(&self, source: &MemorySourceEntry) -> anyhow::Result<Vec<SourceItem>>;
    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
    ) -> anyhow::Result<SourceContent>;
}

/// Host capabilities required by sync pipelines.
#[derive(Clone)]
pub struct SyncContext {
    pub events: Arc<dyn SyncEventSink>,
    pub documents: Arc<dyn SkillDocSink>,
    pub state: Arc<dyn super::state::SyncStateStore>,
    pub local_documents: Option<Arc<dyn LocalDocumentSink>>,
    pub external_sources: Option<Arc<dyn ExternalSourceReader>>,
    pub summariser: Option<Arc<dyn Summariser>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncOutcome {
    pub records_ingested: u32,
    pub more_pending: bool,
    #[serde(default)]
    pub actions_called: u32,
    #[serde(default)]
    pub provider_cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SyncRunError {
    pub actions_called: u32,
    pub provider_cost_usd: f64,
    message: String,
}

impl SyncRunError {
    pub fn new(message: impl Into<String>, actions_called: u32, provider_cost_usd: f64) -> Self {
        Self {
            actions_called,
            provider_cost_usd,
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait SyncPipeline: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> SyncPipelineKind;
    async fn init(&self, config: &MemoryConfig, context: &SyncContext) -> anyhow::Result<()>;
    async fn tick(
        &self,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> anyhow::Result<SyncOutcome>;
}
