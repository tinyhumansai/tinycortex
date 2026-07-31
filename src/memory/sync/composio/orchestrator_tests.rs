//! Tests for the too-large-page retry.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::memory::sync::composio::client::ExecuteResponse;
use crate::memory::sync::state::SyncStateStore;
use crate::memory::sync::traits::{SkillDocSink, SkillDocument, SyncEvent, SyncEventSink};

/// Executor that mimics a provider with a response-size ceiling: it refuses any
/// page asking for more than `accepts` items and records every size it was
/// asked for.
struct SizeLimitedExecutor {
    accepts: u64,
    requested: Mutex<Vec<u64>>,
}

impl SizeLimitedExecutor {
    fn new(accepts: u64) -> Self {
        Self {
            accepts,
            requested: Mutex::new(Vec::new()),
        }
    }

    fn requested_sizes(&self) -> Vec<u64> {
        self.requested.lock().unwrap().clone()
    }
}

#[async_trait]
impl ActionExecutor for SizeLimitedExecutor {
    async fn execute(
        &self,
        _action: &str,
        arguments: Value,
        _connection_id: Option<&str>,
    ) -> anyhow::Result<ExecuteResponse> {
        let requested = arguments
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.requested.lock().unwrap().push(requested);

        let mut response: ExecuteResponse = serde_json::from_value(json!({})).unwrap();
        if requested > self.accepts {
            response.successful = false;
            response.error = Some(
                "413 {\"error\":{\"message\":\"The tool response payload is too large.\",\
                 \"code\":1613,\"slug\":\"Upstream_PayloadTooLarge\"}}"
                    .to_string(),
            );
            return Ok(response);
        }
        response.successful = true;
        response.data = json!({
            "messages": [{ "id": format!("m{requested}"), "date": "1700000000000" }],
        });
        Ok(response)
    }
}

/// Minimal paged source: one page, one item, page size declared as
/// `max_results` unless `declare_page_size` is off.
struct StubSource {
    declare_page_size: bool,
    initial_page_size: u64,
}

#[async_trait]
impl IncrementalSource for StubSource {
    fn toolkit(&self) -> &'static str {
        "stub"
    }
    fn action(&self) -> &'static str {
        "STUB_FETCH"
    }
    fn max_pages(&self) -> usize {
        1
    }
    fn page_size_arg_key(&self) -> Option<&'static str> {
        self.declare_page_size.then_some("max_results")
    }
    fn arguments(
        &self,
        _scope: &SyncScope,
        _config: &MemoryConfig,
        _state: &SyncState,
        _page: Option<&str>,
    ) -> Value {
        json!({ "max_results": self.initial_page_size })
    }
    fn extract_page(&self, data: &Value, _page: Option<&str>) -> PageFetch {
        PageFetch {
            items: data
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            next: None,
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        item.get("id").and_then(Value::as_str).map(str::to_string)
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        item.get("date").and_then(Value::as_str).map(str::to_string)
    }
    async fn document(
        &self,
        _scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _executor: &dyn ActionExecutor,
        _state: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        Ok(SkillDocument {
            namespace_skill_id: "stub".into(),
            connection_id: connection_id.into(),
            document_id: item.dedup_key,
            title: "stub".into(),
            content: "stub".into(),
            toolkit: "stub".into(),
            metadata: Value::Null,
        })
    }
}

#[derive(Default)]
struct NoopHost(Mutex<std::collections::HashMap<String, Value>>);

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
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<Value>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&format!("{namespace}:{key}"))
            .cloned())
    }
    async fn set(&self, namespace: &str, key: &str, value: &Value) -> anyhow::Result<()> {
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
async fn an_oversized_page_is_halved_until_the_provider_accepts_it() {
    // The provider takes 6 at a time; the source asks for 25.
    let executor = SizeLimitedExecutor::new(6);
    let source = StubSource {
        declare_page_size: true,
        initial_page_size: 25,
    };

    let outcome = run_incremental_sync(
        &source,
        &executor,
        "conn-1",
        &MemoryConfig::new("/tmp/unused-orchestrator-tests"),
        &context(),
    )
    .await
    .expect("a too-large page must not fail the run");

    assert_eq!(
        outcome.records_ingested, 1,
        "the page is ingested after the retry"
    );
    assert_eq!(
        executor.requested_sizes(),
        vec![25, 12, 6],
        "each rejection halves the request until it fits"
    );
}

#[tokio::test]
async fn a_source_without_a_page_size_argument_still_fails_fast() {
    // No `page_size_arg_key` — there is nothing to shrink, so the old
    // behaviour (surface the provider failure) must be preserved rather than
    // looping on a request that can never change.
    let executor = SizeLimitedExecutor::new(6);
    let source = StubSource {
        declare_page_size: false,
        initial_page_size: 25,
    };

    let error = run_incremental_sync(
        &source,
        &executor,
        "conn-1",
        &MemoryConfig::new("/tmp/unused-orchestrator-tests"),
        &context(),
    )
    .await
    .expect_err("an unshrinkable too-large page is still a failure");

    assert!(error.to_string().contains("provider failure"), "{error}");
    assert_eq!(
        executor.requested_sizes(),
        vec![25],
        "no pointless retry of an identical request"
    );
}
