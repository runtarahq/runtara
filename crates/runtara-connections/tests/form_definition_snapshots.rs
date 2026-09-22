// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormFingerprint {
    integration_id: &'static str,
    field_count: usize,
    serialized_bytes: usize,
    fnv1a64: String,
}

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

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn normalized_descriptor(meta: &runtara_dsl::agent_meta::ConnectionTypeMeta) -> Value {
    canonicalize(serde_json::json!({
        "form": runtara_dsl::form::connection_form_definition(meta),
        "fieldBehavior": meta.fields.iter().map(|field| {
            (field.name, field.behavior)
        }).collect::<std::collections::BTreeMap<_, _>>()
    }))
}

fn assert_snapshot(relative_path: &str, actual: &str, expected: &str, context: &str) {
    if std::env::var_os("UPDATE_CONNECTION_FORM_SNAPSHOTS").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
        std::fs::write(path, format!("{actual}\n")).expect("update connection form snapshot");
        return;
    }
    assert_eq!(actual, expected.trim(), "{context} snapshot changed");
}

#[test]
fn every_registered_connection_form_matches_the_stable_snapshot() {
    let mut metadata = runtara_agents::registry::get_all_connection_types().collect::<Vec<_>>();
    metadata.sort_by_key(|meta| meta.integration_id);
    let fingerprints = metadata
        .into_iter()
        .map(|meta| {
            let serialized = serde_json::to_vec(&normalized_descriptor(meta)).unwrap();
            FormFingerprint {
                integration_id: meta.integration_id,
                field_count: meta.fields.len(),
                serialized_bytes: serialized.len(),
                fnv1a64: format!("{:016x}", fnv1a64(&serialized)),
            }
        })
        .collect::<Vec<_>>();
    let actual = serde_json::to_string_pretty(&fingerprints).unwrap();
    let expected = include_str!("fixtures/connection_form_fingerprints.json").trim();
    assert_snapshot(
        "tests/fixtures/connection_form_fingerprints.json",
        &actual,
        expected,
        "connection form",
    );
}

#[test]
fn mcp_condition_heavy_form_has_a_readable_snapshot() {
    let meta = runtara_agents::registry::find_connection_type("mcp").unwrap();
    let actual = serde_json::to_string_pretty(&normalized_descriptor(meta)).unwrap();
    assert_snapshot(
        "tests/fixtures/connection_form_mcp.json",
        &actual,
        include_str!("fixtures/connection_form_mcp.json"),
        "mcp",
    );
}
