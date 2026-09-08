// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable timer identities follow both definition and invocation ancestry.
use super::*;
use serde_json::json;

fn manifest() -> DirectJsonManifest {
    DirectJsonManifest::parse(&serde_json::to_vec(&json!({"graph":{"steps":[
        {"id":"loop/[]λ","stepType":"While","nestedGraphs":[{"role":"while.subgraph","graph":{"steps":[{"id":"loop/[]λ","stepType":"While"}]}}]},
        {"id":"finish","stepType":"Finish"}
    ]}})).unwrap()).unwrap()
}
fn source(path: Value, loops: Value, namespace: &str) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"variables":{"_manifest_graph_path":path,"_durable_key_version":2,
        "_workflow_id":"wf","_loop_path":loops,"_cache_key_prefix":namespace}}),
    )
    .unwrap()
}

#[test]
fn audit_05_timer_records_separate_definition_invocation_and_terminal_state() {
    let manifest = manifest();
    let root = source(json!([]), json!([]), "");
    let timer = manifest
        .loop_deadline_key("loop/[]λ", &root, false)
        .unwrap();
    assert_eq!(
        timer,
        manifest
            .loop_deadline_key("loop/[]λ", &root, false)
            .unwrap()
    );
    assert_ne!(
        timer,
        manifest.loop_deadline_key("loop/[]λ", &root, true).unwrap()
    );
    for input in [
        source(json!([["while.subgraph", "loop/[]λ"]]), json!([]), ""),
        source(json!([]), json!([["While", "parent", 1]]), ""),
        source(json!([]), json!([]), "child/[]λ"),
    ] {
        assert_ne!(
            timer,
            manifest
                .loop_deadline_key("loop/[]λ", &input, false)
                .unwrap()
        );
    }
    assert!(
        manifest
            .loop_deadline_key("finish", &root, false)
            .unwrap_err()
            .contains("not a loop")
    );
}

#[test]
fn agent_deadline_identity_is_distinct_and_stable_across_attempts() {
    step_deadline_identity("Agent", "agent-deadline");
}

#[test]
fn embed_deadline_identity_is_distinct_and_stable_across_attempts() {
    step_deadline_identity("EmbedWorkflow", "embed-deadline");
}

fn step_deadline_identity(step_type: &str, kind: &str) {
    let manifest = DirectJsonManifest::parse(
        &serde_json::to_vec(&json!({"graph":{"steps":[
            {"id":"fetch","stepType":step_type,"nestedGraphs":[{"role":"embed.child","graph":{
                "steps":[{"id":"fetch","stepType":step_type}]}}]}
        ]}}))
        .unwrap(),
    )
    .unwrap();
    let root = source(json!([]), json!([]), "");
    let key = manifest.loop_deadline_key("fetch", &root, false).unwrap();
    assert!(key.starts_with(&format!("runtara:v2:[\"{kind}\",")));
    assert!(manifest.loop_deadline_key("fetch", &root, true).is_err());
    for attempt in [0, 1, 2, u32::MAX] {
        let mut context: Value = serde_json::from_slice(&root).unwrap();
        context["steps"] = json!({"__error":{"attempt":attempt}});
        assert_eq!(
            key,
            manifest
                .loop_deadline_key("fetch", &serde_json::to_vec(&context).unwrap(), false)
                .unwrap()
        );
    }
    for input in [
        source(json!([["embed.child", "fetch"]]), json!([]), ""),
        source(json!([]), json!([["While", "parent", 1]]), ""),
        source(json!([]), json!([]), "child"),
    ] {
        assert_ne!(
            key,
            manifest.loop_deadline_key("fetch", &input, false).unwrap()
        );
    }
}

#[test]
fn audit_05_large_timer_identity_resolves_arena_handles() {
    reset_value_store();
    let manifest = manifest();
    let input = source(json!([]), json!([["While", "x".repeat(40_000), 1]]), "");
    let value: Value = serde_json::from_slice(&input).unwrap();
    let interned = build_source(
        b"{}",
        &serde_json::to_vec(&value["variables"]).unwrap(),
        b"{}",
    )
    .unwrap();
    assert_eq!(
        manifest
            .loop_deadline_key("loop/[]λ", &input, false)
            .unwrap(),
        manifest
            .loop_deadline_key("loop/[]λ", &interned, false)
            .unwrap()
    );
}
