//! Keys of a workflow definition that deserialization silently drops.
//!
//! Step structs reject unknown fields, but the graph, edge, variable and
//! schema-field structs accept and discard them, so a typo such as
//! `inputSchema.name.requird` validates clean and changes behaviour. Stored
//! definitions may carry such keys, so they are reported as warnings (W086)
//! rather than rejected.
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
