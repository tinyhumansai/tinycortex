use serde_json::Value;

use crate::memory::sync::traits::SkillDocument;

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
