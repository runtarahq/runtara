use super::workflow_runtime::WorkflowRuntimeError;
use crate::{
    api::{dto::operations::OperationQueue, repositories::operations::OperationsRepository},
    runtime_client::RuntimeClient,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Default queues come from current steps plus outstanding older requests.
pub async fn discover_queues(
    repository: &OperationsRepository,
    client: &RuntimeClient,
    tenant: &str,
) -> Result<Vec<OperationQueue>, WorkflowRuntimeError> {
    let graphs = repository
        .current_graphs(tenant)
        .await
        .map_err(|e| WorkflowRuntimeError::Runtime(e.to_string()))?;
    let mut queues = BTreeMap::new();
    let mut metadata = BTreeMap::new();
    for (id, graph) in graphs {
        let name = graph
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        let schema = graph
            .get("stateSchema")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mut keys = BTreeMap::new();
        collect_action_keys(&graph, &mut keys);
        for (key, label) in keys {
            queues.insert(
                (id.clone(), key.clone()),
                OperationQueue {
                    workflow_id: id.clone(),
                    workflow_name: name.clone(),
                    action_key: key,
                    name: label,
                    count: 0,
                    state_schema: schema.clone(),
                },
            );
        }
        metadata.insert(id, (name, schema));
    }
    for active in client
        .active_operation_queues(tenant)
        .await
        .map_err(|e| WorkflowRuntimeError::Runtime(e.to_string()))?
    {
        let (name, schema) = metadata
            .get(&active.workflow_id)
            .cloned()
            .unwrap_or_else(|| (active.workflow_id.clone(), json!({})));
        queues
            .entry((active.workflow_id.clone(), active.action_key.clone()))
            .or_insert_with(|| OperationQueue {
                workflow_id: active.workflow_id,
                workflow_name: name,
                name: active.action_key.replace('_', " "),
                action_key: active.action_key,
                count: 0,
                state_schema: schema,
            })
            .count = active.count;
    }
    Ok(queues.into_values().collect())
}

fn collect_action_keys(graph: &Value, keys: &mut BTreeMap<String, String>) {
    // Descend only through graph structure, not arbitrary input/context data.
    if let Some(steps) = graph.get("steps").and_then(Value::as_object) {
        for (id, step) in steps {
            if step.get("stepType").and_then(Value::as_str) == Some("WaitForSignal")
                && let Some(key) = step
                    .pointer("/action/key")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            {
                keys.entry(key.into()).or_insert_with(|| {
                    step.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .into()
                });
            }
            collect_action_keys(step, keys);
            for field in ["subgraph"] {
                if let Some(nested) = step.get(field) {
                    collect_action_keys(nested, keys);
                }
            }
        }
    }
}

pub fn validate_view(
    view: &crate::api::dto::operations::OperationViewConfig,
) -> Result<(), String> {
    use crate::api::{dto::executions::QueryExecutionsRequest, handlers::executions::parse_query};
    if view.name.trim().is_empty() || view.name.len() > 160 {
        return Err("View name must contain 1–160 bytes".into());
    }
    if view.workflow.is_empty()
        || view.workflow.len() > 256
        || view.workflow.contains(':')
        || view.workflow.chars().any(char::is_control)
    {
        return Err("A view requires one workflow".into());
    }
    if view
        .filter
        .open_request
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err("Invalid request key".into());
    }
    let mut fields = view.columns.clone();
    fields.extend(
        [&view.roles.key, &view.roles.stage, &view.roles.due]
            .into_iter()
            .flatten()
            .cloned(),
    );
    fields.sort();
    fields.dedup();
    runtara_environment::operations::validate_fields(
        &fields,
        view.sort.as_ref().map(Into::into).as_ref(),
    )?;
    if view.formats.len() > 32 || view.labels.len() > 32 {
        return Err("Configure at most 32 column formats and labels".into());
    }
    for format in view.formats.values() {
        if format.decimals.is_some_and(|n| n > 20)
            || [&format.prefix, &format.suffix]
                .into_iter()
                .flatten()
                .any(|s| s.len() > 64)
        {
            return Err("Use 0–20 decimal places and prefixes/suffixes up to 64 bytes".into());
        }
    }
    if view.labels.values().any(|s| s.len() > 160) {
        return Err("Column labels must be at most 160 bytes".into());
    }
    let mut state = view.filter.state.clone();
    for filter in &mut state {
        if filter.value.is_object() {
            if filter.value.get("relative").and_then(Value::as_str) != Some("now")
                || filter
                    .value
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|k| k != "relative" && k != "offsetSeconds")
            {
                return Err("Relative dates require {relative: now, offsetSeconds: number}".into());
            }
            let offset = filter
                .value
                .get("offsetSeconds")
                .map(|v| v.as_i64().ok_or("Invalid relative offset"))
                .transpose()?
                .unwrap_or(0);
            if offset.unsigned_abs() > 315_360_000 {
                return Err("Relative time offset is out of range".into());
            }
            filter.value = Value::String(
                (chrono::Utc::now() + chrono::Duration::seconds(offset)).to_rfc3339(),
            );
        }
    }
    parse_query(&QueryExecutionsRequest {
        state,
        status: view.filter.status.clone(),
        ..Default::default()
    })?;
    if serde_json::to_vec(view).map_err(|e| e.to_string())?.len() > 32 * 1024 {
        return Err("View configuration is too large".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_nested_graph_actions_without_reading_domain_data() {
        let graph = serde_json::json!({"steps": {
            "split": {"stepType": "Split", "subgraph": {"steps": {
                "wait": {"stepType": "WaitForSignal", "name": "Review", "action": {"key": "review"}}
            }}},
            "input": {"stepType": "Agent", "inputMapping": {"steps": {
                "fake": {"stepType": "WaitForSignal", "action": {"key": "fake"}}
            }}}
        }});
        let mut keys = BTreeMap::new();
        collect_action_keys(&graph, &mut keys);
        assert_eq!(keys, BTreeMap::from([("review".into(), "Review".into())]));
    }
}
