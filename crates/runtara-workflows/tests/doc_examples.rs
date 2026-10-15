//! Every JSON example the DSL ships parses, and the validation examples
//! produce the outcome their names promise.
//!
//! Sources: the ```json blocks in `runtara-dsl/src/schema_types.rs` doc
//! comments and in the `runtara-dsl` README, and the files under
//! `examples/validation` (`valid_*` validates clean, `error_*` reports an
//! error, `warning_*` reports a warning and no error).

use std::collections::HashMap;
use std::path::Path;

use runtara_dsl::agent_meta::AgentCatalog;
use runtara_dsl::{
    ConditionExpression, ExecutionGraph, MappingValue, SchemaField, Step, VisibleWhen,
};
use serde_json::Value;

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// ```json blocks, with the line each starts on, from `text`. Doc-comment
/// prefixes (`///`, `//!`) are stripped first when `doc_comments` is set.
fn json_blocks(text: &str, doc_comments: bool) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, Vec<String>)> = None;
    for (index, line) in text.lines().enumerate() {
        let body = if doc_comments {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed
                .strip_prefix("///")
                .or_else(|| trimmed.strip_prefix("//!"))
            else {
                current = None;
                continue;
            };
            rest.strip_prefix(' ').unwrap_or(rest).to_string()
        } else {
            line.to_string()
        };
        let fence = body.trim();
        if current.is_none() && fence == "```json" {
            current = Some((index + 1, Vec::new()));
        } else if fence == "```" {
            if let Some((start, lines)) = current.take() {
                blocks.push((start, lines.join("\n")));
            }
        } else if let Some((_, lines)) = current.as_mut() {
            lines.push(body);
        }
    }
    blocks
}

/// Parse a DSL fragment as the type its shape says it illustrates.
fn parse_dsl_fragment(value: &Value) -> Result<(), String> {
    let object = value.as_object().ok_or("not an object")?;
    let attempt = |result: Result<(), serde_json::Error>| result.map_err(|e| e.to_string());
    if object.contains_key("stepType") {
        return attempt(serde_json::from_value::<Step>(value.clone()).map(drop));
    }
    if object.contains_key("valueType") {
        return attempt(serde_json::from_value::<MappingValue>(value.clone()).map(drop));
    }
    if object.contains_key("field") {
        return attempt(serde_json::from_value::<VisibleWhen>(value.clone()).map(drop));
    }
    if object.contains_key("inputs") {
        // An execution start request.
        let inputs = value["inputs"].clone();
        return runtara_dsl::input_validation::validate_workflow_inputs(inputs)
            .map(drop)
            .map_err(|e| e.message);
    }
    if object.contains_key("entryPoint") || object.contains_key("executionGraph") {
        let graph = value.get("executionGraph").unwrap_or(value).clone();
        return attempt(serde_json::from_value::<ExecutionGraph>(graph).map(drop));
    }
    if let Some(condition) = object.get("condition")
        && object.len() == 1
    {
        return attempt(serde_json::from_value::<ConditionExpression>(condition.clone()).map(drop));
    }
    // A map of fields: a schema or an input mapping.
    let schema = serde_json::from_value::<HashMap<String, SchemaField>>(value.clone());
    let mapping = serde_json::from_value::<HashMap<String, MappingValue>>(value.clone());
    match (schema, mapping) {
        (Ok(_), _) | (_, Ok(_)) => Ok(()),
        (Err(schema), Err(_)) => Err(schema.to_string()),
    }
}

#[test]
fn dsl_doc_examples_parse() {
    let mut failures = Vec::new();
    for (file, doc_comments) in [
        ("../runtara-dsl/src/schema_types.rs", true),
        ("../runtara-dsl/README.md", false),
    ] {
        let text = std::fs::read_to_string(manifest_dir().join(file)).expect("source file");
        let blocks = json_blocks(&text, doc_comments);
        assert!(!blocks.is_empty(), "no ```json blocks found in {file}");
        for (line, block) in blocks {
            let parsed = serde_json::from_str::<Value>(&block)
                .map_err(|e| e.to_string())
                .and_then(|value| parse_dsl_fragment(&value));
            if let Err(error) = parsed {
                failures.push(format!("{file}:{line}: {error}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "doc examples that do not parse:\n{}",
        failures.join("\n")
    );
}

#[test]
fn validation_examples_produce_the_outcome_their_names_promise() {
    let catalog = AgentCatalog::from_json(include_str!("catalog/agent_catalog.json"))
        .expect("catalog fixture parses");
    let dir = manifest_dir().join("examples/validation");
    let mut failures = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("examples dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}: not JSON: {e}"));
        let graph = value.get("executionGraph").unwrap_or(&value).clone();
        let graph: ExecutionGraph = match serde_json::from_value(graph) {
            Ok(graph) => graph,
            Err(error) => {
                failures.push(format!("{name}: does not parse: {error}"));
                continue;
            }
        };
        let result = runtara_workflows::validation::validate_workflow(&graph, &catalog);
        let errors: Vec<String> = result.errors.iter().map(|e| e.code().to_string()).collect();
        let warnings: Vec<String> = result
            .warnings
            .iter()
            .map(|w| w.code().to_string())
            .collect();
        let ok = if name.starts_with("valid") {
            errors.is_empty()
        } else if name.starts_with("error") {
            !errors.is_empty()
        } else if name.starts_with("warning") {
            errors.is_empty() && !warnings.is_empty()
        } else {
            true
        };
        if !ok {
            failures.push(format!("{name}: errors {errors:?}, warnings {warnings:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "examples that do not match their names:\n{}",
        failures.join("\n")
    );
}
