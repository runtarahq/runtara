// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Structural tests for the direct workflow compiler.
//!
//! The emitter writes raw Wasm bytes, so these tests build fixture graphs, run
//! them through manifest/plan/emit, and parse the result back with `wasmparser`
//! to assert structure rather than behaviour: the expected host/stdlib/agent
//! imports are present, calls appear in the right order and position (e.g. a
//! breakpoint check before its import), the manifest/support/ABI custom sections
//! are embedded, `wasi:cli/run` is exported, and each supported graph shape lowers
//! at all. They are the fast safety net that catches a malformed module without
//! executing it — observable runtime parity with the generated compiler is the job
//! of the separate A/B integration suite under `tests/`.

use std::collections::HashMap;
use std::fs;
use std::process::{Command, Stdio};

use super::super::manifest::build_direct_workflow_manifest;
use super::*;
use wasmparser::{ComponentExternalKind, Encoding, Operator, Parser, Payload, TypeRef, Validator};
use wit_parser::abi::WasmType;
use wit_parser::{Function as WitFunction, ManglingAndAbi, WasmImport, WorldItem, WorldKey};

fn fixture(name: &str) -> ExecutionGraph {
    let json = match name {
        "simple" => include_str!("../../../tests/fixtures/simple_passthrough.json"),
        "conditional" => include_str!("../../../tests/fixtures/conditional_workflow.json"),
        "conditional_diamond" => {
            include_str!("../../../tests/fixtures/conditional_diamond.json")
        }
        "conditional_diamond_asymmetric" => {
            include_str!("../../../tests/fixtures/conditional_diamond_asymmetric.json")
        }
        "conditional_nested" => {
            include_str!("../../../tests/fixtures/conditional_nested.json")
        }
        "filter" => include_str!("../../../tests/fixtures/filter_simple.json"),
        "switch_value" => include_str!("../../../tests/fixtures/switch_value_simple.json"),
        "switch_routing" => include_str!("../../../tests/fixtures/switch_routing_simple.json"),
        "group_by" => include_str!("../../../tests/fixtures/group_by_simple.json"),
        "delay_simple" => include_str!("../../../tests/fixtures/delay_simple.json"),
        "delay_dynamic" => include_str!("../../../tests/fixtures/delay_dynamic.json"),
        "log" => include_str!("../../../tests/fixtures/log_no_context.json"),
        "error" => include_str!("../../../tests/fixtures/error_direct_simple.json"),
        "edge_condition" => include_str!("../../../tests/fixtures/edge_condition_priority.json"),
        "edge_condition_diamond" => {
            include_str!("../../../tests/fixtures/edge_condition_diamond.json")
        }
        "split" => include_str!("../../../tests/fixtures/split_workflow.json"),
        "split_on_error" => include_str!("../../../tests/fixtures/split_on_error.json"),
        "split_timeout" => include_str!("../../../tests/fixtures/split_timeout.json"),
        "split_with_error" => include_str!("../../../tests/fixtures/split_with_error.json"),
        "split_with_schemas" => include_str!("../../../tests/fixtures/split_with_schemas.json"),
        "split_with_schemas_failing" => {
            include_str!("../../../tests/fixtures/split_with_schemas_failing.json")
        }
        "split_nested_split" => include_str!("../../../tests/fixtures/split_nested_split.json"),
        "split_dont_stop_nested_split_error" => {
            include_str!("../../../tests/fixtures/split_dont_stop_nested_split_error.json")
        }
        "split_dont_stop_deep_nested_while_split_error" => {
            include_str!(
                "../../../tests/fixtures/split_dont_stop_deep_nested_while_split_error.json"
            )
        }
        "while_simple" => include_str!("../../../tests/fixtures/while_simple.json"),
        "while_nested_split" => include_str!("../../../tests/fixtures/while_nested_split.json"),
        "while_on_error" => include_str!("../../../tests/fixtures/while_on_error.json"),
        "while_timeout" => include_str!("../../../tests/fixtures/while_timeout.json"),
        "ai_agent_single_shot" => {
            include_str!("../../../tests/fixtures/ai_agent_single_shot.json")
        }
        "ai_agent_structured" => {
            include_str!("../../../tests/fixtures/ai_agent_structured.json")
        }
        "ai_agent_tool_loop" => {
            include_str!("../../../tests/fixtures/ai_agent_tool_loop.json")
        }
        "ai_agent_multi_tool" => {
            include_str!("../../../tests/fixtures/ai_agent_multi_tool.json")
        }
        "ai_agent_memory" => {
            include_str!("../../../tests/fixtures/ai_agent_memory.json")
        }
        "ai_agent_memory_compaction" => {
            include_str!("../../../tests/fixtures/ai_agent_memory_compaction.json")
        }
        "ai_agent_memory_summarize" => {
            include_str!("../../../tests/fixtures/ai_agent_memory_summarize.json")
        }
        "ai_agent_mcp" => {
            include_str!("../../../tests/fixtures/ai_agent_mcp.json")
        }
        "ai_agent_tool_error" => {
            include_str!("../../../tests/fixtures/ai_agent_tool_error.json")
        }
        "ai_agent_on_error" => {
            include_str!("../../../tests/fixtures/ai_agent_on_error.json")
        }
        "ai_agent_embed_tool" => {
            include_str!("../../../tests/fixtures/ai_agent_embed_tool.json")
        }
        "embed_tool_child" => {
            include_str!("../../../tests/fixtures/embed_tool_child.json")
        }
        "ai_agent_wait_tool" => {
            include_str!("../../../tests/fixtures/ai_agent_wait_tool.json")
        }
        "ai_agent_wait_tool_on_wait" => {
            include_str!("../../../tests/fixtures/ai_agent_wait_tool_on_wait.json")
        }
        "embed_agent_child_parent" => {
            include_str!("../../../tests/fixtures/embed_agent_child_parent.json")
        }
        "embed_agent_child" => {
            include_str!("../../../tests/fixtures/embed_agent_child.json")
        }
        "embed_split_child_parent" => {
            include_str!("../../../tests/fixtures/embed_split_child_parent.json")
        }
        "wait_simple" => {
            include_str!("../../../tests/fixtures/wait_for_signal_direct_simple.json")
        }
        "wait_timeout" => {
            include_str!("../../../tests/fixtures/wait_for_signal_direct_timeout.json")
        }
        "wait_on_wait" => {
            include_str!("../../../tests/fixtures/wait_for_signal_direct_on_wait.json")
        }
        "wait_on_wait_error" => {
            include_str!("../../../tests/fixtures/wait_for_signal_direct_on_wait_error.json")
        }
        "wait_for_signal_nested_on_wait" => {
            include_str!("../../../tests/fixtures/wait_for_signal_nested_on_wait.json")
        }
        "embed_workflow" => include_str!("../../../tests/fixtures/embed_workflow_workflow.json"),
        "embed_workflow_on_error_parent" => {
            include_str!("../../../tests/fixtures/embed_workflow_on_error_parent.json")
        }
        "embed_workflow_error_child" => {
            include_str!("../../../tests/fixtures/embed_workflow_error_child.json")
        }
        "embed_workflow_transient_error_child" => {
            include_str!("../../../tests/fixtures/embed_workflow_transient_error_child.json")
        }
        "embed_workflow_retry_parent" => {
            include_str!("../../../tests/fixtures/embed_workflow_retry_parent.json")
        }
        "embed_workflow_no_retry_parent" => {
            include_str!("../../../tests/fixtures/embed_workflow_no_retry_parent.json")
        }
        "embed_workflow_retry_on_error_parent" => {
            include_str!("../../../tests/fixtures/embed_workflow_retry_on_error_parent.json")
        }
        "embed_workflow_child_local_on_error_parent" => {
            include_str!("../../../tests/fixtures/embed_workflow_child_local_on_error_parent.json")
        }
        "embed_workflow_child_local_on_error_child" => {
            include_str!("../../../tests/fixtures/embed_workflow_child_local_on_error_child.json")
        }
        "embed_workflow_retry_nested_child" => {
            include_str!("../../../tests/fixtures/embed_workflow_retry_nested_child.json")
        }
        "embed_workflow_transient_error_grandchild" => {
            include_str!("../../../tests/fixtures/embed_workflow_transient_error_grandchild.json")
        }
        "embed_workflow_conditional_error_child" => {
            include_str!("../../../tests/fixtures/embed_workflow_conditional_error_child.json")
        }
        "embed_workflow_nested_parent" => {
            include_str!("../../../tests/fixtures/embed_workflow_nested_parent.json")
        }
        "embed_workflow_nested_child" => {
            include_str!("../../../tests/fixtures/embed_workflow_nested_child.json")
        }
        "embed_workflow_nested_grandchild" => {
            include_str!("../../../tests/fixtures/embed_workflow_nested_grandchild.json")
        }
        "embed_workflow_nested_great_grandchild" => {
            include_str!("../../../tests/fixtures/embed_workflow_nested_great_grandchild.json")
        }
        "embed_workflow_nested_error_great_grandchild" => {
            include_str!(
                "../../../tests/fixtures/embed_workflow_nested_error_great_grandchild.json"
            )
        }
        "fanout_diamond" => include_str!("../../../tests/fixtures/fanout_diamond.json"),
        "transform" => include_str!("../../../tests/fixtures/transform_workflow.json"),
        other => panic!("unknown fixture {other}"),
    };
    serde_json::from_str(json).expect("fixture should parse")
}

