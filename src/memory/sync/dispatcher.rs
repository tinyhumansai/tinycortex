//! Pipeline registry and fault-isolated synchronization dispatcher.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::memory::config::MemoryConfig;
use crate::memory::sync::traits::{SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncRunResult {
    pub pipeline_id: String,
    pub kind: SyncPipelineKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<SyncOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Default)]
pub struct SyncDispatcher {
    pipelines: BTreeMap<String, Arc<dyn SyncPipeline>>,
}

impl SyncDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, pipeline: Arc<dyn SyncPipeline>) -> anyhow::Result<()> {
        let id = pipeline.id().trim();
        anyhow::ensure!(!id.is_empty(), "sync pipeline id must not be empty");
        anyhow::ensure!(
            !self.pipelines.contains_key(id),
            "sync pipeline already registered: {id}"
        );
        tracing::debug!(
            pipeline_id = id,
            kind = pipeline.kind().as_str(),
            "[memory_sync:dispatcher] registering pipeline"
        );
        self.pipelines.insert(id.to_owned(), pipeline);
        Ok(())
    }

    pub fn ids(&self) -> Vec<&str> {
        self.pipelines.keys().map(String::as_str).collect()
    }

    pub async fn init_all(
        &self,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> Vec<SyncRunResult> {
        let mut results = Vec::with_capacity(self.pipelines.len());
        for (id, pipeline) in &self.pipelines {
            tracing::debug!(
                pipeline_id = id,
                "[memory_sync:dispatcher] initializing pipeline"
            );
            let result = pipeline.init(config, context).await;
            results.push(SyncRunResult {
                pipeline_id: id.clone(),
                kind: pipeline.kind(),
                outcome: result.as_ref().ok().map(|_| SyncOutcome::default()),
                error: result.err().map(|error| error.to_string()),
            });
        }
        results
    }

    pub async fn tick(
        &self,
        pipeline_id: &str,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> anyhow::Result<SyncOutcome> {
        let pipeline = self
            .pipelines
            .get(pipeline_id)
            .ok_or_else(|| anyhow::anyhow!("unknown sync pipeline: {pipeline_id}"))?;
        tracing::debug!(
            pipeline_id,
            "[memory_sync:dispatcher] pipeline tick starting"
        );
        let outcome = pipeline.tick(config, context).await;
        match &outcome {
            Ok(outcome) => tracing::debug!(
                pipeline_id,
                records = outcome.records_ingested,
                more_pending = outcome.more_pending,
                "[memory_sync:dispatcher] pipeline tick completed"
            ),
            Err(error) => {
                tracing::warn!(pipeline_id, %error, "[memory_sync:dispatcher] pipeline tick failed")
            }
        }
        outcome
    }

    pub async fn tick_all(
        &self,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> Vec<SyncRunResult> {
        let mut results = Vec::with_capacity(self.pipelines.len());
        for (id, pipeline) in &self.pipelines {
            let result = pipeline.tick(config, context).await;
            results.push(SyncRunResult {
                pipeline_id: id.clone(),
                kind: pipeline.kind(),
                outcome: result.as_ref().ok().cloned(),
                error: result.err().map(|error| error.to_string()),
            });
        }
        results
    }
}

#[cfg(test)]
#[path = "dispatcher_tests.rs"]
mod tests;
