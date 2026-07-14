//! Composio v3 login/connect helpers.
//!
//! This module owns the reusable, host-agnostic pieces of the Composio
//! connection flow so a harness (or a server-side host) can drive an OAuth
//! login without re-deriving the wire contract:
//!
//! * a small per-integration **entity-id store** ([`EntityStore`]) that
//!   remembers the `user_id` chosen for each toolkit across runs so re-runs
//!   reuse the same Composio "entity" instead of orphaning connections;
//! * pure parsers for the connected-account **status** lifecycle; and
//! * thin async wrappers over the three v3 endpoints the connect flow needs.
//!
//! ## Verified v3 endpoints
//!
//! All confirmed against the Composio SDK source (the generated OpenAPI client
//! these SDKs wrap) — <https://github.com/ComposioHQ/composio> — and the v3 API
//! reference at <https://docs.composio.dev/reference/api-reference>:
//!
//! * `GET  /api/v3/auth_configs?toolkit_slug={slug}` — list auth configs;
//!   response `{ items: [ { id, toolkit: { slug } } ] }`.
//!   (`ts/packages/core/src/models/AuthConfigs.ts`, `authConfigs.types.ts`.)
//! * `POST /api/v3/connected_accounts/link` — create a Composio Connect Link;
//!   body `{ auth_config_id, user_id, callback_url? }`, response
//!   `{ connected_account_id, redirect_url }`.
//!   (`ConnectedAccounts.ts` `link()`, `connected_accounts.py` `link()`.)
//! * `GET  /api/v3/connected_accounts/{nanoid}` — poll status; top-level
//!   `status` in `INITIALIZING | INITIATED | ACTIVE | EXPIRED | FAILED |
//!   REVOKED`. (`connected_accounts.py` `wait_for_connection`.)
//!
//! Authentication is the direct-mode `x-api-key` header, matching
//! [`super::client::ComposioClient`]. No secret is ever logged and error paths
//! discard raw response bodies (they can echo the key back verbatim).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Connection statuses that will never recover on their own — polling should
/// stop and fail. Mirrors the Composio SDK's `terminalErrorStates`
/// (`FAILED`, `EXPIRED`, `REVOKED`); `INACTIVE` is intentionally excluded
/// because it can transition back to `ACTIVE`.
const TERMINAL_STATUSES: &[&str] = &["FAILED", "EXPIRED", "REVOKED", "DELETED"];

/// Generate a fresh Composio entity id (`user_id`) for a new connection.
///
/// The `tinycortex-` prefix keeps harness-created entities recognisable in the
/// Composio dashboard while the UUID guarantees uniqueness.
pub fn generate_entity_id() -> String {
    format!("tinycortex-{}", Uuid::new_v4())
}

/// True when a connected-account status string means the account is live and
/// usable for tool execution. Case-insensitive.
pub fn status_is_active(status: &str) -> bool {
    status.trim().eq_ignore_ascii_case("ACTIVE")
}

/// True when a status string is a terminal failure that polling must give up
/// on. Case-insensitive.
pub fn status_is_terminal(status: &str) -> bool {
    let status = status.trim();
    TERMINAL_STATUSES
        .iter()
        .any(|terminal| status.eq_ignore_ascii_case(terminal))
}

