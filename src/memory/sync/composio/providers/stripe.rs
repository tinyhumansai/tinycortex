//! Incremental Stripe synchronization through Composio.
//!
//! Stripe is a document-shaped source: [`STRIPE_LIST_ALL_CHARGES`] returns a
//! Stripe list envelope (`{ "data": [charge, ...], "has_more": bool }`) whose
//! items each carry a stable object id (`ch_...`) and a `created` unix
//! timestamp. Pagination is cursor-based: the next page is requested with
//! `starting_after = <last item id>` while `has_more` is true, mirroring the
//! Stripe REST API.
//!
//! Charges are sensitive financial records, so nothing item-specific is logged
//! here; the shared orchestrator only emits toolkit/connection identifiers.

use async_trait::async_trait;
use serde_json::Value;

use super::common::{document, first_array, pick_str};
use crate::memory::config::MemoryConfig;
use crate::memory::sync::composio::{
    run_incremental_sync, ActionExecutor, ComposioClient, IncrementalSource, PageFetch, SyncItem,
    SyncScope,
};
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

const ACTION_LIST_CHARGES: &str = "STRIPE_LIST_ALL_CHARGES";

pub struct StripeSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl StripeSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            page_size: 50,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        self.page_size = page_size.max(1);
        self
    }
}

#[async_trait]
impl SyncPipeline for StripeSyncPipeline {
    fn id(&self) -> &str {
        "composio:stripe"
    }
    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Composio
    }
    async fn init(&self, _: &MemoryConfig, _: &SyncContext) -> anyhow::Result<()> {
        Ok(())
    }
    async fn tick(
        &self,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> anyhow::Result<SyncOutcome> {
        run_incremental_sync(self, &self.client, &self.connection_id, config, context).await
    }
}

#[async_trait]
impl IncrementalSource for StripeSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "stripe"
    }
    fn action(&self) -> &'static str {
        ACTION_LIST_CHARGES
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn arguments(
        &self,
        _: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let mut args = serde_json::json!({"limit": self.page_size});
        if let Some(page) = page {
            args["starting_after"] = serde_json::json!(page);
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        let items = first_array(
            data,
            &[
                "/data/data",
                "/data/data/data",
                "/data/response_data/data",
                "/data/results",
                "/results",
                "/data/items",
                "/items",
            ],
        );
        // Stripe cursor pagination: request the next page with the last object's
        // id as `starting_after`, but only while the list reports `has_more`.
        let has_more = ["/data/has_more", "/has_more", "/data/data/has_more"]
            .iter()
            .find_map(|path| data.pointer(path).and_then(Value::as_bool))
            .unwrap_or(false);
        let next = has_more
            .then(|| {
                items
                    .last()
                    .and_then(|item| pick_str(item, &["id", "data.id"]))
            })
            .flatten();
        PageFetch { items, next }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id"])?;
        Some(match self.sort_cursor(item) {
            Some(created) => format!("{id}@{created}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(item, &["created", "data.created"])
    }
    async fn document(
        &self,
        _: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        // The Stripe object id (`ch_...`) is the stable upsert key. Never derive
        // `document_id` from a per-run cursor: that reintroduces the duplicate
        // charges fixed by tinyhumansai/openhuman#4953.
        let id = pick_str(&item.raw, &["id", "data.id"]).unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(
            &item.raw,
            &[
                "description",
                "data.description",
                "statement_descriptor",
                "data.statement_descriptor",
            ],
        )
        .unwrap_or_else(|| format!("Stripe charge {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "stripe",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::config::{ComposioMode, ComposioSyncConfig};

    fn pipeline() -> StripeSyncPipeline {
        let config = ComposioSyncConfig {
            mode: ComposioMode::Direct,
            base_url: "http://localhost".into(),
            api_key: None,
            bearer_token: None,
            entity_id: None,
        };
        StripeSyncPipeline::new(ComposioClient::new(config), "conn-test")
    }

    fn sample_payload() -> Value {
        // Composio wraps the Stripe list envelope under its own `data` key.
        serde_json::json!({
            "successful": true,
            "data": {
                "object": "list",
                "has_more": true,
                "data": [
                    {"id": "ch_1", "object": "charge", "created": 1700000001, "amount": 500, "description": "Pro plan"},
                    {"id": "ch_2", "object": "charge", "created": 1700000002, "amount": 900}
                ]
            }
        })
    }

    #[test]
    fn toolkit_and_action_match_composio_slug() {
        let pipeline = pipeline();
        assert_eq!(pipeline.toolkit(), "stripe");
        assert_eq!(pipeline.action(), "STRIPE_LIST_ALL_CHARGES");
    }

    #[test]
    fn extract_page_reads_charges_and_cursor() {
        let pipeline = pipeline();
        let page = pipeline.extract_page(&sample_payload(), None);
        assert_eq!(page.items.len(), 2);
        // `starting_after` for the next request is the last charge's id.
        assert_eq!(page.next.as_deref(), Some("ch_2"));
    }

    #[test]
    fn extract_page_stops_without_has_more() {
        let pipeline = pipeline();
        let data = serde_json::json!({
            "data": {"has_more": false, "data": [{"id": "ch_9", "created": 1700000009}]}
        });
        let page = pipeline.extract_page(&data, None);
        assert_eq!(page.items.len(), 1);
        assert!(page.next.is_none());
    }

    #[test]
    fn dedup_key_combines_id_and_created() {
        let pipeline = pipeline();
        let item = serde_json::json!({"id": "ch_1", "created": 1700000001});
        assert_eq!(
            pipeline.dedup_key(&item).as_deref(),
            Some("ch_1@1700000001")
        );
    }

    #[tokio::test]
    async fn document_uses_stable_document_id_and_taint() {
        let pipeline = pipeline();
        let mut state = SyncState::new("stripe", "conn-test");
        let raw =
            serde_json::json!({"id": "ch_1", "created": 1700000001, "description": "Pro plan"});
        let item = SyncItem {
            dedup_key: "ch_1@1700000001".into(),
            sort_cursor: Some("1700000001".into()),
            raw,
        };
        let doc = pipeline
            .document(
                &SyncScope::flat(),
                "conn-test",
                item,
                &pipeline.client,
                &mut state,
            )
            .await
            .unwrap();
        // Stable upsert key: derived from the object id, not the per-run cursor.
        assert_eq!(doc.document_id, "stripe:ch_1");
        assert_eq!(doc.namespace_skill_id, "stripe");
        assert_eq!(doc.toolkit, "stripe");
        assert_eq!(doc.title, "Pro plan");
        assert_eq!(doc.metadata["taint"], "external_sync");
    }
}