fn enable_step_breakpoint(graph: &mut ExecutionGraph, step_id: &str) {
    match graph
        .steps
        .get_mut(step_id)
        .unwrap_or_else(|| panic!("missing fixture step '{step_id}'"))
    {
        runtara_dsl::Step::Finish(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Agent(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Conditional(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Split(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Switch(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::EmbedWorkflow(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::While(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Log(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Error(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Filter(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::GroupBy(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::Delay(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::WaitForSignal(step) => step.breakpoint = Some(true),
        runtara_dsl::Step::AiAgent(step) => step.breakpoint = Some(true),
    }
}

fn non_durable_agent_graph() -> ExecutionGraph {
    serde_json::from_value(serde_json::json!({
        "durable": false,
        "steps": {
            "agent": {
                "stepType": "Agent",
                "id": "agent",
                "name": "Normalize Data",
                "agentId": "utils",
                "capabilityId": "normalize",
                "maxRetries": 0,
                "inputMapping": {
                    "value": { "valueType": "reference", "value": "data.value" }
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "result": { "valueType": "reference", "value": "steps.agent.outputs.value" }
                }
            }
        },
        "entryPoint": "agent",
        "executionPlan": [
            { "fromStep": "agent", "toStep": "finish" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("agent graph parses")
}

fn non_durable_agent_default_retry_graph() -> ExecutionGraph {
    let mut graph = non_durable_agent_graph();
    let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
        panic!("expected Agent step");
    };
    agent.max_retries = None;
    graph
}

fn non_durable_agent_connection_graph() -> ExecutionGraph {
    let mut graph = non_durable_agent_graph();
    let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
        panic!("expected Agent step");
    };
    agent.connection_id = Some("shopify-main".to_string());
    graph
}

fn durable_agent_no_retry_graph() -> ExecutionGraph {
    let mut graph = non_durable_agent_graph();
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
        panic!("expected Agent step");
    };
    agent.max_retries = Some(0);
    agent.durable = Some(true);
    graph
}

fn durable_agent_retry_graph() -> ExecutionGraph {
    let mut graph = non_durable_agent_graph();
    graph.durable = Some(true);
    graph.rate_limit_budget_ms = 2_500;
    let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
        panic!("expected Agent step");
    };
    agent.max_retries = Some(2);
    agent.retry_delay = Some(750);
    agent.durable = Some(true);
    graph
}

fn non_durable_agent_on_error_finish_graph() -> ExecutionGraph {
    serde_json::from_value(serde_json::json!({
        "durable": false,
        "steps": {
            "agent": {
                "stepType": "Agent",
                "id": "agent",
                "agentId": "utils",
                "capabilityId": "normalize",
                "maxRetries": 0,
                "inputMapping": {
                    "value": { "valueType": "reference", "value": "data.value" }
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "result": { "valueType": "reference", "value": "steps.agent.outputs.value" }
                }
            },
            "handled": {
                "stepType": "Finish",
                "id": "handled",
                "inputMapping": {
                    "handled": { "valueType": "immediate", "value": true },
                    "message": { "valueType": "reference", "value": "steps.__error.message" }
                }
            }
        },
        "entryPoint": "agent",
        "executionPlan": [
            { "fromStep": "agent", "toStep": "finish" },
            { "fromStep": "agent", "toStep": "handled", "label": "onError" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("agent onError graph parses")
}

fn non_durable_agent_conditional_on_error_graph() -> ExecutionGraph {
    serde_json::from_value(serde_json::json!({
        "durable": false,
        "steps": {
            "agent": {
                "stepType": "Agent",
                "id": "agent",
                "agentId": "utils",
                "capabilityId": "normalize",
                "maxRetries": 0,
                "inputMapping": {
                    "value": { "valueType": "reference", "value": "data.value" }
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "result": { "valueType": "reference", "value": "steps.agent.outputs.value" }
                }
            },
            "handled": {
                "stepType": "Finish",
                "id": "handled",
                "inputMapping": {
                    "handled": { "valueType": "immediate", "value": true }
                }
            },
            "fail": {
                "stepType": "Error",
                "id": "fail",
                "code": "AGENT_FAILED",
                "message": "Unhandled agent failure",
                "category": "permanent",
                "severity": "error"
            }
        },
        "entryPoint": "agent",
        "executionPlan": [
            { "fromStep": "agent", "toStep": "finish" },
            {
                "fromStep": "agent",
                "toStep": "handled",
                "label": "onError",
                "priority": 10,
                "condition": {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                        { "valueType": "reference", "value": "steps.__error.category" },
                        { "valueType": "immediate", "value": "unknown" }
                    ]
                }
            },
            { "fromStep": "agent", "toStep": "fail", "label": "onError" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("agent conditional onError graph parses")
}

fn durable_agent_conditional_on_error_graph() -> ExecutionGraph {
    let mut graph = non_durable_agent_conditional_on_error_graph();
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::Agent(agent)) = graph.steps.get_mut("agent") else {
        panic!("expected Agent step");
    };
    agent.durable = Some(true);
    graph
}

fn collect_run_plan_ids(
    plan: &DirectRunPlan,
    condition_ids: &mut Vec<u32>,
    mapping_ids: &mut Vec<u32>,
) {
    match plan {
        DirectRunPlan::Finish { mapping_id, .. } => mapping_ids.push(*mapping_id),
        DirectRunPlan::Filter { next_plan, .. } => {
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::SwitchValue { next_plan, .. } => {
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::SwitchRoute {
            branches,
            default_plan,
            merge_plan,
            ..
        } => {
            for branch in branches {
                collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
            }
            collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
            if let Some(merge_plan) = merge_plan {
                collect_run_plan_ids(merge_plan, condition_ids, mapping_ids);
            }
        }
        DirectRunPlan::EdgeRoute {
            branches,
            default_plan,
            merge_plan,
        } => {
            for branch in branches {
                condition_ids.push(branch.condition_id);
                collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
            }
            collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
            if let Some(merge_plan) = merge_plan {
                collect_run_plan_ids(merge_plan, condition_ids, mapping_ids);
            }
        }
        DirectRunPlan::Fanout {
            branches,
            merge_plan,
        } => {
            for branch in branches {
                collect_run_plan_ids(branch, condition_ids, mapping_ids);
            }
            if let Some(merge_plan) = merge_plan {
                collect_run_plan_ids(merge_plan, condition_ids, mapping_ids);
            }
        }
        DirectRunPlan::GroupBy { next_plan, .. } => {
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::Split {
            nested_plan,
            next_plan,
            ..
        } => {
            collect_run_plan_ids(nested_plan, condition_ids, mapping_ids);
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::While {
            nested_plan,
            next_plan,
            ..
        } => {
            collect_run_plan_ids(nested_plan, condition_ids, mapping_ids);
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::EmbedWorkflow {
            input_mapping_id,
            child_plan,
            next_plan,
            error_plan,
            ..
        } => {
            mapping_ids.push(*input_mapping_id);
            collect_run_plan_ids(child_plan, condition_ids, mapping_ids);
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            if let Some(error_plan) = error_plan {
                for branch in &error_plan.branches {
                    condition_ids.push(branch.condition_id);
                    collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
                }
                if let Some(default_plan) = &error_plan.default_plan {
                    collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
                }
            }
        }
        DirectRunPlan::Delay { next_plan, .. } => {
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::WaitForSignal {
            on_wait_plan,
            next_plan,
            ..
        } => {
            if let Some(on_wait_plan) = on_wait_plan {
                collect_run_plan_ids(on_wait_plan, condition_ids, mapping_ids);
            }
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::Log { next_plan, .. } => {
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::AiAgentLoop {
            input_mapping_id,
            next_plan,
            ..
        } => {
            mapping_ids.push(*input_mapping_id);
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
        }
        DirectRunPlan::Agent {
            input_mapping_id,
            next_plan,
            error_plan,
            ..
        }
        | DirectRunPlan::AiAgent {
            input_mapping_id,
            next_plan,
            error_plan,
            ..
        } => {
            mapping_ids.push(*input_mapping_id);
            collect_run_plan_ids(next_plan, condition_ids, mapping_ids);
            if let Some(error_plan) = error_plan {
                for branch in &error_plan.branches {
                    condition_ids.push(branch.condition_id);
                    collect_run_plan_ids(&branch.plan, condition_ids, mapping_ids);
                }
                if let Some(default_plan) = &error_plan.default_plan {
                    collect_run_plan_ids(default_plan, condition_ids, mapping_ids);
                }
            }
        }
        DirectRunPlan::Error { .. } => {}
        DirectRunPlan::Conditional {
            condition_id,
            true_plan,
            false_plan,
            merge_plan,
            ..
        } => {
            condition_ids.push(*condition_id);
            collect_run_plan_ids(true_plan, condition_ids, mapping_ids);
            collect_run_plan_ids(false_plan, condition_ids, mapping_ids);
            if let Some(merge_plan) = merge_plan {
                collect_run_plan_ids(merge_plan, condition_ids, mapping_ids);
            }
        }
        DirectRunPlan::Join => {}
        DirectRunPlan::ImplicitFinish => {}
    }
}

fn tool_installed(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn shared_components_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target/wasm32-wasip2/release")
        });
    let missing: Vec<_> = super::super::component::DIRECT_SHARED_COMPONENT_REQUIREMENTS
        .iter()
        .filter_map(|component| {
            let wasm = dir.join(component.bundle_wasm_filename);
            (!wasm.exists()).then_some(wasm)
        })
        .collect();
    if missing.is_empty() {
        let stdlib_wasm = dir.join("runtara_workflow_stdlib.wasm");
        let stdlib_bytes = fs::read(&stdlib_wasm).ok()?;
        for marker in [
            b"agent-error-info".as_slice(),
            b"retry-sleep-key",
            b"retry-delay-ms",
            b"workflow-error-retryable",
            b"workflow-error-rate-limited",
            b"workflow-error-retry-after-ms",
            b"agent-retry-sleep-key",
            b"agent-retry-delay-ms",
            b"agent-retry-error-info",
            b"agent-error-from-info",
            b"delay-duration-ms",
            b"wait-debug-start",
        ] {
            if !stdlib_bytes
                .windows(marker.len())
                .any(|window| window == marker)
            {
                eprintln!(
                    "SKIP: direct shared workflow stdlib component is stale: {:?}",
                    stdlib_wasm
                );
                return None;
            }
        }
        Some(dir)
    } else {
        eprintln!(
            "SKIP: direct shared workflow components are not staged: {:?}",
            missing
        );
        None
    }
}

#[test]
fn direct_core_variables_include_compile_workflow_id() {
    let graph = durable_agent_no_retry_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new_with_workflow_id(
        &manifest,
        &manifest_json,
        false,
        "wf-cache-key",
        &std::collections::HashMap::new(),
    )
    .expect("core config");

    let variables: serde_json::Value =
        serde_json::from_slice(&core_config.static_data.variables.data).expect("variables");

    assert_eq!(variables["_workflow_id"], "wf-cache-key");
}

#[test]
fn direct_core_variables_override_user_workflow_id_variable() {
    let mut graph = durable_agent_no_retry_graph();
    graph.variables.insert(
        "_workflow_id".to_string(),
        runtara_dsl::Variable {
            var_type: runtara_dsl::VariableType::String,
            value: serde_json::json!("user-provided"),
            description: None,
        },
    );
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new_with_workflow_id(
        &manifest,
        &manifest_json,
        false,
        "compiled-id",
        &std::collections::HashMap::new(),
    )
    .expect("core config");

    let variables: serde_json::Value =
        serde_json::from_slice(&core_config.static_data.variables.data).expect("variables");

    assert_eq!(variables["_workflow_id"], "compiled-id");
}

fn imported_wit_function<'a>(
    resolve: &'a Resolve,
    world: WorldId,
    interface_prefix: &str,
    function_name: &str,
) -> (&'a WorldKey, &'a WitFunction) {
    resolve.worlds[world]
        .imports
        .iter()
        .find_map(|(key, item)| match item {
            WorldItem::Interface { id, .. }
                if resolve.name_world_key(key).starts_with(interface_prefix) =>
            {
                Some((key, &resolve.interfaces[*id].functions[function_name]))
            }
            _ => None,
        })
        .expect("imported WIT function")
}

fn direct_core_imports_and_run_calls(core: &[u8]) -> (HashMap<String, u32>, Vec<u32>) {
    let mut imports = HashMap::new();
    let mut next_function_index = 0;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        imports.insert(
                            format!("{}::{}", import.module, import.name),
                            next_function_index,
                        );
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    (imports, run_calls)
}

fn direct_core_import(imports: &HashMap<String, u32>, module: &str, name: &str) -> u32 {
    *imports
        .get(&format!("{module}::{name}"))
        .unwrap_or_else(|| panic!("missing import {module}::{name}"))
}

fn direct_core_call_position(run_calls: &[u32], import_index: u32) -> usize {
    run_calls
        .iter()
        .position(|call| *call == import_index)
        .unwrap_or_else(|| panic!("missing call to import index {import_index}: {run_calls:?}"))
}

fn direct_core_call_position_after(
    run_calls: &[u32],
    import_index: u32,
    after_position: usize,
) -> usize {
    run_calls
        .iter()
        .enumerate()
        .skip(after_position + 1)
        .find_map(|(position, call)| (*call == import_index).then_some(position))
        .unwrap_or_else(|| {
            panic!(
                "missing call to import index {import_index} after {after_position}: {run_calls:?}"
            )
        })
}

fn direct_run_plan_breakpoint(run_plan: &DirectRunPlan) -> Option<bool> {
    match run_plan {
        DirectRunPlan::Finish { breakpoint, .. }
        | DirectRunPlan::Filter { breakpoint, .. }
        | DirectRunPlan::SwitchValue { breakpoint, .. }
        | DirectRunPlan::SwitchRoute { breakpoint, .. }
        | DirectRunPlan::GroupBy { breakpoint, .. }
        | DirectRunPlan::Split { breakpoint, .. }
        | DirectRunPlan::While { breakpoint, .. }
        | DirectRunPlan::EmbedWorkflow { breakpoint, .. }
        | DirectRunPlan::Delay { breakpoint, .. }
        | DirectRunPlan::WaitForSignal { breakpoint, .. }
        | DirectRunPlan::Log { breakpoint, .. }
        | DirectRunPlan::Agent { breakpoint, .. }
        | DirectRunPlan::AiAgent { breakpoint, .. }
        | DirectRunPlan::Error { breakpoint, .. }
        | DirectRunPlan::Conditional { breakpoint, .. } => Some(*breakpoint),
        DirectRunPlan::EdgeRoute { .. }
        | DirectRunPlan::Fanout { .. }
        | DirectRunPlan::AiAgentLoop { .. }
        | DirectRunPlan::Join
        | DirectRunPlan::ImplicitFinish => None,
    }
}

fn assert_direct_breakpoint_before_import(core: &[u8], module: &str, name: &str) {
    let (imports, run_calls) = direct_core_imports_and_run_calls(core);
    let debug_mode_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "debug-mode-enabled",
        ),
    );
    let breakpoint_key_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "breakpoint-key",
        ),
    );
    let checkpoint_position = direct_core_call_position_after(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "checkpoint",
        ),
        breakpoint_key_position,
    );
    let breakpoint_event_position = direct_core_call_position_after(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "breakpoint-event",
        ),
        checkpoint_position,
    );
    let custom_event_position = direct_core_call_position_after(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "custom-event",
        ),
        breakpoint_event_position,
    );
    let breakpoint_pause_position = direct_core_call_position_after(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "breakpoint-pause",
        ),
        custom_event_position,
    );
    let target_position = direct_core_call_position_after(
        &run_calls,
        direct_core_import(&imports, module, name),
        breakpoint_pause_position,
    );

    assert!(
        debug_mode_position < breakpoint_key_position
            && breakpoint_key_position < checkpoint_position
            && checkpoint_position < breakpoint_event_position
            && breakpoint_event_position < custom_event_position
            && custom_event_position < breakpoint_pause_position
            && breakpoint_pause_position < target_position,
        "breakpoint pause path should run before {module}::{name}: {run_calls:?}"
    );
}

#[test]
fn direct_compile_emits_finish_only_artifact_without_rust_crate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "simple/workflow".to_string(),
        version: 7,
        source_checksum: Some("source-sha256".to_string()),
        execution_graph: fixture("simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct artifact should validate as a Wasm component");

    assert_eq!(result.wasm_path, result.workflow_logic_wasm_path);
    assert_eq!(result.wasm_size, wasm.len());
    assert_eq!(result.workflow_logic_wasm_size, wasm.len());
    assert_eq!(result.wasm_checksum, result.workflow_logic_wasm_checksum);
    assert!(result.wasm_path.ends_with("workflow-logic.wasm"));
    assert!(!result.build_dir.join("workflow.wasm").exists());
    assert!(result.composed_wasm_path.is_none());
    assert!(result.composed_wasm_size.is_none());
    assert!(result.composed_wasm_checksum.is_none());
    assert_eq!(result.manifest_checksum.len(), 64);
    assert!(result.manifest_path.exists());
    assert!(result.support_report_path.exists());
    assert!(result.artifact_metadata_path.exists());
    assert!(result.world_wit_path.exists());
    assert!(result.wac_path.exists());
    assert!(!result.build_dir.join("Cargo.toml").exists());
    assert!(!result.build_dir.join("src/lib.rs").exists());

    let metadata: DirectArtifactMetadata =
        serde_json::from_slice(&fs::read(&result.artifact_metadata_path).expect("metadata"))
            .expect("artifact metadata json");
    assert_eq!(metadata, result.artifact_metadata);
    assert_eq!(
        metadata.schema_version,
        DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION
    );
    assert_eq!(metadata.workflow_id, "simple/workflow");
    assert_eq!(metadata.workflow_version, 7);
    assert_eq!(metadata.source_checksum.as_deref(), Some("source-sha256"));
    assert_eq!(
        metadata.template_major_version,
        crate::compile::TEMPLATE_MAJOR_VERSION
    );
    assert_eq!(metadata.manifest_checksum, result.manifest_checksum);
    assert_eq!(
        metadata.workflow_logic_wasm.sha256,
        result.workflow_logic_wasm_checksum
    );
    assert_eq!(metadata.workflow_logic_wasm.size_bytes, wasm.len() as u64);
    assert!(metadata.composed_wasm.is_none());
    assert_eq!(metadata.shared_components.len(), 2);
    assert!(
        metadata
            .shared_components
            .iter()
            .all(|component| component.wasm.is_none())
    );
    assert!(metadata.child_workflows.is_empty());
    assert!(metadata.agent_components.is_empty());
}

#[test]
fn direct_compile_emits_handler_step_with_inert_on_error_edge() {
    // Regression for the reported repro: a step inside an onError handler subtree
    // (`err_persist`) that itself carries an onError edge. The support gate used
    // to reject this with an execution-plan-routing cascade; the emitter lowers
    // the handler step normally and ignores its inert onError edge. Confirm the
    // full emit pipeline produces a VALID Wasm component end-to-end.
    let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
        r##"{
          "entryPoint": "a",
          "executionPlan": [
            {"fromStep":"a","toStep":"b"},
            {"fromStep":"b","toStep":"finish_ok"},
            {"fromStep":"a","label":"onError","toStep":"err_persist"},
            {"fromStep":"err_persist","toStep":"finish_err"},
            {"fromStep":"err_persist","label":"onError","toStep":"finish_err"}
          ],
          "steps": {
            "a": {"id":"a","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
            "b": {"id":"b","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
            "err_persist": {"id":"err_persist","stepType":"Agent","agentId":"utils","capabilityId":"get-current-iso-datetime","inputMapping":{}},
            "finish_ok": {"id":"finish_ok","stepType":"Finish","inputMapping":{"out":{"value":"ok","valueType":"immediate"}}},
            "finish_err": {"id":"finish_err","stepType":"Finish","inputMapping":{"out":{"value":"err","valueType":"immediate"}}}
          }
        }"##,
    )
    .expect("graph parses");

    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "repro/dup_to_finish".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("emit should succeed for a handler step that carries an onError edge");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted artifact should validate as a Wasm component");
}

#[test]
fn direct_compile_emits_fanout_inside_conditional_branch() {
    // Regression: a Conditional whose false branch enters a step (`miss_gate`)
    // that fans out to two parallel normal successors (`b`, `c`) which re-converge
    // at `join`. The support gate accepted this, but the plan builder used to bail
    // with "unsupported parallel normal branches" because the fan-out is off the
    // topological backbone. The Fanout plan node now linearizes it. Confirm the
    // full emit pipeline produces a valid Wasm component.
    let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
        r##"{
          "entryPoint": "cond",
          "executionPlan": [
            {"fromStep": "cond", "label": "true",  "toStep": "hit"},
            {"fromStep": "cond", "label": "false", "toStep": "miss_gate"},
            {"fromStep": "miss_gate", "toStep": "b"},
            {"fromStep": "miss_gate", "toStep": "c"},
            {"fromStep": "b", "toStep": "join"},
            {"fromStep": "c", "toStep": "join"},
            {"fromStep": "hit", "toStep": "join"},
            {"fromStep": "join", "toStep": "finish"}
          ],
          "steps": {
            "cond": {"id": "cond", "stepType": "Conditional", "condition": {"type": "operation", "op": "EQ", "arguments": [{"value": "x", "valueType": "immediate"}, {"value": "y", "valueType": "immediate"}]}},
            "hit": {"id": "hit", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {}},
            "miss_gate": {"id": "miss_gate", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {}},
            "b": {"id": "b", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {}},
            "c": {"id": "c", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {}},
            "join": {"id": "join", "stepType": "Agent", "agentId": "utils", "capabilityId": "get-current-iso-datetime", "inputMapping": {}},
            "finish": {"id": "finish", "stepType": "Finish", "inputMapping": {"out": {"value": "ok", "valueType": "immediate"}}}
          }
        }"##,
    )
    .expect("graph parses");

    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "repro/cond_then_fanout".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("emit should succeed for fan-out inside a Conditional branch");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted artifact should validate as a Wasm component");
}

#[test]
fn direct_compile_embeds_manifest_and_support_sections() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "simple".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    let mut saw_component_header = false;
    let mut saw_abi = false;
    let mut saw_manifest = false;
    let mut saw_support = false;

    for payload in Parser::new(0).parse_all(&wasm) {
        match payload.expect("wasm payload") {
            Payload::Version { encoding, .. } if !saw_component_header => {
                assert_eq!(encoding, Encoding::Component);
                saw_component_header = true;
            }
            Payload::CustomSection(section) if section.name() == DIRECT_WORKFLOW_ABI_SECTION => {
                let abi: serde_json::Value =
                    serde_json::from_slice(section.data()).expect("abi json");
                assert_eq!(
                    abi["abiVersion"].as_u64(),
                    Some(u64::from(DIRECT_WORKFLOW_ABI_VERSION))
                );
                assert_eq!(abi["artifactKind"], "direct-run-component");
                assert_eq!(abi["componentRunExport"], "wasi:cli/run@0.2.3");
                assert_eq!(abi["entryPointExecutable"].as_bool(), Some(true));
                assert_eq!(abi["runtimeExecutable"].as_bool(), Some(true));
                assert_eq!(abi["outputMode"], "stdlib-apply-mapping");
                assert_eq!(
                    abi["manifestVersion"].as_u64(),
                    Some(u64::from(DIRECT_WORKFLOW_MANIFEST_VERSION))
                );
                saw_abi = true;
            }
            Payload::CustomSection(section)
                if section.name() == DIRECT_WORKFLOW_MANIFEST_SECTION =>
            {
                let manifest: DirectWorkflowManifest =
                    serde_json::from_slice(section.data()).expect("manifest json");
                assert_eq!(manifest.checksum(), result.manifest_checksum);
                saw_manifest = true;
            }
            Payload::CustomSection(section)
                if section.name() == DIRECT_WORKFLOW_SUPPORT_SECTION =>
            {
                let report: DirectWorkflowSupportReport =
                    serde_json::from_slice(section.data()).expect("support json");
                assert!(report.supported);
                saw_support = true;
            }
            _ => {}
        }
    }

    assert!(
        saw_component_header,
        "direct artifact should be a component"
    );
    assert!(saw_abi, "direct ABI custom section should exist");
    assert!(saw_manifest, "manifest custom section should exist");
    assert!(saw_support, "support-report custom section should exist");
}

#[test]
fn direct_compile_exports_wasi_cli_run_and_imports_components() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "simple".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    let mut saw_stdlib_import = false;
    let mut saw_runtime_import = false;
    let mut saw_run_export = false;

    for payload in Parser::new(0).parse_all(&wasm) {
        match payload.expect("wasm payload") {
            Payload::ComponentImportSection(reader) => {
                for import in reader {
                    let import = import.expect("component import");
                    saw_stdlib_import |=
                        import.name.0.contains("runtara:workflow-stdlib/json@0.1.0");
                    saw_runtime_import |= import
                        .name
                        .0
                        .contains("runtara:workflow-runtime/runtime@0.1.0");
                }
            }
            Payload::ComponentExportSection(reader) => {
                for export in reader {
                    let export = export.expect("component export");
                    if export.name.0 == "wasi:cli/run@0.2.3" {
                        assert_eq!(export.kind, ComponentExternalKind::Instance);
                        saw_run_export = true;
                    }
                }
            }
            _ => {}
        }
    }

    assert!(saw_stdlib_import, "stdlib interface import should exist");
    assert!(saw_runtime_import, "runtime interface import should exist");
    assert!(saw_run_export, "wasi:cli/run export should exist");
}

#[test]
fn direct_compile_supports_conditional_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "conditional".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("conditional"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct conditional compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct conditional artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.conditions.len(), 1);
    assert_eq!(manifest.graph.mappings.len(), 2);
}

#[test]
fn direct_compile_supports_nested_conditional_tree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "conditional-nested".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("conditional_nested"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested conditional compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested conditional artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.conditions.len(), 2);
    assert_eq!(manifest.graph.mappings.len(), 3);
}

#[test]
fn direct_compile_supports_static_embed_workflow_with_finish_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: fixture("simple"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct EmbedWorkflow artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.entry_point, "call_child");
    assert_eq!(manifest.child_workflows.len(), 1);
    assert_eq!(manifest.child_workflows[0].step_id, "call_child");
    assert_eq!(manifest.child_workflows[0].graph.entry_point, "finish");
    assert_eq!(manifest.graph.mappings.len(), 2);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow {
        max_retries,
        retry_delay_ms,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected EmbedWorkflow run plan");
    };
    assert_eq!(*max_retries, 3);
    assert_eq!(*retry_delay_ms, 1_000);

    assert_eq!(result.artifact_metadata.child_workflows.len(), 1);
    assert_eq!(
        result.artifact_metadata.child_workflows[0].workflow_id,
        "child_workflow"
    );
}

#[test]
fn direct_core_run_lowers_embed_workflow_breakpoint_after_child_input_mapping() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("embed_workflow");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::EmbedWorkflow(embed)) = graph.steps.get_mut("call_child") else {
        panic!("expected EmbedWorkflow fixture step");
    };
    embed.breakpoint = Some(true);

    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent-breakpoint".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: fixture("simple"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow breakpoint compile should succeed");

    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow {
        breakpoint,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected EmbedWorkflow run plan");
    };
    assert!(*breakpoint, "durable EmbedWorkflow breakpoint should lower");
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("EmbedWorkflow breakpoint core module validates");

    let mut next_function_index = 0;
    let mut stdlib_apply_mapping_index = None;
    let mut stdlib_build_source_index = None;
    let mut runtime_debug_mode_enabled_index = None;
    let mut stdlib_breakpoint_key_index = None;
    let mut runtime_checkpoint_index = None;
    let mut stdlib_breakpoint_event_index = None;
    let mut runtime_custom_event_index = None;
    let mut runtime_breakpoint_pause_index = None;
    let mut stdlib_embed_workflow_cache_key_index = None;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                stdlib_apply_mapping_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                stdlib_build_source_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "debug-mode-enabled",
                            ) => runtime_debug_mode_enabled_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-key") => {
                                stdlib_breakpoint_key_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "checkpoint") => {
                                runtime_checkpoint_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-event") => {
                                stdlib_breakpoint_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                runtime_custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "breakpoint-pause") => {
                                runtime_breakpoint_pause_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-stdlib/json@0.1",
                                "embed-workflow-cache-key",
                            ) => stdlib_embed_workflow_cache_key_index = Some(next_function_index),
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let stdlib_apply_mapping_index = stdlib_apply_mapping_index.expect("apply-mapping import");
    let stdlib_build_source_index = stdlib_build_source_index.expect("build-source import");
    let runtime_debug_mode_enabled_index =
        runtime_debug_mode_enabled_index.expect("debug-mode-enabled import");
    let stdlib_breakpoint_key_index = stdlib_breakpoint_key_index.expect("breakpoint-key import");
    let runtime_checkpoint_index = runtime_checkpoint_index.expect("checkpoint import");
    let stdlib_breakpoint_event_index =
        stdlib_breakpoint_event_index.expect("breakpoint-event import");
    let runtime_custom_event_index = runtime_custom_event_index.expect("custom-event import");
    let runtime_breakpoint_pause_index =
        runtime_breakpoint_pause_index.expect("breakpoint-pause import");
    let stdlib_embed_workflow_cache_key_index =
        stdlib_embed_workflow_cache_key_index.expect("embed-workflow-cache-key import");

    let position = |index| {
        run_calls
            .iter()
            .position(|call| *call == index)
            .expect("expected EmbedWorkflow breakpoint call")
    };
    let position_after = |index, after| {
        run_calls
            .iter()
            .enumerate()
            .find_map(|(position, call)| (*call == index && position > after).then_some(position))
            .expect("expected EmbedWorkflow breakpoint call after prior call")
    };

    let apply_mapping_position = position(stdlib_apply_mapping_index);
    let breakpoint_source_position =
        position_after(stdlib_build_source_index, apply_mapping_position);
    let debug_mode_position =
        position_after(runtime_debug_mode_enabled_index, breakpoint_source_position);
    let breakpoint_key_position = position_after(stdlib_breakpoint_key_index, debug_mode_position);
    let checkpoint_position = position_after(runtime_checkpoint_index, breakpoint_key_position);
    let breakpoint_event_position =
        position_after(stdlib_breakpoint_event_index, checkpoint_position);
    let custom_event_position =
        position_after(runtime_custom_event_index, breakpoint_event_position);
    let breakpoint_pause_position =
        position_after(runtime_breakpoint_pause_index, custom_event_position);
    let embed_cache_key_position = position_after(
        stdlib_embed_workflow_cache_key_index,
        breakpoint_pause_position,
    );

    assert!(
        apply_mapping_position < breakpoint_source_position
            && breakpoint_source_position < debug_mode_position
            && debug_mode_position < breakpoint_key_position
            && breakpoint_key_position < checkpoint_position
            && checkpoint_position < breakpoint_event_position
            && breakpoint_event_position < custom_event_position
            && custom_event_position < breakpoint_pause_position
            && breakpoint_pause_position < embed_cache_key_position,
        "EmbedWorkflow breakpoint should pause after child input mapping and before child execution: {run_calls:?}"
    );
}

#[test]
fn direct_compile_supports_static_embed_workflow_retry_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("embed_workflow");
    let Some(runtara_dsl::Step::EmbedWorkflow(embed)) = graph.steps.get_mut("call_child") else {
        panic!("expected EmbedWorkflow fixture step");
    };
    embed.max_retries = Some(2);
    embed.retry_delay = Some(0);

    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent-retry".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: fixture("embed_workflow_error_child"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow retry override compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct EmbedWorkflow retry override artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow {
        max_retries,
        retry_delay_ms,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected EmbedWorkflow run plan");
    };
    assert_eq!(*max_retries, 2);
    assert_eq!(*retry_delay_ms, 0);
}

#[test]
fn direct_compile_supports_nested_static_embed_workflow_retry_frame_isolation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent-nested-retry".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow_retry_parent"),
        child_workflows: vec![
            crate::compile::ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_retry_nested_child"),
            },
            crate::compile::ChildWorkflowInput {
                step_id: "call_grandchild".to_string(),
                workflow_id: "grandchild_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 7,
                execution_graph: fixture("embed_workflow_transient_error_grandchild"),
            },
        ],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested EmbedWorkflow retry compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested EmbedWorkflow retry artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow {
        max_retries,
        retry_delay_ms,
        child_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected root EmbedWorkflow run plan");
    };
    assert_eq!(*max_retries, 2);
    assert_eq!(*retry_delay_ms, 0);

    let DirectRunPlan::EmbedWorkflow {
        step_id,
        max_retries,
        retry_delay_ms,
        child_plan,
        ..
    } = child_plan.as_ref()
    else {
        panic!("expected child EmbedWorkflow run plan");
    };
    assert_eq!(step_id, "call_grandchild");
    assert_eq!(*max_retries, 0);
    assert_eq!(*retry_delay_ms, 0);
    assert!(matches!(child_plan.as_ref(), DirectRunPlan::Error { .. }));
}

#[test]
fn direct_compile_supports_static_embed_workflow_with_terminal_error_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: fixture("embed_workflow_error_child"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow terminal Error child compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct EmbedWorkflow terminal Error child artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.child_workflows.len(), 1);
    assert_eq!(manifest.child_workflows[0].graph.entry_point, "fail");
    assert_eq!(manifest.child_workflows[0].graph.errors.len(), 1);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow { child_plan, .. } = &core_config.run_plan else {
        panic!("expected EmbedWorkflow run plan");
    };
    assert!(matches!(child_plan.as_ref(), DirectRunPlan::Error { .. }));
}

#[test]
fn direct_compile_supports_static_embed_workflow_with_conditional_error_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: fixture("embed_workflow_conditional_error_child"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow conditional Error child compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct EmbedWorkflow conditional Error child artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.child_workflows.len(), 1);
    assert_eq!(manifest.child_workflows[0].graph.entry_point, "check");
    assert_eq!(manifest.child_workflows[0].graph.conditions.len(), 1);
    assert_eq!(manifest.child_workflows[0].graph.errors.len(), 1);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow { child_plan, .. } = &core_config.run_plan else {
        panic!("expected EmbedWorkflow run plan");
    };
    assert!(matches!(
        child_plan.as_ref(),
        DirectRunPlan::Conditional { .. }
    ));
}

#[test]
fn direct_compile_supports_static_embed_workflow_parent_on_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow_on_error_parent"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: fixture("embed_workflow_error_child"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow parent onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct EmbedWorkflow parent onError artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow { error_plan, .. } = &core_config.run_plan else {
        panic!("expected EmbedWorkflow run plan");
    };
    let error_plan = error_plan.as_ref().expect("EmbedWorkflow onError plan");
    assert!(error_plan.branches.is_empty());
    assert!(error_plan.default_plan.is_some());
}

#[test]
fn direct_compile_supports_static_embed_workflow_child_local_on_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent-child-local-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow_child_local_on_error_parent"),
        child_workflows: vec![
            crate::compile::ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_child_local_on_error_child"),
            },
            crate::compile::ChildWorkflowInput {
                step_id: "call_grandchild".to_string(),
                workflow_id: "grandchild_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 7,
                execution_graph: fixture("embed_workflow_transient_error_grandchild"),
            },
        ],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct EmbedWorkflow child-local onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct EmbedWorkflow child-local onError artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow { child_plan, .. } = &core_config.run_plan else {
        panic!("expected root EmbedWorkflow run plan");
    };
    let DirectRunPlan::EmbedWorkflow { error_plan, .. } = child_plan.as_ref() else {
        panic!("expected child-local EmbedWorkflow run plan");
    };
    let error_plan = error_plan.as_ref().expect("child-local onError plan");
    assert!(error_plan.branches.is_empty());
    assert!(error_plan.default_plan.is_some());
}

#[test]
fn direct_compile_supports_nested_static_embed_workflow_child_closure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow_nested_parent"),
        child_workflows: vec![
            crate::compile::ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_nested_child"),
            },
            crate::compile::ChildWorkflowInput {
                step_id: "call_grandchild".to_string(),
                workflow_id: "grandchild_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 7,
                execution_graph: fixture("embed_workflow_nested_grandchild"),
            },
            crate::compile::ChildWorkflowInput {
                step_id: "call_greatgrandchild".to_string(),
                workflow_id: "great_grandchild_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 11,
                execution_graph: fixture("embed_workflow_nested_great_grandchild"),
            },
        ],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested EmbedWorkflow compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested EmbedWorkflow artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.child_workflows.len(), 3);
    assert!(
        manifest
            .child_workflows
            .iter()
            .any(|child| child.step_id == "call_child"
                && child.workflow_id == "child_workflow"
                && child.graph.entry_point == "call_grandchild")
    );
    assert!(
        manifest
            .child_workflows
            .iter()
            .any(|child| child.step_id == "call_grandchild"
                && child.workflow_id == "grandchild_workflow"
                && child.graph.entry_point == "call_greatgrandchild")
    );
    assert!(
        manifest
            .child_workflows
            .iter()
            .any(|child| child.step_id == "call_greatgrandchild"
                && child.workflow_id == "great_grandchild_workflow"
                && child.graph.entry_point == "finish_great_grandchild")
    );
    assert_eq!(result.artifact_metadata.child_workflows.len(), 3);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::EmbedWorkflow { child_plan, .. } = &core_config.run_plan else {
        panic!("expected root EmbedWorkflow run plan");
    };
    assert!(matches!(
        child_plan.as_ref(),
        DirectRunPlan::EmbedWorkflow { .. }
    ));
}

#[test]
fn direct_compile_supports_nested_static_embed_workflow_failure_closure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_workflow_nested_parent"),
        child_workflows: vec![
            crate::compile::ChildWorkflowInput {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 3,
                execution_graph: fixture("embed_workflow_nested_child"),
            },
            crate::compile::ChildWorkflowInput {
                step_id: "call_grandchild".to_string(),
                workflow_id: "grandchild_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 7,
                execution_graph: fixture("embed_workflow_nested_grandchild"),
            },
            crate::compile::ChildWorkflowInput {
                step_id: "call_greatgrandchild".to_string(),
                workflow_id: "great_grandchild_workflow".to_string(),
                version_requested: "latest".to_string(),
                version_resolved: 11,
                execution_graph: fixture("embed_workflow_nested_error_great_grandchild"),
            },
        ],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested EmbedWorkflow failure compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested EmbedWorkflow failure artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert!(manifest.child_workflows.iter().any(|child| {
        child.step_id == "call_greatgrandchild"
            && child.graph.entry_point == "fail_great_grandchild"
    }));
}

#[test]
fn direct_compile_supports_group_by_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "group-by".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("group_by"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct GroupBy compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct GroupBy artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.group_bys.len(), 1);
    assert_eq!(manifest.graph.mappings.len(), 1);
}

#[test]
fn direct_compile_supports_sequential_split_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("split");
    graph.durable = Some(false);
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Split compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Split artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits.len(), 1);
    assert_eq!(manifest.graph.splits[0].step_id, "split");
    let split_step = manifest
        .graph
        .steps
        .iter()
        .find(|step| step.id == "split")
        .expect("split step");
    let nested = split_step
        .nested_graphs
        .iter()
        .find(|nested| nested.role == "split.subgraph")
        .expect("split nested graph");
    assert_eq!(nested.graph.agents.len(), 1);
    assert_eq!(nested.graph.mappings.len(), 2);
}

#[test]
fn direct_compile_supports_nested_split_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-nested".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("split_nested_split"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested Split compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested Split artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits.len(), 1);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::Split { nested_plan, .. } = &core_config.run_plan else {
        panic!("expected root Split run plan");
    };
    assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Split { .. }));
}

#[test]
fn direct_compile_supports_dont_stop_split_with_nested_split_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-dont-stop-nested".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("split_dont_stop_nested_split_error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct dontStop nested Split compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct dontStop nested Split artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits[0].value["dontStopOnFailed"], true);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::Split {
        dont_stop_on_failed,
        nested_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected root Split run plan");
    };
    assert!(*dont_stop_on_failed);
    assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Split { .. }));
}

#[test]
fn direct_compile_supports_dont_stop_split_with_deep_nested_while_split_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-dont-stop-deep-nested".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("split_dont_stop_deep_nested_while_split_error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct dontStop deep nested Split/While compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct dontStop deep nested Split/While artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits[0].value["dontStopOnFailed"], true);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::Split {
        dont_stop_on_failed,
        nested_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected root Split run plan");
    };
    assert!(*dont_stop_on_failed);
    let DirectRunPlan::While {
        nested_plan: while_nested_plan,
        ..
    } = nested_plan.as_ref()
    else {
        panic!("expected nested While run plan");
    };
    assert!(matches!(
        while_nested_plan.as_ref(),
        DirectRunPlan::Split { .. }
    ));
}

