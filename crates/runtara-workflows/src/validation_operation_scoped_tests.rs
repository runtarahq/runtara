// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every v1 context rule for operation-scoped steps, in one graph.
use super::*;
use crate::validation::validate_workflow;
use runtara_dsl::step_context_rules::{OperationScopedKind, STEP_CONTEXT_RULES};
use serde_json::{Value, json};

/// `control` (start, get; wait suspends), `waiter` (pause suspends) and an
/// ordinary `utils` agent.
fn catalog() -> AgentCatalog {
    let capability = |id: &str, suspends: bool| {
        json!({"id": id, "name": id, "inputType": "Input", "inputs": [],
            "output": {"type": "object"}, "hasSideEffects": false, "isIdempotent": true,
            "rateLimited": false, "suspends": suspends})
    };
    let agent = |id: &str, capabilities: Vec<Value>| {
        json!({"id": id, "name": id, "description": "fixture", "hasSideEffects": false,
            "supportsConnections": false, "integrationIds": [], "capabilities": capabilities})
    };
    AgentCatalog::from_json(
        &json!([
            agent(
                "control",
                vec![
                    capability("start", false),
                    capability("get", false),
                    capability("wait", true),
                ]
            ),
            agent("waiter", vec![capability("pause", true)]),
            agent("utils", vec![capability("plain", false)]),
        ])
        .to_string(),
    )
    .expect("fixture catalog")
}

/// A durable, timed, non-retrying Agent step.
fn agent(id: &str, agent: &str, capability: &str) -> Value {
    json!({"id": id, "stepType": "Agent", "agentId": agent, "capabilityId": capability,
        "maxRetries": 0, "timeout": 60_000})
}

fn suspending(id: &str) -> Value {
    agent(id, "waiter", "pause")
}

fn control(id: &str) -> Value {
    agent(id, "control", "get")
}

fn finish(id: &str) -> Value {
    json!({"id": id, "stepType": "Finish"})
}

