//! Keys of a workflow definition that deserialization silently drops.
//!
//! Step structs reject unknown fields, but the graph, edge, variable and
//! schema-field structs accept and discard them, so a typo such as
//! `inputSchema.name.requird` validates clean and changes behaviour. Stored
//! definitions may carry such keys, so they are reported as warnings (W086)
//! rather than rejected.
//!
//! Deprecated key spellings (W093) are folded into their canonical field by
//! serde aliases, so they are invisible after parsing too and are found by the
//! same walk over the raw definition.
//!
//! Steps are an internally tagged enum, whose content serde buffers before
//! picking a variant, so keys dropped inside a step are invisible to a single
//! pass over the root. The lenient structs a step can hold — nested graphs and
//! schema maps — are therefore checked again on their own.

use std::collections::HashMap;

use runtara_dsl::{ExecutionGraph, SchemaField};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::validation::ValidationWarning;

/// Every warning only the raw definition shows: ignored keys (W086) and
/// deprecated key spellings (W093). Save, graph validation, stored-workflow
/// validation and the editor's WASM validator all report these.
pub fn raw_definition_warnings(definition: &Value) -> Vec<ValidationWarning> {
    let mut warnings = ignored_definition_fields(definition);
    warnings.extend(deprecated_definition_spellings(definition));
    warnings
}

/// Deprecated millisecond field names, by step type and where they sit:
/// `(step types, inside config, deprecated, canonical)`.
const DEPRECATED_KEYS: &[(&[&str], bool, &str, &str)] = &[
    (&["Agent", "EmbedWorkflow"], false, "timeout", "timeoutMs"),
    (
        &["Agent", "EmbedWorkflow"],
        false,
        "retryDelay",
        "retryDelayMs",
    ),
    (&["Split", "While"], true, "timeout", "timeoutMs"),
    (&["Split", "AiAgent"], true, "retryDelay", "retryDelayMs"),
    (&["AiAgent"], true, "turnTimeout", "turnTimeoutMs"),
];

/// Warn (W093) on every deprecated key spelling in `definition`, in every
/// nested graph.
pub fn deprecated_definition_spellings(definition: &Value) -> Vec<ValidationWarning> {
    fn walk(graph: &Value, prefix: &str, out: &mut Vec<ValidationWarning>) {
        let Some(steps) = graph.get("steps").and_then(Value::as_object) else {
            return;
        };
        for (step_id, step) in steps {
            let step_prefix = format!("{prefix}steps.{step_id}.");
            let step_type = step.get("stepType").and_then(Value::as_str).unwrap_or("");
            for (types, in_config, found, canonical) in DEPRECATED_KEYS {
                if !types.contains(&step_type) {
                    continue;
                }
                let (holder, at) = if *in_config {
                    (step.get("config"), format!("{step_prefix}config."))
                } else {
                    (Some(step), step_prefix.clone())
                };
                if holder.and_then(|holder| holder.get(*found)).is_some() {
                    out.push(ValidationWarning::DeprecatedSpelling {
                        path: format!("{at}{found}"),
                        found: found.to_string(),
                        canonical: canonical.to_string(),
                    });
                }
            }
            for key in ["subgraph", "onWait"] {
                if let Some(nested) = step.get(key).filter(|value| value.is_object()) {
                    walk(nested, &format!("{step_prefix}{key}."), out);
                }
            }
        }
    }
    /// A reference anywhere in the definition with the deprecated `json` hint.
    fn json_hints(value: &Value, path: &str, out: &mut Vec<ValidationWarning>) {
        match value {
            Value::Object(map) => {
                if map.get("valueType").and_then(Value::as_str) == Some("reference")
                    && map.get("type").and_then(Value::as_str) == Some("json")
                {
                    out.push(ValidationWarning::DeprecatedSpelling {
                        path: format!("{path}.type"),
                        found: "json".to_string(),
                        canonical: "object or array, or no type hint".to_string(),
                    });
                }
                for (key, child) in map {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    json_hints(child, &child_path, out);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    json_hints(item, &format!("{path}.{index}"), out);
                }
            }
            _ => {}
        }
    }
    /// Every GroupBy step id, in every nested graph.
    fn group_by_ids(graph: &Value, out: &mut Vec<String>) {
        let Some(steps) = graph.get("steps").and_then(Value::as_object) else {
            return;
        };
        for (step_id, step) in steps {
            if step.get("stepType").and_then(Value::as_str) == Some("GroupBy") {
                out.push(step_id.clone());
            }
            for key in ["subgraph", "onWait"] {
                if let Some(nested) = step.get(key) {
                    group_by_ids(nested, out);
                }
            }
        }
    }
    /// References and templates reading a GroupBy's `total_groups`.
    fn total_groups_reads(
        value: &Value,
        path: &str,
        groups: &[String],
        out: &mut Vec<ValidationWarning>,
    ) {
        match value {
            Value::Object(map) => {
                let text = match map.get("valueType").and_then(Value::as_str) {
                    Some("reference" | "template") => map.get("value").and_then(Value::as_str),
                    _ => None,
                };
                if let Some(text) = text
                    && groups.iter().any(|id| {
                        [
                            format!("steps.{id}.outputs.total_groups"),
                            format!("steps['{id}'].outputs.total_groups"),
                            format!("steps[\"{id}\"].outputs.total_groups"),
                        ]
                        .iter()
                        .any(|read| text.contains(read.as_str()))
                    })
                {
                    out.push(ValidationWarning::DeprecatedSpelling {
                        path: format!("{path}.value"),
                        found: "total_groups".to_string(),
                        canonical: "totalGroups".to_string(),
                    });
                }
                for (key, child) in map {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    total_groups_reads(child, &child_path, groups, out);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    total_groups_reads(item, &format!("{path}.{index}"), groups, out);
                }
            }
            _ => {}
        }
    }
    let mut warnings = Vec::new();
    walk(definition, "", &mut warnings);
    json_hints(definition, "", &mut warnings);
    let mut groups = Vec::new();
    group_by_ids(definition, &mut groups);
    if !groups.is_empty() {
        total_groups_reads(definition, "", &groups, &mut warnings);
    }
    warnings
}

