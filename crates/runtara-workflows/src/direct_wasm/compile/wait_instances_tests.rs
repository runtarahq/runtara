// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hermetic compiler coverage of WaitForInstances: the placement backstops,
//! the `runtara:workflow/waits` import only where a step needs it, the plan
//! classification, and the result offsets the lowering reads.
use super::*;
use crate::direct_wasm::component::WorkflowRole;
use crate::direct_wasm::plan::{plan_contains_operation_scoped, plan_contains_suspension};
use serde_json::{Value, json};

fn wait(id: &str) -> Value {
    json!({"id": id, "stepType": "WaitForInstances",
        "instanceIds": {"valueType": "reference", "value": "data.children"}})
}

fn single(step: Value) -> Value {
    let id = step["id"].as_str().unwrap().to_string();
    json!({"entryPoint": id, "steps": {id.clone(): step,
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": id, "toStep": "finish"}]})
}

fn compile(graph: Value, abi: WorkflowRole) -> Result<DirectCompilationResult, DirectCompileError> {
    thread_local! {
        static DIRS: std::cell::RefCell<Vec<tempfile::TempDir>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "waits".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph).expect("graph"),
            child_workflows: vec![],
            output_dir: dir.path().into(),
            track_events: true,
            agent_catalog: None,
            agent_slug: None,
        },
        abi,
        false,
    );
    DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    result
}

fn refusal(graph: Value, abi: WorkflowRole) -> String {
    compile(graph, abi)
        .map(|_| ())
        .expect_err("refused")
        .to_string()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn a_wait_imports_the_instance_waits_and_nothing_else_does() {
    let result =
        compile(single(wait("wait")), WorkflowRole::Root).expect("a top-level wait compiles");
    let world = fs::read_to_string(&result.world_wit_path).unwrap();
    let logic = fs::read(&result.workflow_logic_wasm_path).unwrap();
    assert!(world.contains(runtara_wit::workflow::WAITS), "{world}");
    assert!(result.component_artifacts.wait_instances);
    assert!(contains(&logic, runtara_wit::workflow::WAITS.as_bytes()));
    // It is not an Agent site: no operation scope.
    assert!(!world.contains("workflow-operation"), "{world}");

    let result = compile(
        single(json!({"id": "log", "stepType": "Log", "message": "hi"})),
        WorkflowRole::Root,
    )
    .expect("compiles");
    let world = fs::read_to_string(&result.world_wit_path).unwrap();
    let logic = fs::read(&result.workflow_logic_wasm_path).unwrap();
    assert!(!world.contains("workflow-wait"), "{world}");
    assert!(!contains(&logic, b"runtara:workflow/waits"));
    assert!(!result.component_artifacts.wait_instances);
}

#[test]
fn the_backstops_refuse_what_validation_reports() {
    let text = refusal(single(wait("wait")), WorkflowRole::PublishedAgent);
    assert!(
        text.contains("cannot be published as a workflow-agent"),
        "{text}"
    );

    let mut graph = single(wait("wait"));
    graph["durable"] = json!(false);
    let text = refusal(graph, WorkflowRole::Root);
    assert!(text.contains("durable"), "{text}");

    let on_error = json!({"entryPoint": "wait", "steps": {
        "wait": wait("wait"), "handler": wait("handler"),
        "finish": {"id": "finish", "stepType": "Finish"}},
        "executionPlan": [{"fromStep": "wait", "toStep": "finish"},
            {"fromStep": "wait", "toStep": "handler", "label": "onError"},
            {"fromStep": "handler", "toStep": "finish"}]});
    let text = refusal(on_error, WorkflowRole::Root);
    assert!(text.contains("onError handler"), "{text}");

    let on_wait = single(json!({"id": "signal", "stepType": "WaitForSignal",
        "onWait": single(wait("inner"))}));
    let text = refusal(on_wait, WorkflowRole::Root);
    assert!(text.contains("onWait"), "{text}");
}

#[test]
fn the_plan_treats_a_wait_as_a_suspending_operation_scoped_step() {
    let manifest = crate::direct_wasm::manifest::build_direct_workflow_manifest(
        &serde_json::from_value(single(wait("wait"))).unwrap(),
    )
    .unwrap();
    assert!(manifest.has_wait_for_instances());
    let plan = crate::direct_wasm::plan::direct_run_plan(&manifest).unwrap();
    assert!(matches!(plan, DirectRunPlan::WaitForInstances { .. }));
    assert!(plan_contains_operation_scoped(&plan));
    assert!(plan_contains_suspension(&plan));
}

/// The offsets the lowering reads from `stdlib.wait-instances-state` are the
/// canonical layout of the WIT.
#[test]
fn the_progress_offsets_match_the_wit_layout() {
    use wit_parser::{Int, Resolve, SizeAlign, Type, TypeDefKind};
    let resolve: Resolve = runtara_wit::resolve().unwrap();
    let stdlib = runtara_package(&resolve, runtara_wit::stdlib::PACKAGE);
    let mut sizes = SizeAlign::default();
    sizes.fill(&resolve);
    let json = &resolve.interfaces[resolve.packages[stdlib].interfaces["json"]];
    let Some(Type::Id(result)) = json.functions["wait-instances-state"].result else {
        panic!("wait-instances-state returns a result");
    };
    let TypeDefKind::Result(result) = &resolve.types[result].kind else {
        panic!("wait-instances-state returns a result");
    };
    let payload = sizes
        .payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()])
        .size_wasm32() as u64;
    let Some(Type::Id(record)) = result.ok else {
        panic!("a record ok arm");
    };
    let TypeDefKind::Record(record) = &resolve.types[record].kind else {
        panic!("a record ok arm");
    };
    let fields = sizes.field_offsets(record.fields.iter().map(|field| &field.ty));
    let offset = |name: &str| {
        let index = record
            .fields
            .iter()
            .position(|field| field.name == name)
            .unwrap();
        payload + fields[index].0.size_wasm32() as u64
    };
    assert_eq!(
        offset("pending"),
        super::wait_instances::PROGRESS_PENDING_OFFSET
    );
    let deadline = offset("deadline-ms");
    assert_eq!(
        deadline,
        super::wait_instances::PROGRESS_DEADLINE_TAG_OFFSET
    );
    // option<u64>: the value follows the tag at its 8-byte alignment.
    assert_eq!(
        deadline + 8,
        super::wait_instances::PROGRESS_DEADLINE_VALUE_OFFSET
    );
    assert_eq!(
        payload,
        super::abi::RETPTR_WIDE_ERR_PTR_OFFSET,
        "an 8-aligned ok arm: the call site moves its error string"
    );
}

fn runtara_package(resolve: &wit_parser::Resolve, name: &str) -> wit_parser::PackageId {
    resolve
        .packages
        .iter()
        .find(|(_, package)| package.name.to_string() == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("{name} is in the resolve"))
}