#[test]
fn direct_compile_supports_simple_while_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "while".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("while_simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct While compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct While artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.whiles.len(), 1);
    assert_eq!(manifest.graph.whiles[0].step_id, "loop");
}

#[test]
fn direct_compile_supports_split_on_error_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("split_on_error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Split onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Split onError artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits.len(), 1);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::Split { error_plan, .. } = &core_config.run_plan else {
        panic!("expected Split run plan");
    };
    let error_plan = error_plan.as_ref().expect("Split onError plan");
    assert!(error_plan.branches.is_empty());
    assert!(error_plan.default_plan.is_some());
}

#[test]
fn direct_compile_supports_ai_agent_single_shot_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-single-shot".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_single_shot"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct single-shot AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");

    // The AiAgent step lowers as an invoke of the ai-tools chat-completion
    // capability, so the workflow imports the ai-tools agent component.
    assert!(
        manifest
            .feature_summary
            .agent_ids
            .iter()
            .any(|id| id == "ai-tools"),
        "expected ai-tools in agent_ids: {:?}",
        manifest.feature_summary.agent_ids
    );

    // The AiAgent step is recorded as an ai-tools/chat-completion agent entry.
    let ai_agent = manifest
        .graph
        .agents
        .iter()
        .find(|agent| agent.step_id == "ai")
        .expect("ai-agent manifest entry");
    assert_eq!(ai_agent.agent_id, "ai-tools");
    assert_eq!(ai_agent.capability_id, "chat-completion");
    assert_eq!(ai_agent.step_type, "AiAgent");

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    assert!(
        matches!(core_config.run_plan, DirectRunPlan::AiAgent { .. }),
        "expected AiAgent run plan, got {:?}",
        core_config.run_plan
    );
}

#[test]
fn direct_compile_supports_ai_agent_structured_output_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-structured".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_structured"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct structured-output AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    // The synthesized chat-completion input mapping carries the converted
    // JSON-schema under `outputSchema`.
    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let ai_agent = manifest
        .graph
        .agents
        .iter()
        .find(|agent| agent.step_id == "ai")
        .expect("ai-agent manifest entry");
    let mapping = manifest
        .graph
        .mappings
        .iter()
        .find(|mapping| mapping.id == ai_agent.input_mapping_id)
        .expect("input mapping");
    assert!(
        mapping.value.get("output_schema").is_some(),
        "expected output_schema in mapping: {:?}",
        mapping.value
    );
}

#[test]
fn direct_compile_supports_ai_agent_tool_loop_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-tool-loop".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_tool_loop"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct tool-loop AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent tool-loop artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    // The AiAgent targets the chat-turn capability and the workflow imports both
    // ai-tools and the tool agent (utils).
    let ai_agent = manifest
        .graph
        .agents
        .iter()
        .find(|agent| agent.step_id == "ai")
        .expect("ai-agent manifest entry");
    assert_eq!(ai_agent.capability_id, "chat-turn");
    assert!(
        manifest
            .feature_summary
            .agent_ids
            .iter()
            .any(|id| id == "ai-tools")
    );
    assert!(
        manifest
            .feature_summary
            .agent_ids
            .iter()
            .any(|id| id == "utils")
    );

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    assert!(
        matches!(core_config.run_plan, DirectRunPlan::AiAgentLoop { .. }),
        "expected AiAgentLoop run plan, got {:?}",
        core_config.run_plan
    );
}

#[test]
fn direct_compile_supports_ai_agent_embed_workflow_tool_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-embed-tool".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_embed_tool"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "tool_weather".to_string(),
            workflow_id: "weather-workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 1,
            execution_graph: fixture("embed_tool_child"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct embed-tool AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent embed-tool artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    // The embed tool's child workflow is composed into the artifact.
    assert!(
        manifest
            .child_workflows
            .iter()
            .any(|child| child.step_id == "tool_weather"),
        "embed tool child workflow should be preloaded"
    );

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { tools, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    assert_eq!(tools.len(), 1, "expected the single embed tool");
    assert!(
        matches!(
            &tools[0],
            crate::direct_wasm::plan::DirectAiToolPlan::Embed { step_id, .. }
                if step_id == "tool_weather"
        ),
        "the tool should be an EmbedWorkflow tool, got {:?}",
        tools[0]
    );
}

#[test]
fn direct_compile_supports_ai_agent_wait_for_signal_tool_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-wait-tool".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_wait_tool"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct wait-tool AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent wait-tool artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { tools, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    assert_eq!(tools.len(), 1, "expected the single wait tool");
    assert!(
        matches!(
            &tools[0],
            crate::direct_wasm::plan::DirectAiToolPlan::Wait { step_id, label }
                if step_id == "ask_human" && label == "get_approval"
        ),
        "the tool should be a WaitForSignal tool, got {:?}",
        tools[0]
    );
}

#[test]
fn direct_compile_supports_ai_agent_wait_tool_with_on_wait_subgraph() {
    // A WaitForSignal tool whose target carries an onWait subgraph must still
    // compile directly (the generated tool arm ignores onWait), not fall back.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-wait-tool-on-wait".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_wait_tool_on_wait"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct wait-tool-with-onWait AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent wait-tool-with-onWait artifact should validate");
    assert!(
        result.support_report.supported,
        "wait tool with onWait must lower directly (no fallback): {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_wait_for_signal_with_nested_on_wait() {
    // A WaitForSignal whose onWait subgraph contains a nested WaitForSignal must
    // lower directly. The onWait emission saves/restores the outer wait's
    // signal-id/deadline/timeout locals around the subgraph (LIFO, nesting-safe),
    // so the nested wait reusing those shared locals does not corrupt the outer
    // poll. This must compile, validate, and not fall back.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "wait-nested-on-wait".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("wait_for_signal_nested_on_wait"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested-onWait wait compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested-onWait wait artifact should validate");
    assert!(
        result.support_report.supported,
        "WaitForSignal with a nested WaitForSignal in onWait must lower directly (no fallback): {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_embed_workflow_child_with_agent_step() {
    // An EmbedWorkflow whose child graph contains a real Agent step must lower
    // directly (not fall back); the composed parent must import the child's agent.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "embed-agent-child".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_agent_child_parent"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "agent-child".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 1,
            execution_graph: fixture("embed_agent_child"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct embed-with-agent-child compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct embed-with-agent-child artifact should validate");
    assert!(
        result.support_report.supported,
        "embed child with an Agent step must lower directly: {:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert!(
        manifest
            .feature_summary
            .agent_ids
            .iter()
            .any(|id| id == "utils"),
        "the child's utils agent must be imported by the composed parent, got {:?}",
        manifest.feature_summary.agent_ids
    );
}

#[test]
fn direct_compile_supports_embed_workflow_child_with_split_step() {
    // A frame-heavy Split step inside an embed child must emit valid WASM (no
    // local/frame conflict with the embed child attempt).
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "embed-split-child".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("embed_split_child_parent"),
        child_workflows: vec![crate::compile::ChildWorkflowInput {
            step_id: "call_split".to_string(),
            workflow_id: "split-child".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 1,
            execution_graph: fixture("split"),
        }],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct embed-with-split-child compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct embed-with-split-child artifact should validate");
    assert!(
        result.support_report.supported,
        "embed child with a Split step must lower directly: {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_conditional_diamond_graph() {
    // A Conditional whose branches re-merge and continue must lower directly
    // (diamond) instead of falling back, with the merge as a shared continuation.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "conditional-diamond".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("conditional_diamond"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct conditional-diamond compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct conditional-diamond artifact should validate");
    assert!(
        result.support_report.supported,
        "a re-merging conditional must lower directly: {:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::Conditional { merge_plan, .. } = &core_config.run_plan else {
        panic!(
            "expected a Conditional run plan, got {:?}",
            core_config.run_plan
        );
    };
    assert!(
        merge_plan.is_some(),
        "the diamond's shared continuation should be a merge plan, not duplicated"
    );
}

#[test]
fn direct_compile_supports_nested_conditional_diamond_graph() {
    // A nested diamond (a Conditional inside the true branch, all re-merging at a
    // shared step) must lower directly — exercising recursive merge handling.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "conditional-diamond-nested".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("conditional_diamond_asymmetric"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct nested conditional-diamond compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct nested conditional-diamond artifact should validate");
    assert!(
        result.support_report.supported,
        "a nested re-merging conditional must lower directly: {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_edge_condition_diamond_graph() {
    // A step whose conditioned NORMAL-flow edges (an EdgeRoute) re-merge and
    // continue must lower directly with a single shared continuation.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "edge-condition-diamond".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("edge_condition_diamond"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct edge-condition-diamond compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct edge-condition-diamond artifact should validate");
    assert!(
        result.support_report.supported,
        "a re-merging EdgeRoute must lower directly: {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_ai_agent_with_inert_on_error_edge() {
    // An onError edge on an AiAgent is inert (generated never routes AiAgent
    // failures to it). Direct must accept the graph (not fall back) and leave the
    // dead handler unlowered, matching generated.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_on_error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct AiAgent-with-onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent-with-onError artifact should validate");
    assert!(
        result.support_report.supported,
        "an AiAgent with an inert onError edge must lower directly: {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_ai_agent_multi_tool_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-multi-tool".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_multi_tool"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct multi-tool AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent multi-tool artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { tools, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    assert_eq!(tools.len(), 2, "expected two dispatchable tools");
}

#[test]
fn direct_compile_supports_ai_agent_memory_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-memory".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_memory"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct memory AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent memory artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    // Memory forces the chat-turn capability and records load/save provider
    // entries targeting the object-model agent.
    let ai_agent = manifest
        .graph
        .agents
        .iter()
        .find(|agent| agent.step_id == "ai" && agent.purpose == "agent.config")
        .expect("ai-agent config");
    assert_eq!(ai_agent.capability_id, "chat-turn");
    assert!(manifest.graph.agents.iter().any(|agent| {
        agent.purpose == "memory.load"
            && agent.capability_id == "load-memory"
            && agent.agent_id == "object-model"
    }));
    assert!(
        manifest
            .graph
            .agents
            .iter()
            .any(|agent| agent.purpose == "memory.save" && agent.capability_id == "save-memory")
    );
    assert!(
        manifest
            .feature_summary
            .agent_ids
            .iter()
            .any(|id| id == "object-model")
    );

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { memory, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    assert!(memory.is_some(), "expected a memory plan");
    // No explicit compaction config → the generated default sliding window (50).
    assert_eq!(memory.as_ref().unwrap().max_messages, 50);
}

#[test]
fn direct_compile_supports_ai_agent_memory_compaction_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-memory-compaction".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_memory_compaction"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct memory-compaction AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent memory-compaction artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { memory, tools, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    // The explicit sliding-window threshold is carried into the plan, alongside
    // the tool that drives the multi-message conversation.
    let memory = memory.as_ref().expect("memory plan");
    assert_eq!(memory.max_messages, 2);
    assert_eq!(tools.len(), 1, "expected the single echo tool");
    // Default strategy → sliding window, no summarize provider.
    assert!(memory.summarize.is_none());
}

#[test]
fn direct_compile_supports_ai_agent_memory_summarize_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-memory-summarize".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_memory_summarize"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct memory-summarize AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent memory-summarize artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    // Summarize strategy records a memory.summarize provider agent (the
    // ai-tools summarize-memory capability).
    assert!(manifest.graph.agents.iter().any(|agent| {
        agent.purpose == "memory.summarize"
            && agent.capability_id == "summarize-memory"
            && agent.agent_id == "ai-tools"
    }));

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { memory, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    let memory = memory.as_ref().expect("memory plan");
    assert_eq!(memory.max_messages, 2);
    assert!(
        memory.summarize.is_some(),
        "Summarize strategy should carry a summarize provider plan"
    );
}

#[test]
fn direct_compile_supports_ai_agent_mcp_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-mcp".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_mcp"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct MCP AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent MCP artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    // MCP forces the chat-turn capability and advertises two synthetic tools.
    let ai_agent = manifest
        .graph
        .agents
        .iter()
        .find(|agent| agent.step_id == "ai" && agent.purpose == "agent.config")
        .expect("ai-agent config");
    assert_eq!(ai_agent.capability_id, "chat-turn");
    let tool_mapping = manifest
        .graph
        .mappings
        .iter()
        .find(|mapping| mapping.step_id == "ai" && mapping.purpose == "agent.inputMapping")
        .expect("ai-agent input mapping");
    let tool_names: Vec<&str> = tool_mapping
        .value
        .get("tools")
        .and_then(|tools| tools.get("value"))
        .and_then(|value| value.as_array())
        .map(|defs| {
            defs.iter()
                .filter_map(|def| def.get("name").and_then(|n| n.as_str()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(tool_names, vec!["github_search", "github_invoke"]);
    // A system-prompt suffix guides the LLM to the search→invoke pattern.
    assert!(
        tool_mapping
            .value
            .get("system_prompt_suffix")
            .and_then(|suffix| suffix.get("value"))
            .and_then(|value| value.as_str())
            .is_some_and(|text| text.contains("github")),
        "expected an MCP system-prompt suffix"
    );
    // Two provider entries (mcp-tool-search / mcp-tool-invoke) on the mcp agent.
    let mcp_providers: Vec<&str> = manifest
        .graph
        .agents
        .iter()
        .filter(|agent| agent.step_id == "ai" && agent.purpose == "agent.tool.mcp")
        .map(|agent| agent.capability_id.as_str())
        .collect();
    assert_eq!(mcp_providers, vec!["mcp-tool-search", "mcp-tool-invoke"]);
    assert!(
        manifest
            .feature_summary
            .agent_ids
            .iter()
            .any(|id| id == "mcp")
    );

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::AiAgentLoop { tools, .. } = &core_config.run_plan else {
        panic!(
            "expected AiAgentLoop run plan, got {:?}",
            core_config.run_plan
        );
    };
    // The run plan's tool list mirrors the advertised order: search then invoke.
    assert_eq!(tools.len(), 2, "expected the two MCP meta-tools");
    assert!(
        tools.iter().all(|tool| matches!(
            tool,
            crate::direct_wasm::plan::DirectAiToolPlan::Agent {
                agent_component_id,
                ..
            } if agent_component_id == "mcp"
        )),
        "MCP tools dispatch to the mcp component"
    );
}

#[test]
fn direct_compile_supports_ai_agent_tool_error_graph() {
    // A tool loop whose tool can fail at runtime compiles like any other tool
    // loop; the tool failure is fed back to the LLM at execution time (covered
    // by the A/B test), not rejected at compile time.
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "ai-agent-tool-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("ai_agent_tool_error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct tool-error AiAgent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct AiAgent tool-error artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_fanout_diamond_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "fanout-diamond".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("fanout_diamond"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct fan-out diamond compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct fan-out diamond artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );

    // The diamond linearizes topologically into a single chain that runs each
    // step once: start -> left -> right -> join. The plan is a linear chain of
    // Agent plans terminating in the join Finish.
    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");

    let mut plan = &core_config.run_plan;
    let mut chain = Vec::new();
    loop {
        match plan {
            DirectRunPlan::Agent {
                step_id, next_plan, ..
            } => {
                chain.push(step_id.clone());
                plan = next_plan;
            }
            DirectRunPlan::Finish { step_id, .. } => {
                chain.push(step_id.clone());
                break;
            }
            other => panic!("unexpected plan node in fan-out chain: {other:?}"),
        }
    }
    assert_eq!(
        chain,
        vec!["start", "left", "right", "join"],
        "fan-out diamond should linearize to start -> left -> right -> join"
    );
}

#[test]
fn direct_compile_supports_split_timeout_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-timeout".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("split_timeout"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Split timeout compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Split timeout artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits.len(), 1);
    assert_eq!(manifest.graph.splits[0].value["timeout"], 10);
}

#[test]
fn direct_compile_supports_while_timeout_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "while-timeout".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("while_timeout"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct While timeout compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct While timeout artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.whiles.len(), 1);
    assert_eq!(manifest.graph.whiles[0].value["timeout"], 10);
}

#[test]
fn direct_compile_supports_while_on_error_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "while-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("while_on_error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct While onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct While onError artifact should validate");
    assert!(
        result.support_report.supported,
        "{:?}",
        result.support_report.unsupported
    );
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.whiles.len(), 1);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::While { error_plan, .. } = &core_config.run_plan else {
        panic!("expected While run plan");
    };
    let error_plan = error_plan.as_ref().expect("While onError plan");
    assert!(error_plan.branches.is_empty());
    assert!(error_plan.default_plan.is_some());
}

#[test]
fn direct_compile_supports_while_with_nested_split_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "while-nested-split".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("while_nested_split"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct While with nested Split compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct While with nested Split artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.whiles.len(), 1);

    let core_config = DirectCoreConfig::new(
        &manifest,
        &manifest.to_canonical_json().expect("manifest json"),
        false,
    )
    .expect("core config");
    let DirectRunPlan::While { nested_plan, .. } = &core_config.run_plan else {
        panic!("expected root While run plan");
    };
    assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Split { .. }));
}

#[test]
fn direct_compile_supports_split_schema_validation_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("split_with_schemas");
    graph.durable = Some(false);
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-with-schemas".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Split schema compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Split schema artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(
        manifest.graph.splits[0].input_schema["value"]["required"],
        true
    );
    assert_eq!(
        manifest.graph.splits[0].output_schema["processed"]["required"],
        true
    );
}

#[test]
fn direct_compile_supports_split_dont_stop_on_failed_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("split_with_schemas_failing");
    graph.durable = Some(false);
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "split-dont-stop".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Split dontStopOnFailed compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Split dontStopOnFailed artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.splits[0].value["dontStopOnFailed"], true);
}

#[test]
fn direct_compile_supports_durable_delay_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "delay".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("delay_simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Delay compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Delay artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.delays.len(), 1);
    assert_eq!(manifest.graph.delays[0].step_id, "delay");
    assert!(manifest.graph.delays[0].durable);
    assert_eq!(manifest.graph.delays[0].duration_ms["value"], 1000);
    assert_eq!(manifest.graph.mappings.len(), 1);
}

#[test]
fn direct_compile_supports_dynamic_durable_delay_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "delay-dynamic".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("delay_dynamic"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct dynamic Delay compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct dynamic Delay artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.delays.len(), 1);
    assert_eq!(
        manifest.graph.delays[0].duration_ms["value"],
        "data.waitTime"
    );
}

#[test]
fn direct_compile_supports_non_durable_delay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("delay_simple");
    graph.durable = Some(false);

    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "delay-non-durable".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("non-durable Delay should compile");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct non-durable Delay artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.delays.len(), 1);
    assert!(!manifest.graph.delays[0].durable);
}

#[test]
fn direct_compile_supports_filter_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "filter".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("filter"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Filter compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Filter artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.filters.len(), 1);
    assert_eq!(manifest.graph.mappings.len(), 1);
}

#[test]
fn direct_compile_supports_value_switch_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "switch-value".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("switch_value"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct value Switch compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct value Switch artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.switches.len(), 1);
    assert_eq!(manifest.graph.mappings.len(), 1);
}

#[test]
fn direct_compile_supports_routing_switch_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "switch-routing".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("switch_routing"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct routing Switch compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct routing Switch artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.switches.len(), 1);
    assert_eq!(manifest.graph.mappings.len(), 3);
}

#[test]
fn direct_compile_supports_log_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "log".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("log"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Log compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Log artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.logs.len(), 2);
    assert_eq!(manifest.graph.mappings.len(), 1);
}

#[test]
fn direct_compile_supports_error_entry_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("error"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Error compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Error artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.errors.len(), 1);
    assert_eq!(manifest.graph.mappings.len(), 0);
}

#[test]
fn direct_compile_supports_edge_condition_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "edge-condition".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("edge_condition"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct edge-condition compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct edge-condition artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.logs.len(), 1);
    assert_eq!(manifest.graph.conditions.len(), 2);
    assert_eq!(manifest.graph.mappings.len(), 3);
}

#[test]
fn direct_compile_supports_non_durable_agent_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: non_durable_agent_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Agent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Agent artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.agents.len(), 1);
    assert_eq!(manifest.graph.agents[0].agent_id, "utils");
    assert_eq!(manifest.graph.agents[0].capability_id, "normalize");
    assert!(!manifest.graph.agents[0].durable);
    assert_eq!(manifest.graph.agents[0].max_retries, Some(0));
    assert_eq!(manifest.graph.mappings.len(), 2);
}

#[test]
fn direct_compile_supports_non_durable_agent_default_retry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent-non-durable-default-retry".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: non_durable_agent_default_retry_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("non-durable Agent default retry compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct non-durable Agent default retry artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.agents.len(), 1);
    assert!(!manifest.graph.agents[0].durable);
    assert_eq!(manifest.graph.agents[0].max_retries, None);
    assert_eq!(manifest.graph.agents[0].retry_delay, None);
}

#[test]
fn direct_compile_supports_durable_agent_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "durable-agent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("transform"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct durable Agent compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct durable Agent artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.agents.len(), 1);
    assert_eq!(manifest.graph.agents[0].agent_id, "transform");
    assert_eq!(manifest.graph.agents[0].capability_id, "map-fields");
    assert!(manifest.graph.agents[0].durable);
    assert_eq!(manifest.graph.agents[0].max_retries, None);
    assert_eq!(manifest.graph.agents[0].retry_delay, None);
}

#[test]
fn direct_compile_supports_durable_agent_retry_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "durable-agent-retry".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: durable_agent_retry_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct durable Agent retry compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct durable Agent retry artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.rate_limit_budget_ms, 2_500);
    assert_eq!(manifest.graph.agents.len(), 1);
    assert!(manifest.graph.agents[0].durable);
    assert_eq!(manifest.graph.agents[0].max_retries, Some(2));
    assert_eq!(manifest.graph.agents[0].retry_delay, Some(750));
}

#[test]
fn direct_compile_supports_non_durable_agent_connection_finish_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent-connection".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: non_durable_agent_connection_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Agent connection compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Agent connection artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(
        manifest.graph.agents[0].connection_id.as_deref(),
        Some("shopify-main")
    );
}

#[test]
fn direct_compile_supports_non_durable_agent_default_on_error_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: non_durable_agent_on_error_finish_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Agent onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Agent onError artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest.graph.agents.len(), 1);
    assert!(
        manifest
            .graph
            .edges
            .iter()
            .any(|edge| edge.label.as_deref() == Some("onError"))
    );
}

#[test]
fn direct_compile_supports_non_durable_agent_conditional_on_error_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent-conditional-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: non_durable_agent_conditional_on_error_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct Agent conditional onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct Agent conditional onError artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    let on_error_condition = manifest
        .graph
        .edges
        .iter()
        .find(|edge| edge.label.as_deref() == Some("onError") && edge.condition_id.is_some())
        .expect("conditioned onError edge");
    assert_eq!(on_error_condition.priority, Some(10));
}

#[test]
fn direct_compile_supports_durable_agent_conditional_on_error_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "durable-agent-conditional-on-error".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: durable_agent_conditional_on_error_graph(),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct durable Agent conditional onError compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct durable Agent conditional onError artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);

    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).expect("manifest"))
            .expect("manifest json");
    assert!(manifest.graph.agents[0].durable);
    assert!(
        manifest.graph.edges.iter().any(|edge| {
            edge.label.as_deref() == Some("onError") && edge.condition_id.is_some()
        })
    );
}

#[test]
fn direct_compile_supports_next_label_edge_condition_graph() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut graph = fixture("edge_condition");
    for edge in &mut graph.execution_plan {
        edge.label = Some("next".to_string());
    }
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "next-edge-condition".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct next edge-condition compile should succeed");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("direct next edge-condition artifact should validate");
    assert!(result.support_report.supported);
    assert_eq!(result.support_report.unsupported, vec![]);
}

