// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Labels belong to the execution start contract, never workflow expressions.
use runtara_dsl::ExecutionGraph;
use serde_json::json;

#[test]
fn finish_rejects_label_assignment() {
    for label in [
        json!("order_123"),
        json!({"valueType":"reference","value":"data.id"}),
        json!(null),
    ] {
        let result = serde_json::from_value::<ExecutionGraph>(json!({
            "entryPoint":"finish",
            "steps":{"finish":{"id":"finish","stepType":"Finish","runLabel":label}}
        }));
        assert!(result.is_err(), "Finish must reject runLabel");
    }
}

#[test]
fn finish_preserves_unrelated_output_fields() {
    let graph: ExecutionGraph = serde_json::from_value(json!({
        "entryPoint":"finish",
        "steps":{"finish":{"id":"finish","stepType":"Finish",
            "inputMapping":{"runLabel":{"valueType":"immediate","value":"business output"}}}}
    }))
    .unwrap();
    let saved = serde_json::to_value(graph).unwrap();
    assert_eq!(
        saved["steps"]["finish"]["inputMapping"]["runLabel"]["value"],
        "business output"
    );
}