/// Warn on every key of `definition` (an `executionGraph` object) that
/// deserialization ignores. A definition that does not parse yields nothing;
/// the parse error is reported elsewhere.
pub fn ignored_definition_fields(definition: &Value) -> Vec<ValidationWarning> {
    let mut paths = Vec::new();
    collect_graph(definition, "", &mut paths);
    paths
        .into_iter()
        .map(|path| ValidationWarning::IgnoredField { path })
        .collect()
}

fn collect_graph(graph: &Value, prefix: &str, paths: &mut Vec<String>) {
    collect::<ExecutionGraph>(graph, prefix, paths);
    let Some(steps) = graph.get("steps").and_then(Value::as_object) else {
        return;
    };
    for (step_id, step) in steps {
        let step_prefix = format!("{prefix}steps.{step_id}.");
        for key in ["subgraph", "onWait"] {
            if let Some(nested) = step.get(key).filter(|value| value.is_object()) {
                collect_graph(nested, &format!("{step_prefix}{key}."), paths);
            }
        }
        // Split input/output, WaitForSignal response, AiAgent structured output.
        for key in ["inputSchema", "outputSchema", "responseSchema"] {
            if let Some(schema) = step.get(key) {
                collect::<HashMap<String, SchemaField>>(
                    schema,
                    &format!("{step_prefix}{key}."),
                    paths,
                );
            }
        }
        if let Some(schema) = step
            .get("config")
            .and_then(|config| config.get("outputSchema"))
        {
            collect::<HashMap<String, SchemaField>>(
                schema,
                &format!("{step_prefix}config.outputSchema."),
                paths,
            );
        }
    }
}