#[test]
fn direct_core_run_lowers_finish_mapping_through_stdlib() {
    let graph = fixture("simple");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let variables_json = serde_json::to_vec(&manifest.graph.variables).expect("variables json");

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let expected_imports = [
        (
            "runtime.load-input",
            "runtara:workflow-runtime/runtime",
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "load-input",
            vec![WasmType::Pointer],
        ),
        (
            "stdlib.init-manifest",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "init-manifest",
            vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
        ),
        (
            "stdlib.build-source",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "build-source",
            vec![
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.apply-mapping",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "apply-mapping",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.eval-condition",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "eval-condition",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.filter",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "filter",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.log-event",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "log-event",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.log",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "log",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.process-switch",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "process-switch",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.value-switch",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "value-switch",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.group-by",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "group-by",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.delay-duration-ms",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "delay-duration-ms",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.delay",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "delay",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::I64,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.agent-output",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "agent-output",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.step-debug-start",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "step-debug-start",
            vec![
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.step-debug-end",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "step-debug-end",
            vec![
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "runtime.complete",
            "runtara:workflow-runtime/runtime",
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "complete",
            vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
        ),
        (
            "runtime.fail",
            "runtara:workflow-runtime/runtime",
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "fail",
            vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
        ),
        (
            "runtime.custom-event",
            "runtara:workflow-runtime/runtime",
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "custom-event",
            vec![
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.error-event",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "error-event",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
        (
            "stdlib.error",
            "runtara:workflow-stdlib/json",
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "error",
            vec![
                WasmType::I32,
                WasmType::Pointer,
                WasmType::Length,
                WasmType::Pointer,
            ],
        ),
    ];

    for (label, interface_prefix, module, name, params) in &expected_imports {
        let (interface_key, function) =
            imported_wit_function(&resolve, world, interface_prefix, name);
        let signature =
            resolve.wasm_signature(ManglingAndAbi::Standard32.import_variant(), function);
        assert_eq!(&signature.params, params, "{label} params");
        assert!(signature.retptr, "{label} should use retptr");
        assert!(signature.results.is_empty(), "{label} has no core results");

        let (actual_module, actual_name) = resolve.wasm_import_name(
            ManglingAndAbi::Standard32,
            WasmImport::Func {
                interface: Some(interface_key),
                func: function,
            },
        );
        assert_eq!(actual_module, *module, "{label} module");
        assert_eq!(actual_name, *name, "{label} name");
    }

    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("core module validates");

    let mut next_function_index = 0;
    let mut init_manifest_index = None;
    let mut load_input_index = None;
    let mut build_source_index = None;
    let mut apply_mapping_index = None;
    let mut eval_condition_index = None;
    let mut process_switch_index = None;
    let mut log_event_index = None;
    let mut log_index = None;
    let mut error_event_index = None;
    let mut error_index = None;
    let mut complete_index = None;
    let mut fail_index = None;
    let mut custom_event_index = None;
    let mut saw_manifest_data = false;
    let mut saw_variables_data = false;
    let mut saw_steps_data = false;
    let mut saw_mapping_id = false;
    let mut saw_run_retptr_tag_load = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "init-manifest") => {
                                init_manifest_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "load-input") => {
                                load_input_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                eval_condition_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "process-switch") => {
                                process_switch_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "log-event") => {
                                log_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "log") => {
                                log_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "error-event") => {
                                error_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "error") => {
                                error_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                complete_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                fail_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                custom_event_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value }
                                if matches!(
                                    &core_config.run_plan,
                                    DirectRunPlan::Finish { mapping_id, .. }
                                        if value == *mapping_id as i32
                                ) =>
                            {
                                saw_mapping_id = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == 0 && memarg.memory == 0 =>
                            {
                                saw_run_retptr_tag_load = true;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            Payload::DataSection(reader) => {
                for data in reader {
                    let data = data.expect("data segment");
                    saw_manifest_data |= data.data == manifest_json;
                    saw_variables_data |= data.data == variables_json;
                    saw_steps_data |= data.data == DIRECT_EMPTY_STEPS_CONTEXT;
                }
            }
            _ => {}
        }
    }

    // Each setup/stdlib call is followed by a fail-on-error guard (`runtime.fail`
    // inside an `if error` block) so an unhandled error surfaces as a `failed`
    // SDK event instead of a silent non-zero exit.
    let expected_call_order = [
        init_manifest_index.expect("init-manifest import"),
        fail_index.expect("fail import"),
        load_input_index.expect("load-input import"),
        fail_index.expect("fail import"),
        build_source_index.expect("build-source import"),
        fail_index.expect("fail import"),
        apply_mapping_index.expect("apply-mapping import"),
        fail_index.expect("fail import"),
        complete_index.expect("complete import"),
    ];
    assert!(
        eval_condition_index.is_some(),
        "eval-condition import should exist for conditional lowering"
    );
    assert!(
        process_switch_index.is_some(),
        "process-switch import should exist for routing Switch lowering"
    );
    assert!(
        log_event_index.is_some(),
        "log-event import should exist for Log lowering"
    );
    assert!(
        log_index.is_some(),
        "log import should exist for Log lowering"
    );
    assert!(
        error_event_index.is_some(),
        "error-event import should exist for Error lowering"
    );
    assert!(
        error_index.is_some(),
        "error import should exist for Error lowering"
    );
    assert!(
        fail_index.is_some(),
        "fail import should exist for Error lowering"
    );
    assert!(
        custom_event_index.is_some(),
        "custom-event import should exist for Log/Error lowering"
    );
    assert_eq!(
        run_calls, expected_call_order,
        "run body should lower Finish through stdlib/runtime calls in order"
    );
    assert!(saw_manifest_data, "manifest JSON should be static data");
    assert!(saw_variables_data, "variables JSON should be static data");
    assert!(saw_steps_data, "empty steps context should be static data");
    assert!(saw_mapping_id, "run body should pass manifest mapping id");
    assert!(
        saw_run_retptr_tag_load,
        "run body should return runtime.complete result tag"
    );
}

#[test]
fn direct_core_run_lowers_finish_breakpoint_after_output_mapping() {
    let mut graph = fixture("simple");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::Finish(finish)) = graph.steps.get_mut("finish") else {
        panic!("expected Finish fixture step");
    };
    finish.breakpoint = Some(true);

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Finish {
        breakpoint,
        mapping_id,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Finish run plan");
    };
    assert!(*breakpoint, "durable Finish breakpoint should lower");

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Finish breakpoint core module validates");

    let mut next_function_index = 0;
    let mut stdlib_apply_mapping_index = None;
    let mut runtime_debug_mode_enabled_index = None;
    let mut stdlib_breakpoint_key_index = None;
    let mut runtime_checkpoint_index = None;
    let mut stdlib_breakpoint_event_index = None;
    let mut runtime_custom_event_index = None;
    let mut runtime_breakpoint_pause_index = None;
    let mut runtime_complete_index = None;
    let mut saw_mapping_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                stdlib_apply_mapping_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "debug-mode-enabled",
                            ) => runtime_debug_mode_enabled_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-key") => {
                                stdlib_breakpoint_key_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "checkpoint") => {
                                runtime_checkpoint_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-event") => {
                                stdlib_breakpoint_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                runtime_custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "breakpoint-pause") => {
                                runtime_breakpoint_pause_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                runtime_complete_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } if value == *mapping_id as i32 => {
                                saw_mapping_id = true;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let stdlib_apply_mapping_index = stdlib_apply_mapping_index.expect("apply-mapping import");
    let runtime_debug_mode_enabled_index =
        runtime_debug_mode_enabled_index.expect("debug-mode-enabled import");
    let stdlib_breakpoint_key_index = stdlib_breakpoint_key_index.expect("breakpoint-key import");
    let runtime_checkpoint_index = runtime_checkpoint_index.expect("checkpoint import");
    let stdlib_breakpoint_event_index =
        stdlib_breakpoint_event_index.expect("breakpoint-event import");
    let runtime_custom_event_index = runtime_custom_event_index.expect("custom-event import");
    let runtime_breakpoint_pause_index =
        runtime_breakpoint_pause_index.expect("breakpoint-pause import");
    let runtime_complete_index = runtime_complete_index.expect("complete import");

    let position = |index| {
        run_calls
            .iter()
            .position(|call| *call == index)
            .expect("expected Finish breakpoint call")
    };
    let apply_mapping_position = position(stdlib_apply_mapping_index);
    let debug_mode_position = position(runtime_debug_mode_enabled_index);
    let breakpoint_key_position = position(stdlib_breakpoint_key_index);
    let checkpoint_position = position(runtime_checkpoint_index);
    let breakpoint_event_position = position(stdlib_breakpoint_event_index);
    let custom_event_position = position(runtime_custom_event_index);
    let breakpoint_pause_position = position(runtime_breakpoint_pause_index);
    let complete_position = position(runtime_complete_index);

    assert!(
        apply_mapping_position < debug_mode_position
            && debug_mode_position < breakpoint_key_position
            && breakpoint_key_position < checkpoint_position
            && checkpoint_position < breakpoint_event_position
            && breakpoint_event_position < custom_event_position
            && custom_event_position < breakpoint_pause_position
            && breakpoint_pause_position < complete_position,
        "Finish breakpoint should pause after output mapping and before completion: {run_calls:?}"
    );
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
}

#[test]
fn direct_core_run_lowers_conditional_breakpoint_before_condition_eval() {
    let mut graph = fixture("conditional");
    graph.durable = Some(true);
    enable_step_breakpoint(&mut graph, "check");

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    assert_eq!(
        direct_run_plan_breakpoint(&core_config.run_plan),
        Some(true),
        "durable Conditional breakpoint should lower"
    );

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Conditional breakpoint core module validates");

    assert_direct_breakpoint_before_import(
        &core,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "eval-condition",
    );
}

#[test]
fn direct_core_run_lowers_step_context_breakpoints_before_step_helpers() {
    for (fixture_name, step_id, helper_name) in [
        ("filter", "filter", "filter"),
        ("switch_value", "switch", "value-switch"),
        ("switch_routing", "switch", "process-switch"),
        ("group_by", "group", "group-by"),
    ] {
        let mut graph = fixture(fixture_name);
        graph.durable = Some(true);
        enable_step_breakpoint(&mut graph, step_id);

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        assert_eq!(
            direct_run_plan_breakpoint(&core_config.run_plan),
            Some(true),
            "durable {fixture_name} breakpoint should lower"
        );

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .unwrap_or_else(|_| panic!("{fixture_name} breakpoint core module validates"));

        assert_direct_breakpoint_before_import(
            &core,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            helper_name,
        );
    }
}

#[test]
fn direct_core_run_lowers_log_and_error_breakpoints_before_side_effects() {
    for (fixture_name, step_id, helper_name) in [
        ("log", "simple_log", "log-event"),
        ("error", "fail", "error-event"),
    ] {
        let mut graph = fixture(fixture_name);
        graph.durable = Some(true);
        enable_step_breakpoint(&mut graph, step_id);

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config =
            DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
        assert_eq!(
            direct_run_plan_breakpoint(&core_config.run_plan),
            Some(true),
            "durable {fixture_name} breakpoint should lower"
        );

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .unwrap_or_else(|_| panic!("{fixture_name} breakpoint core module validates"));

        assert_direct_breakpoint_before_import(
            &core,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            helper_name,
        );
    }
}

#[test]
fn direct_core_run_lowers_agent_breakpoint_after_input_mapping_before_validation() {
    let mut graph = durable_agent_no_retry_graph();
    enable_step_breakpoint(&mut graph, "agent");

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, true).expect("core config");
    let DirectRunPlan::Agent {
        breakpoint,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Agent run plan");
    };
    assert!(*breakpoint, "durable Agent breakpoint should lower");
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Agent breakpoint core module validates");

    let (imports, run_calls) = direct_core_imports_and_run_calls(&core);
    let apply_mapping_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "apply-mapping",
        ),
    );
    let debug_mode_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "debug-mode-enabled",
        ),
    );
    let breakpoint_key_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "breakpoint-key",
        ),
    );
    let checkpoint_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "checkpoint",
        ),
    );
    let breakpoint_event_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "breakpoint-event",
        ),
    );
    let custom_event_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "custom-event",
        ),
    );
    let breakpoint_pause_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-runtime/runtime@0.1",
            "breakpoint-pause",
        ),
    );
    let step_debug_start_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "step-debug-start",
    );
    let step_debug_start_position = direct_core_call_position(&run_calls, step_debug_start_index);
    let agent_validate_position = direct_core_call_position(
        &run_calls,
        direct_core_import(
            &imports,
            "cm32p2|runtara:workflow-stdlib/json@0.1",
            "agent-validate-input",
        ),
    );

    let mut debug_start_retptr_locals = Vec::new();
    let mut remaining_debug_start_local_sets = 0u8;
    let mut saw_agent_debug_start = false;
    let mut code_body_index = 0;
    for payload in Parser::new(0).parse_all(&core) {
        if let Payload::CodeSectionEntry(body) = payload.expect("core wasm payload") {
            if code_body_index == 0 {
                for operator in body.get_operators_reader().expect("operators") {
                    match operator.expect("operator") {
                        Operator::Call { function_index }
                            if function_index == step_debug_start_index
                                && !saw_agent_debug_start =>
                        {
                            saw_agent_debug_start = true;
                            remaining_debug_start_local_sets = 2;
                        }
                        Operator::LocalSet { local_index }
                            if remaining_debug_start_local_sets > 0 =>
                        {
                            debug_start_retptr_locals.push(local_index);
                            remaining_debug_start_local_sets -= 1;
                        }
                        _ => {}
                    }
                }
            }
            code_body_index += 1;
        }
    }
    const ROUTE_PTR_LOCAL: u32 = 8;
    const ROUTE_LEN_LOCAL: u32 = 9;
    assert_eq!(
        debug_start_retptr_locals,
        vec![ROUTE_PTR_LOCAL, ROUTE_LEN_LOCAL],
        "Agent debug-start payload must use scratch route locals so mapped inputs stay in output locals"
    );

    assert!(
        apply_mapping_position < debug_mode_position
            && debug_mode_position < breakpoint_key_position
            && breakpoint_key_position < checkpoint_position
            && checkpoint_position < breakpoint_event_position
            && breakpoint_event_position < custom_event_position
            && custom_event_position < breakpoint_pause_position
            && breakpoint_pause_position < step_debug_start_position
            && step_debug_start_position < agent_validate_position,
        "Agent breakpoint should pause after input mapping and before debug start/validation: {run_calls:?}"
    );
}

#[test]
fn direct_core_metadata_can_import_agent_capabilities() {
    let graph = fixture("simple");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let agents = vec!["crypto".to_string(), "object-model".to_string()];
    let (resolve, world) =
        build_direct_component_resolve_with_agents(&agents).expect("agent resolve");
    let (interface_key, function) = imported_wit_function(
        &resolve,
        world,
        "runtara:agent-crypto/capabilities",
        "invoke",
    );
    let (actual_module, actual_name) = resolve.wasm_import_name(
        ManglingAndAbi::Standard32,
        WasmImport::Func {
            interface: Some(interface_key),
            func: function,
        },
    );
    assert!(actual_module.contains("runtara:agent-crypto/capabilities"));
    assert_eq!(actual_name, "invoke");

    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("agent-importing core module validates");

    let mut saw_crypto_invoke = false;
    let mut saw_object_model_invoke = false;
    for payload in Parser::new(0).parse_all(&core) {
        if let Payload::ImportSection(reader) = payload.expect("core wasm payload") {
            for import in reader.into_imports() {
                let import = import.expect("core import");
                saw_crypto_invoke |= import.name == "invoke"
                    && import.module.contains("runtara:agent-crypto/capabilities");
                saw_object_model_invoke |= import.name == "invoke"
                    && import
                        .module
                        .contains("runtara:agent-object-model/capabilities");
            }
        }
    }

    assert!(
        saw_crypto_invoke,
        "core metadata should import crypto capabilities.invoke"
    );
    assert!(
        saw_object_model_invoke,
        "core metadata should import object-model capabilities.invoke"
    );
}

