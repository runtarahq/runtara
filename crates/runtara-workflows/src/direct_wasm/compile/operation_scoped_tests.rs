// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hermetic compiler classification of operation-scoped (suspending or
//! control) Agent sites: the `check_sites` backstops, byte-identical
//! manifests without such sites, and the serialized parallel windows.
use super::*;
use crate::direct_wasm::component::WorkflowAbi;
use crate::direct_wasm::plan::plan_contains_operation_scoped;
use runtara_dsl::agent_meta::AgentCatalog;
use serde_json::{Value, json};
use std::sync::Arc;

/// `control` (start, get), `waiter` (plain; pause suspends), `utils`.
fn catalog() -> Arc<AgentCatalog> {
    let capability = |id: &str, suspends: bool| {
        json!({"id": id, "name": id, "inputType": "Input", "inputs": [],
            "output": {"type": "object"}, "hasSideEffects": false, "isIdempotent": true,
            "rateLimited": false, "suspends": suspends})
    };
    let agent = |id: &str, capabilities: Vec<Value>| {
        json!({"id": id, "name": id, "description": "fixture", "hasSideEffects": false,
            "supportsConnections": false, "integrationIds": [], "capabilities": capabilities})
    };
    Arc::new(
        AgentCatalog::from_json(
            &json!([
                agent(
                    "control",
                    vec![capability("start", false), capability("get", false)]
                ),
                agent(
                    "waiter",
                    vec![capability("plain", false), capability("pause", true)]
                ),
                agent("utils", vec![capability("plain", false)]),
            ])
            .to_string(),
        )
        .expect("fixture catalog"),
    )
}

fn agent(id: &str, agent: &str, capability: &str) -> Value {
    json!({"id": id, "stepType": "Agent", "agentId": agent, "capabilityId": capability,
        "maxRetries": 0, "timeout": 60_000})
}