/// Pull the connected-account `status` out of a get-by-id response.
///
/// Composio has shipped the status both at the top level and nested under
/// `state.val` / `connectionData.val`; probe the known shapes.
pub fn extract_status(record: &Value) -> Option<String> {
    [
        record.get("status"),
        record.pointer("/state/val/status"),
        record.pointer("/connectionData/val/status"),
        record.pointer("/connection_data/val/status"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .map(str::trim)
    .filter(|status| !status.is_empty())
    .map(str::to_owned)
}

/// Extract the OAuth redirect URL from a create-link response. The v3 `/link`
/// endpoint returns a flat `redirect_url`; older shapes nested it under
/// `connectionData.val.redirectUrl`, so probe both.
pub fn extract_redirect_url(record: &Value) -> Option<String> {
    [
        record.get("redirect_url"),
        record.get("redirectUrl"),
        record.pointer("/connectionData/val/redirectUrl"),
        record.pointer("/connection_data/val/redirect_url"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .map(str::trim)
    .filter(|url| !url.is_empty())
    .map(str::to_owned)
}

/// Extract the connected-account id from a create-link response. The v3
/// `/link` endpoint returns `connected_account_id`; legacy `initiate` returned
/// a top-level `id`.
pub fn extract_account_id(record: &Value) -> Option<String> {
    ["connected_account_id", "connectedAccountId", "id", "nanoid"]
        .iter()
        .find_map(|key| record.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Resolve an auth-config id for `toolkit` from a `GET /auth_configs` response.
///
/// Prefers an item whose `toolkit.slug` matches (case-insensitively); falls
/// back to the first listed config when the toolkit was already used as a
/// server-side filter and the slug field is shaped differently.
pub fn resolve_auth_config_id(list: &Value, toolkit: &str) -> Option<String> {
    let items = list
        .pointer("/items")
        .and_then(Value::as_array)
        .or_else(|| list.get("data").and_then(Value::as_array))
        .or_else(|| list.as_array())?;

    let matches_toolkit = |item: &Value| {
        [
            item.pointer("/toolkit/slug"),
            item.pointer("/toolkit/name"),
            item.get("toolkit"),
        ]
        .into_iter()
        .flatten()
        .find_map(Value::as_str)
        .map(|slug| slug.trim().eq_ignore_ascii_case(toolkit))
        .unwrap_or(false)
    };
    let auth_config_id = |item: &Value| {
        ["id", "nanoid"]
            .iter()
            .find_map(|key| item.get(key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };

    items
        .iter()
        .find(|item| matches_toolkit(item))
        .and_then(auth_config_id)
        .or_else(|| items.iter().find_map(auth_config_id))
}

/// A newly-created Composio Connect Link.
#[derive(Debug, Clone)]
pub struct ConnectionLink {
    /// The pending connected-account id to poll for `ACTIVE`.
    pub connected_account_id: String,
    /// The OAuth URL the user must open to complete login, when the scheme is
    /// redirect-based. `None` for schemes that activate without a browser step.
    pub redirect_url: Option<String>,
}

/// Persistent, per-toolkit map of the entity id (`user_id`) chosen for each
/// integration.
///
/// Stored as a small JSON object on disk (e.g. `.composio-harness.json`) so a
/// re-run reuses the same Composio entity instead of creating a fresh — and
/// therefore orphaned — connection every time. Load is best-effort: a missing
/// or corrupt file yields an empty store rather than an error.
#[derive(Debug, Clone)]
pub struct EntityStore {
    path: PathBuf,
    entries: BTreeMap<String, String>,
}

#[derive(Default, Serialize, Deserialize)]
struct EntityStoreFile {
    /// toolkit slug -> entity id (`user_id`).
    #[serde(default)]
    entities: BTreeMap<String, String>,
}

impl EntityStore {
    /// Load the store from `path`, tolerating a missing or unreadable file.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<EntityStoreFile>(&raw).ok())
            .map(|file| file.entities)
            .unwrap_or_default();
        Self { path, entries }
    }

    /// The backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The entity id recorded for `toolkit`, if any.
    pub fn get(&self, toolkit: &str) -> Option<&str> {
        self.entries.get(toolkit).map(String::as_str)
    }

    /// Record `entity_id` for `toolkit` in memory (call [`Self::save`] to
    /// persist).
    pub fn set(&mut self, toolkit: &str, entity_id: impl Into<String>) {
        self.entries.insert(toolkit.to_owned(), entity_id.into());
    }

    /// Resolve the entity id to use when connecting `toolkit`, persisting the
    /// choice so future runs are stable.
    ///
    /// Precedence: a value already stored for this toolkit wins; otherwise an
    /// explicit `override_id` (e.g. `COMPOSIO_ENTITY_ID`) is adopted; otherwise
    /// a fresh id is generated. The resolved id is written back and saved.
    pub fn entity_id_for(
        &mut self,
        toolkit: &str,
        override_id: Option<&str>,
    ) -> std::io::Result<String> {
        if let Some(existing) = self.get(toolkit) {
            return Ok(existing.to_owned());
        }
        let chosen = override_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(generate_entity_id);
        self.set(toolkit, chosen.clone());
        self.save()?;
        Ok(chosen)
    }

    /// Serialize the store to its backing file (pretty JSON).
    pub fn save(&self) -> std::io::Result<()> {
        let file = EntityStoreFile {
            entities: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(&self.path, json)
    }
}

/// `GET /api/v3/auth_configs?toolkit_slug={toolkit}` — list auth configs,
/// optionally filtered to one toolkit. Returns the decoded JSON body so the
/// caller can resolve an id via [`resolve_auth_config_id`].
pub async fn list_auth_configs(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    toolkit: Option<&str>,
) -> anyhow::Result<Value> {
    let mut request = http
        .get(format!("{}/auth_configs", base_url.trim_end_matches('/')))
        .header("x-api-key", api_key);
    if let Some(toolkit) = toolkit {
        request = request.query(&[("toolkit_slug", toolkit)]);
    }
    let response = request
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("auth_configs request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        // Never echo the body; it can contain the key back verbatim.
        let _ = response.bytes().await;
        anyhow::bail!("auth_configs returned HTTP {status}");
    }
    response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("auth_configs decode failed: {error}"))
}

/// `POST /api/v3/connected_accounts/link` — create a Composio Connect Link for
/// `auth_config_id` scoped to `user_id`. Returns the pending account id plus an
/// optional OAuth redirect URL.
pub async fn create_connection_link(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    auth_config_id: &str,
    user_id: &str,
    callback_url: Option<&str>,
) -> anyhow::Result<ConnectionLink> {
    let mut body = serde_json::json!({
        "auth_config_id": auth_config_id,
        "user_id": user_id,
    });
    if let Some(callback_url) = callback_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body["callback_url"] = serde_json::json!(callback_url);
    }
    let response = http
        .post(format!(
            "{}/connected_accounts/link",
            base_url.trim_end_matches('/')
        ))
        .header("x-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("connected_accounts/link request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let _ = response.bytes().await;
        anyhow::bail!("connected_accounts/link returned HTTP {status}");
    }
    let record: Value = response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("connected_accounts/link decode failed: {error}"))?;
    let connected_account_id = extract_account_id(&record).ok_or_else(|| {
        anyhow::anyhow!("connected_accounts/link response missing a connected account id")
    })?;
    Ok(ConnectionLink {
        connected_account_id,
        redirect_url: extract_redirect_url(&record),
    })
}

/// `GET /api/v3/connected_accounts/{account_id}` — fetch the current status of
/// a (possibly pending) connected account. Returns `None` if the response had
/// no recognisable status field.
pub async fn get_connection_status(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    account_id: &str,
) -> anyhow::Result<Option<String>> {
    let response = http
        .get(format!(
            "{}/connected_accounts/{account_id}",
            base_url.trim_end_matches('/')
        ))
        .header("x-api-key", api_key)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("connected_accounts/{{id}} request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let _ = response.bytes().await;
        anyhow::bail!("connected_accounts/{{id}} returned HTTP {status}");
    }
    let record: Value = response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("connected_accounts/{{id}} decode failed: {error}"))?;
    Ok(extract_status(&record))
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