#[test]
fn direct_core_lowers_non_durable_agent_call() {
    let graph = non_durable_agent_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent {
        agent_id,
        agent_component_id,
        input_mapping_id,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Agent run plan");
    };
    assert_eq!(*agent_id, 0);
    assert_eq!(agent_component_id, "utils");
    assert_eq!(*input_mapping_id, 0);
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let (interface_key, function) = imported_wit_function(
        &resolve,
        world,
        "runtara:agent-utils/capabilities",
        "invoke",
    );
    let signature = resolve.wasm_signature(ManglingAndAbi::Standard32.import_variant(), function);
    assert_eq!(signature.params, vec![WasmType::Pointer, WasmType::Pointer]);
    assert!(signature.results.is_empty());
    assert_eq!(signature.params.last(), Some(&WasmType::Pointer));

    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Agent core module validates");

    let (actual_module, actual_name) = resolve.wasm_import_name(
        ManglingAndAbi::Standard32,
        WasmImport::Func {
            interface: Some(interface_key),
            func: function,
        },
    );
    let mut saw_agent_invoke = false;
    let mut saw_agent_output = false;
    let mut saw_agent_validate_input = false;
    let mut saw_agent_error = false;
    let mut saw_agent_debug_error = false;
    let mut saw_runtime_fail = false;
    let mut saw_agent_ok_ptr_load = false;
    let mut saw_agent_ok_len_load = false;
    let mut saw_agent_retry_after_value_load = false;
    let mut agent_invoke_index = None;
    let mut agent_validate_input_index = None;
    let mut saw_validate_before_invoke = false;
    let mut code_body_index = 0;
    let mut next_function_index = 0;
    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if import.module == actual_module && import.name == actual_name {
                        saw_agent_invoke = true;
                        agent_invoke_index = Some(next_function_index);
                    }
                    saw_agent_output |= import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "agent-output";
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "agent-validate-input"
                    {
                        saw_agent_validate_input = true;
                        agent_validate_input_index = Some(next_function_index);
                    }
                    saw_agent_error |= import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "agent-error";
                    saw_agent_debug_error |= import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "agent-debug-error";
                    saw_runtime_fail |= import.module.contains("runtara:workflow-runtime/runtime")
                        && import.name == "fail";
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    let mut saw_validate_call = false;
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == agent_validate_input_index =>
                            {
                                saw_validate_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_invoke_index =>
                            {
                                saw_validate_before_invoke = saw_validate_call;
                            }
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_AGENT_RESULT_OK_PTR_OFFSET =>
                            {
                                saw_agent_ok_ptr_load = true;
                            }
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_AGENT_RESULT_OK_LEN_OFFSET =>
                            {
                                saw_agent_ok_len_load = true;
                            }
                            Operator::I64Load { memarg }
                                if memarg.offset
                                    == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET =>
                            {
                                saw_agent_retry_after_value_load = true;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        saw_agent_invoke,
        "core should import Agent capabilities.invoke"
    );
    assert!(saw_agent_output, "core should import stdlib.agent-output");
    assert!(
        saw_agent_validate_input,
        "core should import stdlib.agent-validate-input"
    );
    assert!(saw_agent_error, "core should import stdlib.agent-error");
    assert!(
        saw_agent_debug_error,
        "core should import stdlib.agent-debug-error"
    );
    assert!(saw_runtime_fail, "core should import runtime.fail");
    assert!(
        saw_agent_ok_ptr_load,
        "Agent success should load list pointer from result payload offset 8"
    );
    assert!(
        saw_agent_ok_len_load,
        "Agent success should load list length from result payload offset 12"
    );
    assert!(
        saw_agent_retry_after_value_load,
        "Agent error path should pass retry-after-ms from error-info"
    );
    assert!(
        saw_validate_before_invoke,
        "Agent input validation should run before capabilities.invoke"
    );
}

#[test]
fn direct_core_lowers_non_durable_agent_retry_loop() {
    let graph = non_durable_agent_default_retry_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent {
        durable_checkpoint,
        max_retries,
        retry_delay_ms,
        rate_limit_budget_ms,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Agent run plan");
    };
    assert!(!*durable_checkpoint);
    assert_eq!(*max_retries, 3);
    assert_eq!(*retry_delay_ms, 1_000);
    assert_eq!(*rate_limit_budget_ms, 60_000);
    assert!(!manifest.graph.agents[0].durable);

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("non-durable Agent retry core module validates");

    let mut next_function_index = 0;
    let mut get_checkpoint_index = None;
    let mut checkpoint_index = None;
    let mut durable_sleep_index = None;
    let mut durable_sleep_checkpoint_index = None;
    let mut blocking_sleep_index = None;
    let mut agent_retry_sleep_key_index = None;
    let mut agent_retry_delay_index = None;
    let mut agent_retry_error_info_index = None;
    let mut agent_error_from_info_index = None;
    let mut record_retry_attempt_index = None;
    let mut agent_invoke_index = None;
    let mut saw_blocking_sleep_import = false;
    let mut saw_agent_retry_delay_import = false;
    let mut saw_agent_retry_error_info_import = false;
    let mut saw_agent_error_from_info_import = false;
    let mut saw_retry_loop = false;
    let mut saw_retry_continue_branch = false;
    let mut saw_retryable_load = false;
    let mut saw_retry_info_retryable_load = false;
    let mut saw_retry_after_tag_load = false;
    let mut saw_retry_after_value_load = false;
    let mut saw_rate_limit_wait_accumulator = false;
    let mut saw_rate_limit_base_delay_const = false;
    let mut saw_rate_limit_budget_const = false;
    let mut saw_rate_limit_budget_compare = false;
    let mut saw_retry_bound = false;
    let mut saw_invoke = false;
    let mut saw_retry_info_after_invoke = false;
    let mut saw_retry_delay_after_retry_info = false;
    let mut saw_blocking_sleep_after_retry_delay = false;
    let mut saw_error_from_info_after_retry_info = false;
    let mut saw_get_checkpoint_call = false;
    let mut saw_checkpoint_call = false;
    let mut saw_durable_sleep_call = false;
    let mut saw_durable_sleep_checkpoint_call = false;
    let mut saw_retry_sleep_key_call = false;
    let mut saw_record_retry_attempt_call = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            (module, "get-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                get_checkpoint_index = Some(next_function_index);
                            }
                            (module, "checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                checkpoint_index = Some(next_function_index);
                            }
                            (module, "durable-sleep")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                durable_sleep_index = Some(next_function_index);
                            }
                            (module, "durable-sleep-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                durable_sleep_checkpoint_index = Some(next_function_index);
                            }
                            (module, "blocking-sleep")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_blocking_sleep_import = true;
                                blocking_sleep_index = Some(next_function_index);
                            }
                            (module, "agent-retry-sleep-key")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                agent_retry_sleep_key_index = Some(next_function_index);
                            }
                            (module, "agent-retry-delay-ms")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_retry_delay_import = true;
                                agent_retry_delay_index = Some(next_function_index);
                            }
                            (module, "agent-retry-error-info")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_retry_error_info_import = true;
                                agent_retry_error_info_index = Some(next_function_index);
                            }
                            (module, "agent-error-from-info")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_error_from_info_import = true;
                                agent_error_from_info_index = Some(next_function_index);
                            }
                            (module, "record-retry-attempt")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                record_retry_attempt_index = Some(next_function_index);
                            }
                            (module, "invoke")
                                if module.contains("runtara:agent-utils/capabilities") =>
                            {
                                agent_invoke_index = Some(next_function_index);
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    let mut saw_invoke_call = false;
                    let mut saw_retry_info_call = false;
                    let mut saw_retry_delay_call = false;
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == get_checkpoint_index =>
                            {
                                saw_get_checkpoint_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == checkpoint_index =>
                            {
                                saw_checkpoint_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == durable_sleep_index =>
                            {
                                saw_durable_sleep_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == durable_sleep_checkpoint_index =>
                            {
                                saw_durable_sleep_checkpoint_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_retry_sleep_key_index =>
                            {
                                saw_retry_sleep_key_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == record_retry_attempt_index =>
                            {
                                saw_record_retry_attempt_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_invoke_index =>
                            {
                                saw_invoke = true;
                                saw_invoke_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_retry_error_info_index =>
                            {
                                saw_retry_info_after_invoke = saw_invoke_call;
                                saw_retry_info_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_retry_delay_index =>
                            {
                                saw_retry_delay_after_retry_info = saw_retry_info_call;
                                saw_retry_delay_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == blocking_sleep_index =>
                            {
                                saw_blocking_sleep_after_retry_delay = saw_retry_delay_call;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_error_from_info_index =>
                            {
                                saw_error_from_info_after_retry_info = saw_retry_info_call;
                            }
                            Operator::Loop { .. } => saw_retry_loop = true,
                            Operator::Br { relative_depth: 2 } => {
                                saw_retry_continue_branch = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET =>
                            {
                                saw_retryable_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET =>
                            {
                                saw_retry_info_retryable_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset
                                    == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET =>
                            {
                                saw_retry_after_tag_load = true;
                            }
                            Operator::I64Load { memarg }
                                if memarg.offset
                                    == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET =>
                            {
                                saw_retry_after_value_load = true;
                            }
                            Operator::I64Add => saw_rate_limit_wait_accumulator = true,
                            Operator::I64Const { value: 1_000 } => {
                                saw_rate_limit_base_delay_const = true;
                            }
                            Operator::I64Const { value: 60_000 } => {
                                saw_rate_limit_budget_const = true;
                            }
                            Operator::I64LeU => saw_rate_limit_budget_compare = true,
                            Operator::I32Const { value: 3 } => {
                                saw_retry_bound = true;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        saw_blocking_sleep_import,
        "core should import runtime.blocking-sleep"
    );
    assert!(
        saw_agent_retry_delay_import,
        "core should import stdlib.agent-retry-delay-ms"
    );
    assert!(
        saw_agent_retry_error_info_import,
        "core should import stdlib.agent-retry-error-info"
    );
    assert!(
        saw_agent_error_from_info_import,
        "core should import stdlib.agent-error-from-info"
    );
    assert!(saw_invoke, "retry loop should invoke the Agent capability");
    assert!(
        saw_retry_loop && saw_retry_continue_branch,
        "non-durable retry Agent should lower a retry loop"
    );
    assert!(
        saw_retryable_load && saw_retry_info_retryable_load,
        "retry decision should inspect retryable Agent error metadata"
    );
    assert!(
        saw_retry_after_tag_load && saw_retry_after_value_load,
        "retry path should inspect typed retryAfterMs hints"
    );
    assert!(
        saw_rate_limit_wait_accumulator,
        "rate-limited retry path should accumulate wait time"
    );
    assert!(
        saw_rate_limit_base_delay_const
            && saw_rate_limit_budget_const
            && saw_rate_limit_budget_compare,
        "retry path should apply generated retry delay and budget defaults"
    );
    assert!(saw_retry_bound, "retry loop should compare maxRetries=3");
    assert!(
        saw_retry_info_after_invoke,
        "retry error payload should be built after failed invoke"
    );
    assert!(
        saw_retry_delay_after_retry_info,
        "retry delay should be computed from preserved retry payload"
    );
    assert!(
        saw_blocking_sleep_after_retry_delay,
        "non-durable retries should use runtime.blocking-sleep after delay calculation"
    );
    assert!(
        saw_error_from_info_after_retry_info,
        "non-retried errors should format the preserved retry payload"
    );
    assert!(
        !saw_get_checkpoint_call
            && !saw_checkpoint_call
            && !saw_durable_sleep_call
            && !saw_durable_sleep_checkpoint_call
            && !saw_retry_sleep_key_call
            && !saw_record_retry_attempt_call,
        "non-durable retry lowering must not call checkpoint, durable sleep, sleep-key, or retry-attempt APIs"
    );
}

#[test]
fn direct_core_lowers_durable_agent_no_retry_checkpoint_path() {
    let graph = durable_agent_no_retry_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent {
        durable_checkpoint,
        max_retries,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Agent run plan");
    };
    assert!(
        *durable_checkpoint,
        "maxRetries=0 durable Agent should use checkpoint lowering"
    );
    assert_eq!(*max_retries, 0);
    assert!(manifest.graph.agents[0].durable);

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("durable Agent core module validates");

    let mut next_function_index = 0;
    let mut agent_cache_key_index = None;
    let mut get_checkpoint_index = None;
    let mut checkpoint_index = None;
    let mut handle_checkpoint_signal_index = None;
    let mut agent_invoke_index = None;
    let mut saw_cache_key_import = false;
    let mut saw_get_checkpoint_import = false;
    let mut saw_checkpoint_import = false;
    let mut saw_handle_checkpoint_signal_import = false;
    let mut saw_get_checkpoint_option_tag_load = false;
    let mut saw_cached_payload_ptr_load = false;
    let mut saw_cached_payload_len_load = false;
    let mut saw_checkpoint_signal_tag_load = false;
    let mut saw_checkpoint_signal_handled = false;
    let mut saw_checkpoint_signal_bool_load = false;
    let mut saw_checkpoint_signal_early_return = false;
    let mut saw_cache_key_before_lookup = false;
    let mut saw_lookup_before_invoke = false;
    let mut saw_checkpoint_after_invoke = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            (module, "agent-cache-key")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_cache_key_import = true;
                                agent_cache_key_index = Some(next_function_index);
                            }
                            (module, "get-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_get_checkpoint_import = true;
                                get_checkpoint_index = Some(next_function_index);
                            }
                            (module, "checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_checkpoint_import = true;
                                checkpoint_index = Some(next_function_index);
                            }
                            (module, "handle-checkpoint-signal")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_handle_checkpoint_signal_import = true;
                                handle_checkpoint_signal_index = Some(next_function_index);
                            }
                            (module, "invoke")
                                if module.contains("runtara:agent-utils/capabilities") =>
                            {
                                agent_invoke_index = Some(next_function_index);
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    let mut saw_cache_key_call = false;
                    let mut saw_lookup_call = false;
                    let mut saw_invoke_call = false;
                    let mut saw_checkpoint_call = false;
                    let mut saw_handle_checkpoint_signal_call = false;
                    let mut last_i32_const_after_signal_handler = None;
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == agent_cache_key_index =>
                            {
                                saw_cache_key_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == get_checkpoint_index =>
                            {
                                saw_cache_key_before_lookup = saw_cache_key_call;
                                saw_lookup_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_invoke_index =>
                            {
                                saw_lookup_before_invoke = saw_lookup_call;
                                saw_invoke_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == checkpoint_index =>
                            {
                                saw_checkpoint_after_invoke = saw_invoke_call;
                                saw_checkpoint_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == handle_checkpoint_signal_index =>
                            {
                                saw_checkpoint_signal_handled = saw_checkpoint_call;
                                saw_handle_checkpoint_signal_call = true;
                            }
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_LIST_PTR_OFFSET =>
                            {
                                saw_cached_payload_ptr_load = true;
                            }
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_LIST_LEN_OFFSET =>
                            {
                                saw_cached_payload_len_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET =>
                            {
                                saw_checkpoint_signal_tag_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_RET_BOOL_OK_OFFSET
                                    && saw_handle_checkpoint_signal_call =>
                            {
                                saw_checkpoint_signal_bool_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_TAG_OFFSET =>
                            {
                                saw_get_checkpoint_option_tag_load = true;
                            }
                            Operator::I32Const { value } if saw_handle_checkpoint_signal_call => {
                                last_i32_const_after_signal_handler = Some(value);
                            }
                            Operator::Return if saw_handle_checkpoint_signal_call => {
                                saw_checkpoint_signal_early_return |=
                                    last_i32_const_after_signal_handler == Some(0);
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        saw_cache_key_import,
        "core should import stdlib.agent-cache-key"
    );
    assert!(
        saw_get_checkpoint_import,
        "core should import runtime.get-checkpoint"
    );
    assert!(
        saw_checkpoint_import,
        "core should import runtime.checkpoint"
    );
    assert!(
        saw_handle_checkpoint_signal_import,
        "core should import runtime.handle-checkpoint-signal"
    );
    assert!(
        saw_get_checkpoint_option_tag_load,
        "core should inspect get-checkpoint option tag"
    );
    assert!(
        saw_cached_payload_ptr_load && saw_cached_payload_len_load,
        "core should load cached checkpoint payload bytes"
    );
    assert!(
        saw_cache_key_before_lookup,
        "Agent cache key should be computed before checkpoint lookup"
    );
    assert!(
        saw_lookup_before_invoke,
        "checkpoint lookup should run before capability invoke"
    );
    assert!(
        saw_checkpoint_after_invoke,
        "successful capability output should be checkpointed after invoke"
    );
    assert!(
        saw_checkpoint_signal_tag_load,
        "checkpoint result should inspect the pending signal option tag"
    );
    assert!(
        saw_checkpoint_signal_handled,
        "checkpoint pending signals should be handled after checkpoint save"
    );
    assert!(
        saw_checkpoint_signal_bool_load,
        "checkpoint signal handler result should decide whether to stop execution"
    );
    assert!(
        saw_checkpoint_signal_early_return,
        "handled checkpoint signals should return success before workflow completion"
    );
}

#[test]
fn direct_core_checkpoint_replay_skips_agent_invoke_and_checkpoint_save() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ReplayOp {
        CallGetCheckpoint,
        CallAgentInvoke,
        CallCheckpoint,
        If,
        Else,
        End,
        LoadCachedPtr,
        LoadCachedLen,
    }

    fn if_else_blocks(ops: &[ReplayOp]) -> Vec<(usize, usize, usize)> {
        let mut blocks = Vec::new();
        for (if_index, op) in ops.iter().enumerate() {
            if *op != ReplayOp::If {
                continue;
            }

            let mut depth = 1u32;
            let mut else_index = None;
            for (index, op) in ops.iter().enumerate().skip(if_index + 1) {
                match op {
                    ReplayOp::If => depth += 1,
                    ReplayOp::Else if depth == 1 => else_index = Some(index),
                    ReplayOp::End => {
                        depth -= 1;
                        if depth == 0 {
                            if let Some(else_index) = else_index {
                                blocks.push((if_index, else_index, index));
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        blocks
    }

    let graph = durable_agent_no_retry_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("durable Agent replay core module validates");

    let mut next_function_index = 0;
    let mut get_checkpoint_index = None;
    let mut checkpoint_index = None;
    let mut agent_invoke_index = None;
    let mut code_body_index = 0;
    let mut ops = Vec::new();

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            (module, "get-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                get_checkpoint_index = Some(next_function_index);
                            }
                            (module, "checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                checkpoint_index = Some(next_function_index);
                            }
                            (module, "invoke")
                                if module.contains("runtara:agent-utils/capabilities") =>
                            {
                                agent_invoke_index = Some(next_function_index);
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == get_checkpoint_index =>
                            {
                                ops.push(ReplayOp::CallGetCheckpoint);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_invoke_index =>
                            {
                                ops.push(ReplayOp::CallAgentInvoke);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == checkpoint_index =>
                            {
                                ops.push(ReplayOp::CallCheckpoint);
                            }
                            Operator::If { .. } => ops.push(ReplayOp::If),
                            Operator::Else => ops.push(ReplayOp::Else),
                            Operator::End => ops.push(ReplayOp::End),
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_LIST_PTR_OFFSET =>
                            {
                                ops.push(ReplayOp::LoadCachedPtr);
                            }
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_LIST_LEN_OFFSET =>
                            {
                                ops.push(ReplayOp::LoadCachedLen);
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let lookup_index = ops
        .iter()
        .position(|op| *op == ReplayOp::CallGetCheckpoint)
        .expect("checkpoint lookup call");
    let (if_index, else_index, end_index) = if_else_blocks(&ops)
        .into_iter()
        .find(|(if_index, else_index, _)| {
            *if_index > lookup_index
                && ops[*if_index + 1..*else_index].contains(&ReplayOp::LoadCachedPtr)
                && ops[*if_index + 1..*else_index].contains(&ReplayOp::LoadCachedLen)
        })
        .expect("checkpoint replay branch");

    let cached_branch = &ops[if_index + 1..else_index];
    assert!(
        !cached_branch.contains(&ReplayOp::CallAgentInvoke),
        "cached checkpoint replay branch must not invoke the Agent"
    );
    assert!(
        !cached_branch.contains(&ReplayOp::CallCheckpoint),
        "cached checkpoint replay branch must not write another checkpoint"
    );

    let fresh_branch = &ops[else_index + 1..end_index];
    let invoke_index = fresh_branch
        .iter()
        .position(|op| *op == ReplayOp::CallAgentInvoke)
        .expect("fresh branch invokes Agent");
    let checkpoint_index = fresh_branch
        .iter()
        .position(|op| *op == ReplayOp::CallCheckpoint)
        .expect("fresh branch checkpoints Agent output");
    assert!(
        invoke_index < checkpoint_index,
        "fresh execution branch should checkpoint only after Agent invoke"
    );
}

#[test]
fn direct_core_lowers_durable_agent_retry_loop() {
    let graph = durable_agent_retry_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent {
        durable_checkpoint,
        max_retries,
        retry_delay_ms,
        rate_limit_budget_ms,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Agent run plan");
    };
    assert!(*durable_checkpoint);
    assert_eq!(*max_retries, 2);
    assert_eq!(*retry_delay_ms, 750);
    assert_eq!(*rate_limit_budget_ms, 2_500);
    assert!(manifest.graph.agents[0].durable);

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("durable Agent retry core module validates");

    let mut next_function_index = 0;
    let mut get_checkpoint_index = None;
    let mut checkpoint_index = None;
    let mut handle_checkpoint_signal_index = None;
    let mut durable_sleep_index = None;
    let mut durable_sleep_checkpoint_index = None;
    let mut agent_retry_sleep_key_index = None;
    let mut agent_retry_delay_index = None;
    let mut agent_retry_error_info_index = None;
    let mut agent_error_from_info_index = None;
    let mut record_retry_attempt_index = None;
    let mut agent_invoke_index = None;
    let mut saw_durable_sleep_import = false;
    let mut saw_durable_sleep_checkpoint_import = false;
    let mut saw_handle_checkpoint_signal_import = false;
    let mut saw_agent_retry_sleep_key_import = false;
    let mut saw_agent_retry_delay_import = false;
    let mut saw_agent_retry_error_info_import = false;
    let mut saw_agent_error_from_info_import = false;
    let mut saw_record_retry_attempt_import = false;
    let mut saw_retry_loop = false;
    let mut saw_retry_continue_branch = false;
    let mut saw_retryable_load = false;
    let mut saw_retry_info_retryable_load = false;
    let mut saw_retry_info_rate_limited_load = false;
    let mut saw_retry_after_tag_load = false;
    let mut saw_retry_after_value_load = false;
    let mut saw_rate_limit_wait_accumulator = false;
    let mut saw_rate_limit_base_delay_const = false;
    let mut saw_rate_limit_budget_const = false;
    let mut saw_rate_limit_budget_compare = false;
    let mut saw_retry_bound = false;
    let mut saw_lookup_before_invoke = false;
    let mut saw_retry_info_after_invoke = false;
    let mut saw_retry_delay_after_retry_info = false;
    let mut saw_sleep_key_after_retry_info = false;
    let mut saw_generic_sleep_after_retry_delay = false;
    let mut saw_durable_sleep_after_sleep_key = false;
    let mut saw_record_after_durable_sleep = false;
    let mut saw_record_after_generic_sleep = false;
    let mut saw_record_after_invoke = false;
    let mut saw_error_from_info_after_retry_info = false;
    let mut saw_checkpoint_after_invoke = false;
    let mut saw_checkpoint_signal_after_checkpoint = false;
    let mut saw_rate_limit_wait_state = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            (module, "get-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                get_checkpoint_index = Some(next_function_index);
                            }
                            (module, "checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                checkpoint_index = Some(next_function_index);
                            }
                            (module, "handle-checkpoint-signal")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_handle_checkpoint_signal_import = true;
                                handle_checkpoint_signal_index = Some(next_function_index);
                            }
                            (module, "durable-sleep")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_durable_sleep_import = true;
                                durable_sleep_index = Some(next_function_index);
                            }
                            (module, "durable-sleep-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_durable_sleep_checkpoint_import = true;
                                durable_sleep_checkpoint_index = Some(next_function_index);
                            }
                            (module, "agent-retry-sleep-key")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_retry_sleep_key_import = true;
                                agent_retry_sleep_key_index = Some(next_function_index);
                            }
                            (module, "agent-retry-delay-ms")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_retry_delay_import = true;
                                agent_retry_delay_index = Some(next_function_index);
                            }
                            (module, "agent-retry-error-info")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_retry_error_info_import = true;
                                agent_retry_error_info_index = Some(next_function_index);
                            }
                            (module, "agent-error-from-info")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                saw_agent_error_from_info_import = true;
                                agent_error_from_info_index = Some(next_function_index);
                            }
                            (module, "record-retry-attempt")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                saw_record_retry_attempt_import = true;
                                record_retry_attempt_index = Some(next_function_index);
                            }
                            (module, "invoke")
                                if module.contains("runtara:agent-utils/capabilities") =>
                            {
                                agent_invoke_index = Some(next_function_index);
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    let mut saw_lookup_call = false;
                    let mut saw_invoke_call = false;
                    let mut saw_retry_info_call = false;
                    let mut saw_retry_delay_call = false;
                    let mut saw_sleep_key_call = false;
                    let mut saw_durable_sleep_call = false;
                    let mut saw_generic_sleep_call = false;
                    let mut saw_checkpoint_call = false;
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == get_checkpoint_index =>
                            {
                                saw_lookup_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_invoke_index =>
                            {
                                saw_lookup_before_invoke = saw_lookup_call;
                                saw_invoke_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_retry_error_info_index =>
                            {
                                saw_retry_info_after_invoke = saw_invoke_call;
                                saw_retry_info_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_retry_delay_index =>
                            {
                                saw_retry_delay_after_retry_info = saw_retry_info_call;
                                saw_retry_delay_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_retry_sleep_key_index =>
                            {
                                saw_sleep_key_after_retry_info = saw_retry_info_call;
                                saw_sleep_key_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == durable_sleep_checkpoint_index =>
                            {
                                saw_durable_sleep_after_sleep_key = saw_sleep_key_call;
                                saw_durable_sleep_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == durable_sleep_index =>
                            {
                                saw_generic_sleep_after_retry_delay = saw_retry_delay_call;
                                saw_generic_sleep_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == record_retry_attempt_index =>
                            {
                                saw_record_after_invoke = saw_invoke_call;
                                saw_record_after_durable_sleep = saw_durable_sleep_call;
                                saw_record_after_generic_sleep = saw_generic_sleep_call;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_error_from_info_index =>
                            {
                                saw_error_from_info_after_retry_info = saw_retry_info_call;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == checkpoint_index =>
                            {
                                saw_checkpoint_after_invoke = saw_invoke_call;
                                saw_checkpoint_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == handle_checkpoint_signal_index =>
                            {
                                saw_checkpoint_signal_after_checkpoint = saw_checkpoint_call;
                            }
                            Operator::Loop { .. } => saw_retry_loop = true,
                            Operator::Br { relative_depth: 2 } => {
                                saw_retry_continue_branch = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET =>
                            {
                                saw_retryable_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET =>
                            {
                                saw_retry_info_retryable_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET =>
                            {
                                saw_retry_info_rate_limited_load = true;
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset
                                    == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET =>
                            {
                                saw_retry_after_tag_load = true;
                            }
                            Operator::I64Load { memarg }
                                if memarg.offset
                                    == DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET =>
                            {
                                saw_retry_after_value_load = true;
                            }
                            Operator::I64Add => saw_rate_limit_wait_accumulator = true,
                            Operator::I64Const { value: 750 } => {
                                saw_rate_limit_base_delay_const = true;
                            }
                            Operator::I64Const { value: 2_500 } => {
                                saw_rate_limit_budget_const = true;
                            }
                            Operator::I64LeU => saw_rate_limit_budget_compare = true,
                            Operator::I32Const { value: 2 } => {
                                saw_retry_bound = true;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            Payload::DataSection(reader) => {
                for data in reader {
                    let data = data.expect("data segment");
                    saw_rate_limit_wait_state |= data.data == DIRECT_AGENT_RATE_LIMIT_WAIT;
                }
            }
            _ => {}
        }
    }

    assert!(
        saw_record_retry_attempt_import,
        "core should import runtime.record-retry-attempt"
    );
    assert!(
        saw_durable_sleep_import,
        "core should import runtime.durable-sleep"
    );
    assert!(
        saw_durable_sleep_checkpoint_import,
        "core should import runtime.durable-sleep-checkpoint"
    );
    assert!(
        saw_handle_checkpoint_signal_import,
        "core should import runtime.handle-checkpoint-signal"
    );
    assert!(
        saw_agent_retry_sleep_key_import,
        "core should import stdlib.agent-retry-sleep-key"
    );
    assert!(
        saw_agent_retry_delay_import,
        "core should import stdlib.agent-retry-delay-ms"
    );
    assert!(
        saw_agent_retry_error_info_import,
        "core should import stdlib.agent-retry-error-info"
    );
    assert!(
        saw_agent_error_from_info_import,
        "core should import stdlib.agent-error-from-info"
    );
    assert!(saw_retry_loop, "durable retry Agent should lower a loop");
    assert!(
        saw_retry_continue_branch,
        "retry path should branch back to the loop"
    );
    assert!(
        saw_retryable_load,
        "retry decision should inspect Agent error-info.retryable"
    );
    assert!(
        saw_retry_info_retryable_load && saw_retry_info_rate_limited_load,
        "retry decision should use stdlib retry classification"
    );
    assert!(
        saw_retry_after_tag_load && saw_retry_after_value_load,
        "retry path should inspect typed retryAfterMs hints"
    );
    assert!(
        saw_rate_limit_wait_accumulator,
        "rate-limited retry path should accumulate wait time"
    );
    assert!(
        saw_rate_limit_base_delay_const,
        "rate-limited retry path should use base retry delay without retryAfterMs"
    );
    assert!(
        saw_rate_limit_budget_const && saw_rate_limit_budget_compare,
        "rate-limited retry path should compare against graph rateLimitBudgetMs"
    );
    assert!(
        saw_retry_bound,
        "retry loop should compare against maxRetries"
    );
    assert!(
        saw_lookup_before_invoke,
        "checkpoint lookup should run before capability invoke"
    );
    assert!(
        saw_record_after_invoke,
        "retry attempt recording should run after failed invoke"
    );
    assert!(
        saw_retry_info_after_invoke,
        "retry error payload should be built after failed invoke"
    );
    assert!(
        saw_retry_delay_after_retry_info,
        "retry delay should be computed from the preserved retry payload"
    );
    assert!(
        saw_sleep_key_after_retry_info,
        "retry sleep key should be built after preserving the error payload"
    );
    assert!(
        saw_durable_sleep_after_sleep_key,
        "typed retryAfterMs should lower to runtime.durable-sleep-checkpoint"
    );
    assert!(
        saw_record_after_durable_sleep,
        "retry attempt recording should run after the typed durable sleep"
    );
    assert!(
        saw_generic_sleep_after_retry_delay,
        "normal retries should lower to runtime.durable-sleep"
    );
    assert!(
        saw_record_after_generic_sleep,
        "retry attempt recording should run after generic backoff sleep"
    );
    assert!(
        saw_error_from_info_after_retry_info,
        "non-retried durable Agent errors should format the preserved retry payload"
    );
    assert!(
        saw_rate_limit_wait_state,
        "retry sleep should use the generated rate-limit wait state"
    );
    assert!(
        saw_checkpoint_after_invoke,
        "successful retry output should be checkpointed after invoke"
    );
    assert!(
        saw_checkpoint_signal_after_checkpoint,
        "successful retry checkpoint should handle pending lifecycle signals"
    );
}

#[test]
fn direct_core_lowers_non_durable_agent_connection_call() {
    let graph = non_durable_agent_connection_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let (interface_key, function) = imported_wit_function(
        &resolve,
        world,
        "runtara:agent-utils/capabilities",
        "invoke",
    );
    let (actual_module, actual_name) = resolve.wasm_import_name(
        ManglingAndAbi::Standard32,
        WasmImport::Func {
            interface: Some(interface_key),
            func: function,
        },
    );
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Agent connection core module validates");

    let mut agent_invoke_index = None;
    let mut agent_connection_input_index = None;
    let mut saw_connection_input_before_invoke = false;
    let mut saw_connection_some_tag_store = false;
    let mut pending_connection_tag_value = false;
    let mut previous_i32_const = None;
    let mut code_body_index = 0;
    let mut next_function_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if import.module == actual_module && import.name == actual_name {
                        agent_invoke_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "agent-connection-input"
                    {
                        agent_connection_input_index = Some(next_function_index);
                    }
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    let mut saw_connection_input_call = false;
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == agent_connection_input_index =>
                            {
                                saw_connection_input_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_invoke_index =>
                            {
                                saw_connection_input_before_invoke = saw_connection_input_call;
                            }
                            Operator::I32Const { value } => {
                                pending_connection_tag_value = previous_i32_const
                                    == Some(DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET)
                                    && value == 1;
                                previous_i32_const = Some(value);
                            }
                            Operator::I32Store { .. } if pending_connection_tag_value => {
                                saw_connection_some_tag_store = true;
                                pending_connection_tag_value = false;
                                previous_i32_const = None;
                            }
                            _ => {
                                pending_connection_tag_value = false;
                                previous_i32_const = None;
                            }
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        agent_connection_input_index.is_some(),
        "core should import stdlib.agent-connection-input"
    );
    assert!(
        saw_connection_input_before_invoke,
        "Agent connection input injection should run before capabilities.invoke"
    );
    assert!(
        saw_connection_some_tag_store,
        "Agent connection lowering should store option<connection-info> discriminant 1"
    );
}

#[test]
fn direct_core_lowers_non_durable_agent_on_error_route() {
    let graph = non_durable_agent_conditional_on_error_graph();
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent { error_plan, .. } = &core_config.run_plan else {
        panic!("expected Agent run plan");
    };
    let error_plan = error_plan.as_ref().expect("Agent onError plan");
    assert_eq!(error_plan.branches.len(), 1);
    assert!(error_plan.default_plan.is_some());

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Agent onError core module validates");

    let mut error_steps_index = None;
    let mut eval_condition_index = None;
    let mut complete_index = None;
    let mut fail_index = None;
    let mut saw_error_steps_call = false;
    let mut saw_condition_after_error_steps = false;
    let mut saw_complete_after_error_steps = false;
    let mut code_body_index = 0;
    let mut next_function_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "error-steps"
                    {
                        error_steps_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "eval-condition"
                    {
                        eval_condition_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-runtime/runtime")
                        && import.name == "complete"
                    {
                        complete_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-runtime/runtime")
                        && import.name == "fail"
                    {
                        fail_index = Some(next_function_index);
                    }
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            if Some(function_index) == error_steps_index {
                                saw_error_steps_call = true;
                            }
                            if saw_error_steps_call && Some(function_index) == eval_condition_index
                            {
                                saw_condition_after_error_steps = true;
                            }
                            if saw_error_steps_call && Some(function_index) == complete_index {
                                saw_complete_after_error_steps = true;
                            }
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        error_steps_index.is_some(),
        "core should import stdlib.error-steps"
    );
    assert!(
        fail_index.is_some(),
        "core should retain runtime.fail fallback for unmatched onError routes"
    );
    assert!(
        saw_error_steps_call,
        "Agent error path should insert __error into steps context"
    );
    assert!(
        saw_condition_after_error_steps,
        "conditional onError route should evaluate after error source construction"
    );
    assert!(
        saw_complete_after_error_steps,
        "handled onError Finish branch should complete the workflow"
    );
}

#[test]
fn direct_core_run_emits_step_debug_events_when_tracking_enabled() {
    let graph = fixture("simple");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, true).expect("core config");

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("tracked core module validates");

    let mut next_function_index = 0;
    let mut init_manifest_index = None;
    let mut load_input_index = None;
    let mut build_source_index = None;
    let mut apply_mapping_index = None;
    let mut complete_index = None;
    let mut custom_event_index = None;
    let mut fail_index = None;
    let mut step_debug_start_index = None;
    let mut step_debug_end_index = None;
    let mut saw_step_debug_start_kind = false;
    let mut saw_step_debug_end_kind = false;
    let mut saw_finish_step_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "init-manifest") => {
                                init_manifest_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "load-input") => {
                                load_input_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                complete_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                fail_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "step-debug-start") => {
                                step_debug_start_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "step-debug-end") => {
                                step_debug_end_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            Payload::DataSection(reader) => {
                for data in reader {
                    let data = data.expect("data segment");
                    saw_step_debug_start_kind |= data.data == DIRECT_STEP_DEBUG_START_KIND;
                    saw_step_debug_end_kind |= data.data == DIRECT_STEP_DEBUG_END_KIND;
                    saw_finish_step_id |= data.data == b"finish";
                }
            }
            _ => {}
        }
    }

    // Each setup/stdlib call (including the step-debug-start/end and their
    // custom-event emits) is followed by a fail-on-error guard (`runtime.fail`
    // inside an `if error` block) so an unhandled error surfaces as a `failed`
    // SDK event instead of a silent non-zero exit.
    let expected_call_order = [
        init_manifest_index.expect("init-manifest import"),
        fail_index.expect("fail import"),
        load_input_index.expect("load-input import"),
        fail_index.expect("fail import"),
        build_source_index.expect("build-source import"),
        fail_index.expect("fail import"),
        step_debug_start_index.expect("step-debug-start import"),
        fail_index.expect("fail import"),
        custom_event_index.expect("custom-event import"),
        fail_index.expect("fail import"),
        apply_mapping_index.expect("apply-mapping import"),
        fail_index.expect("fail import"),
        step_debug_end_index.expect("step-debug-end import"),
        fail_index.expect("fail import"),
        custom_event_index.expect("custom-event import"),
        fail_index.expect("fail import"),
        complete_index.expect("complete import"),
    ];
    assert_eq!(
        run_calls, expected_call_order,
        "tracked Finish run should emit start/end debug custom events around mapping"
    );
    assert!(
        saw_step_debug_start_kind,
        "step_debug_start custom-event kind should be static data"
    );
    assert!(
        saw_step_debug_end_kind,
        "step_debug_end custom-event kind should be static data"
    );
    assert!(
        saw_finish_step_id,
        "tracked debug events should pass the Finish step id as static data"
    );
}

#[test]
fn direct_core_run_lowers_conditional_finish_branches_through_stdlib() {
    let graph = fixture("conditional");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Conditional {
        condition_id,
        true_plan,
        false_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected conditional run plan");
    };
    let DirectRunPlan::Finish {
        mapping_id: true_mapping_id,
        ..
    } = true_plan.as_ref()
    else {
        panic!("expected true branch finish plan");
    };
    let DirectRunPlan::Finish {
        mapping_id: false_mapping_id,
        ..
    } = false_plan.as_ref()
    else {
        panic!("expected false branch finish plan");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("conditional core module validates");

    let mut next_function_index = 0;
    let mut eval_condition_index = None;
    let mut apply_mapping_index = None;
    let mut saw_condition_id = false;
    let mut saw_true_mapping_id = false;
    let mut saw_false_mapping_id = false;
    let mut saw_condition_bool_load = false;
    let mut saw_branch = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                eval_condition_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *condition_id as i32 {
                                    saw_condition_id = true;
                                }
                                if value == *true_mapping_id as i32 {
                                    saw_true_mapping_id = true;
                                }
                                if value == *false_mapping_id as i32 {
                                    saw_false_mapping_id = true;
                                }
                            }
                            Operator::I32Load8U { memarg }
                                if memarg.offset == 4 && memarg.memory == 0 =>
                            {
                                saw_condition_bool_load = true;
                            }
                            Operator::If { .. } => saw_branch = true,
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let eval_condition_index = eval_condition_index.expect("eval-condition import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert!(run_calls.contains(&eval_condition_index));
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        2,
        "conditional run should contain one apply-mapping call per branch"
    );
    assert!(saw_condition_id, "condition id should be passed to stdlib");
    assert!(
        saw_true_mapping_id,
        "true branch mapping id should be present"
    );
    assert!(
        saw_false_mapping_id,
        "false branch mapping id should be present"
    );
    assert!(
        saw_condition_bool_load,
        "condition result bool should be loaded from retptr payload"
    );
    assert!(saw_branch, "run body should branch on condition result");
}

#[test]
fn direct_core_run_lowers_nested_conditional_tree_through_stdlib() {
    let graph = fixture("conditional_nested");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let mut condition_ids = Vec::new();
    let mut mapping_ids = Vec::new();
    collect_run_plan_ids(&core_config.run_plan, &mut condition_ids, &mut mapping_ids);
    assert_eq!(condition_ids.len(), 2);
    assert_eq!(mapping_ids.len(), 3);

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("nested conditional core module validates");

    let mut next_function_index = 0;
    let mut eval_condition_index = None;
    let mut apply_mapping_index = None;
    let mut seen_condition_ids = Vec::new();
    let mut seen_mapping_ids = Vec::new();
    let mut branch_count = 0;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                eval_condition_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if condition_ids.contains(&(value as u32)) {
                                    seen_condition_ids.push(value as u32);
                                }
                                if mapping_ids.contains(&(value as u32)) {
                                    seen_mapping_ids.push(value as u32);
                                }
                            }
                            Operator::If { .. } => branch_count += 1,
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let eval_condition_index = eval_condition_index.expect("eval-condition import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == eval_condition_index)
            .count(),
        2,
        "nested conditional run should evaluate both condition sites"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        3,
        "nested conditional run should contain one apply-mapping call per Finish leaf"
    );
    condition_ids.sort_unstable();
    mapping_ids.sort_unstable();
    seen_condition_ids.sort_unstable();
    seen_condition_ids.dedup();
    seen_mapping_ids.sort_unstable();
    seen_mapping_ids.dedup();
    assert_eq!(seen_condition_ids, condition_ids);
    assert_eq!(seen_mapping_ids, mapping_ids);
    assert!(
        branch_count >= 2,
        "nested conditional run should emit Wasm branches"
    );
}

#[test]
fn direct_core_run_lowers_group_by_finish_through_stdlib() {
    let graph = fixture("group_by");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::GroupBy {
        group_id,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected GroupBy run plan");
    };
    let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
        panic!("expected GroupBy to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("GroupBy core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut group_by_index = None;
    let mut apply_mapping_index = None;
    let mut saw_group_id = false;
    let mut saw_mapping_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "group-by") => {
                                group_by_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *group_id as i32 {
                                    saw_group_id = true;
                                }
                                if value == *mapping_id as i32 {
                                    saw_mapping_id = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let group_by_index = group_by_index.expect("group-by import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "GroupBy run should rebuild source after updating steps context"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == group_by_index)
            .count(),
        1,
        "GroupBy run should call the stdlib GroupBy helper once"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        1,
        "GroupBy run should apply the terminal Finish mapping once"
    );
    assert!(saw_group_id, "GroupBy id should be passed to stdlib");
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
}

#[test]
fn direct_core_run_lowers_split_loop_through_stdlib() {
    let mut graph = fixture("split");
    graph.durable = Some(false);
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Split {
        split_id,
        nested_plan,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Split run plan");
    };
    assert_eq!(*split_id, 0);
    assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Agent { .. }));
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Split core module validates");

    let mut next_function_index = 0;
    let mut split_item_count_index = None;
    let mut split_item_index = None;
    let mut split_iteration_variables_index = None;
    let mut split_validate_input_index = None;
    let mut split_validate_output_index = None;
    let mut split_initial_results_index = None;
    let mut split_append_output_index = None;
    let mut split_output_index = None;
    let mut saw_loop = false;
    let mut saw_split_item_count_call = false;
    let mut saw_split_item_call = false;
    let mut saw_split_iteration_variables_call = false;
    let mut saw_split_validate_input_call = false;
    let mut saw_split_validate_output_call = false;
    let mut saw_split_initial_results_call = false;
    let mut saw_split_append_output_call = false;
    let mut saw_split_output_call = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if import.module.contains("runtara:workflow-stdlib/json") {
                        match import.name {
                            "split-item-count" => {
                                split_item_count_index = Some(next_function_index)
                            }
                            "split-item" => split_item_index = Some(next_function_index),
                            "split-iteration-variables" => {
                                split_iteration_variables_index = Some(next_function_index)
                            }
                            "split-validate-input" => {
                                split_validate_input_index = Some(next_function_index)
                            }
                            "split-validate-output" => {
                                split_validate_output_index = Some(next_function_index)
                            }
                            "split-initial-results" => {
                                split_initial_results_index = Some(next_function_index)
                            }
                            "split-append-output" => {
                                split_append_output_index = Some(next_function_index)
                            }
                            "split-output" => split_output_index = Some(next_function_index),
                            _ => {}
                        }
                    }
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Loop { .. } => saw_loop = true,
                            Operator::Call { function_index } => {
                                if Some(function_index) == split_item_count_index {
                                    saw_split_item_count_call = true;
                                }
                                if Some(function_index) == split_item_index {
                                    saw_split_item_call = true;
                                }
                                if Some(function_index) == split_iteration_variables_index {
                                    saw_split_iteration_variables_call = true;
                                }
                                if Some(function_index) == split_validate_input_index {
                                    saw_split_validate_input_call = true;
                                }
                                if Some(function_index) == split_validate_output_index {
                                    saw_split_validate_output_call = true;
                                }
                                if Some(function_index) == split_initial_results_index {
                                    saw_split_initial_results_call = true;
                                }
                                if Some(function_index) == split_append_output_index {
                                    saw_split_append_output_call = true;
                                }
                                if Some(function_index) == split_output_index {
                                    saw_split_output_call = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(saw_loop, "Split run should emit a Wasm loop");
    assert!(
        saw_split_item_count_call,
        "Split run should call split-item-count"
    );
    assert!(saw_split_item_call, "Split run should call split-item");
    assert!(
        saw_split_iteration_variables_call,
        "Split run should call split-iteration-variables"
    );
    assert!(
        saw_split_validate_input_call,
        "Split run should call split-validate-input"
    );
    assert!(
        saw_split_validate_output_call,
        "Split run should call split-validate-output"
    );
    assert!(
        saw_split_initial_results_call,
        "Split run should call split-initial-results"
    );
    assert!(
        saw_split_append_output_call,
        "Split run should call split-append-output"
    );
    assert!(saw_split_output_call, "Split run should call split-output");
}

#[test]
fn direct_core_run_lowers_split_breakpoint_before_split_execution() {
    let mut graph = fixture("split");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::Split(split)) = graph.steps.get_mut("split") else {
        panic!("expected Split fixture step");
    };
    split.breakpoint = Some(true);

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Split { breakpoint, .. } = &core_config.run_plan else {
        panic!("expected Split run plan");
    };
    assert!(*breakpoint, "durable Split breakpoint should lower");

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Split breakpoint core module validates");

    assert_direct_breakpoint_before_import(
        &core,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "split-cache-key",
    );
}

#[test]
fn direct_core_run_lowers_split_retry_helpers() {
    let mut graph = fixture("split");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::Split(split)) = graph.steps.get_mut("split") else {
        panic!("expected Split fixture step");
    };
    let config = split.config.as_mut().expect("split fixture config");
    config.max_retries = Some(2);
    config.retry_delay = Some(250);

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Split {
        durable,
        max_retries,
        retry_delay_ms,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Split run plan");
    };
    assert!(*durable, "Split retry test should use durable Split");
    assert_eq!(*max_retries, 2);
    assert_eq!(*retry_delay_ms, 250);

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Split retry core module validates");

    let (imports, run_calls) = direct_core_imports_and_run_calls(&core);
    let retry_delay_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "retry-delay-ms",
    );
    let retry_sleep_key_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "retry-sleep-key",
    );
    let workflow_retryable_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "workflow-error-retryable",
    );
    let workflow_rate_limited_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "workflow-error-rate-limited",
    );
    let workflow_retry_after_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "workflow-error-retry-after-ms",
    );
    let blocking_sleep_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-runtime/runtime@0.1",
        "blocking-sleep",
    );
    let durable_sleep_checkpoint_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-runtime/runtime@0.1",
        "durable-sleep-checkpoint",
    );
    let record_retry_index = direct_core_import(
        &imports,
        "cm32p2|runtara:workflow-runtime/runtime@0.1",
        "record-retry-attempt",
    );

    for (name, index) in [
        ("retry-delay-ms", retry_delay_index),
        ("retry-sleep-key", retry_sleep_key_index),
        ("workflow-error-retryable", workflow_retryable_index),
        ("workflow-error-rate-limited", workflow_rate_limited_index),
        ("workflow-error-retry-after-ms", workflow_retry_after_index),
        ("blocking-sleep", blocking_sleep_index),
        ("durable-sleep-checkpoint", durable_sleep_checkpoint_index),
        ("record-retry-attempt", record_retry_index),
    ] {
        assert!(
            run_calls.contains(&index),
            "Split retry lowering should call {name}: {run_calls:?}"
        );
    }
}

#[test]
fn direct_core_lowers_durable_split_checkpoint_path() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SplitCheckpointOp {
        CallSplitCacheKey,
        CallGetCheckpoint,
        CallSplitItemCount,
        CallSplitResult,
        CallCheckpoint,
        CallSplitOutputFromResult,
        Else,
        LoadCachedPtr,
        LoadCachedLen,
    }

    let graph: ExecutionGraph = serde_json::from_str(
        r#"{
          "entryPoint": "split",
          "durable": true,
          "steps": {
            "split": {
              "id": "split",
              "stepType": "Split",
              "config": {
                "value": { "valueType": "reference", "value": "data.items" },
                "sequential": true
              },
              "subgraph": {
                "entryPoint": "finish-item",
                "steps": {
                  "finish-item": {
                    "id": "finish-item",
                    "stepType": "Finish",
                    "inputMapping": {
                      "value": { "valueType": "reference", "value": "data.value" }
                    }
                  }
                },
                "executionPlan": []
              }
            },
            "finish": {
              "id": "finish",
              "stepType": "Finish",
              "inputMapping": {
                "results": { "valueType": "reference", "value": "steps.split.outputs" }
              }
            }
          },
          "executionPlan": [
            { "fromStep": "split", "toStep": "finish" }
          ]
        }"#,
    )
    .expect("durable split graph");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Split { durable, .. } = &core_config.run_plan else {
        panic!("expected Split run plan");
    };
    assert!(*durable, "Split run plan should be durable");

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("durable Split core module validates");

    let mut next_function_index = 0;
    let mut split_cache_key_index = None;
    let mut get_checkpoint_index = None;
    let mut split_item_count_index = None;
    let mut split_result_index = None;
    let mut checkpoint_index = None;
    let mut split_output_from_result_index = None;
    let mut code_body_index = 0;
    let mut ops = Vec::new();

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            (module, "split-cache-key")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                split_cache_key_index = Some(next_function_index);
                            }
                            (module, "get-checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                get_checkpoint_index = Some(next_function_index);
                            }
                            (module, "split-item-count")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                split_item_count_index = Some(next_function_index);
                            }
                            (module, "split-result")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                split_result_index = Some(next_function_index);
                            }
                            (module, "checkpoint")
                                if module.contains("runtara:workflow-runtime/runtime") =>
                            {
                                checkpoint_index = Some(next_function_index);
                            }
                            (module, "split-output-from-result")
                                if module.contains("runtara:workflow-stdlib/json") =>
                            {
                                split_output_from_result_index = Some(next_function_index);
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == split_cache_key_index =>
                            {
                                ops.push(SplitCheckpointOp::CallSplitCacheKey);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == get_checkpoint_index =>
                            {
                                ops.push(SplitCheckpointOp::CallGetCheckpoint);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == split_item_count_index =>
                            {
                                ops.push(SplitCheckpointOp::CallSplitItemCount);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == split_result_index =>
                            {
                                ops.push(SplitCheckpointOp::CallSplitResult);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == checkpoint_index =>
                            {
                                ops.push(SplitCheckpointOp::CallCheckpoint);
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == split_output_from_result_index =>
                            {
                                ops.push(SplitCheckpointOp::CallSplitOutputFromResult);
                            }
                            Operator::Else => ops.push(SplitCheckpointOp::Else),
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_LIST_PTR_OFFSET =>
                            {
                                ops.push(SplitCheckpointOp::LoadCachedPtr);
                            }
                            Operator::I32Load { memarg }
                                if memarg.offset == DIRECT_RESULT_OPTION_LIST_LEN_OFFSET =>
                            {
                                ops.push(SplitCheckpointOp::LoadCachedLen);
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let lookup_index = ops
        .iter()
        .position(|op| *op == SplitCheckpointOp::CallGetCheckpoint)
        .expect("checkpoint lookup");
    let cached_ptr_index = ops[lookup_index + 1..]
        .iter()
        .position(|op| *op == SplitCheckpointOp::LoadCachedPtr)
        .map(|offset| lookup_index + 1 + offset)
        .expect("cached Split payload pointer load");
    let replay_else_index = ops[cached_ptr_index + 1..]
        .iter()
        .position(|op| *op == SplitCheckpointOp::Else)
        .map(|offset| cached_ptr_index + 1 + offset)
        .expect("checkpoint replay else branch");
    let first_cache_key_index = ops
        .iter()
        .position(|op| *op == SplitCheckpointOp::CallSplitCacheKey)
        .expect("first Split cache key");
    assert!(
        first_cache_key_index < lookup_index,
        "Split cache key should be computed before checkpoint lookup"
    );
    assert!(
        ops[cached_ptr_index..replay_else_index].contains(&SplitCheckpointOp::LoadCachedPtr),
        "cached Split branch should load checkpoint payload pointer"
    );
    assert!(
        ops[cached_ptr_index..replay_else_index].contains(&SplitCheckpointOp::LoadCachedLen),
        "cached Split branch should load checkpoint payload length"
    );
    assert!(
        !ops[lookup_index + 1..replay_else_index].contains(&SplitCheckpointOp::CallSplitItemCount),
        "cached Split branch must not enter the iteration loop"
    );

    let item_count_index = ops[replay_else_index + 1..]
        .iter()
        .position(|op| *op == SplitCheckpointOp::CallSplitItemCount)
        .map(|offset| replay_else_index + 1 + offset)
        .expect("fresh Split item count");
    let split_result_index = ops[item_count_index + 1..]
        .iter()
        .position(|op| *op == SplitCheckpointOp::CallSplitResult)
        .map(|offset| item_count_index + 1 + offset)
        .expect("fresh Split result");
    let checkpoint_index = ops[split_result_index + 1..]
        .iter()
        .position(|op| *op == SplitCheckpointOp::CallCheckpoint)
        .map(|offset| split_result_index + 1 + offset)
        .expect("fresh Split checkpoint save");
    let output_from_result_index = ops[checkpoint_index + 1..]
        .iter()
        .position(|op| *op == SplitCheckpointOp::CallSplitOutputFromResult)
        .map(|offset| checkpoint_index + 1 + offset)
        .expect("Split output-from-result");
    assert!(
        checkpoint_index < output_from_result_index,
        "Split final result should be checkpointed before steps context insertion"
    );
}

#[test]
fn direct_core_run_lowers_while_loop_through_stdlib() {
    let graph = fixture("while_simple");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent { next_plan, .. } = &core_config.run_plan else {
        panic!("expected root Agent run plan");
    };
    let DirectRunPlan::While {
        while_id,
        nested_plan,
        next_plan,
        ..
    } = next_plan.as_ref()
    else {
        panic!("expected While run plan after init Agent");
    };
    assert_eq!(*while_id, 0);
    assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Agent { .. }));
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("While core module validates");

    let mut next_function_index = 0;
    let mut while_max_iterations_index = None;
    let mut while_initial_state_index = None;
    let mut while_condition_source_index = None;
    let mut while_condition_index = None;
    let mut while_iteration_variables_index = None;
    let mut while_advance_state_index = None;
    let mut while_output_index = None;
    let mut runtime_heartbeat_index = None;
    let mut runtime_is_cancelled_index = None;
    let mut runtime_check_signals_index = None;
    let mut saw_loop = false;
    let mut saw_while_id = false;
    let mut saw_while_max_iterations_call = false;
    let mut saw_while_initial_state_call = false;
    let mut saw_while_condition_source_call = false;
    let mut saw_while_condition_call = false;
    let mut saw_while_iteration_variables_call = false;
    let mut saw_while_advance_state_call = false;
    let mut saw_while_output_call = false;
    let mut saw_runtime_heartbeat_call = false;
    let mut saw_runtime_is_cancelled_call = false;
    let mut saw_runtime_check_signals_call = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    match import.module {
                        module if module.contains("runtara:workflow-stdlib/json") => {
                            match import.name {
                                "while-max-iterations" => {
                                    while_max_iterations_index = Some(next_function_index)
                                }
                                "while-initial-state" => {
                                    while_initial_state_index = Some(next_function_index)
                                }
                                "while-condition-source" => {
                                    while_condition_source_index = Some(next_function_index)
                                }
                                "while-condition" => {
                                    while_condition_index = Some(next_function_index)
                                }
                                "while-iteration-variables" => {
                                    while_iteration_variables_index = Some(next_function_index)
                                }
                                "while-advance-state" => {
                                    while_advance_state_index = Some(next_function_index)
                                }
                                "while-output" => while_output_index = Some(next_function_index),
                                _ => {}
                            }
                        }
                        module if module.contains("runtara:workflow-runtime/runtime") => {
                            match import.name {
                                "heartbeat" => runtime_heartbeat_index = Some(next_function_index),
                                "is-cancelled" => {
                                    runtime_is_cancelled_index = Some(next_function_index)
                                }
                                "check-signals" => {
                                    runtime_check_signals_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Loop { .. } => saw_loop = true,
                            Operator::I32Const { value } if value == *while_id as i32 => {
                                saw_while_id = true;
                            }
                            Operator::Call { function_index } => {
                                if Some(function_index) == while_max_iterations_index {
                                    saw_while_max_iterations_call = true;
                                }
                                if Some(function_index) == while_initial_state_index {
                                    saw_while_initial_state_call = true;
                                }
                                if Some(function_index) == while_condition_source_index {
                                    saw_while_condition_source_call = true;
                                }
                                if Some(function_index) == while_condition_index {
                                    saw_while_condition_call = true;
                                }
                                if Some(function_index) == while_iteration_variables_index {
                                    saw_while_iteration_variables_call = true;
                                }
                                if Some(function_index) == while_advance_state_index {
                                    saw_while_advance_state_call = true;
                                }
                                if Some(function_index) == while_output_index {
                                    saw_while_output_call = true;
                                }
                                if Some(function_index) == runtime_heartbeat_index {
                                    saw_runtime_heartbeat_call = true;
                                }
                                if Some(function_index) == runtime_is_cancelled_index {
                                    saw_runtime_is_cancelled_call = true;
                                }
                                if Some(function_index) == runtime_check_signals_index {
                                    saw_runtime_check_signals_call = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(saw_loop, "While run should emit a Wasm loop");
    assert!(saw_while_id, "While id should be passed to stdlib");
    assert!(
        saw_while_max_iterations_call,
        "While run should call while-max-iterations"
    );
    assert!(
        saw_while_initial_state_call,
        "While run should call while-initial-state"
    );
    assert!(
        saw_while_condition_source_call,
        "While run should call while-condition-source"
    );
    assert!(
        saw_while_condition_call,
        "While run should call while-condition"
    );
    assert!(
        saw_while_iteration_variables_call,
        "While run should call while-iteration-variables"
    );
    assert!(
        saw_while_advance_state_call,
        "While run should call while-advance-state"
    );
    assert!(saw_while_output_call, "While run should call while-output");
    assert!(
        saw_runtime_is_cancelled_call,
        "While run should check cancellation before each iteration body"
    );
    assert!(
        saw_runtime_heartbeat_call,
        "While run should heartbeat after each iteration body"
    );
    assert!(
        saw_runtime_check_signals_call,
        "While run should check lifecycle signals after each iteration body"
    );
}

#[test]
fn direct_core_run_lowers_while_breakpoint_before_loop_execution() {
    let mut graph = fixture("while_simple");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::While(while_step)) = graph.steps.get_mut("loop") else {
        panic!("expected While fixture step");
    };
    while_step.breakpoint = Some(true);

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Agent { next_plan, .. } = &core_config.run_plan else {
        panic!("expected root Agent run plan");
    };
    let DirectRunPlan::While { breakpoint, .. } = next_plan.as_ref() else {
        panic!("expected While run plan after init Agent");
    };
    assert!(*breakpoint, "durable While breakpoint should lower");

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("While breakpoint core module validates");

    assert_direct_breakpoint_before_import(
        &core,
        "cm32p2|runtara:workflow-stdlib/json@0.1",
        "while-max-iterations",
    );
}

#[test]
fn direct_core_run_collects_split_validation_errors_when_dont_stop_is_enabled() {
    let mut graph = fixture("split_with_schemas_failing");
    graph.durable = Some(false);
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Split {
        dont_stop_on_failed,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Split run plan");
    };
    assert!(*dont_stop_on_failed);

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Split dontStop core module validates");

    let mut next_function_index = 0;
    let mut agent_failure_index = None;
    let mut agent_validate_input_index = None;
    let mut apply_mapping_index = None;
    let mut split_append_error_index = None;
    let mut saw_split_append_error_call = false;
    let mut saw_agent_failure_call = false;
    let mut saw_apply_mapping_failure_path = false;
    let mut pending_apply_mapping_failure_path = false;
    let mut saw_split_append_error_after_agent_failure = false;
    let mut saw_continue_after_split_append_error = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "split-append-error"
                    {
                        split_append_error_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "apply-mapping"
                    {
                        apply_mapping_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "agent-validate-input"
                    {
                        agent_validate_input_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && matches!(import.name, "agent-error" | "agent-error-from-info")
                    {
                        agent_failure_index = Some(next_function_index);
                    }
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == apply_mapping_index =>
                            {
                                pending_apply_mapping_failure_path = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_validate_input_index =>
                            {
                                pending_apply_mapping_failure_path = false;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == agent_failure_index =>
                            {
                                saw_agent_failure_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == split_append_error_index =>
                            {
                                if saw_agent_failure_call {
                                    saw_split_append_error_after_agent_failure = true;
                                }
                                if pending_apply_mapping_failure_path {
                                    saw_apply_mapping_failure_path = true;
                                }
                                saw_split_append_error_call = true;
                            }
                            Operator::Br { relative_depth: 1 } if saw_split_append_error_call => {
                                saw_continue_after_split_append_error = true;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        saw_split_append_error_call,
        "Split dontStop run should append validation failures"
    );
    assert!(
        saw_split_append_error_after_agent_failure,
        "Split dontStop run should append nested Agent failures"
    );
    assert!(
        saw_apply_mapping_failure_path,
        "Split dontStop run should append nested mapping failures"
    );
    assert!(
        saw_continue_after_split_append_error,
        "Split dontStop validation failure path should continue the loop"
    );
}

#[test]
fn direct_core_run_collects_split_error_steps_when_dont_stop_is_enabled() {
    let mut graph = fixture("split_with_error");
    graph.durable = Some(false);
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");

    let DirectRunPlan::Split {
        dont_stop_on_failed,
        nested_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Split run plan");
    };
    assert!(*dont_stop_on_failed);
    assert!(matches!(nested_plan.as_ref(), DirectRunPlan::Error { .. }));

    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids)
            .expect("agent resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Split Error dontStop core module validates");

    let mut next_function_index = 0;
    let mut error_index = None;
    let mut split_append_error_index = None;
    let mut saw_error_call = false;
    let mut saw_split_append_error_after_error = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "error"
                    {
                        error_index = Some(next_function_index);
                    }
                    if import.module.contains("runtara:workflow-stdlib/json")
                        && import.name == "split-append-error"
                    {
                        split_append_error_index = Some(next_function_index);
                    }
                    if matches!(import.ty, TypeRef::Func(_)) {
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators").into_iter() {
                        match operator.expect("operator") {
                            Operator::Call { function_index }
                                if Some(function_index) == error_index =>
                            {
                                saw_error_call = true;
                            }
                            Operator::Call { function_index }
                                if Some(function_index) == split_append_error_index =>
                            {
                                if saw_error_call {
                                    saw_split_append_error_after_error = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    assert!(
        saw_split_append_error_after_error,
        "Split dontStop run should append explicit Error step failures"
    );
}

#[test]
fn direct_core_run_lowers_durable_delay_finish_through_stdlib_and_runtime() {
    let graph = fixture("delay_simple");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Delay {
        delay_id,
        durable,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Delay run plan");
    };
    assert!(*durable);
    let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
        panic!("expected Delay to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Delay core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut delay_duration_index = None;
    let mut durable_sleep_checkpoint_index = None;
    let mut delay_index = None;
    let mut apply_mapping_index = None;
    let mut saw_delay_id = false;
    let mut saw_mapping_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay-duration-ms") => {
                                delay_duration_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "durable-sleep-checkpoint",
                            ) => durable_sleep_checkpoint_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay") => {
                                delay_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *delay_id as i32 {
                                    saw_delay_id = true;
                                }
                                if value == *mapping_id as i32 {
                                    saw_mapping_id = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let delay_duration_index = delay_duration_index.expect("delay-duration-ms import");
    let durable_sleep_checkpoint_index =
        durable_sleep_checkpoint_index.expect("durable-sleep-checkpoint import");
    let delay_index = delay_index.expect("delay import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    let delay_duration_position = run_calls
        .iter()
        .position(|&index| index == delay_duration_index)
        .expect("Delay duration call");
    let durable_sleep_position = run_calls
        .iter()
        .position(|&index| index == durable_sleep_checkpoint_index)
        .expect("durable sleep checkpoint call");
    let delay_position = run_calls
        .iter()
        .position(|&index| index == delay_index)
        .expect("Delay output call");
    let finish_position = run_calls
        .iter()
        .position(|&index| index == apply_mapping_index)
        .expect("Finish mapping call");

    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "Delay run should rebuild source after updating steps context"
    );
    assert!(
        delay_duration_position < durable_sleep_position,
        "Delay duration must be resolved before durable sleep"
    );
    assert!(
        durable_sleep_position < delay_position,
        "Delay output should be stored after durable sleep"
    );
    assert!(
        delay_position < finish_position,
        "Finish mapping should run after Delay updates steps context"
    );
    assert!(saw_delay_id, "Delay id should be passed to stdlib");
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
}

#[test]
fn direct_core_run_lowers_delay_breakpoint_pause_before_sleep() {
    let mut graph = fixture("delay_simple");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::Delay(delay)) = graph.steps.get_mut("delay") else {
        panic!("expected Delay fixture step");
    };
    delay.breakpoint = Some(true);

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Delay {
        breakpoint,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Delay run plan");
    };
    assert!(*breakpoint, "durable Delay breakpoint should lower");
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Delay breakpoint core module validates");

    let mut next_function_index = 0;
    let mut stdlib_build_source_index = None;
    let mut runtime_debug_mode_enabled_index = None;
    let mut stdlib_breakpoint_key_index = None;
    let mut runtime_checkpoint_index = None;
    let mut stdlib_breakpoint_event_index = None;
    let mut runtime_custom_event_index = None;
    let mut runtime_breakpoint_pause_index = None;
    let mut stdlib_delay_duration_index = None;
    let mut runtime_durable_sleep_checkpoint_index = None;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                stdlib_build_source_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "debug-mode-enabled",
                            ) => runtime_debug_mode_enabled_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-key") => {
                                stdlib_breakpoint_key_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "checkpoint") => {
                                runtime_checkpoint_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-event") => {
                                stdlib_breakpoint_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                runtime_custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "breakpoint-pause") => {
                                runtime_breakpoint_pause_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay-duration-ms") => {
                                stdlib_delay_duration_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "durable-sleep-checkpoint",
                            ) => runtime_durable_sleep_checkpoint_index = Some(next_function_index),
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let stdlib_build_source_index = stdlib_build_source_index.expect("build-source import");
    let runtime_debug_mode_enabled_index =
        runtime_debug_mode_enabled_index.expect("debug-mode-enabled import");
    let stdlib_breakpoint_key_index = stdlib_breakpoint_key_index.expect("breakpoint-key import");
    let runtime_checkpoint_index = runtime_checkpoint_index.expect("checkpoint import");
    let stdlib_breakpoint_event_index =
        stdlib_breakpoint_event_index.expect("breakpoint-event import");
    let runtime_custom_event_index = runtime_custom_event_index.expect("custom-event import");
    let runtime_breakpoint_pause_index =
        runtime_breakpoint_pause_index.expect("breakpoint-pause import");
    let stdlib_delay_duration_index =
        stdlib_delay_duration_index.expect("delay-duration-ms import");
    let runtime_durable_sleep_checkpoint_index =
        runtime_durable_sleep_checkpoint_index.expect("durable-sleep-checkpoint import");

    let position = |index| {
        run_calls
            .iter()
            .position(|call| *call == index)
            .expect("expected Delay breakpoint call")
    };

    let build_source_position = position(stdlib_build_source_index);
    let debug_mode_position = position(runtime_debug_mode_enabled_index);
    let breakpoint_key_position = position(stdlib_breakpoint_key_index);
    let checkpoint_position = position(runtime_checkpoint_index);
    let breakpoint_event_position = position(stdlib_breakpoint_event_index);
    let custom_event_position = position(runtime_custom_event_index);
    let breakpoint_pause_position = position(runtime_breakpoint_pause_index);
    let delay_duration_position = position(stdlib_delay_duration_index);
    let durable_sleep_position = position(runtime_durable_sleep_checkpoint_index);

    assert!(
        build_source_position < debug_mode_position
            && debug_mode_position < breakpoint_key_position
            && breakpoint_key_position < checkpoint_position
            && checkpoint_position < breakpoint_event_position
            && breakpoint_event_position < custom_event_position
            && custom_event_position < breakpoint_pause_position
            && breakpoint_pause_position < delay_duration_position
            && delay_duration_position < durable_sleep_position,
        "Delay breakpoint should pause before duration resolution and sleep: {run_calls:?}"
    );
}

#[test]
fn direct_core_run_lowers_non_durable_delay_finish_through_blocking_sleep() {
    let mut graph = fixture("delay_simple");
    graph.durable = Some(false);
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Delay {
        delay_id,
        durable,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Delay run plan");
    };
    assert!(!*durable);
    let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
        panic!("expected Delay to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("non-durable Delay core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut delay_duration_index = None;
    let mut durable_sleep_checkpoint_index = None;
    let mut blocking_sleep_index = None;
    let mut delay_index = None;
    let mut apply_mapping_index = None;
    let mut saw_delay_id = false;
    let mut saw_mapping_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay-duration-ms") => {
                                delay_duration_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "durable-sleep-checkpoint",
                            ) => durable_sleep_checkpoint_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "blocking-sleep") => {
                                blocking_sleep_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "delay") => {
                                delay_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *delay_id as i32 {
                                    saw_delay_id = true;
                                }
                                if value == *mapping_id as i32 {
                                    saw_mapping_id = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let delay_duration_index = delay_duration_index.expect("delay-duration-ms import");
    let durable_sleep_checkpoint_index =
        durable_sleep_checkpoint_index.expect("durable-sleep-checkpoint import");
    let blocking_sleep_index = blocking_sleep_index.expect("blocking-sleep import");
    let delay_index = delay_index.expect("delay import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    let delay_duration_position = run_calls
        .iter()
        .position(|&index| index == delay_duration_index)
        .expect("Delay duration call");
    let blocking_sleep_position = run_calls
        .iter()
        .position(|&index| index == blocking_sleep_index)
        .expect("blocking sleep call");
    let delay_position = run_calls
        .iter()
        .position(|&index| index == delay_index)
        .expect("Delay output call");
    let finish_position = run_calls
        .iter()
        .position(|&index| index == apply_mapping_index)
        .expect("Finish mapping call");

    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "Delay run should rebuild source after updating steps context"
    );
    assert!(
        !run_calls.contains(&durable_sleep_checkpoint_index),
        "non-durable Delay must not call durable sleep checkpoint"
    );
    assert!(
        delay_duration_position < blocking_sleep_position,
        "Delay duration must be resolved before blocking sleep"
    );
    assert!(
        blocking_sleep_position < delay_position,
        "Delay output should be stored after blocking sleep"
    );
    assert!(
        delay_position < finish_position,
        "Finish mapping should run after Delay updates steps context"
    );
    assert!(saw_delay_id, "Delay id should be passed to stdlib");
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
}

#[test]
fn direct_core_run_lowers_wait_for_signal_finish_through_runtime_polling() {
    let graph = fixture("wait_timeout");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::WaitForSignal {
        step_id, next_plan, ..
    } = &core_config.run_plan
    else {
        panic!("expected WaitForSignal run plan");
    };
    assert_eq!(step_id, "wait");
    let DirectRunPlan::Finish { .. } = next_plan.as_ref() else {
        panic!("expected WaitForSignal to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("WaitForSignal core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut wait_signal_id_index = None;
    let mut wait_timeout_index = None;
    let mut wait_timeout_error_index = None;
    let mut wait_poll_interval_index = None;
    let mut wait_event_index = None;
    let mut wait_output_index = None;
    let mut apply_mapping_index = None;
    let mut runtime_instance_id_index = None;
    let mut runtime_now_ms_index = None;
    let mut runtime_fail_index = None;
    let mut runtime_custom_event_index = None;
    let mut runtime_check_signals_index = None;
    let mut runtime_poll_custom_signal_index = None;
    let mut runtime_heartbeat_index = None;
    let mut runtime_blocking_sleep_index = None;
    let mut run_calls = Vec::new();
    let mut saw_loop = false;
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-signal-id") => {
                                wait_signal_id_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-timeout-ms") => {
                                wait_timeout_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-timeout-error") => {
                                wait_timeout_error_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-stdlib/json@0.1",
                                "wait-poll-interval-ms",
                            ) => wait_poll_interval_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-event") => {
                                wait_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-output") => {
                                wait_output_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "instance-id") => {
                                runtime_instance_id_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "now-ms") => {
                                runtime_now_ms_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                runtime_fail_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                runtime_custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "check-signals") => {
                                runtime_check_signals_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "poll-custom-signal",
                            ) => runtime_poll_custom_signal_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "heartbeat") => {
                                runtime_heartbeat_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "blocking-sleep") => {
                                runtime_blocking_sleep_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Loop { .. } => saw_loop = true,
                            Operator::Call { function_index } => run_calls.push(function_index),
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let ordered = [
        runtime_instance_id_index.expect("instance-id import"),
        wait_signal_id_index.expect("wait-signal-id import"),
        wait_timeout_index.expect("wait-timeout-ms import"),
        runtime_now_ms_index.expect("now-ms import"),
        wait_event_index.expect("wait-event import"),
        runtime_custom_event_index.expect("custom-event import"),
        wait_poll_interval_index.expect("wait-poll-interval-ms import"),
        runtime_check_signals_index.expect("check-signals import"),
        runtime_poll_custom_signal_index.expect("poll-custom-signal import"),
        runtime_heartbeat_index.expect("heartbeat import"),
        runtime_blocking_sleep_index.expect("blocking-sleep import"),
        wait_output_index.expect("wait-output import"),
        apply_mapping_index.expect("apply-mapping import"),
    ];
    let positions = ordered
        .iter()
        .map(|index| {
            run_calls
                .iter()
                .position(|call| call == index)
                .expect("expected WaitForSignal lowering call")
        })
        .collect::<Vec<_>>();

    assert!(saw_loop, "WaitForSignal run should poll in a Wasm loop");
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "WaitForSignal lowering calls should preserve generated-code order: {positions:?}"
    );
    assert!(
        run_calls.contains(&wait_timeout_error_index.expect("wait-timeout-error import")),
        "WaitForSignal timeout lowering should format generated-compatible timeout errors"
    );
    assert!(
        run_calls.contains(&runtime_fail_index.expect("fail import")),
        "WaitForSignal timeout lowering should report timeout through runtime.fail"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "WaitForSignal run should rebuild source after updating steps context"
    );
}

#[test]
fn direct_core_run_lowers_wait_for_signal_debug_events_with_tracking() {
    let graph = fixture("wait_timeout");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, true).expect("core config");

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("tracked WaitForSignal core module validates");

    let mut next_function_index = 0;
    let mut wait_signal_id_index = None;
    let mut wait_timeout_index = None;
    let mut wait_debug_start_index = None;
    let mut wait_event_index = None;
    let mut wait_output_index = None;
    let mut step_debug_end_index = None;
    let mut apply_mapping_index = None;
    let mut runtime_custom_event_index = None;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-signal-id") => {
                                wait_signal_id_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-timeout-ms") => {
                                wait_timeout_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-debug-start") => {
                                wait_debug_start_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-event") => {
                                wait_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-output") => {
                                wait_output_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "step-debug-end") => {
                                step_debug_end_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                runtime_custom_event_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let wait_signal_id_index = wait_signal_id_index.expect("wait-signal-id import");
    let wait_timeout_index = wait_timeout_index.expect("wait-timeout-ms import");
    let wait_debug_start_index = wait_debug_start_index.expect("wait-debug-start import");
    let wait_event_index = wait_event_index.expect("wait-event import");
    let wait_output_index = wait_output_index.expect("wait-output import");
    let step_debug_end_index = step_debug_end_index.expect("step-debug-end import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    let runtime_custom_event_index = runtime_custom_event_index.expect("custom-event import");

    let position = |index| {
        run_calls
            .iter()
            .position(|call| *call == index)
            .expect("expected tracked WaitForSignal call")
    };
    let wait_signal_id_pos = position(wait_signal_id_index);
    let wait_timeout_pos = position(wait_timeout_index);
    let wait_debug_start_pos = position(wait_debug_start_index);
    let wait_event_pos = position(wait_event_index);
    let wait_output_pos = position(wait_output_index);
    let step_debug_end_pos = position(step_debug_end_index);
    let apply_mapping_pos = position(apply_mapping_index);
    let custom_event_positions = run_calls
        .iter()
        .enumerate()
        .filter_map(|(position, &index)| (index == runtime_custom_event_index).then_some(position))
        .collect::<Vec<_>>();

    assert!(
        custom_event_positions.len() >= 3,
        "tracked WaitForSignal should emit wait debug-start, wait-request, and wait debug-end custom events"
    );
    assert!(
        wait_signal_id_pos < wait_timeout_pos
            && wait_timeout_pos < wait_debug_start_pos
            && wait_debug_start_pos < custom_event_positions[0]
            && custom_event_positions[0] < wait_event_pos
            && wait_event_pos < custom_event_positions[1]
            && wait_event_pos < wait_output_pos
            && wait_output_pos < step_debug_end_pos
            && step_debug_end_pos < custom_event_positions[2]
            && custom_event_positions[2] < apply_mapping_pos,
        "tracked WaitForSignal debug calls should bracket wait request/output: {run_calls:?}"
    );
}

#[test]
fn direct_core_run_lowers_wait_for_signal_breakpoint_pause() {
    let mut graph = fixture("wait_simple");
    graph.durable = Some(true);
    let Some(runtara_dsl::Step::WaitForSignal(wait)) = graph.steps.get_mut("wait") else {
        panic!("expected WaitForSignal fixture step");
    };
    wait.breakpoint = Some(true);

    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::WaitForSignal {
        breakpoint,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected WaitForSignal run plan");
    };
    assert!(*breakpoint, "durable WaitForSignal breakpoint should lower");
    assert!(matches!(next_plan.as_ref(), DirectRunPlan::Finish { .. }));

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("WaitForSignal breakpoint core module validates");

    let mut next_function_index = 0;
    let mut runtime_debug_mode_enabled_index = None;
    let mut stdlib_breakpoint_key_index = None;
    let mut runtime_checkpoint_index = None;
    let mut stdlib_breakpoint_event_index = None;
    let mut runtime_custom_event_index = None;
    let mut runtime_breakpoint_pause_index = None;
    let mut runtime_instance_id_index = None;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            (
                                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                                "debug-mode-enabled",
                            ) => runtime_debug_mode_enabled_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-key") => {
                                stdlib_breakpoint_key_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "checkpoint") => {
                                runtime_checkpoint_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "breakpoint-event") => {
                                stdlib_breakpoint_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                runtime_custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "breakpoint-pause") => {
                                runtime_breakpoint_pause_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "instance-id") => {
                                runtime_instance_id_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let runtime_debug_mode_enabled_index =
        runtime_debug_mode_enabled_index.expect("debug-mode-enabled import");
    let stdlib_breakpoint_key_index = stdlib_breakpoint_key_index.expect("breakpoint-key import");
    let runtime_checkpoint_index = runtime_checkpoint_index.expect("checkpoint import");
    let stdlib_breakpoint_event_index =
        stdlib_breakpoint_event_index.expect("breakpoint-event import");
    let runtime_custom_event_index = runtime_custom_event_index.expect("custom-event import");
    let runtime_breakpoint_pause_index =
        runtime_breakpoint_pause_index.expect("breakpoint-pause import");
    let runtime_instance_id_index = runtime_instance_id_index.expect("instance-id import");

    let position = |index| {
        run_calls
            .iter()
            .position(|call| *call == index)
            .expect("expected WaitForSignal breakpoint call")
    };
    let debug_mode_position = position(runtime_debug_mode_enabled_index);
    let breakpoint_key_position = position(stdlib_breakpoint_key_index);
    let checkpoint_position = position(runtime_checkpoint_index);
    let breakpoint_event_position = position(stdlib_breakpoint_event_index);
    let breakpoint_pause_position = position(runtime_breakpoint_pause_index);
    let instance_id_position = position(runtime_instance_id_index);
    let first_custom_event_position = run_calls
        .iter()
        .position(|&index| index == runtime_custom_event_index)
        .expect("breakpoint custom-event call");

    assert!(
        debug_mode_position < breakpoint_key_position
            && breakpoint_key_position < checkpoint_position
            && checkpoint_position < breakpoint_event_position
            && breakpoint_event_position < first_custom_event_position
            && first_custom_event_position < breakpoint_pause_position
            && breakpoint_pause_position < instance_id_position,
        "WaitForSignal breakpoint should pause before wait setup: {run_calls:?}"
    );
}

#[test]
fn direct_core_run_executes_wait_on_wait_callback_before_wait_event() {
    let graph = fixture("wait_on_wait");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::WaitForSignal {
        step_id,
        on_wait_plan: Some(on_wait_plan),
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected WaitForSignal run plan with onWait callback");
    };
    assert_eq!(step_id, "wait");
    let DirectRunPlan::Log {
        next_plan: on_wait_next,
        ..
    } = on_wait_plan.as_ref()
    else {
        panic!("expected onWait callback to start with Log");
    };
    let DirectRunPlan::Finish { .. } = on_wait_next.as_ref() else {
        panic!("expected onWait callback to finish");
    };
    let DirectRunPlan::Finish { .. } = next_plan.as_ref() else {
        panic!("expected WaitForSignal to flow into parent Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("WaitForSignal onWait core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut wait_on_wait_variables_index = None;
    let mut log_event_index = None;
    let mut log_index = None;
    let mut wait_event_index = None;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            (
                                "cm32p2|runtara:workflow-stdlib/json@0.1",
                                "wait-on-wait-variables",
                            ) => wait_on_wait_variables_index = Some(next_function_index),
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "log-event") => {
                                log_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "log") => {
                                log_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-event") => {
                                wait_event_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let wait_on_wait_variables_index =
        wait_on_wait_variables_index.expect("wait-on-wait-variables import");
    let log_event_index = log_event_index.expect("log-event import");
    let log_index = log_index.expect("log import");
    let wait_event_index = wait_event_index.expect("wait-event import");
    let wait_on_wait_variables_position = run_calls
        .iter()
        .position(|&index| index == wait_on_wait_variables_index)
        .expect("wait-on-wait variables call");
    let log_event_position = run_calls
        .iter()
        .position(|&index| index == log_event_index)
        .expect("Log event call");
    let log_position = run_calls
        .iter()
        .position(|&index| index == log_index)
        .expect("Log step call");
    let wait_event_position = run_calls
        .iter()
        .position(|&index| index == wait_event_index)
        .expect("WaitForSignal event call");

    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        5,
        "WaitForSignal onWait should build initial, callback, restored parent, and resumed sources"
    );
    assert!(
        wait_on_wait_variables_position < log_event_position,
        "onWait variables must be prepared before executing the callback"
    );
    assert!(
        log_event_position < log_position,
        "Log callback should emit its event before updating nested steps"
    );
    assert!(
        log_position < wait_event_position,
        "onWait callback should complete before external input is requested"
    );
}

#[test]
fn direct_core_run_wraps_wait_on_wait_error_before_runtime_fail() {
    let graph = fixture("wait_on_wait_error");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::WaitForSignal {
        on_wait_plan: Some(on_wait_plan),
        ..
    } = &core_config.run_plan
    else {
        panic!("expected WaitForSignal run plan with onWait callback");
    };
    let DirectRunPlan::Error { step_id, .. } = on_wait_plan.as_ref() else {
        panic!("expected onWait callback to fail with Error");
    };
    assert_eq!(step_id, "fail");

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("WaitForSignal failing onWait core module validates");

    let mut next_function_index = 0;
    let mut error_index = None;
    let mut wait_on_wait_error_index = None;
    let mut runtime_fail_index = None;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "error") => {
                                error_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "wait-on-wait-error") => {
                                wait_on_wait_error_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                runtime_fail_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        if let Operator::Call { function_index } = operator.expect("operator") {
                            run_calls.push(function_index);
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let error_index = error_index.expect("error import");
    let wait_on_wait_error_index = wait_on_wait_error_index.expect("wait-on-wait-error import");
    let runtime_fail_index = runtime_fail_index.expect("runtime fail import");
    let error_position = run_calls
        .iter()
        .position(|&index| index == error_index)
        .expect("Error failure payload call");
    let wait_on_wait_error_position = run_calls
        .iter()
        .enumerate()
        .filter_map(|(position, &index)| {
            (index == wait_on_wait_error_index && position > error_position).then_some(position)
        })
        .next()
        .expect("onWait failure wrapper call after nested Error payload");
    let runtime_fail_position = run_calls
        .iter()
        .enumerate()
        .filter_map(|(position, &index)| {
            (index == runtime_fail_index && position > wait_on_wait_error_position)
                .then_some(position)
        })
        .next()
        .expect("runtime fail call after onWait wrapper");

    assert!(
        wait_on_wait_error_position < runtime_fail_position,
        "onWait wrapper should feed runtime.fail"
    );
}

#[test]
fn direct_core_run_lowers_filter_finish_through_stdlib() {
    let graph = fixture("filter");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Filter {
        filter_id,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected Filter run plan");
    };
    let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
        panic!("expected Filter to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Filter core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut filter_index = None;
    let mut apply_mapping_index = None;
    let mut saw_filter_id = false;
    let mut saw_mapping_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "filter") => {
                                filter_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *filter_id as i32 {
                                    saw_filter_id = true;
                                }
                                if value == *mapping_id as i32 {
                                    saw_mapping_id = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let filter_index = filter_index.expect("filter import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "Filter run should rebuild source after updating steps context"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == filter_index)
            .count(),
        1,
        "Filter run should call the stdlib Filter helper once"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        1,
        "Filter run should apply the terminal Finish mapping once"
    );
    assert!(saw_filter_id, "Filter id should be passed to stdlib");
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
}

#[test]
fn direct_core_run_lowers_value_switch_finish_through_stdlib() {
    let graph = fixture("switch_value");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::SwitchValue {
        switch_id,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected value Switch run plan");
    };
    let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
        panic!("expected value Switch to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("value Switch core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut value_switch_index = None;
    let mut apply_mapping_index = None;
    let mut saw_switch_id = false;
    let mut saw_mapping_id = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "value-switch") => {
                                value_switch_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *switch_id as i32 {
                                    saw_switch_id = true;
                                }
                                if value == *mapping_id as i32 {
                                    saw_mapping_id = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let value_switch_index = value_switch_index.expect("value-switch import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "value Switch run should rebuild source after updating steps context"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == value_switch_index)
            .count(),
        1,
        "value Switch run should call the stdlib value-switch helper once"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        1,
        "value Switch run should apply the terminal Finish mapping once"
    );
    assert!(saw_switch_id, "Switch id should be passed to stdlib");
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
}

#[test]
fn direct_core_run_lowers_routing_switch_finish_through_stdlib() {
    let graph = fixture("switch_routing");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::SwitchRoute {
        switch_id,
        branches,
        default_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected routing Switch run plan");
    };
    assert_eq!(
        branches
            .iter()
            .map(|branch| branch.label.as_str())
            .collect::<Vec<_>>(),
        vec!["active", "pending"]
    );
    let DirectRunPlan::Finish {
        mapping_id: default_mapping_id,
        ..
    } = default_plan.as_ref()
    else {
        panic!("expected routing Switch default branch to Finish");
    };
    let mut mapping_ids = branches
        .iter()
        .map(|branch| match branch.plan.as_ref() {
            DirectRunPlan::Finish { mapping_id, .. } => *mapping_id,
            other => panic!("expected routing Switch branch to Finish, got {other:?}"),
        })
        .collect::<Vec<_>>();
    mapping_ids.push(*default_mapping_id);

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("routing Switch core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut process_switch_index = None;
    let mut value_switch_index = None;
    let mut apply_mapping_index = None;
    let mut saw_switch_id = false;
    let mut seen_mapping_ids = Vec::new();
    let mut saw_active_label_len = false;
    let mut saw_pending_label_len = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "process-switch") => {
                                process_switch_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "value-switch") => {
                                value_switch_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *switch_id as i32 {
                                    saw_switch_id = true;
                                }
                                if mapping_ids.contains(&(value as u32)) {
                                    seen_mapping_ids.push(value as u32);
                                }
                                saw_active_label_len |= value == "active".len() as i32;
                                saw_pending_label_len |= value == "pending".len() as i32;
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let process_switch_index = process_switch_index.expect("process-switch import");
    let value_switch_index = value_switch_index.expect("value-switch import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "routing Switch run should rebuild source after updating steps context"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == process_switch_index)
            .count(),
        1,
        "routing Switch run should call process-switch once"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == value_switch_index)
            .count(),
        1,
        "routing Switch run should call value-switch once"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        3,
        "routing Switch run should apply one Finish mapping per route leaf"
    );
    mapping_ids.sort_unstable();
    seen_mapping_ids.sort_unstable();
    seen_mapping_ids.dedup();
    assert_eq!(seen_mapping_ids, mapping_ids);
    assert!(saw_switch_id, "Switch id should be passed to stdlib");
    assert!(
        saw_active_label_len,
        "active route comparison should be emitted"
    );
    assert!(
        saw_pending_label_len,
        "pending route comparison should be emitted"
    );
}

#[test]
fn direct_core_run_lowers_log_finish_through_stdlib_and_runtime() {
    let graph = fixture("log");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Log {
        log_id: first_log_id,
        next_plan,
        ..
    } = &core_config.run_plan
    else {
        panic!("expected first Log run plan");
    };
    let DirectRunPlan::Log {
        log_id: second_log_id,
        next_plan,
        ..
    } = next_plan.as_ref()
    else {
        panic!("expected second Log run plan");
    };
    let DirectRunPlan::Finish { mapping_id, .. } = next_plan.as_ref() else {
        panic!("expected Log chain to flow into Finish");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Log core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut log_event_index = None;
    let mut log_index = None;
    let mut custom_event_index = None;
    let mut apply_mapping_index = None;
    let mut saw_first_log_id = false;
    let mut saw_second_log_id = false;
    let mut saw_mapping_id = false;
    let mut saw_workflow_log_kind = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "log-event") => {
                                log_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "log") => {
                                log_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => run_calls.push(function_index),
                            Operator::I32Const { value } => {
                                if value == *first_log_id as i32 {
                                    saw_first_log_id = true;
                                }
                                if value == *second_log_id as i32 {
                                    saw_second_log_id = true;
                                }
                                if value == *mapping_id as i32 {
                                    saw_mapping_id = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            Payload::DataSection(reader) => {
                for data in reader {
                    let data = data.expect("data segment");
                    saw_workflow_log_kind |= data.data == DIRECT_WORKFLOW_LOG_KIND;
                }
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let log_event_index = log_event_index.expect("log-event import");
    let log_index = log_index.expect("log import");
    let custom_event_index = custom_event_index.expect("custom-event import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        3,
        "Log chain should build initial source and rebuild after each Log step"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == log_event_index)
            .count(),
        2,
        "Log chain should build one event payload per Log step"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == log_index)
            .count(),
        2,
        "Log chain should update steps context once per Log step"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == custom_event_index)
            .count(),
        2,
        "Log chain should emit one runtime custom event per Log step"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        1,
        "Log chain should apply the terminal Finish mapping once"
    );
    assert!(saw_first_log_id, "first Log id should be passed to stdlib");
    assert!(
        saw_second_log_id,
        "second Log id should be passed to stdlib"
    );
    assert!(
        saw_mapping_id,
        "Finish mapping id should be passed to stdlib"
    );
    assert!(
        saw_workflow_log_kind,
        "workflow_log custom-event kind should be static data"
    );
}

#[test]
fn direct_core_run_lowers_error_through_stdlib_and_runtime() {
    let graph = fixture("error");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Error { error_id, .. } = &core_config.run_plan else {
        panic!("expected Error run plan");
    };

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("Error core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut error_event_index = None;
    let mut error_index = None;
    let mut custom_event_index = None;
    let mut fail_index = None;
    let mut complete_index = None;
    let mut saw_error_id = false;
    let mut saw_workflow_error_kind = false;
    let mut saw_failed_run_return = false;
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "error-event") => {
                                error_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "error") => {
                                error_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "custom-event") => {
                                custom_event_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "fail") => {
                                fail_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                complete_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    let mut previous_was_failure_const = false;
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => {
                                run_calls.push(function_index);
                                previous_was_failure_const = false;
                            }
                            Operator::I32Const { value } => {
                                if value == *error_id as i32 {
                                    saw_error_id = true;
                                }
                                previous_was_failure_const = value == 1;
                            }
                            Operator::Return if previous_was_failure_const => {
                                saw_failed_run_return = true;
                                previous_was_failure_const = false;
                            }
                            _ => previous_was_failure_const = false,
                        }
                    }
                }
                code_body_index += 1;
            }
            Payload::DataSection(reader) => {
                for data in reader {
                    let data = data.expect("data segment");
                    saw_workflow_error_kind |= data.data == DIRECT_WORKFLOW_ERROR_KIND;
                }
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let error_event_index = error_event_index.expect("error-event import");
    let error_index = error_index.expect("error import");
    let custom_event_index = custom_event_index.expect("custom-event import");
    let fail_index = fail_index.expect("fail import");
    let complete_index = complete_index.expect("complete import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        1,
        "Error run should build the source once"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == error_event_index)
            .count(),
        1,
        "Error run should build one event payload"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == custom_event_index)
            .count(),
        1,
        "Error run should emit one custom event"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == error_index)
            .count(),
        1,
        "Error run should build one failure payload"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == fail_index)
            .count(),
        4,
        "Error run should emit runtime.fail four times: one terminal fail for the \
         Error step plus the three fail-on-error guards after init-manifest, \
         load-input, and build-source (each guarded by an `if error` block)"
    );
    assert!(
        run_calls
            .iter()
            .position(|&index| index == fail_index)
            .expect("runtime.fail call")
            < run_calls
                .iter()
                .position(|&index| index == complete_index)
                .expect("runtime.complete call"),
        "runtime.fail should be emitted before the unreachable completion tail"
    );
    assert!(saw_error_id, "Error id should be passed to stdlib");
    assert!(
        saw_workflow_error_kind,
        "workflow_error custom-event kind should be static data"
    );
    assert!(
        saw_failed_run_return,
        "Error lowering should return a failed wasi:cli/run result after runtime.fail"
    );
}

#[test]
fn direct_core_run_lowers_edge_conditions_through_stdlib() {
    let graph = fixture("edge_condition");
    let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
    let manifest_json = manifest.to_canonical_json().expect("manifest json");
    let core_config = DirectCoreConfig::new(&manifest, &manifest_json, false).expect("core config");
    let DirectRunPlan::Log { next_plan, .. } = &core_config.run_plan else {
        panic!("expected Log entry run plan");
    };
    let DirectRunPlan::EdgeRoute {
        branches,
        default_plan,
        ..
    } = next_plan.as_ref()
    else {
        panic!("expected edge-condition route plan");
    };
    assert_eq!(
        branches
            .iter()
            .map(|branch| branch.condition_id)
            .collect::<Vec<_>>(),
        vec![1, 0],
        "conditioned edges should be checked by descending priority"
    );
    let mut mapping_ids = branches
        .iter()
        .map(|branch| match branch.plan.as_ref() {
            DirectRunPlan::Finish { mapping_id, .. } => *mapping_id,
            other => panic!("expected conditioned edge branch to Finish, got {other:?}"),
        })
        .collect::<Vec<_>>();
    let DirectRunPlan::Finish {
        mapping_id: default_mapping_id,
        ..
    } = default_plan.as_ref()
    else {
        panic!("expected edge-condition default branch to Finish");
    };
    mapping_ids.push(*default_mapping_id);

    let (resolve, world) = build_direct_component_resolve().expect("resolve");
    let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
    Validator::new()
        .validate_all(&core)
        .expect("edge-condition core module validates");

    let mut next_function_index = 0;
    let mut build_source_index = None;
    let mut eval_condition_index = None;
    let mut apply_mapping_index = None;
    let mut seen_mapping_ids = Vec::new();
    let mut run_calls = Vec::new();
    let mut code_body_index = 0;

    for payload in Parser::new(0).parse_all(&core) {
        match payload.expect("core wasm payload") {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("core import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        match (import.module, import.name) {
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                build_source_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "eval-condition") => {
                                eval_condition_index = Some(next_function_index)
                            }
                            ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                apply_mapping_index = Some(next_function_index)
                            }
                            _ => {}
                        }
                        next_function_index += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if code_body_index == 0 {
                    for operator in body.get_operators_reader().expect("operators") {
                        match operator.expect("operator") {
                            Operator::Call { function_index } => {
                                run_calls.push(function_index);
                            }
                            Operator::I32Const { value } => {
                                if mapping_ids.contains(&(value as u32)) {
                                    seen_mapping_ids.push(value as u32);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                code_body_index += 1;
            }
            _ => {}
        }
    }

    let build_source_index = build_source_index.expect("build-source import");
    let eval_condition_index = eval_condition_index.expect("eval-condition import");
    let apply_mapping_index = apply_mapping_index.expect("apply-mapping import");
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == build_source_index)
            .count(),
        2,
        "edge-condition Log chain should build initial source and rebuild after Log"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == eval_condition_index)
            .count(),
        2,
        "edge-condition dispatch should evaluate both conditioned edges"
    );
    assert_eq!(
        run_calls
            .iter()
            .filter(|&&index| index == apply_mapping_index)
            .count(),
        3,
        "edge-condition dispatch should emit one Finish mapping per possible leaf"
    );
    mapping_ids.sort_unstable();
    seen_mapping_ids.sort_unstable();
    seen_mapping_ids.dedup();
    assert_eq!(seen_mapping_ids, mapping_ids);
}

#[test]
fn direct_compile_writes_component_scaffold_sidecars() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "simple".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct compile should succeed");

    let world_wit = fs::read_to_string(&result.world_wit_path).expect("world wit");
    let wac = fs::read_to_string(&result.wac_path).expect("wac");

    assert_eq!(world_wit, result.component_artifacts.world_wit);
    assert_eq!(wac, result.component_artifacts.wac_source);
    assert!(world_wit.contains("import runtara:workflow-stdlib/json@0.1.0;"));
    assert!(world_wit.contains("import runtara:workflow-runtime/runtime@0.1.0;"));
    assert!(world_wit.contains("export wasi:cli/run@0.2.3;"));
    assert!(wac.contains("new runtara:workflow-stdlib"));
    assert!(wac.contains("new runtara:workflow-runtime"));
    assert!(wac.contains("new runtara:workflow-logic"));
    assert!(wac.contains("export wf...;"));
}

#[test]
fn direct_compile_composes_finish_with_shared_components_when_available() {
    if !tool_installed("wac") {
        eprintln!("SKIP: wac not installed. `cargo install wac-cli --locked` first.");
        return;
    }
    let Some(components_dir) = shared_components_dir() else {
        return;
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let mut result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "simple".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("simple"),
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct compile should succeed");

    let composed = compose_direct_workflow(&mut result, &components_dir)
        .expect("direct workflow composition should succeed");
    let wasm = fs::read(&composed).expect("composed wasm");
    assert!(!wasm.is_empty());
    assert_eq!(composed, result.build_dir.join("workflow.wasm"));
    assert_eq!(result.wasm_path, composed);
    assert_eq!(
        result.composed_wasm_path.as_deref(),
        Some(composed.as_path())
    );
    assert_eq!(result.wasm_size, wasm.len());
    assert_eq!(result.composed_wasm_size, Some(wasm.len()));
    assert_eq!(
        result.composed_wasm_checksum.as_deref(),
        Some(result.wasm_checksum.as_str())
    );
    assert_eq!(
        result.workflow_logic_wasm_path,
        result.build_dir.join("workflow-logic.wasm")
    );
    assert!(result.workflow_logic_wasm_path.exists());
    assert_eq!(
        result
            .artifact_metadata
            .composed_wasm
            .as_ref()
            .map(|file| file.sha256.as_str()),
        Some(result.wasm_checksum.as_str())
    );
    assert_eq!(result.artifact_metadata.shared_components.len(), 2);
    for component in &result.artifact_metadata.shared_components {
        let wasm = component.wasm.as_ref().expect("resolved shared component");
        let actual =
            fs::read(components_dir.join(&component.wasm_filename)).expect("shared component wasm");
        assert_eq!(wasm.sha256, sha256_hex(&actual));
        assert_eq!(wasm.size_bytes, actual.len() as u64);
        if components_dir.join(&component.meta_filename).exists() {
            assert!(
                component.meta.is_some(),
                "existing component metadata sidecar should be captured"
            );
        }
    }
    let metadata: DirectArtifactMetadata =
        serde_json::from_slice(&fs::read(&result.artifact_metadata_path).expect("metadata"))
            .expect("artifact metadata json");
    assert_eq!(metadata, result.artifact_metadata);
    Validator::new()
        .validate_all(&wasm)
        .expect("composed direct workflow should validate");
}

#[test]
fn direct_compile_composition_rejects_stale_component_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "simple".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: fixture("simple"),
        child_workflows: vec![],
        output_dir: temp.path().join("out"),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct compile should succeed");
    let component = &result.component_artifacts.shared_components[0];
    fs::write(
        temp.path().join(component.bundle_wasm_filename),
        b"component",
    )
    .expect("dummy shared component");
    fs::write(
        temp.path().join(component.bundle_meta_filename),
        serde_json::json!({
            "schemaVersion": 1,
            "kind": "workflow-component",
            "package": component.package,
            "witVersion": "0.1.0",
            "crate": "dummy",
            "crateVersion": "0.0.0",
            "wasm": component.bundle_wasm_filename,
            "sha256": "not-the-real-sha",
            "sizeBytes": 9
        })
        .to_string(),
    )
    .expect("stale shared metadata");

    let err = compose_direct_workflow(&mut result, temp.path())
        .expect_err("stale component metadata should fail before wac");
    let DirectCompileError::Component(message) = err else {
        panic!("expected component metadata error");
    };
    assert!(message.contains("declares sha256"));
    assert!(message.contains("actual"));
}

#[test]
fn direct_compile_composition_reports_missing_agent_component() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: non_durable_agent_graph(),
        child_workflows: vec![],
        output_dir: temp.path().join("out"),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct agent compile should succeed");
    for component in &result.component_artifacts.shared_components {
        fs::write(
            temp.path().join(component.bundle_wasm_filename),
            b"placeholder",
        )
        .expect("dummy shared component");
    }

    let err = compose_direct_workflow(&mut result, temp.path())
        .expect_err("missing agent component should fail before wac");
    let DirectCompileError::Io(err) = err else {
        panic!("expected missing agent component IO error");
    };
    let message = err.to_string();
    assert!(message.contains("direct agent component `utils` missing"));
    assert!(message.contains("runtara_agent_utils.wasm"));
}

#[test]
fn direct_compile_composed_returns_final_workflow_wasm_when_available() {
    if !tool_installed("wac") {
        eprintln!("SKIP: wac not installed. `cargo install wac-cli --locked` first.");
        return;
    }
    let Some(components_dir) = shared_components_dir() else {
        return;
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: fixture("simple"),
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            connection_integration_ids: std::collections::HashMap::new(),
        },
        &components_dir,
    )
    .expect("direct composed compile should succeed");

    assert_eq!(result.wasm_path, result.build_dir.join("workflow.wasm"));
    assert_eq!(
        result.workflow_logic_wasm_path,
        result.build_dir.join("workflow-logic.wasm")
    );
    assert_eq!(
        result.composed_wasm_path.as_deref(),
        Some(result.wasm_path.as_path())
    );
    assert!(result.wasm_path.exists());
    assert!(result.workflow_logic_wasm_path.exists());

    let wasm = fs::read(&result.wasm_path).expect("composed wasm");
    assert_eq!(result.wasm_size, wasm.len());
    assert!(result.artifact_metadata.composed_wasm.is_some());
    assert!(
        result
            .artifact_metadata
            .shared_components
            .iter()
            .all(|component| component.wasm.is_some())
    );
    Validator::new()
        .validate_all(&wasm)
        .expect("composed direct workflow should validate");
}

#[test]
fn direct_compile_rejects_unsupported_graphs_before_writing_artifacts() {
    // Parallel fan-out to two distinct Finish steps (multiple unconditioned
    // normal edges that never re-converge) is a permanently-invalid graph — an
    // ambiguous exit the shared validation layer rejects (E073). It is a stable
    // choice for asserting direct's defense-in-depth unsupported-graph rejection,
    // since it will never become supported as step features are lowered.
    let graph: ExecutionGraph = serde_json::from_value(serde_json::json!({
        "steps": {
            "log": { "stepType": "Log", "id": "log", "message": "fanout" },
            "finish_a": { "stepType": "Finish", "id": "finish_a" },
            "finish_b": { "stepType": "Finish", "id": "finish_b" }
        },
        "entryPoint": "log",
        "executionPlan": [
            { "fromStep": "log", "toStep": "finish_a" },
            { "fromStep": "log", "toStep": "finish_b" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("graph parses");

    let temp = tempfile::tempdir().expect("tempdir");
    let err = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "parallel-fanout".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect_err("parallel fan-out is not supported in direct mode");

    let DirectCompileError::Unsupported { report } = err else {
        panic!("expected unsupported error");
    };
    assert!(!report.supported);
    assert!(
        report
            .unsupported
            .iter()
            .any(|feature| feature.feature == "execution-plan-routing")
    );
    assert!(
        fs::read_dir(temp.path())
            .expect("temp dir")
            .next()
            .is_none(),
        "unsupported graphs should not create build output"
    );
}

#[test]
fn direct_compile_supports_single_agent_without_finish() {
    // A workflow that is a single Agent step with no Finish and no edges (the
    // agent is both entry point and terminal). The generated compiler returns
    // `Ok(Value::Null)` for a graph with no Finish; direct must compile it via
    // an implicit finish rather than erroring "missing normal branch".
    let graph: ExecutionGraph = serde_json::from_value(serde_json::json!({
        "steps": {
            "agent": {
                "stepType": "Agent",
                "id": "agent",
                "name": "Random Double",
                "agentId": "utils",
                "capabilityId": "random-double",
                "maxRetries": 1,
                "retryDelay": 1000
            }
        },
        "entryPoint": "agent",
        "executionPlan": [],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("graph parses");

    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "single-agent-no-finish".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("single-agent-no-finish should compile direct (implicit finish)");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("implicit-finish artifact should validate");
    assert!(
        result.support_report.supported,
        "single agent without a Finish must lower directly: {:?}",
        result.support_report.unsupported
    );
}

#[test]
fn direct_compile_supports_agent_chain_without_finish() {
    // A chain of two Agent steps with no Finish: the first agent flows into the
    // second (`next` edge), and the second is terminal. Unlike the single-agent
    // case (which slips through because there are no edges to flag), the chain
    // has an edge — so a too-strict support gate reports it as
    // `execution-plan-routing`. The terminal Agent must instead lower as an
    // implicit finish (workflow output `null`), matching the generated compiler.
    let graph: ExecutionGraph = serde_json::from_value(serde_json::json!({
        "steps": {
            "first": {
                "stepType": "Agent",
                "id": "first",
                "name": "List Owners",
                "agentId": "utils",
                "capabilityId": "random-double",
                "maxRetries": 1,
                "retryDelay": 1000
            },
            "second": {
                "stepType": "Agent",
                "id": "second",
                "name": "List Brands",
                "agentId": "utils",
                "capabilityId": "random-double",
                "maxRetries": 1,
                "retryDelay": 1000
            }
        },
        "entryPoint": "first",
        "executionPlan": [
            { "fromStep": "first", "toStep": "second", "label": "next" }
        ],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    }))
    .expect("graph parses");

    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "agent-chain-no-finish".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("agent-chain-no-finish should compile direct (implicit finish)");

    let wasm = fs::read(&result.wasm_path).expect("wasm");
    Validator::new()
        .validate_all(&wasm)
        .expect("implicit-finish artifact should validate");
    assert!(
        result.support_report.supported,
        "agent chain without a Finish must lower directly: {:?}",
        result.support_report.unsupported
    );
}
