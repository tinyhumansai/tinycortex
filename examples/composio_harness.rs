//! Composio connection + memory-sync harness.
//!
//! An end-to-end, runnable check that a Composio API key is wired correctly and
//! that TinyCortex's Composio sync pipelines actually ingest memory for every
//! connected toolkit.
//!
//! ## Usage
//!
//! Either export the key inline, or copy `.env.example` to `.env` and fill it
//! in (`.env` is gitignored; the harness loads it automatically, and real
//! process env still overrides it):
//!
//! ```sh
//! cp .env.example .env   # then edit
//! cargo run --example composio_harness --features sync
//! # or:
//! COMPOSIO_API_KEY=ak_... cargo run --example composio_harness --features sync
//! ```
//!
//! ### Phase 1 — Connection test
//! Validates the API key by listing connected accounts against the Composio v3
//! API (`GET /connected_accounts`). This proves the key is live and discovers
//! which toolkits are connected together with their `connected_account_id`s.
//!
//! ### Phase 1.5 — Login / connect flow
//! When you request specific toolkits with `COMPOSIO_TOOLKITS` and one of them
//! has no ACTIVE connected account (discovery found none, or its status is a
//! terminal FAILED/EXPIRED/REVOKED), the harness initiates a Composio Connect
//! link for it: it resolves the toolkit's auth-config id (or an explicit
//! `COMPOSIO_<TOOLKIT>_AUTH_CONFIG_ID`), creates a connection scoped to a
//! remembered per-toolkit entity id, prints the OAuth URL for you to open, then
//! polls until the account is ACTIVE (or times out) before syncing it. This
//! step is skipped entirely when `COMPOSIO_TOOLKITS` is unset, so the default
//! run stays non-interactive and CI-safe.
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
//! - `COMPOSIO_ENTITY_ID`          Composio `user_id` scoping accounts. When the
//!   connect flow mints a new connection it uses this if set, else a generated
//!   `tinycortex-<uuid>` remembered per toolkit in `.composio-harness.json`.
//! - `COMPOSIO_TOOLKITS`           comma list restricting which toolkits to sync;
//!   also the set the connect flow may log in when not already active.
//! - `COMPOSIO_MAX_ITEMS`          per-toolkit ingest cap (default 25).
//! - `COMPOSIO_<TOOLKIT>_CONNECTION_ID`  pin/override a connection id, e.g.
//!   `COMPOSIO_GMAIL_CONNECTION_ID`. Also seeds a toolkit if discovery is empty.
//! - `COMPOSIO_<TOOLKIT>_AUTH_CONFIG_ID`  pin the auth-config id used when
//!   initiating a login for that toolkit, e.g. `COMPOSIO_GMAIL_AUTH_CONFIG_ID`.
//!   When unset the harness looks one up via `GET /auth_configs`.
//! - `COMPOSIO_CALLBACK_URL`       optional OAuth callback/redirect passed to the
//!   connect link.
//! - `COMPOSIO_CONNECT_TIMEOUT_SECS`  how long to poll a pending login for
//!   `ACTIVE` before failing (default 120).
//! - `TINYCORTEX_WORKSPACE`        when set, persist ingested documents (via
//!   `KvSkillDocSink`) and a `sync_manifest.json` run manifest into this
//!   workspace directory so the memory viewer can inspect what sync produced.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tinycortex::memory::config::{ComposioMode, ComposioSyncConfig, MemoryConfig, SecretString};
use tinycortex::memory::sync::{
    create_connection_link, get_connection_status, list_auth_configs, resolve_auth_config_id,
    status_is_active, status_is_terminal, ClickUpSyncPipeline, ComposioClient, EntityStore,
    GitHubSyncPipeline, GmailSyncPipeline, KvSkillDocSink, LinearSyncPipeline, NotionSyncPipeline,
    SkillDocSink, SkillDocument, SlackSyncPipeline, SyncContext, SyncEvent, SyncEventSink,
    SyncPipeline, SyncState, SyncStateStore,
};

const DEFAULT_BASE_URL: &str = "https://backend.composio.dev/api/v3";
const DEFAULT_MAX_ITEMS: u32 = 25;
/// Local, gitignored file remembering the entity id (`user_id`) per toolkit.
const ENTITY_STORE_FILE: &str = ".composio-harness.json";
/// How long to poll a pending connection for `ACTIVE` before giving up.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 120;
/// Seconds between connection-status polls.
const CONNECT_POLL_INTERVAL_SECS: u64 = 3;

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
    /// The account's Composio `user_id`. Composio v3 requires it alongside
    /// `connected_account_id` on every tool execution, so it is captured here
    /// during discovery and passed through to the sync transport.
    user_id: Option<String>,
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
            user_id: record
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
        });
    }
    Ok(connections)
}

