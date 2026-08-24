use serde_json::Value;

use crate::memory::sync::traits::SkillDocument;

/// Walk a JSON document by dotted path and return the first non-empty scalar.
///
/// # Not interchangeable with `tinymemory_sync::helpers::pick_str`
///
/// A second `pick_str` lives in the `tinymemory-sync` crate, and the two
/// differ. This one resolves paths with [`Value::pointer`] (so a numeric
/// segment indexes into an array) and **coerces `Number` to its string form**;
/// that one walks with [`Value::get`] (objects only) and returns `None` for any
/// non-string leaf. Swapping one for the other changes what normalisers emit
/// for numeric fields. Keep them separate.
///
/// The other one used to sit beside this file, under
/// `providers::normalize::helpers`. It moved out of this crate entirely with
/// the host-side normalisers (tinymemory#18 §B3), which is why this note names
/// a crate rather than a sibling module.
pub fn pick_str(value: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|path| {
        let pointer = format!("/{}", path.replace('.', "/"));
        value
            .pointer(&pointer)
            .and_then(|value| match value {
                Value::String(value) => Some(value.clone()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            })
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

pub fn first_array(data: &Value, pointers: &[&str]) -> Vec<Value> {
    pointers
        .iter()
        .find_map(|pointer| data.pointer(pointer).and_then(Value::as_array))
        .cloned()
        .unwrap_or_default()
}

/// Reads a Google-style `nextPageToken` from the common Composio response
/// envelopes (single- and double-`data`-wrapped), trimming and dropping empty
/// tokens. Shared by the Google provider pipelines to avoid drift.
pub fn next_page_token(data: &Value) -> Option<String> {
    [
        "/data/nextPageToken",
        "/nextPageToken",
        "/data/data/nextPageToken",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|token| !token.is_empty())
    .map(str::to_owned)
}

pub fn document(
    toolkit: &str,
    connection_id: &str,
    id: &str,
    title: String,
    content: String,
    raw: Value,
) -> SkillDocument {
    SkillDocument {
        namespace_skill_id: toolkit.into(),
        connection_id: connection_id.into(),
        document_id: format!("{toolkit}:{id}"),
        title,
        content,
        toolkit: toolkit.into(),
        metadata: serde_json::json!({
            "source": "composio-provider-incremental",
            "taint": "external_sync",
            "provider_id": id,
            "raw": raw,
        }),
    }
}

pub async fn checked_execute(
    executor: &dyn super::super::client::ActionExecutor,
    action: &str,
    arguments: Value,
    connection_id: &str,
    state: &mut crate::memory::sync::state::SyncState,
) -> anyhow::Result<super::super::client::ExecuteResponse> {
    let response = match executor
        .execute(action, arguments, Some(connection_id))
        .await
    {
        Ok(response) => response,
        Err(error) => {
            if let Some(error) = error.downcast_ref::<super::super::client::ExecuteError>() {
                state.record_requests(error.attempts);
            }
            return Err(error);
        }
    };
    state.record_action(response.attempts, response.cost_usd);
    anyhow::ensure!(
        response.successful,
        "{action} provider failure: {}",
        response
            .error
            .as_deref()
            .unwrap_or("unknown provider error")
    );
    Ok(response)
}
