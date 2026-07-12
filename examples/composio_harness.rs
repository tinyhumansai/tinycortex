//! Composio connection + memory-sync harness.
//!
//! An end-to-end, runnable check that a Composio API key is wired correctly and
//! that TinyCortex's Composio sync pipelines actually ingest memory for every
//! connected toolkit.
//!
//! ## Usage
//!
//! ```sh
//! COMPOSIO_API_KEY=ak_... cargo run --example composio_harness --features sync
//! ```
//!
//! ### Phase 1 — Connection test
//! Validates the API key by listing connected accounts against the Composio v3
//! API (`GET /connected_accounts`). This proves the key is live and discovers
//! which toolkits are connected together with their `connected_account_id`s.
//!
//! ### Phase 2 — Memory sync
//! For every discovered toolkit that TinyCortex has a pipeline for (Gmail,
//! GitHub, Linear, Notion, ClickUp, Slack), the harness runs `tick()` twice
//! against real Composio using in-memory sinks, then reports records ingested,
//! provider actions, cost, cursor advance, taint tagging, and idempotency.
//!
//! The process exits non-zero if the connection test fails or if any toolkit's
//! sync errors, so the harness is usable as a CI smoke test.
//!
//! ## Environment
//! - `COMPOSIO_API_KEY`            (required) direct-mode API key.
//! - `COMPOSIO_BASE_URL`           override the API base (default v3 backend).
//! - `COMPOSIO_ENTITY_ID`          Composio `user_id` scoping accounts.
//! - `COMPOSIO_TOOLKITS`           comma list restricting which toolkits to sync.
//! - `COMPOSIO_MAX_ITEMS`          per-toolkit ingest cap (default 25).
//! - `COMPOSIO_<TOOLKIT>_CONNECTION_ID`  pin/override a connection id, e.g.
//!   `COMPOSIO_GMAIL_CONNECTION_ID`. Also seeds a toolkit if discovery is empty.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tinycortex::memory::config::{ComposioMode, ComposioSyncConfig, MemoryConfig, SecretString};
use tinycortex::memory::sync::{
    ClickUpSyncPipeline, ComposioClient, GitHubSyncPipeline, GmailSyncPipeline, LinearSyncPipeline,
    NotionSyncPipeline, SkillDocSink, SkillDocument, SlackSyncPipeline, SyncContext, SyncEvent,
    SyncEventSink, SyncPipeline, SyncState, SyncStateStore,
};

const DEFAULT_BASE_URL: &str = "https://backend.composio.dev/api/v3";
const DEFAULT_MAX_ITEMS: u32 = 25;

/// Toolkits with a TinyCortex sync pipeline, in a stable report order.
const SUPPORTED_TOOLKITS: &[&str] = &["gmail", "github", "linear", "notion", "clickup", "slack"];

// ── In-memory host sinks ────────────────────────────────────────────────────
// The harness stands in for a real host: it captures skill documents, sync
// events, and cursor/dedup state entirely in process so nothing is persisted.

#[derive(Default)]
struct Captures {
    documents: Mutex<Vec<SkillDocument>>,
    events: Mutex<Vec<SyncEvent>>,
    state: Mutex<HashMap<String, Value>>,
}

#[async_trait]
impl SkillDocSink for Captures {
    async fn store(&self, document: SkillDocument) -> anyhow::Result<()> {
        self.documents.lock().unwrap().push(document);
        Ok(())
    }

    async fn delete(&self, namespace_skill_id: &str, document_id: &str) -> anyhow::Result<()> {
        self.documents.lock().unwrap().retain(|document| {
            document.namespace_skill_id != namespace_skill_id || document.document_id != document_id
        });
        Ok(())
    }
}