/// Phase 1.5: drive a Composio Connect login for one toolkit and return an
/// ACTIVE connection, or an error carrying manual next steps.
///
/// Resolves an auth-config id (explicit override or `GET /auth_configs`),
/// creates a connect link scoped to `entity_id`, prints the OAuth URL, then
/// polls `GET /connected_accounts/{id}` until the account is ACTIVE, hits a
/// terminal failure, or `timeout` elapses. Never prints secrets.
#[allow(clippy::too_many_arguments)]
async fn connect_toolkit(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    toolkit: &str,
    entity_id: &str,
    auth_config_override: Option<&str>,
    callback_url: Option<&str>,
    timeout: std::time::Duration,
) -> anyhow::Result<Connection> {
    // 1. Resolve the auth-config id for this toolkit.
    let auth_config_id = match auth_config_override {
        Some(id) => id.to_owned(),
        None => {
            let list = list_auth_configs(http, base_url, api_key, Some(toolkit)).await?;
            resolve_auth_config_id(&list, toolkit).ok_or_else(|| {
                anyhow::anyhow!(
                    "no auth config found for '{toolkit}'. Create one in the Composio dashboard \
                     or set COMPOSIO_{}_AUTH_CONFIG_ID.",
                    toolkit.to_ascii_uppercase()
                )
            })?
        }
    };
    println!("  auth config: {auth_config_id}");

    // 2. Create the Composio Connect link.
    let link = create_connection_link(
        http,
        base_url,
        api_key,
        &auth_config_id,
        entity_id,
        callback_url,
    )
    .await?;

    // 3. Prompt the user to complete the browser login.
    match &link.redirect_url {
        Some(url) => {
            println!("  action required — open this URL to authorize {toolkit}, then return here:");
            println!("    {url}");
        }
        None => println!("  no browser step reported for {toolkit}; waiting for activation…"),
    }
    println!(
        "  polling connection {} for up to {}s…",
        link.connected_account_id,
        timeout.as_secs()
    );

    // 4. Poll until ACTIVE / terminal / timeout.
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match get_connection_status(http, base_url, api_key, &link.connected_account_id).await {
            Ok(Some(status)) if status_is_active(&status) => {
                println!("  {toolkit} connection is ACTIVE.");
                return Ok(Connection {
                    toolkit: toolkit.to_owned(),
                    connection_id: link.connected_account_id,
                    status: Some(status),
                    // The connection was minted for exactly this entity id.
                    user_id: Some(entity_id.to_owned()),
                });
            }
            Ok(Some(status)) if status_is_terminal(&status) => {
                anyhow::bail!("{toolkit} login ended in terminal state {status}; re-run to retry.")
            }
            Ok(Some(status)) => println!("    status: {status} …"),
            Ok(None) => println!("    status: (unknown) …"),
            // A transient poll error shouldn't abort the whole wait.
            Err(error) => println!("    poll error: {error}"),
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for {toolkit} to become ACTIVE after {}s. Finish the login in \
                 the browser, then re-run — the entity id is remembered in {}.",
                timeout.as_secs(),
                ENTITY_STORE_FILE
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(CONNECT_POLL_INTERVAL_SECS)).await;
    }
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

