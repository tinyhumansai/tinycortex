//! Incremental Shopify product synchronization through Composio.
//!
//! A Shopify catalog is document-shaped — each product is a stable, addressable
//! record — so this pipeline mirrors the Notion/Linear providers rather than the
//! message-shaped Gmail one. We ingest the product catalog (products carry no
//! customer PII, unlike orders) and key each memory document on the stable
//! numeric product id (`shopify:<product_id>`). Re-syncing an edited product
//! therefore upserts in place instead of duplicating — the stable-`document_id`
//! lesson from openhuman#4953. `document()` also stamps
//! `metadata.taint = "external_sync"`.

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

/// Composio action that retrieves a list of products from a Shopify store.
const ACTION_FETCH_PRODUCTS: &str = "SHOPIFY_GET_PRODUCTS";

pub struct ShopifySyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl ShopifySyncPipeline {
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
impl SyncPipeline for ShopifySyncPipeline {
    fn id(&self) -> &str {
        "composio:shopify"
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
impl IncrementalSource for ShopifySyncPipeline {
    fn toolkit(&self) -> &'static str {
        "shopify"
    }
    fn action(&self) -> &'static str {
        ACTION_FETCH_PRODUCTS
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    /// Incremental depth is enforced server-side via `updated_at_min` (see
    /// [`Self::arguments`]), so the engine's client-side depth floor — whose
    /// lexical compare would misbehave on timezone-qualified Shopify timestamps —
    /// is skipped.
    fn server_side_depth(&self) -> bool {
        true
    }
    fn arguments(
        &self,
        _: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value {
        // Shopify cursor pagination requires `page_info` to travel alone; only
        // `limit` may accompany it. The incremental window rides the first-page
        // request instead.
        if let Some(page) = page {
            return serde_json::json!({ "limit": self.page_size, "page_info": page });
        }
        let mut args = serde_json::json!({ "limit": self.page_size });
        let since = state.cursor.clone().or_else(|| {
            config.sync.budget.sync_depth_days.map(|days| {
                (chrono::Utc::now() - chrono::Duration::days(days as i64))
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string()
            })
        });
        if let Some(since) = since {
            args["updated_at_min"] = Value::String(since);
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/products",
                    "/products",
                    "/data/data/products",
                    "/data/results",
                    "/results",
                    "/data/items",
                    "/items",
                ],
            ),
            next: [
                "/data/next_page_info",
                "/next_page_info",
                "/data/pagination/next_page_info",
                "/data/page_info",
                "/page_info",
            ]
            .iter()
            .find_map(|path| data.pointer(path).and_then(Value::as_str))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = product_id(item)?;
        // Fold the modification time into the dedupe key so an edited product is
        // re-ingested, while its stable `document_id` upserts it in place.
        Some(match self.sort_cursor(item) {
            Some(updated) => format!("{id}@{updated}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "updated_at",
                "data.updated_at",
                "updatedAt",
                "data.updatedAt",
            ],
        )
        .map(|ts| normalize_timestamp(&ts))
    }
    async fn document(
        &self,
        _: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = product_id(&item.raw).unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["title", "data.title", "handle", "data.handle"])
            .unwrap_or_else(|| format!("Shopify product {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        // `document()` stamps `document_id = "shopify:<id>"` and the
        // `external_sync` taint — the stable upsert key that keeps re-syncs idempotent.
        Ok(document(
            "shopify",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

fn product_id(item: &Value) -> Option<String> {
    pick_str(
        item,
        &[
            "id",
            "data.id",
            "product_id",
            "data.product_id",
            "admin_graphql_api_id",
        ],
    )
}

/// Normalize a Shopify timestamp to a lexically comparable UTC form so the
/// engine's cursor comparisons order correctly regardless of the store's
/// timezone offset. Falls back to the trimmed raw value when it cannot be parsed.
fn normalize_timestamp(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map(|dt| {
            dt.with_timezone(&chrono::Utc)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_else(|_| raw.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::config::{ComposioMode, ComposioSyncConfig};

    fn pipeline() -> ShopifySyncPipeline {
        let config = ComposioSyncConfig {
            mode: ComposioMode::Direct,
            base_url: "http://localhost".into(),
            api_key: None,
            bearer_token: None,
            entity_id: None,
        };
        ShopifySyncPipeline::new(ComposioClient::new(config), "conn-1")
    }

    fn sample_page() -> Value {
        // Shape mirrors Composio's `SHOPIFY_GET_PRODUCTS` envelope: the Shopify
        // REST `{ "products": [...] }` body nested under `data`.
        serde_json::json!({
            "successful": true,
            "data": {
                "products": [
                    {
                        "id": 8001,
                        "title": "Aurora Hoodie",
                        "handle": "aurora-hoodie",
                        "updated_at": "2026-05-01T12:30:00-04:00"
                    },
                    {
                        "id": 8002,
                        "title": "Nimbus Cap",
                        "updated_at": "2026-05-02T09:00:00Z"
                    }
                ],
                "next_page_info": "cursor-page-2"
            }
        })
    }

    #[test]
    fn toolkit_and_action_are_stable_slugs() {
        let pipeline = pipeline();
        assert_eq!(pipeline.toolkit(), "shopify");
        assert_eq!(pipeline.action(), "SHOPIFY_GET_PRODUCTS");
        assert_eq!(pipeline.id(), "composio:shopify");
    }

    #[test]
    fn extract_page_reads_products_and_cursor() {
        let page = pipeline().extract_page(&sample_page(), None);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0]["id"], 8001);
        assert_eq!(page.next.as_deref(), Some("cursor-page-2"));
    }

    #[test]
    fn sort_cursor_normalizes_to_utc() {
        let pipeline = pipeline();
        // Offset timestamp is converted to UTC so lexical cursor compares hold.
        assert_eq!(
            pipeline.sort_cursor(&sample_page()["data"]["products"][0]),
            Some("2026-05-01T16:30:00Z".into())
        );
    }

    #[test]
    fn arguments_page_request_sends_cursor_alone() {
        let args = pipeline().arguments(
            &SyncScope::flat(),
            &MemoryConfig::new("/tmp/tinycortex-shopify-test"),
            &SyncState::new("shopify", "conn-1"),
            Some("cursor-page-2"),
        );
        assert_eq!(args["page_info"], "cursor-page-2");
        assert_eq!(args["limit"], 50);
        assert!(args.get("updated_at_min").is_none());
    }

    #[test]
    fn arguments_first_request_carries_persisted_cursor_window() {
        let mut state = SyncState::new("shopify", "conn-1");
        state.advance_cursor("2026-04-01T00:00:00Z");
        let args = pipeline().arguments(
            &SyncScope::flat(),
            &MemoryConfig::new("/tmp/tinycortex-shopify-test"),
            &state,
            None,
        );
        assert_eq!(args["updated_at_min"], "2026-04-01T00:00:00Z");
        assert!(args.get("page_info").is_none());
    }

    #[tokio::test]
    async fn document_has_stable_id_and_external_sync_taint() {
        let pipeline = pipeline();
        let raw = sample_page()["data"]["products"][0].clone();
        let mut state = SyncState::new("shopify", "conn-1");
        let doc = pipeline
            .document(
                &SyncScope::flat(),
                "conn-1",
                SyncItem {
                    dedup_key: "8001@2026-05-01T16:30:00Z".into(),
                    sort_cursor: Some("2026-05-01T16:30:00Z".into()),
                    raw,
                },
                &pipeline.client,
                &mut state,
            )
            .await
            .unwrap();
        // Stable across runs: keyed on the product id, never a per-run token.
        assert_eq!(doc.document_id, "shopify:8001");
        assert_eq!(doc.title, "Aurora Hoodie");
        assert_eq!(doc.toolkit, "shopify");
        assert_eq!(doc.metadata["taint"], "external_sync");
    }

    #[test]
    fn dedup_key_folds_in_modification_time() {
        let pipeline = pipeline();
        let key = pipeline.dedup_key(&sample_page()["data"]["products"][0]);
        assert_eq!(key.as_deref(), Some("8001@2026-05-01T16:30:00Z"));
    }
}