#[async_trait]
impl SyncEventSink for Captures {
    async fn emit(&self, event: SyncEvent) -> anyhow::Result<()> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

#[async_trait]
impl SyncStateStore for Captures {
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<Value>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .get(&format!("{namespace}:{key}"))
            .cloned())
    }

    async fn set(&self, namespace: &str, key: &str, value: &Value) -> anyhow::Result<()> {
        self.state
            .lock()
            .unwrap()
            .insert(format!("{namespace}:{key}"), value.clone());
        Ok(())
    }
}

impl Captures {
    fn context(self: &Arc<Self>) -> SyncContext {
        SyncContext {
            events: self.clone(),
            documents: self.clone(),
            state: self.clone(),
            local_documents: None,
            external_sources: None,
            summariser: None,
        }
    }

    fn docs_for(&self, toolkit: &str) -> Vec<SkillDocument> {
        self.documents
            .lock()
            .unwrap()
            .iter()
            .filter(|doc| doc.toolkit == toolkit)
            .cloned()
            .collect()
    }
}

// ── A connected account discovered from the Composio API ────────────────────

struct Connection {
    toolkit: String,
    connection_id: String,
    status: Option<String>,
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Best-effort extraction of a toolkit slug from a connected-account record.
/// Composio has shipped several shapes over v3; probe the common ones.
fn record_toolkit(record: &Value) -> Option<String> {
    let candidates = [
        record.pointer("/toolkit/slug"),
        record.pointer("/toolkit/name"),
        record.get("toolkit"),
        record.get("appName"),
        record.get("appUniqueId"),
        record.pointer("/app/uniqueKey"),
        record.pointer("/app/name"),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .map(|slug| slug.trim().to_ascii_lowercase())
        .filter(|slug| !slug.is_empty())
}

fn record_id(record: &Value) -> Option<String> {
    ["id", "connectedAccountId", "connected_account_id", "nanoid"]
        .iter()
        .find_map(|key| record.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Phase 1: validate the key and list connected accounts.
async fn discover_connections(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    entity_id: Option<&str>,
) -> anyhow::Result<Vec<Connection>> {
    let mut request = http
        .get(format!(
            "{}/connected_accounts",
            base_url.trim_end_matches('/')
        ))
        .header("x-api-key", api_key);
    if let Some(entity_id) = entity_id {
        request = request.query(&[("user_ids", entity_id)]);
    }
    let response = request
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("connected_accounts request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        // Never echo the response body; it can contain the key back verbatim.
        let _ = response.bytes().await;
        anyhow::bail!("connected_accounts returned HTTP {status}");
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("connected_accounts decode failed: {error}"))?;

    let items = ["/items", "/data/items", "/data", "/connectedAccounts"]
        .iter()
        .find_map(|pointer| body.pointer(pointer).and_then(Value::as_array))
        .or_else(|| body.as_array())
        .cloned()
        .unwrap_or_default();

    let mut connections = Vec::new();
    for record in &items {
        let (Some(toolkit), Some(connection_id)) = (record_toolkit(record), record_id(record))
        else {
            continue;
        };
        connections.push(Connection {
            toolkit,
            connection_id,
            status: record
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    Ok(connections)
}

fn build_pipeline(
    toolkit: &str,
    client: ComposioClient,
    connection_id: &str,
) -> Option<Box<dyn SyncPipeline>> {
    match toolkit {
        "gmail" => Some(Box::new(GmailSyncPipeline::new(client, connection_id))),
        "github" => Some(Box::new(GitHubSyncPipeline::new(client, connection_id))),
        "linear" => Some(Box::new(LinearSyncPipeline::new(client, connection_id))),
        "notion" => Some(Box::new(NotionSyncPipeline::new(client, connection_id))),
        "clickup" => Some(Box::new(ClickUpSyncPipeline::new(client, connection_id))),
        "slack" => Some(Box::new(SlackSyncPipeline::new(client, connection_id))),
        _ => None,
    }
}

struct ToolkitReport {
    toolkit: String,
    connection_id: String,
    ingested: u32,
    actions: u32,
    cost_usd: f64,
    docs_stored: usize,
    taint_ok: bool,
    cursor_advanced: bool,
    idempotency: &'static str,
    error: Option<String>,
}

impl ToolkitReport {
    fn passed(&self) -> bool {
        self.error.is_none() && self.idempotency != "FAIL"
    }
}

/// Phase 2: run one toolkit's pipeline twice and grade the outcome.
async fn run_toolkit(
    toolkit: &str,
    connection_id: &str,
    config: &MemoryConfig,
    transport: &ComposioSyncConfig,
    captures: &Arc<Captures>,
) -> ToolkitReport {
    let mut report = ToolkitReport {
        toolkit: toolkit.to_owned(),
        connection_id: connection_id.to_owned(),
        ingested: 0,
        actions: 0,
        cost_usd: 0.0,
        docs_stored: 0,
        taint_ok: true,
        cursor_advanced: false,
        idempotency: "n/a",
        error: None,
    };

    let client = ComposioClient::new(transport.clone());
    let Some(pipeline) = build_pipeline(toolkit, client, connection_id) else {
        report.error = Some("no TinyCortex pipeline for toolkit".into());
        return report;
    };
    let context = captures.context();

    let first = match pipeline.tick(config, &context).await {
        Ok(outcome) => outcome,
        Err(error) => {
            report.error = Some(error.to_string());
            return report;
        }
    };
    report.ingested = first.records_ingested;
    report.actions = first.actions_called;
    report.cost_usd = first.provider_cost_usd;

    let docs = captures.docs_for(toolkit);
    report.docs_stored = docs.len();
    report.taint_ok = docs
        .iter()
        .all(|doc| doc.metadata.get("taint").and_then(Value::as_str) == Some("external_sync"));

    if let Ok(state) = SyncState::load(captures.as_ref(), toolkit, connection_id).await {
        report.cursor_advanced = state.cursor.is_some();
    }

    // A second tick against unchanged upstream data must not re-ingest anything
    // — unless the first run was capped (`more_pending`), in which case picking
    // up further items is correct incremental behaviour, not a dedup failure.
    match pipeline.tick(config, &context).await {
        Ok(second) if first.more_pending => {
            report.idempotency = "incremental";
            report.ingested = report.ingested.saturating_add(second.records_ingested);
        }
        Ok(second) if second.records_ingested == 0 => report.idempotency = "PASS",
        Ok(second) => {
            report.idempotency = "FAIL";
            report.error = Some(format!(
                "second tick re-ingested {} records",
                second.records_ingested
            ));
        }
        Err(error) => {
            report.idempotency = "FAIL";
            report.error = Some(format!("second tick errored: {error}"));
        }
    }
    report
}

fn print_report(reports: &[ToolkitReport]) {
    println!("\n── Sync results ──────────────────────────────────────────────");
    println!(
        "{:<9} {:<7} {:>5} {:>5} {:>8} {:<6} {:<12} notes",
        "toolkit", "result", "recs", "acts", "cost$", "taint", "idempotency"
    );
    for report in reports {
        println!(
            "{:<9} {:<7} {:>5} {:>5} {:>8.4} {:<6} {:<12} {}",
            report.toolkit,
            if report.passed() { "PASS" } else { "FAIL" },
            report.ingested,
            report.actions,
            report.cost_usd,
            if report.taint_ok { "ok" } else { "MISS" },
            report.idempotency,
            report
                .error
                .as_deref()
                .map(|error| format!("error: {error}"))
                .unwrap_or_else(|| format!(
                    "conn={} cursor={}",
                    report.connection_id,
                    if report.cursor_advanced {
                        "advanced"
                    } else {
                        "none"
                    }
                )),
        );
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("\nharness aborted: {error}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let api_key = env_opt("COMPOSIO_API_KEY")
        .ok_or_else(|| anyhow::anyhow!("COMPOSIO_API_KEY is required"))?;
    let base_url = env_opt("COMPOSIO_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
    let entity_id = env_opt("COMPOSIO_ENTITY_ID");
    let max_items = env_opt("COMPOSIO_MAX_ITEMS")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_ITEMS);
    let toolkit_filter: Option<Vec<String>> = env_opt("COMPOSIO_TOOLKITS").map(|list| {
        list.split(',')
            .map(|item| item.trim().to_ascii_lowercase())
            .filter(|item| !item.is_empty())
            .collect()
    });

    let http = reqwest::Client::new();

    // ── Phase 1: connection test ────────────────────────────────────────────
    println!("── Phase 1: connection test ──────────────────────────────────");
    println!("base_url : {base_url}");
    println!("entity   : {}", entity_id.as_deref().unwrap_or("(none)"));
    let mut connections =
        discover_connections(&http, &base_url, &api_key, entity_id.as_deref()).await?;
    println!(
        "connection OK — {} connected account(s) discovered",
        connections.len()
    );
    for connection in &connections {
        println!(
            "  • {:<9} {} [{}]",
            connection.toolkit,
            connection.connection_id,
            connection.status.as_deref().unwrap_or("unknown")
        );
    }

    // Explicit `COMPOSIO_<TOOLKIT>_CONNECTION_ID` overrides win and can seed a
    // toolkit that discovery missed (e.g. a freshly connected account).
    for toolkit in SUPPORTED_TOOLKITS {
        if let Some(connection_id) = env_opt(&format!(
            "COMPOSIO_{}_CONNECTION_ID",
            toolkit.to_ascii_uppercase()
        )) {
            connections.retain(|connection| connection.toolkit != *toolkit);
            connections.push(Connection {
                toolkit: (*toolkit).to_owned(),
                connection_id,
                status: Some("env-override".into()),
            });
        }
    }

    // Keep only toolkits we can actually sync, honouring an optional filter, and
    // collapse to one connection per toolkit (the first discovered).
    let mut selected: Vec<Connection> = Vec::new();
    for connection in connections {
        if !SUPPORTED_TOOLKITS.contains(&connection.toolkit.as_str()) {
            continue;
        }
        if toolkit_filter
            .as_ref()
            .is_some_and(|filter| !filter.contains(&connection.toolkit))
        {
            continue;
        }
        if selected
            .iter()
            .any(|kept| kept.toolkit == connection.toolkit)
        {
            continue;
        }
        selected.push(connection);
    }

    if selected.is_empty() {
        println!("\nNo supported+connected toolkits to sync. Connect an account in Composio");
        println!("or set COMPOSIO_<TOOLKIT>_CONNECTION_ID to force one. Phase 1 passed.");
        return Ok(());
    }

    // ── Phase 2: memory sync ────────────────────────────────────────────────
    println!("\n── Phase 2: memory sync ({max_items} item cap/toolkit) ───────");
    let transport = ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url,
        api_key: Some(SecretString::new(api_key)),
        bearer_token: None,
        entity_id,
    };
    let tmp = std::env::temp_dir().join("tinycortex-composio-harness");
    let mut config = MemoryConfig::new(tmp.to_string_lossy().into_owned());
    config.sync.budget.max_items = Some(max_items);

    let captures = Arc::new(Captures::default());
    let mut reports = Vec::new();
    for connection in &selected {
        println!(
            "→ syncing {} ({})",
            connection.toolkit, connection.connection_id
        );
        let report = run_toolkit(
            &connection.toolkit,
            &connection.connection_id,
            &config,
            &transport,
            &captures,
        )
        .await;
        reports.push(report);
    }

    print_report(&reports);

    let total_docs = captures.documents.lock().unwrap().len();
    let passed = reports.iter().filter(|report| report.passed()).count();
    println!(
        "\nSummary: {passed}/{} toolkit(s) passed, {total_docs} document(s) ingested into memory.",
        reports.len()
    );

    if passed == reports.len() {
        println!("HARNESS PASS");
        Ok(())
    } else {
        anyhow::bail!(
            "{}/{} toolkit(s) failed",
            reports.len() - passed,
            reports.len()
        )
    }
}