/// `entry -> finish` with `entry` built by the caller.
fn single(step: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_string();
    json!({"entryPoint": id, "steps": {id.clone(): step, "finish": finish("finish")},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

/// The operation-scoped diagnostics as `(code, step id)`, sorted.
fn diagnostics(graph: Value) -> Vec<(&'static str, String)> {
    let graph: ExecutionGraph = serde_json::from_value(graph).expect("graph");
    let result = validate_workflow(&graph, &catalog());
    operation_diagnostics(&result)
}

fn operation_diagnostics(result: &ValidationResult) -> Vec<(&'static str, String)> {
    let mut found: Vec<(&'static str, String)> = result
        .errors
        .iter()
        .filter_map(|error| match error {
            ValidationError::SuspendingCapabilityNotDurable { step_id, .. }
            | ValidationError::SuspendingCapabilityMissingTimeout { step_id, .. }
            | ValidationError::SuspendingCapabilityUnsupportedContext { step_id, .. }
            | ValidationError::ControlCapabilityUnsupportedContext { step_id, .. } => {
                Some((error.code(), step_id.clone()))
            }
            _ => None,
        })
        .chain(result.warnings.iter().filter_map(|warning| match warning {
            ValidationWarning::ConstantRunLabelInLoop { step_id, .. }
            | ValidationWarning::SerializedOperationScopedStep { step_id, .. }
            | ValidationWarning::OperationScopedStepUnderEnclosingRetry { step_id, .. }
            | ValidationWarning::DynamicControlStartTarget { step_id }
            | ValidationWarning::WaitTimeoutBelowDeadlineMargin { step_id, .. } => {
                Some((warning.code(), step_id.clone()))
            }
            _ => None,
        }))
        .collect();
    found.sort();
    found
}

fn one(code: &'static str, step: &str) -> Vec<(&'static str, String)> {
    vec![(code, step.to_string())]
}

#[test]
fn every_rule_code_maps_to_a_diagnostic_with_that_code() {
    let site = Site {
        step_id: "s",
        child_workflow_id: None,
        timeout_ms: Some(10),
    };
    for rule in STEP_CONTEXT_RULES {
        for kind in [
            OperationScopedKind::Suspending,
            OperationScopedKind::Control,
            OperationScopedKind::WaitForInstances,
        ] {
            let mut result = ValidationResult::default();
            apply_rule(
                rule.context,
                &[(kind, "a:c".into())],
                &site,
                Some("owner"),
                &mut result,
            );
            let codes: Vec<&str> = result
                .errors
                .iter()
                .map(|error| {
                    let text = error.to_string();
                    assert!(text.starts_with(&format!("[{}]", error.code())), "{text}");
                    error.code()
                })
                .chain(result.warnings.iter().map(|warning| {
                    let text = warning.to_string();
                    assert!(text.starts_with(&format!("[{}]", warning.code())), "{text}");
                    warning.code()
                }))
                .collect();
            let expected: Vec<&str> = rule.verdict(kind).code().into_iter().collect();
            assert_eq!(codes, expected, "{} / {kind:?}", rule.key);
        }
    }
}

#[test]
fn supported_contexts_are_clean() {
    // Top level, both kinds; a control read needs no timeout or durability.
    assert!(diagnostics(single(suspending("wait"))).is_empty());
    let mut untimed = control("get");
    untimed["timeout"] = Value::Null;
    untimed["durable"] = json!(false);
    assert!(diagnostics(single(untimed)).is_empty());
    // The step's own retries.
    let mut retrying = suspending("wait");
    retrying["maxRetries"] = json!(3);
    assert!(diagnostics(single(retrying)).is_empty());
    // A Conditional arm.
    let graph = json!({"entryPoint": "check", "steps": {
        "check": {"id": "check", "stepType": "Conditional", "condition": {"type": "operation",
            "op": "EQ", "arguments": [{"valueType": "immediate", "value": 1},
            {"valueType": "immediate", "value": 1}]}},
        "wait": suspending("wait"), "get": control("get"), "finish": finish("finish")},
        "executionPlan": [
            {"fromStep": "check", "toStep": "wait", "label": "true"},
            {"fromStep": "check", "toStep": "get", "label": "false"},
            {"fromStep": "wait", "toStep": "finish"}, {"fromStep": "get", "toStep": "finish"}]});
    assert!(diagnostics(graph).is_empty());
    // A sequential Split and a While.
    for (step_type, config) in [
        (
            "Split",
            json!({"value": {"valueType": "immediate", "value": [1]}}),
        ),
        ("While", json!({"maxIterations": 2})),
    ] {
        let mut outer = json!({"id": "loop", "stepType": step_type, "config": config,
            "subgraph": single(suspending("wait"))});
        if step_type == "While" {
            outer["condition"] = json!({"type": "operation", "op": "EQ", "arguments": [
                {"valueType": "immediate", "value": 1}, {"valueType": "immediate", "value": 2}]});
        }
        assert!(diagnostics(single(outer)).is_empty(), "{step_type}");
    }
}

#[test]
fn a_suspending_step_must_be_durable_and_timed() {
    let mut step = suspending("wait");
    step["durable"] = json!(false);
    assert_eq!(diagnostics(single(step)), one("E028", "wait"));

    let mut graph = single(suspending("wait"));
    graph["durable"] = json!(false);
    assert_eq!(diagnostics(graph), one("E028", "wait"));

    // A nested graph inherits its enclosing graph's durability.
    let mut graph = single(json!({"id": "loop", "stepType": "Split",
        "config": {"value": {"valueType": "immediate", "value": [1]}},
        "subgraph": single(suspending("wait"))}));
    graph["durable"] = json!(false);
    assert_eq!(diagnostics(graph), one("E028", "wait"));

    for timeout in [Value::Null, json!(0)] {
        let mut step = suspending("wait");
        step["timeout"] = timeout;
        assert_eq!(diagnostics(single(step)), one("E029", "wait"));
    }
}

#[test]
fn a_timeout_within_the_deadline_margin_warns() {
    for (timeout, warns) in [(500, true), (1_000, true), (1_001, false)] {
        let mut step = suspending("wait");
        step["timeout"] = json!(timeout);
        let expected = if warns { one("W078", "wait") } else { vec![] };
        assert_eq!(diagnostics(single(step)), expected, "{timeout}");
    }
}

#[test]
fn only_suspending_steps_are_rejected_in_the_on_error_region_but_not_after_its_join() {
    // call --onError--> handler --> join; call --> join. The handler is in
    // the region; the join, reached on both paths, is not.
    let graph = |handler: Value, join: Value| {
        json!({"entryPoint": "call", "steps": {
            "call": agent("call", "utils", "plain"), "handler": handler, "join": join,
            "finish": finish("finish")},
            "executionPlan": [
                {"fromStep": "call", "toStep": "handler", "label": "onError"},
                {"fromStep": "handler", "toStep": "join"},
                {"fromStep": "call", "toStep": "join"},
                {"fromStep": "join", "toStep": "finish"}]})
    };
    assert_eq!(
        diagnostics(graph(suspending("handler"), suspending("join"))),
        one("E131", "handler")
    );
    assert!(diagnostics(graph(control("handler"), control("join"))).is_empty());
}

#[test]
fn both_kinds_are_rejected_in_on_wait() {
    let wait = |inner: Value| {
        single(json!({"id": "signal", "stepType": "WaitForSignal", "onWait": single(inner)}))
    };
    assert_eq!(diagnostics(wait(suspending("inner"))), one("E131", "inner"));
    assert_eq!(diagnostics(wait(control("inner"))), one("E132", "inner"));
}

#[test]
fn both_kinds_are_rejected_as_ai_agent_tools_and_memory_but_not_after_one() {
    let graph = |label: &str, target: Value| {
        let id = target["id"].as_str().unwrap().to_string();
        json!({"entryPoint": "ai", "steps": {
            "ai": {"id": "ai", "stepType": "AiAgent", "connectionId": "llm",
                "config": {"systemPrompt": {"valueType": "immediate", "value": "s"},
                    "userPrompt": {"valueType": "immediate", "value": "u"}}},
            id.clone(): target, "finish": finish("finish")},
            "executionPlan": [{"fromStep": "ai", "toStep": id, "label": label},
                {"fromStep": "ai", "toStep": "finish"}]})
    };
    assert_eq!(
        diagnostics(graph("approve", suspending("tool"))),
        one("E131", "tool")
    );
    assert_eq!(
        diagnostics(graph("approve", control("tool"))),
        one("E132", "tool")
    );
    assert_eq!(
        diagnostics(graph("memory", control("mem"))),
        one("E132", "mem")
    );

    // The AiAgent's normal continuation is an ordinary step.
    let after = json!({"entryPoint": "ai", "steps": {
        "ai": {"id": "ai", "stepType": "AiAgent", "connectionId": "llm"},
        "wait": suspending("wait"), "finish": finish("finish")},
        "executionPlan": [{"fromStep": "ai", "toStep": "wait"},
            {"fromStep": "wait", "toStep": "finish"}]});
    assert!(diagnostics(after).is_empty());
}

#[test]
fn parallel_windows_serialize_operation_scoped_steps() {
    let split = |parallelism: u32| {
        single(json!({"id": "loop", "stepType": "Split",
            "config": {"value": {"valueType": "immediate", "value": [1, 2]},
                "parallelism": parallelism},
            "subgraph": single(control("get"))}))
    };
    assert_eq!(diagnostics(split(4)), one("W075", "get"));
    assert_eq!(diagnostics(split(0)), one("W075", "get"));
    assert!(diagnostics(split(1)).is_empty());

    // start fans out to left and right, which rejoin at join.
    let graph = json!({"entryPoint": "start", "steps": {
        "start": agent("start", "utils", "plain"), "left": control("left"),
        "right": agent("right", "utils", "plain"), "join": suspending("join"),
        "finish": finish("finish")},
        "executionPlan": [
            {"fromStep": "start", "toStep": "left"}, {"fromStep": "start", "toStep": "right"},
            {"fromStep": "left", "toStep": "join"}, {"fromStep": "right", "toStep": "join"},
            {"fromStep": "join", "toStep": "finish"}]});
    let graph: ExecutionGraph = serde_json::from_value(graph).unwrap();
    let result = validate_workflow(&graph, &catalog());
    assert_eq!(operation_diagnostics(&result), one("W075", "left"));
    assert!(result.warnings.iter().any(|warning| matches!(warning,
        ValidationWarning::SerializedOperationScopedStep { context, owner_step_id, .. }
            if context == "parallel-branch-group" && owner_step_id == "start")));
}

#[test]
fn a_retrying_split_around_an_operation_scoped_step_warns() {
    let graph = single(json!({"id": "loop", "stepType": "Split",
        "config": {"value": {"valueType": "immediate", "value": [1]}, "maxRetries": 2},
        "subgraph": single(suspending("wait"))}));
    let graph: ExecutionGraph = serde_json::from_value(graph).unwrap();
    let result = validate_workflow(&graph, &catalog());
    assert_eq!(operation_diagnostics(&result), one("W076", "wait"));
    assert!(result.warnings.iter().any(|warning| matches!(warning,
        ValidationWarning::OperationScopedStepUnderEnclosingRetry { retry_step_id, .. }
            if retry_step_id == "loop")));
}

#[test]
fn control_start_warns_on_constant_labels_in_loops_and_dynamic_targets() {
    let start = |label: Value, target: Value| {
        let mut step = agent("start", "control", "start");
        step["inputMapping"] = json!({"workflowId": target, "runLabel": label});
        step
    };
    let literal = |value: &str| json!({"valueType": "immediate", "value": value});
    let in_while = |step: Value| {
        single(
            json!({"id": "loop", "stepType": "While", "subgraph": single(step),
            "condition": {"type": "operation", "op": "EQ", "arguments": [
                {"valueType": "immediate", "value": 1}, {"valueType": "immediate", "value": 2}]}}),
        )
    };
    assert_eq!(
        diagnostics(in_while(start(literal("order-1"), literal("child")))),
        one("W074", "start")
    );
    let per_iteration = json!({"valueType": "template", "value": "order-{{loop.index}}"});
    assert!(diagnostics(in_while(start(per_iteration, literal("child")))).is_empty());
    assert!(diagnostics(single(start(literal("order-1"), literal("child")))).is_empty());
    let dynamic = json!({"valueType": "reference", "value": "data.target"});
    assert_eq!(
        diagnostics(single(start(literal("order-1"), dynamic))),
        one("W077", "start")
    );
}

fn wait_for_instances(id: &str) -> Value {
    json!({"id": id, "stepType": "WaitForInstances",
        "instanceIds": {"valueType": "reference", "value": "data.children"}})
}

#[test]
fn wait_for_instances_follows_the_suspending_placements() {
    // Supported: the top level of a durable workflow, loops and branch arms,
    // with no timeout needed.
    assert!(diagnostics(single(wait_for_instances("wait"))).is_empty());
    let in_loop = single(json!({"id": "loop", "stepType": "Split",
        "config": {"value": {"valueType": "immediate", "value": [1]}},
        "subgraph": single(wait_for_instances("wait"))}));
    assert!(diagnostics(in_loop).is_empty());

    // Not durable.
    let mut graph = single(wait_for_instances("wait"));
    graph["durable"] = json!(false);
    let parsed: ExecutionGraph = serde_json::from_value(graph.clone()).unwrap();
    let result = validate_workflow(&parsed, &catalog());
    assert!(
        result.errors.iter().any(|error| error
            .to_string()
            .contains("is a WaitForInstances step but is not durable")),
        "{:?}",
        result.errors
    );
    assert_eq!(diagnostics(graph), one("E028", "wait"));

    // onError region, onWait, AiAgent tool.
    let on_error = json!({"entryPoint": "call", "steps": {
        "call": agent("call", "utils", "plain"), "handler": wait_for_instances("handler"),
        "finish": finish("finish")},
        "executionPlan": [
            {"fromStep": "call", "toStep": "handler", "label": "onError"},
            {"fromStep": "call", "toStep": "finish"},
            {"fromStep": "handler", "toStep": "finish"}]});
    assert_eq!(diagnostics(on_error), one("E131", "handler"));
    let on_wait = single(json!({"id": "signal", "stepType": "WaitForSignal",
        "onWait": single(wait_for_instances("inner"))}));
    assert_eq!(diagnostics(on_wait), one("E131", "inner"));
    let tool = json!({"entryPoint": "ai", "steps": {
        "ai": {"id": "ai", "stepType": "AiAgent", "connectionId": "llm",
            "config": {"systemPrompt": {"valueType": "immediate", "value": "s"},
                "userPrompt": {"valueType": "immediate", "value": "u"}}},
        "tool": wait_for_instances("tool"), "finish": finish("finish")},
        "executionPlan": [{"fromStep": "ai", "toStep": "tool", "label": "approve"},
            {"fromStep": "ai", "toStep": "finish"}]});
    assert_eq!(diagnostics(tool), one("E131", "tool"));

    // Serialized in a parallel Split, warned under a retrying one.
    let parallel = single(json!({"id": "loop", "stepType": "Split",
        "config": {"value": {"valueType": "immediate", "value": [1, 2]}, "parallelism": 4},
        "subgraph": single(wait_for_instances("wait"))}));
    assert_eq!(diagnostics(parallel), one("W075", "wait"));
    let retrying = single(json!({"id": "loop", "stepType": "Split",
        "config": {"value": {"valueType": "immediate", "value": [1]}, "maxRetries": 2},
        "subgraph": single(wait_for_instances("wait"))}));
    assert_eq!(diagnostics(retrying), one("W076", "wait"));
}

#[test]
fn an_embed_of_a_wait_for_instances_workflow_is_judged_at_its_call_site() {
    let root = |durable: bool, retries: u32| {
        let graph = single(json!({"id": "embed", "stepType": "EmbedWorkflow",
            "childWorkflowId": "child", "childVersion": "latest",
            "durable": durable, "maxRetries": retries}));
        serde_json::from_value::<ExecutionGraph>(graph).unwrap()
    };
    let child = crate::validation::ClosureChildGraph {
        workflow_id: "child".into(),
        version: 1,
        execution_graph: serde_json::from_value(single(wait_for_instances("wait"))).unwrap(),
    };
    let report = crate::validation::validate_workflow_closure(
        "root",
        &root(false, 0),
        &catalog(),
        std::slice::from_ref(&child),
    );
    assert_eq!(operation_diagnostics(&report.root), one("E028", "embed"));
    assert!(
        report.root.errors[0]
            .to_string()
            .contains("holds a WaitForInstances step"),
        "{}",
        report.root.errors[0]
    );
    let report =
        crate::validation::validate_workflow_closure("root", &root(true, 2), &catalog(), &[child]);
    assert_eq!(operation_diagnostics(&report.root), one("W076", "embed"));
}

#[test]
fn literal_wait_for_instances_config_is_checked() {
    let errors = |instance_ids: Value, timeout: Option<Value>| {
        let mut step = wait_for_instances("wait");
        step["instanceIds"] = json!({"valueType": "immediate", "value": instance_ids});
        if let Some(timeout) = timeout {
            step["timeoutMs"] = json!({"valueType": "immediate", "value": timeout});
        }
        let graph: ExecutionGraph = serde_json::from_value(single(step)).unwrap();
        validate_workflow(&graph, &catalog())
            .errors
            .into_iter()
            .filter_map(|error| match error {
                ValidationError::InvalidWaitForInstancesConfig { field, .. } => Some(field),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(errors(json!(["a", "b", "a"]), Some(json!(1_000))).is_empty());
    for bad in [json!([]), json!("a"), json!([1]), json!([""])] {
        assert_eq!(errors(bad.clone(), None), ["instanceIds"], "{bad}");
    }
    let too_many: Vec<String> = (0..=1000).map(|i| format!("child-{i}")).collect();
    assert_eq!(errors(json!(too_many), None), ["instanceIds"]);
    let at_cap: Vec<String> = (0..1000).map(|i| format!("child-{i}")).collect();
    assert!(errors(json!(at_cap), None).is_empty());
    for bad in [json!(0), json!(-5), json!("soon"), json!(1.5)] {
        assert_eq!(
            errors(json!(["a"]), Some(bad.clone())),
            ["timeoutMs"],
            "{bad}"
        );
    }
    // A mode other than all or any does not parse.
    let mut step = wait_for_instances("wait");
    step["mode"] = json!("some");
    assert!(serde_json::from_value::<ExecutionGraph>(single(step)).is_err());
}
