// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AUDIT-03: invocation identity, encoding and legacy artifact compatibility.
use super::*;
use serde_json::json;

fn source(path: Value) -> Value {
    json!({"variables": {"_durable_key_version": 2, "_workflow_id": "wf", "_loop_path": path}})
}

fn registry() -> DirectJsonManifest {
    DirectJsonManifest::parse(
        &serde_json::to_vec(&json!({"graph": {"steps": [
            {"id":"wait","stepType":"WaitForSignal","body":{"id":"wait","stepType":"WaitForSignal"}}
        ]}}))
        .unwrap(),
    )
    .unwrap()
}

fn agent() -> DirectJsonAgent {
    DirectJsonAgent {
        step_id: "same".into(),
        name: None,
        agent_id: "utils".into(),
        capability_id: "return-input".into(),
        connection_id: None,
        connection_ref: None,
        input_mapping_id: 0,
        required_inputs: vec![],
    }
}

fn split(config: Value) -> DirectJsonSplit {
    DirectJsonSplit {
        step_id: "same".into(),
        name: None,
        value: config,
        input_schema: json!({}),
        output_schema: json!({}),
    }
}

fn all_keys(source: &Value) -> Vec<String> {
    let bytes = serde_json::to_vec(source).unwrap();
    let manifest = registry();
    vec![
        manifest.wait_signal_id("wait", "instance", &bytes).unwrap(),
        manifest.breakpoint_key("wait", &bytes).unwrap(),
        manifest.delay_sleep_key("wait", &bytes).unwrap(),
        manifest
            .ai_wait_tool_signal_id("ai", "instance", "approve", 0, &bytes)
            .unwrap(),
        DirectJsonManifest::ai_turn_cache_key("ai", 0, &bytes).unwrap(),
        agent_cache_key(&agent(), source),
        split_cache_key(&split(json!({})), source),
        embed_workflow_cache_key("same", source),
        child_cache_prefix("same", source),
    ]
}

#[test]
fn audit_03_every_durable_builder_separates_sites_iterations_and_ancestry() {
    let paths = [
        json!([]),
        json!([["While", "a", 0]]),
        json!([["While", "b", 0]]),
        json!([["While", "a", 1]]),
        json!([["Split", "a", 0]]),
        json!([["While", "a", 0], ["Split", "b", 0]]),
        json!([["Split", "b", 0], ["While", "a", 0]]),
    ];
    let mut unique = std::collections::HashSet::new();
    for path in paths {
        let source = source(path);
        let keys = all_keys(&source);
        assert_eq!(keys, all_keys(&source), "replay must be deterministic");
        for key in keys {
            assert!(unique.insert(key.clone()), "collision: {key}");
        }
    }
}

#[test]
fn audit_03_v2_format_is_unambiguous_and_separates_tool_fields() {
    let bytes = serde_json::to_vec(&source(json!([["While", "a/::[]\"λ", 0]]))).unwrap();
    assert_eq!(
        registry()
            .wait_signal_id("wait", "instance", &bytes)
            .unwrap(),
        "runtara:v2:[\"wait\",\"wf\",[],[[\"While\",\"a/::[]\\\"λ\",0]],[\"instance\",\"wait\"]]"
    );
    // These pairs used to collapse to ai.tool.x.tool.y.0.
    let a = registry()
        .ai_wait_tool_signal_id("ai.tool.x", "instance", "y", 0, &bytes)
        .unwrap();
    let b = registry()
        .ai_wait_tool_signal_id("ai", "instance", "x.tool.y", 0, &bytes)
        .unwrap();
    assert_ne!(a, b);
    let tool = |id, label| {
        DirectJsonManifest::agent_tool_scope_input(id, label, 0, b"{}", &bytes).unwrap()
    };
    assert_ne!(tool("ai.tool.x", "y"), tool("ai", "x.tool.y"));
    assert_ne!(
        a,
        registry()
            .ai_wait_tool_signal_id("ai.tool.x", "instance", "y", 1, &bytes)
            .unwrap()
    );
}

#[test]
fn audit_03_real_loop_helpers_restore_identity_after_authored_variables() {
    let parent = source(json!([["While", "outer", 3]]));
    let config = json!({"variables": {
        "_durable_key_version":{"valueType":"immediate","value":1},
        "_loop_path":{"valueType":"immediate","value":[]},
        "_workflow_id":{"valueType":"immediate","value":"forged"},
        "_cache_key_prefix":{"valueType":"immediate","value":"forged"}
    }});
    let while_step = DirectJsonWhile {
        step_id: "same".into(),
        name: None,
        value: config.clone(),
        condition: json!(true),
    };
    let state = DirectJsonWhileState {
        index: 2,
        outputs: Value::Null,
    };
    let while_vars = while_iteration_variables(&while_step, &parent, &state).unwrap();
    let split_vars = split_iteration_variables(&split(config), &parent, json!({}), 2).unwrap();
    for (kind, vars) in [("While", while_vars), ("Split", split_vars)] {
        assert_eq!(
            vars["_loop_path"],
            json!([["While", "outer", 3], [kind, "same", 2]])
        );
        assert_eq!(vars["_durable_key_version"], 2);
        assert_eq!(vars["_workflow_id"], "wf");
        assert!(!vars.contains_key("_cache_key_prefix"));
    }
    assert_eq!(
        parent["variables"]["_loop_path"],
        json!([["While", "outer", 3]])
    );
}