fn collect<T: DeserializeOwned>(value: &Value, prefix: &str, paths: &mut Vec<String>) {
    let _ = serde_ignored::deserialize::<_, _, T>(value, |path| {
        paths.push(format!("{prefix}{path}"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(definition: Value) -> Vec<String> {
        ignored_definition_fields(&definition)
            .into_iter()
            .map(|warning| match warning {
                ValidationWarning::IgnoredField { path } => path,
                other => panic!("unexpected warning {other}"),
            })
            .collect()
    }

    #[test]
    fn typos_in_lenient_structs_are_reported_wherever_they_sit() {
        let found = paths(json!({
            "entryPoint": "split",
            "entrypoint": "split",
            "inputSchema": {"name": {"type": "string", "requird": true}},
            "variables": {"color": {"type": "string", "value": "red", "vaule": "x"}},
            "executionPlan": [{"fromStep": "split", "toStep": "done", "lable": "next"}],
            "steps": {
                "split": {
                    "id": "split", "stepType": "Split",
                    "config": {"value": {"valueType": "immediate", "value": [1]}},
                    "inputSchema": {"x": {"type": "string", "nulable": true}},
                    "subgraph": {
                        "entryPoint": "f",
                        "steps": {"f": {"id": "f", "stepType": "Finish"}},
                        "outputSchema": {"y": {"type": "string", "defualt": "a"}}
                    }
                },
                "done": {"id": "done", "stepType": "Finish"}
            }
        }));
        for expected in [
            "entrypoint",
            "inputSchema.name.requird",
            "variables.color.vaule",
            "executionPlan.0.lable",
            "steps.split.inputSchema.x.nulable",
            "steps.split.subgraph.outputSchema.y.defualt",
        ] {
            assert!(
                found.iter().any(|path| path == expected),
                "{expected}: {found:?}"
            );
        }
        assert_eq!(found.len(), 6, "{found:?}");
    }

    #[test]
    fn deprecated_duration_names_are_reported_wherever_they_sit() {
        let found: Vec<String> = raw_definition_warnings(&json!({
            "entryPoint": "a",
            "steps": {
                "a": {"id": "a", "stepType": "Agent", "agentId": "utils",
                      "capabilityId": "return-input", "timeout": 5, "retryDelayMs": 1},
                "s": {"id": "s", "stepType": "Split", "config": {
                        "value": {"valueType": "immediate", "value": [1]},
                        "timeout": 5, "retryDelay": 1},
                      "subgraph": {"entryPoint": "w", "steps": {
                        "w": {"id": "w", "stepType": "While", "config": {"timeout": 5}}}}},
                "ai": {"id": "ai", "stepType": "AiAgent",
                       "config": {"turnTimeout": 5, "retryDelay": 1}}
            }
        }))
        .into_iter()
        .map(|warning| match warning {
            ValidationWarning::DeprecatedSpelling {
                path, canonical, ..
            } => {
                format!("{path}->{canonical}")
            }
            other => panic!("unexpected warning {other}"),
        })
        .collect();
        let mut found = found;
        found.sort();
        assert_eq!(
            found,
            [
                "steps.a.timeout->timeoutMs",
                "steps.ai.config.retryDelay->retryDelayMs",
                "steps.ai.config.turnTimeout->turnTimeoutMs",
                "steps.s.config.retryDelay->retryDelayMs",
                "steps.s.config.timeout->timeoutMs",
                "steps.s.subgraph.steps.w.config.timeout->timeoutMs",
            ]
        );
    }

    #[test]
    fn the_json_type_hint_is_reported_wherever_it_sits() {
        let found: Vec<String> = deprecated_definition_spellings(&json!({
            "entryPoint": "f",
            "steps": {
                "f": {"id": "f", "stepType": "Finish", "inputMapping": {
                    "a": {"valueType": "reference", "value": "data.a", "type": "json"},
                    "b": {"valueType": "reference", "value": "data.b", "type": "array"}}},
                "c": {"id": "c", "stepType": "Conditional", "condition": {"op": "IS_EMPTY",
                    "arguments": [{"valueType": "reference", "value": "data.c", "type": "json"}]}}
            }
        }))
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .any(|w| w.contains("'steps.f.inputMapping.a.type'"))
        );
        assert!(
            found
                .iter()
                .any(|w| w.contains("'steps.c.condition.arguments.0.type'"))
        );
    }

    #[test]
    fn group_by_total_groups_reads_are_reported() {
        let found: Vec<String> = deprecated_definition_spellings(&json!({
            "entryPoint": "g",
            "steps": {
                "g": {"id": "g", "stepType": "GroupBy"},
                "f": {"id": "f", "stepType": "Finish", "inputMapping": {
                    "old": {"valueType": "reference", "value": "steps.g.outputs.total_groups"},
                    "text": {"valueType": "template", "value": "{{ steps.g.outputs.total_groups }}"},
                    "new": {"valueType": "reference", "value": "steps.g.outputs.totalGroups"}}}
            }
        }))
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found.iter().all(|w| w.contains("'totalGroups'")),
            "{found:?}"
        );
    }

    #[test]
    fn a_clean_definition_reports_nothing() {
        assert!(
            paths(json!({
                "name": "clean",
                "entryPoint": "done",
                "rateLimitBudgetMs": 60000,
                "inputSchema": {"name": {"type": "string", "required": true, "default": "a"}},
                "steps": {"done": {"id": "done", "stepType": "Finish"}}
            }))
            .is_empty()
        );
    }
}
