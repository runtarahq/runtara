// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use runtara_dsl::SchemaField;
use runtara_dsl::form::{schema_fields_form_definition, validate_form_definition};
use serde_json::Value;

const SCHEMA_KEYS: &[&str] = &["inputSchema", "outputSchema", "responseSchema"];

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        value => value,
    }
}

fn json_files(root: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {}: {error}", root.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn visit_schemas(
    value: &Value,
    path: &str,
    visit: &mut impl FnMut(&str, &serde_json::Map<String, Value>),
) {
    match value {
        Value::Object(values) => {
            for (key, child) in values {
                let child_path = format!("{path}.{key}");
                if SCHEMA_KEYS.contains(&key.as_str())
                    && let Some(schema) = child.as_object()
                {
                    visit(&child_path, schema);
                }
                visit_schemas(child, &child_path, visit);
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                visit_schemas(child, &format!("{path}[{index}]"), visit);
            }
        }
        _ => {}
    }
}

#[test]
fn repository_stored_workflow_schemas_normalize_without_data_loss() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let roots = [
        manifest.join("tests/fixtures"),
        manifest.join("examples/validation"),
        manifest.join("../../examples/workflows"),
    ];
    let mut schema_count = 0usize;
    let mut field_count = 0usize;

    for root in roots {
        for file in json_files(&root) {
            let raw = fs::read_to_string(&file).unwrap();
            let document: Value = serde_json::from_str(&raw).unwrap();
            visit_schemas(
                &document,
                &file.display().to_string(),
                &mut |path, schema| {
                    schema_count += 1;
                    let fields: HashMap<String, SchemaField> =
                        serde_json::from_value(Value::Object(schema.clone()))
                            .unwrap_or_else(|error| panic!("{path}: {error}"));
                    let definition = schema_fields_form_definition(&fields);
                    let issues = validate_form_definition(&definition);
                    assert!(issues.is_empty(), "{path}: {issues:?}");

                    for (name, source) in fields {
                        field_count += 1;
                        let normalized = &definition.fields[&name];
                        let mut expected_schema = source.clone();
                        let had_visibility = expected_schema.visible_when.take().is_some();
                        assert_eq!(
                            serde_json::to_value(&normalized.schema).unwrap(),
                            serde_json::to_value(expected_schema).unwrap(),
                            "{path}.{name}: non-UI schema data changed"
                        );
                        if had_visibility {
                            assert!(
                                normalized.conditions.visible.is_some(),
                                "{path}.{name}: visibility was dropped"
                            );
                        }
                    }
                },
            );
        }
    }

    assert!(schema_count >= 200, "audited only {schema_count} schemas");
    assert!(field_count >= 80, "audited only {field_count} fields");
}

#[test]
fn representative_workflow_form_matches_the_stable_snapshot() {
    let fields: HashMap<String, SchemaField> = serde_json::from_value(serde_json::json!({
        "kind": {
            "type": "string", "label": "Kind", "required": true,
            "enum": ["person", "company"], "order": 1
        },
        "company_name": {
            "type": "string", "label": "Company name", "placeholder": "Acme",
            "min": 2, "max": 80, "pattern": "^[A-Za-z]",
            "visibleWhen": { "field": "kind", "equals": "company" }, "order": 2
        },
        "tags": {
            "type": "array", "items": { "type": "string" }, "format": "tags",
            "nullable": true, "order": 3
        }
    }))
    .unwrap();
    let normalized = schema_fields_form_definition(&fields);
    let actual =
        serde_json::to_string_pretty(&canonicalize(serde_json::to_value(normalized).unwrap()))
            .unwrap();
    let expected = include_str!("snapshots/normalized_workflow_form.json").trim();
    assert_eq!(actual, expected);
}