#[test]
fn audit_03_child_scopes_capture_parent_loops_and_reset_local_path() {
    let child = DirectJsonChildWorkflow {
        step_id: "call".into(),
        workflow_id: "child".into(),
        variables: json!({"_loop_path": ["forged"]}),
        input_schema: json!({}),
    };
    let a = embed_child_variables("call", &child, &source(json!([["While", "a", 0]])));
    let b = embed_child_variables("call", &child, &source(json!([["While", "b", 0]])));
    assert_eq!(a["_loop_path"], json!([]));
    assert_eq!(a["_durable_key_version"], 2);
    assert_ne!(
        all_keys(&json!({"variables":a})),
        all_keys(&json!({"variables":b}))
    );
    let mut nested = source(json!([]));
    nested["variables"]["_cache_key_prefix"] = json!(child_cache_prefix("a", &nested));
    let two_sites = child_cache_prefix("b", &nested);
    assert_ne!(two_sites, child_cache_prefix("a__b", &source(json!([]))));
    let frames: Value =
        serde_json::from_str(two_sites.strip_prefix(CHILD_SCOPE_V2_PREFIX).unwrap()).unwrap();
    assert_eq!(
        frames.as_array().unwrap().len(),
        2,
        "flat ancestry, no nested escaping"
    );
}

#[test]
fn audit_03_large_interned_identity_preserves_keys_and_loop_ancestry() {
    reset_value_store();
    let mut plain = source(json!([["While", "long".repeat(20_000), 0]]));
    plain["variables"]["_cache_key_prefix"] = json!("prefix".repeat(20_000));
    let bytes = build_source(
        b"{}",
        &serde_json::to_vec(&plain["variables"]).unwrap(),
        b"{}",
    )
    .unwrap();
    let interned: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wfref_id(&interned["variables"]["_loop_path"]).is_some());
    assert_eq!(all_keys(&plain), all_keys(&interned));
    let vars = split_iteration_variables(&split(json!({})), &interned, json!({}), 2).unwrap();
    assert_eq!(vars["_loop_path"][0], plain["variables"]["_loop_path"][0]);
    assert_eq!(vars["_loop_path"][1], json!(["Split", "same", 2]));
}

#[test]
fn audit_03_start_input_cannot_override_compiler_identity() {
    let data = br#"{"data":{},"variables":{"_durable_key_version":1,"_loop_path":[],"_workflow_id":"forged"}}"#;
    let expected = source(json!([["While", "a", 0]]));
    let built: Value = serde_json::from_slice(
        &build_source(
            data,
            &serde_json::to_vec(&expected["variables"]).unwrap(),
            b"{}",
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(all_keys(&expected), all_keys(&built));
}

#[test]
fn audit_03_legacy_artifacts_keep_exact_checkpoint_and_signal_addresses() {
    let legacy = json!({"variables":{"_workflow_id":"wf", "_loop_indices":[0,2]}});
    assert_eq!(
        all_keys(&legacy),
        [
            "instance/wf/wait/[0,2]",
            "breakpoint::wait::0_2",
            "wait::0_2",
            "instance/wf/ai.tool.approve.0/[0,2]",
            "ai.turn.0/[0,2]",
            "wf::agent::utils::return-input::same::[0,2]",
            "wf::split::same::[0,2]",
            "embed_workflow::same::[0,2]",
            "wf::same[0,2]"
        ]
    );
    let config = json!({"variables": {
        "_durable_key_version":{"valueType":"immediate","value":2},
        "_cache_key_prefix":{"valueType":"immediate","value":"legacy-prefix"}
    }});
    let vars = split_iteration_variables(&split(config), &legacy, Value::Null, 3).unwrap();
    assert!(
        !vars.contains_key("_durable_key_version"),
        "legacy loops must not opt into v2 via authored variables"
    );
    assert_eq!(
        vars["_cache_key_prefix"], "legacy-prefix",
        "legacy authored namespaces retain their original behavior"
    );
    assert_eq!(
        agent_cache_key(&agent(), &json!({"variables":vars})),
        "legacy-prefix::agent::utils::return-input::same::[0,2,3]"
    );
}