/// Persist captured skill documents into `workspace` via the durable
/// [`KvSkillDocSink`], and drop a JSON run manifest (per-toolkit results plus
/// the full sync-event stream) at `<workspace>/sync_manifest.json`. This is
/// what the memory viewer reads to show what a sync run ingested.
async fn persist_to_workspace(
    workspace: &str,
    captures: &Arc<Captures>,
    reports: &[ToolkitReport],
) -> anyhow::Result<()> {
    let root = std::path::Path::new(workspace);
    let sink = KvSkillDocSink::open_in_workspace(root)?;
    let docs = captures.documents.lock().unwrap().clone();
    let stored = docs.len();
    for doc in docs {
        sink.store(doc).await?;
    }

    let toolkits: Vec<Value> = reports
        .iter()
        .map(|report| {
            serde_json::json!({
                "toolkit": report.toolkit,
                "connectionId": report.connection_id,
                "ingested": report.ingested,
                "actions": report.actions,
                "costUsd": report.cost_usd,
                "docsStored": report.docs_stored,
                "taintOk": report.taint_ok,
                "cursorAdvanced": report.cursor_advanced,
                "idempotency": report.idempotency,
                "passed": report.passed(),
                "error": report.error,
            })
        })
        .collect();
    let events = serde_json::to_value(&*captures.events.lock().unwrap())?;
    let manifest = serde_json::json!({
        "toolkits": toolkits,
        "events": events,
        "documentsPersisted": stored,
    });
    let manifest_path = root.join("sync_manifest.json");
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    println!(
        "\nPersisted {stored} document(s) to {} and a run manifest to {}.",
        root.join(tinycortex::memory::sync::SKILL_DOCS_DB).display(),
        manifest_path.display()
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("\nharness aborted: {error}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    // Load a `.env` from the working dir (and any ancestor) if present. Existing
    // process env always wins, so `COMPOSIO_API_KEY=... cargo run` still works.
    match dotenvy::dotenv() {
        Ok(path) => println!("loaded env from {}", path.display()),
        Err(error) if error.not_found() => {}
        Err(error) => eprintln!("warning: could not read .env: {error}"),
    }

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
    // When set, persist ingested documents + a run manifest into this workspace
    // so the memory viewer (or any host) can inspect what sync produced.
    let workspace = env_opt("TINYCORTEX_WORKSPACE");

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
                // Unknown for a pinned id; Phase 2 falls back to COMPOSIO_ENTITY_ID.
                user_id: None,
            });
        }
    }

    // Keep only toolkits we can actually sync, honouring an optional filter, and
    // collapse to one connection per toolkit (the first discovered). A
    // connection whose status is a terminal failure is treated as absent so the
    // connect flow can re-establish it.
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
        if connection.status.as_deref().is_some_and(status_is_terminal) {
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

    // ── Phase 1.5: login/connect flow for requested-but-missing toolkits ────
    // Only runs for explicitly requested toolkits so the default (no filter)
    // run stays non-interactive. Each new login reuses/records a per-toolkit
    // entity id so re-runs don't orphan connections.
    if let Some(requested) = toolkit_filter.as_ref() {
        let store_path = std::env::current_dir()
            .unwrap_or_default()
            .join(ENTITY_STORE_FILE);
        let mut entity_store = EntityStore::load(&store_path);
        let connect_timeout = std::time::Duration::from_secs(
            env_opt("COMPOSIO_CONNECT_TIMEOUT_SECS")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
        );
        let callback_url = env_opt("COMPOSIO_CALLBACK_URL");

        for toolkit in requested {
            if !SUPPORTED_TOOLKITS.contains(&toolkit.as_str()) {
                continue;
            }
            if selected.iter().any(|kept| kept.toolkit == *toolkit) {
                continue;
            }
            println!("\n── Phase 1.5: connecting {toolkit} ───────────────────────────");
            let toolkit_entity = match entity_store.entity_id_for(toolkit, entity_id.as_deref()) {
                Ok(id) => id,
                Err(error) => {
                    println!("  could not persist entity id for {toolkit}: {error}");
                    continue;
                }
            };
            println!("  entity id: {toolkit_entity} (stored in {ENTITY_STORE_FILE})");
            let auth_config_override = env_opt(&format!(
                "COMPOSIO_{}_AUTH_CONFIG_ID",
                toolkit.to_ascii_uppercase()
            ));
            match connect_toolkit(
                &http,
                &base_url,
                &api_key,
                toolkit,
                &toolkit_entity,
                auth_config_override.as_deref(),
                callback_url.as_deref(),
                connect_timeout,
            )
            .await
            {
                Ok(connection) => selected.push(connection),
                Err(error) => println!("  connect failed for {toolkit}: {error}"),
            }
        }
    }

    if selected.is_empty() {
        println!("\nNo supported+connected toolkits to sync. Connect an account in Composio");
        println!("or set COMPOSIO_<TOOLKIT>_CONNECTION_ID to force one. Phase 1 passed.");
        return Ok(());
    }

    // ── Phase 2: memory sync ────────────────────────────────────────────────
    println!("\n── Phase 2: memory sync ({max_items} item cap/toolkit) ───────");
    let api_secret = SecretString::new(api_key);
    let tmp = std::env::temp_dir().join("tinycortex-composio-harness");
    let mut config = MemoryConfig::new(tmp.to_string_lossy().into_owned());
    config.sync.budget.max_items = Some(max_items);

    let captures = Arc::new(Captures::default());
    let mut reports = Vec::new();
    for connection in &selected {
        // Composio v3 requires the account's user_id alongside its
        // connected_account_id, so scope each toolkit's transport to that
        // account's user_id (falling back to a global COMPOSIO_ENTITY_ID).
        let conn_entity = connection.user_id.clone().or_else(|| entity_id.clone());
        if conn_entity.is_none() {
            println!(
                "  ! no user_id for {} — set COMPOSIO_ENTITY_ID; execution will 400",
                connection.toolkit
            );
        }
        let transport = ComposioSyncConfig {
            mode: ComposioMode::Direct,
            base_url: base_url.clone(),
            api_key: Some(api_secret.clone()),
            bearer_token: None,
            entity_id: conn_entity.clone(),
        };
        println!(
            "→ syncing {} ({}, user_id={})",
            connection.toolkit,
            connection.connection_id,
            conn_entity.as_deref().unwrap_or("(none)")
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

    if let Some(workspace) = workspace.as_deref() {
        if let Err(error) = persist_to_workspace(workspace, &captures, &reports).await {
            eprintln!("warning: could not persist to workspace {workspace}: {error}");
        }
    }

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