fn single(step: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_string();
    json!({"entryPoint": id, "steps": {id.clone(): step,
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

fn input(graph: Value, dir: &Path, catalog: Option<Arc<AgentCatalog>>) -> DirectCompilationInput {
    DirectCompilationInput {
        workflow_id: "scoped".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).expect("graph"),
        child_workflows: vec![],
        output_dir: dir.into(),
        track_events: false,
        agent_catalog: catalog,
        agent_slug: None,
    }
}

/// Compile into a fresh directory kept for the rest of the test.
fn compile(graph: Value, abi: WorkflowAbi) -> Result<DirectCompilationResult, DirectCompileError> {
    thread_local! {
        static DIRS: std::cell::RefCell<Vec<tempfile::TempDir>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let result =
        compile_direct_workflow_with_abi(input(graph, dir.path(), Some(catalog())), abi, false);
    DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    result
}

fn refusal(result: Result<DirectCompilationResult, DirectCompileError>) -> String {
    result.map(|_| ()).expect_err("refused").to_string()
}

#[test]
fn control_sites_compile_under_the_invoke_abi_and_are_marked_in_the_manifest() {
    let result = compile(
        single(agent("get", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("a top-level control read compiles");
    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    let site = &manifest.graph.agents[0];
    assert!(site.operation_scoped && !site.suspends);
    let json: Value = serde_json::from_slice(&manifest.to_canonical_json().unwrap()).unwrap();
    assert_eq!(json["graph"]["agents"][0]["operationScoped"], true);

    // Ordinary sites carry neither flag, so their manifests are unchanged.
    let result = compile(
        single(agent("plain", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    let json: Value = serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    let site = &json["graph"]["agents"][0];
    assert!(site.get("operationScoped").is_none() && site.get("suspends").is_none());
}

#[test]
fn the_backstops_refuse_what_validation_reports() {
    let control = || single(agent("get", "control", "get"));
    let text = refusal(compile(control(), WorkflowAbi::AgentCapabilities));
    assert!(
        text.contains("cannot be published as a workflow-agent"),
        "{text}"
    );
    let text = refusal(compile(control(), WorkflowAbi::CliRunHttp));
    assert!(text.contains("CliRunHttp"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let text = refusal(compile_direct_workflow_with_abi(
        input(control(), dir.path(), None),
        WorkflowAbi::InvokeHostImports,
        false,
    ));
    assert!(text.contains("without the agent catalog"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let text = refusal(compile_direct_workflow_with_scoped_agents(
        input(control(), dir.path(), Some(catalog())),
        WorkflowAbi::InvokeHostImports,
        false,
        ["control".to_string()].into(),
    ));
    assert!(text.contains("scoped isolation"), "{text}");

    // A suspending step without durability or a timeout.
    let mut step = agent("wait", "waiter", "pause");
    step["durable"] = json!(false);
    let text = refusal(compile(single(step), WorkflowAbi::InvokeHostImports));
    assert!(text.contains("must be durable"), "{text}");
    let mut step = agent("wait", "waiter", "pause");
    step["timeout"] = Value::Null;
    let text = refusal(compile(single(step), WorkflowAbi::InvokeHostImports));
    assert!(text.contains("needs a timeout"), "{text}");

    // An AiAgent tool, but not the AiAgent's continuation.
    let ai = |label: Option<&str>| {
        let mut edge = json!({"fromStep": "ai", "toStep": "get"});
        if let Some(label) = label {
            edge["label"] = json!(label);
        }
        json!({"entryPoint": "ai", "steps": {
            "ai": {"id": "ai", "stepType": "AiAgent", "connectionId": "llm",
                "config": {"systemPrompt": {"valueType": "immediate", "value": "s"},
                    "userPrompt": {"valueType": "immediate", "value": "u"}}},
            "get": agent("get", "control", "get"),
            "finish": {"id": "finish", "stepType": "Finish"}},
            "executionPlan": [edge, {"fromStep": if label.is_some() { "ai" } else { "get" },
                "toStep": "finish"}]})
    };
    let text = refusal(compile(ai(Some("lookup")), WorkflowAbi::InvokeHostImports));
    assert!(text.contains("AiAgent tool"), "{text}");
    compile(ai(None), WorkflowAbi::InvokeHostImports)
        .expect("a control step after an AiAgent is an ordinary site");
}

/// Compile `graph` with `children` embedded (one per Embed step id).
fn compile_with_children(
    graph: Value,
    children: &[(&str, Value)],
) -> Result<DirectCompilationResult, DirectCompileError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut input = input(graph, dir.path(), Some(catalog()));
    input.child_workflows = children
        .iter()
        .map(|(step, graph)| crate::ChildWorkflowInput {
            step_id: (*step).into(),
            workflow_id: "child".into(),
            version_requested: "latest".into(),
            version_resolved: 1,
            execution_graph: serde_json::from_value(graph.clone()).expect("child graph"),
        })
        .collect();
    let result = compile_direct_workflow_with_abi(input, WorkflowAbi::InvokeHostImports, false);
    std::mem::forget(dir);
    result
}

/// Suspending sites compile wherever the slice-4 context matrix allows them:
/// branch arms, sequential and parallel (serialized) Split bodies, While
/// bodies, embedded workflows, and retrying steps.
#[test]
fn suspending_sites_compile_wherever_the_matrix_allows() {
    let pause = || agent("pause", "waiter", "pause");
    let mut retrying = pause();
    retrying["maxRetries"] = json!(2);
    retrying["retryDelay"] = json!(0);
    let split = |parallelism: u32| {
        single(json!({"id": "loop", "stepType": "Split",
            "config": {"value": {"valueType": "immediate", "value": [1, 2]},
                "parallelism": parallelism},
            "subgraph": single(pause())}))
    };
    let while_loop = single(json!({"id": "loop", "stepType": "While",
        "condition": {"type": "operation", "op": "LT", "arguments": [
            {"valueType": "reference", "value": "loop.index"},
            {"valueType": "immediate", "value": 2}]},
        "subgraph": single(pause())}));
    let branches = json!({"entryPoint": "start", "steps": {
        "start": agent("start", "utils", "plain"), "left": agent("left", "waiter", "pause"),
        "right": agent("right", "utils", "plain"),
        "join": {"id": "join", "stepType": "Finish"}},
        "executionPlan": [
            {"fromStep": "start", "toStep": "left"}, {"fromStep": "start", "toStep": "right"},
            {"fromStep": "left", "toStep": "join"}, {"fromStep": "right", "toStep": "join"}]});
    for (shape, graph) in [
        ("top level", single(pause())),
        ("retrying", single(retrying)),
        ("sequential Split", split(1)),
        ("parallel Split", split(4)),
        ("While", while_loop),
        ("branch arm", branches),
    ] {
        let result = compile(graph, WorkflowAbi::InvokeHostImports)
            .unwrap_or_else(|error| panic!("{shape}: {error}"));
        assert!(
            result.parallel_pools.is_empty(),
            "{shape}: a suspending site never shares a window: {:?}",
            result.parallel_pools
        );
    }
    let embed = single(json!({"id": "embed", "stepType": "EmbedWorkflow",
        "childWorkflowId": "child", "childVersion": "latest"}));
    compile_with_children(embed, &[("embed", single(pause()))])
        .expect("a suspending site in an embedded workflow compiles");
}

/// The backstops that stay: an onWait graph (for either kind, also through an
/// embedded workflow), an onError handler, no durability, no timeout.
#[test]
fn suspending_and_control_sites_are_refused_in_on_wait_and_on_error() {
    let wait = |on_wait: Value| {
        single(json!({"id": "hold", "stepType": "WaitForSignal", "onWait": on_wait}))
    };
    for site in [
        agent("pause", "waiter", "pause"),
        agent("get", "control", "get"),
    ] {
        let text = refusal(compile(wait(single(site)), WorkflowAbi::InvokeHostImports));
        assert!(text.contains("onWait"), "{text}");
    }
    let embed = json!({"id": "embed", "stepType": "EmbedWorkflow",
        "childWorkflowId": "child", "childVersion": "latest"});
    let text = refusal(compile_with_children(
        wait(single(embed)),
        &[("embed", single(agent("pause", "waiter", "pause")))],
    ));
    assert!(text.contains("onWait"), "{text}");

    let on_error = json!({"entryPoint": "plain", "steps": {
        "plain": agent("plain", "utils", "plain"), "pause": agent("pause", "waiter", "pause"),
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": "plain", "toStep": "finish"},
            {"fromStep": "plain", "toStep": "pause", "label": "onError"},
            {"fromStep": "pause", "toStep": "finish"}]});
    let text = refusal(compile(on_error, WorkflowAbi::InvokeHostImports));
    assert!(text.contains("onError"), "{text}");
}

/// Each site binds its own `(agent, interface)` import: a suspending site the
/// agent's `suspendable`, a plain one its `capabilities`, even on one agent.
/// Only artifacts with a suspending site lay out its refusal error.
#[test]
fn per_site_imports_and_the_suspension_error_follow_the_sites() {
    let two_sites = json!({"entryPoint": "plain", "steps": {
        "plain": agent("plain", "waiter", "plain"), "pause": agent("pause", "waiter", "pause"),
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": "plain", "toStep": "pause"},
            {"fromStep": "pause", "toStep": "finish"}]});
    let result = compile(two_sites, WorkflowAbi::InvokeHostImports).expect("compiles");
    let (world, logic) = world_and_logic(&result);
    for interface in ["capabilities", "suspendable"] {
        assert!(
            world.contains(&format!("import runtara:agent-waiter/{interface}@0.4.0;")),
            "{interface}: {world}"
        );
    }
    assert!(contains(&logic, b"AGENT_INVALID_SUSPENSION"));

    let read_only = compile(
        single(agent("get", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    let (world, logic) = world_and_logic(&read_only);
    assert!(!world.contains("suspendable"), "{world}");
    assert!(!contains(&logic, b"AGENT_INVALID_SUSPENSION"));
}

/// A plan node's `operation_scoped` flag, found through a Split body.
fn split_body_is_scoped(result: &DirectCompilationResult) -> bool {
    let manifest: DirectWorkflowManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    let config = DirectCoreConfig::new(&manifest, &manifest.to_canonical_json().unwrap(), false)
        .expect("core config");
    plan_contains_operation_scoped(&config.run_plan)
}

#[test]
fn a_parallel_split_body_with_a_control_step_runs_sequentially() {
    let split = |body: Value| {
        single(json!({"id": "loop", "stepType": "Split",
            "config": {"value": {"valueType": "immediate", "value": [1, 2, 3]}, "parallelism": 4},
            "subgraph": single(body)}))
    };
    let plain = compile(
        split(agent("call", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        plain.parallel_pools.contains_key("utils"),
        "an ordinary body gets a window: {:?}",
        plain.parallel_pools
    );
    assert!(!split_body_is_scoped(&plain));

    let scoped = compile(
        split(agent("call", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        scoped.parallel_pools.is_empty(),
        "a control body is serialized: {:?}",
        scoped.parallel_pools
    );
    assert!(split_body_is_scoped(&scoped));
}

#[test]
fn a_branch_group_with_a_control_step_runs_sequentially() {
    let fan_out = |left: Value| {
        json!({"entryPoint": "start", "steps": {
            "start": agent("start", "utils", "plain"), "left": left,
            "right": agent("right", "utils", "plain"),
            "join": {"id": "join", "stepType": "Finish"}},
            "executionPlan": [
                {"fromStep": "start", "toStep": "left"}, {"fromStep": "start", "toStep": "right"},
                {"fromStep": "left", "toStep": "join"}, {"fromStep": "right", "toStep": "join"}]})
    };
    let plain = compile(
        fan_out(agent("left", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        !plain.parallel_pools.is_empty(),
        "ordinary branches run concurrently"
    );
    let scoped = compile(
        fan_out(agent("left", "control", "get")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    assert!(
        scoped.parallel_pools.is_empty(),
        "a group holding a control step serializes: {:?}",
        scoped.parallel_pools
    );
}

fn world_and_logic(result: &DirectCompilationResult) -> (String, Vec<u8>) {
    (
        fs::read_to_string(&result.world_wit_path).expect("world"),
        fs::read(&result.workflow_logic_wasm_path).expect("workflow logic"),
    )
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A control site imports the operation scope, durable or not, and lays out
/// the `AGENT_OPERATION_SCOPE` step error its refused enter raises.
#[test]
fn control_sites_import_the_operation_scope_even_when_not_durable() {
    for durable in [true, false] {
        let mut graph = single(agent("stop", "control", "get"));
        graph["durable"] = json!(durable);
        let result = compile(graph, WorkflowAbi::InvokeHostImports).expect("compiles");
        let (world, logic) = world_and_logic(&result);
        assert!(
            world.contains(&format!(
                "import {};",
                runtara_workflow_wit::OPERATION_SCOPE_INTERFACE_NAME
            )),
            "durable={durable}: {world}"
        );
        assert!(
            !world.contains("suspendable"),
            "a non-suspending site keeps the capabilities import only: {world}"
        );
        assert!(
            contains(&logic, b"AGENT_OPERATION_SCOPE"),
            "durable={durable}"
        );
        assert!(contains(
            &logic,
            runtara_workflow_wit::OPERATION_SCOPE_INTERFACE_NAME.as_bytes()
        ));
    }
}

/// Workflows without operation-scoped sites keep their exact artifact: no
/// scope import, no scope error in the static data, and a world identical to
/// the one the compiler emitted before scoped sites existed.
#[test]
fn unscoped_workflows_carry_nothing_of_the_operation_scope() {
    let result = compile(
        single(agent("plain", "utils", "plain")),
        WorkflowAbi::InvokeHostImports,
    )
    .expect("compiles");
    let (world, logic) = world_and_logic(&result);
    assert!(!world.contains("workflow-operation"), "{world}");
    assert!(!contains(&logic, b"workflow-operation"));
    assert!(!contains(&logic, b"AGENT_OPERATION_SCOPE"));
    assert!(!result.component_artifacts.operation_scope);
    // The same world as the unscoped emitter produces from its inputs alone.
    let artifacts = crate::direct_wasm::component::emit_direct_component_artifacts_scoped(
        &["utils".to_string()],
        WorkflowAbi::InvokeHostImports,
        false,
        None,
        &Default::default(),
        false,
        &Default::default(),
        &Default::default(),
        false,
        result.component_artifacts.has_timers,
        result.component_artifacts.needs_monotonic_clock,
    );
    assert_eq!(artifacts.world_wit, result.component_artifacts.world_wit);
}

/// (c) The offsets the emitter reads from a `suspendable.invoke` result are
/// the canonical layout of the WIT, and the two agent interfaces are
/// type-identical in their flat signature (which is why (b) is behavioural).
#[test]
fn the_emitted_result_offsets_match_the_wit_layout() {
    use wit_parser::{Int, Resolve, SizeAlign, Type, TypeDefKind};
    let mut resolve = Resolve::default();
    resolve
        .push_str("agent.wit", runtara_agent_wit::RUNTARA_AGENT_WIT)
        .unwrap();
    resolve
        .push_str("agent-suspension.wit", runtara_agent_suspension::WIT)
        .unwrap();
    let package = resolve
        .push_str(
            "probe.wit",
            &agent_wit_package_with_interfaces("suspend-probe", false, true),
        )
        .unwrap();
    let mut sizes = SizeAlign::default();
    sizes.fill(&resolve);
    let at = |offset: wit_parser::ArchitectureSize| offset.size_wasm32() as u64;
    let interfaces = &resolve.packages[package].interfaces;
    let invoke = |interface: &str| &resolve.interfaces[interfaces[interface]].functions["invoke"];

    let variant = |ty: &Type| match ty {
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Variant(variant) => variant.clone(),
            TypeDefKind::Type(Type::Id(inner)) => match &resolve.types[*inner].kind {
                TypeDefKind::Variant(variant) => variant.clone(),
                other => panic!("not a variant: {other:?}"),
            },
            other => panic!("not a variant: {other:?}"),
        },
        other => panic!("not a variant: {other:?}"),
    };
    let Some(Type::Id(result)) = invoke("suspendable").result else {
        panic!("suspendable.invoke returns a result");
    };
    let TypeDefKind::Result(result) = &resolve.types[result].kind else {
        panic!("suspendable.invoke returns a result");
    };
    let result_payload =
        at(sizes.payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()]));
    let outcome_ty = result.ok.expect("an outcome ok arm");
    let outcome = variant(&outcome_ty);
    let outcome_payload = at(sizes.payload_offset(
        outcome.tag(),
        outcome.cases.iter().map(|case| case.ty.as_ref()),
    ));
    assert_eq!(result_payload, DIRECT_AGENT_RESULT_OK_PTR_OFFSET);
    assert_eq!(
        result_payload,
        agent_suspend::OUTCOME_DISCRIMINANT_OFFSET,
        "outcome's discriminant sits at the result payload"
    );
    assert_eq!(
        result_payload + outcome_payload,
        agent_suspend::COMPLETED_PTR_OFFSET
    );
    assert_eq!(
        agent_suspend::COMPLETED_LEN_OFFSET,
        agent_suspend::COMPLETED_PTR_OFFSET + 4
    );

    let Some(Type::Id(suspension)) = outcome.cases[1].ty else {
        panic!("suspended carries the suspension record");
    };
    let suspension = match &resolve.types[suspension].kind {
        TypeDefKind::Record(record) => record.clone(),
        TypeDefKind::Type(Type::Id(inner)) => match &resolve.types[*inner].kind {
            TypeDefKind::Record(record) => record.clone(),
            other => panic!("not a record: {other:?}"),
        },
        other => panic!("not a record: {other:?}"),
    };
    let fields = sizes.field_offsets(suspension.fields.iter().map(|field| &field.ty));
    let base = result_payload + outcome_payload;
    assert_eq!(
        (suspension.fields[0].name.as_str(), base + at(fields[0].0)),
        ("wakes", agent_suspend::SUSPENDED_WAKES_PTR_OFFSET)
    );
    assert_eq!(
        (suspension.fields[1].name.as_str(), base + at(fields[1].0)),
        ("state", agent_suspend::SUSPENDED_STATE_PTR_OFFSET)
    );
    assert_eq!(
        agent_suspend::SUSPENDED_WAKES_LEN_OFFSET,
        agent_suspend::SUSPENDED_WAKES_PTR_OFFSET + 4
    );
    assert_eq!(
        agent_suspend::SUSPENDED_STATE_LEN_OFFSET,
        agent_suspend::SUSPENDED_STATE_PTR_OFFSET + 4
    );

    let Type::Id(wakes) = suspension.fields[0].ty else {
        panic!("wakes is a list");
    };
    let TypeDefKind::List(wake_ty) = &resolve.types[wakes].kind else {
        panic!("wakes is a list");
    };
    let wake = variant(wake_ty);
    assert_eq!(wake.cases[0].name, "at");
    assert_eq!(
        at(sizes.size(wake_ty)),
        agent_suspend::WAKE_SIZE as u64,
        "wake stride"
    );
    assert_eq!(
        at(sizes.payload_offset(wake.tag(), wake.cases.iter().map(|case| case.ty.as_ref()))),
        agent_suspend::WAKE_AT_VALUE_OFFSET
    );

    // The error arm is where the ordinary Agent error path reads it.
    assert_eq!(
        at(sizes.payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()])),
        DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET
    );

    // Type-identical flat signatures: only the result's shape differs.
    let mangling = wit_parser::ManglingAndAbi::Legacy(wit_parser::LiftLowerAbi::AsyncCallback);
    let signature =
        |interface: &str| resolve.wasm_signature(mangling.import_variant(), invoke(interface));
    assert_eq!(
        signature("capabilities").params,
        signature("suspendable").params
    );
    assert_eq!(
        signature("capabilities").results,
        signature("suspendable").results
    );
}
