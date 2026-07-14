use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use crate::memory::sync::state::SyncStateStore;
use crate::memory::sync::traits::{SkillDocSink, SkillDocument, SyncEvent, SyncEventSink};

struct FakePipeline {
    id: &'static str,
    fail: bool,
    init_fail: bool,
}

#[async_trait]
impl SyncPipeline for FakePipeline {
    fn id(&self) -> &str {
        self.id
    }

    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Workspace
    }

    async fn init(&self, _: &MemoryConfig, _: &SyncContext) -> anyhow::Result<()> {
        if self.init_fail {
            anyhow::bail!("expected init failure")
        }
        Ok(())
    }

    async fn tick(&self, _: &MemoryConfig, _: &SyncContext) -> anyhow::Result<SyncOutcome> {
        if self.fail {
            anyhow::bail!("expected failure")
        }
        Ok(SyncOutcome {
            records_ingested: 3,
            more_pending: false,
            actions_called: 0,
            provider_cost_usd: 0.0,
            note: None,
        })
    }
}

#[derive(Default)]
struct NoopHost(Mutex<HashMap<String, serde_json::Value>>);

#[async_trait]
impl SkillDocSink for NoopHost {
    async fn store(&self, _: SkillDocument) -> anyhow::Result<()> {
        Ok(())
    }

    async fn delete(&self, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl SyncEventSink for NoopHost {
    async fn emit(&self, _: SyncEvent) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl SyncStateStore for NoopHost {
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        Ok(self
            .0
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
        self.0
            .lock()
            .unwrap()
            .insert(format!("{namespace}:{key}"), value.clone());
        Ok(())
    }
}

fn context() -> SyncContext {
    let host = Arc::new(NoopHost::default());
    SyncContext {
        events: host.clone(),
        documents: host.clone(),
        state: host,
        local_documents: None,
        external_sources: None,
        summariser: None,
    }
}

#[tokio::test]
async fn tick_all_is_deterministic_and_isolates_failures() {
    let mut dispatcher = SyncDispatcher::new();
    dispatcher
        .register(Arc::new(FakePipeline {
            id: "z-fail",
            fail: true,
            init_fail: false,
        }))
        .unwrap();
    dispatcher
        .register(Arc::new(FakePipeline {
            id: "a-ok",
            fail: false,
            init_fail: false,
        }))
        .unwrap();
    assert_eq!(dispatcher.ids(), vec!["a-ok", "z-fail"]);
    assert!(dispatcher
        .register(Arc::new(FakePipeline {
            id: "a-ok",
            fail: false,
            init_fail: false,
        }))
        .is_err());
    let results = dispatcher
        .tick_all(&MemoryConfig::new("/tmp/unused"), &context())
        .await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].outcome.as_ref().unwrap().records_ingested, 3);
    assert!(results[1]
        .error
        .as_deref()
        .unwrap()
        .contains("expected failure"));
}

#[tokio::test]
async fn register_rejects_blank_ids_and_tick_reports_unknown_pipeline() {
    let mut dispatcher = SyncDispatcher::new();
    assert!(dispatcher
        .register(Arc::new(FakePipeline {
            id: "  ",
            fail: false,
            init_fail: false,
        }))
        .is_err());
    let error = dispatcher
        .tick("missing", &MemoryConfig::new("/tmp/unused"), &context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown sync pipeline"));
}

#[tokio::test]
async fn init_all_and_individual_tick_preserve_success_and_failure_details() {
    let mut dispatcher = SyncDispatcher::new();
    dispatcher
        .register(Arc::new(FakePipeline {
            id: "a-init-fails",
            fail: false,
            init_fail: true,
        }))
        .unwrap();
    dispatcher
        .register(Arc::new(FakePipeline {
            id: "b-ok",
            fail: false,
            init_fail: false,
        }))
        .unwrap();
    dispatcher
        .register(Arc::new(FakePipeline {
            id: "c-tick-fails",
            fail: true,
            init_fail: false,
        }))
        .unwrap();
    let config = MemoryConfig::new("/tmp/unused");
    let context = context();

    let initialized = dispatcher.init_all(&config, &context).await;
    assert!(initialized[0]
        .error
        .as_deref()
        .unwrap()
        .contains("expected init failure"));
    assert!(initialized[1].outcome.is_some());
    assert_eq!(
        dispatcher
            .tick("b-ok", &config, &context)
            .await
            .unwrap()
            .records_ingested,
        3
    );
    assert!(dispatcher
        .tick("c-tick-fails", &config, &context)
        .await
        .is_err());

    let encoded = serde_json::to_value(&initialized[0]).unwrap();
    assert_eq!(encoded["pipeline_id"], "a-init-fails");
    assert!(encoded.get("outcome").is_none());
}
