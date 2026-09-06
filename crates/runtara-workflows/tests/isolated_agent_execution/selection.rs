//! Policy selection, real composition, byte parity and mixed execution.
use super::*;
use runtara_workflows::direct_wasm::{
    AgentIsolationPolicy, AgentIsolationReason as Reason, AgentIsolationReview, WorkflowAbi,
    compile_direct_workflow_composed_with_isolation_policy,
};
use serde_json::json;

fn input(graph: Value, dir: &Path) -> DirectCompilationInput {
    DirectCompilationInput {
        workflow_id: "isolated-agent-test".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph).unwrap(),
        child_workflows: vec![],
        output_dir: dir.to_owned(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    }
}

fn approved(components: &Path) -> AgentIsolationPolicy {
    AgentIsolationPolicy {
        enabled: true,
        runtime_supports_inventory_v5: true,
        reviews: selected(components)
            .into_iter()
            .map(|(id, sha256)| (id, review(sha256)))
            .collect(),
    }
}

fn review(sha256: String) -> AgentIsolationReview {
    AgentIsolationReview {
        sha256,
        reset_safe: true,
        compiler_checkpoint_contract: true,
    }
}

fn decisions(result: &DirectCompilationResult) -> BTreeMap<String, Reason> {
    let sidecar: DirectArtifactMetadata =
        serde_json::from_slice(&fs::read(&result.artifact_metadata_path).unwrap()).unwrap();
    assert_eq!(sidecar, result.artifact_metadata);
    let report = sidecar.isolation_selection.unwrap();
    assert_eq!(report.version, 1);
    report
        .agents
        .into_iter()
        .map(|agent| (agent.agent_id, agent.reason))
        .collect()
}

#[test]
fn policy_fallback_preserves_legacy_wasm_bytes_and_reports_each_gate() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let graph = super::super::wasm_performance_baseline::random_chain(2, false);
    let mut legacy = compile(graph.clone(), &dir.path().join("legacy"));
    compose_direct_workflow(&mut legacy, &components).unwrap();
    assert!(
        serde_json::to_value(&legacy.artifact_metadata)
            .unwrap()
            .get("isolationSelection")
            .is_none()
    );
    let legacy_bytes = fs::read(&legacy.wasm_path).unwrap();
    let legacy_logic = fs::read(&legacy.workflow_logic_wasm_path).unwrap();
    for reason in [
        Reason::Disabled,
        Reason::RuntimeUnavailable,
        Reason::UnreviewedPackage,
        Reason::ResetNotApproved,
        Reason::CheckpointContractNotApproved,
        Reason::DigestChanged,
    ] {
        let mut policy = approved(&components);
        match reason {
            Reason::Disabled => policy = AgentIsolationPolicy::default(),
            Reason::RuntimeUnavailable => policy.runtime_supports_inventory_v5 = false,
            Reason::UnreviewedPackage => policy.reviews.clear(),
            Reason::ResetNotApproved => policy.reviews.get_mut("utils").unwrap().reset_safe = false,
            Reason::CheckpointContractNotApproved => {
                policy
                    .reviews
                    .get_mut("utils")
                    .unwrap()
                    .compiler_checkpoint_contract = false
            }
            Reason::DigestChanged => {
                policy.reviews.get_mut("utils").unwrap().sha256 = "0".repeat(64)
            }
            _ => unreachable!(),
        }
        // Unrelated review entries must neither select packages nor read paths.
        policy
            .reviews
            .insert("../../not-a-dependency".into(), review("0".repeat(64)));
        let compiled = compile_direct_workflow_composed_with_isolation_policy(
            input(graph.clone(), &dir.path().join(format!("{reason:?}"))),
            WorkflowAbi::InvokeHostImports,
            false,
            &components,
            &[],
            policy,
            limits(),
        )
        .unwrap();
        assert_eq!(decisions(&compiled), [("utils".into(), reason)].into());
        assert!(compiled.scoped_agents.is_empty());
        assert!(compiled.invocation_manifest.is_none());
        assert!(compiled.artifact_metadata.isolation.is_none());
        assert_eq!(
            fs::read(&compiled.workflow_logic_wasm_path).unwrap(),
            legacy_logic
        );
        assert_eq!(fs::read(&compiled.wasm_path).unwrap(), legacy_bytes);
    }
}

#[test]
fn policy_rechecks_selected_bytes_and_does_not_hide_invalid_artifacts() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let graph = super::super::wasm_performance_baseline::random_chain(1, false);
    let legacy = compile(graph.clone(), &dir.path().join("legacy"));
    // Change only private staged copies, never installed component artifacts.
    for dep in legacy
        .artifact_metadata
        .shared_components
        .iter()
        .chain(&legacy.artifact_metadata.agent_components)
    {
        fs::copy(
            components.join(&dep.wasm_filename),
            dir.path().join(&dep.wasm_filename),
        )
        .unwrap();
        let meta = components.join(&dep.meta_filename);
        if meta.exists() {
            fs::copy(meta, dir.path().join(&dep.meta_filename)).unwrap();
        }
    }
    let reviewed = selected(dir.path());
    let mut compiled = compile_direct_workflow_composed_with_isolation_policy(
        input(graph.clone(), &dir.path().join("selected")),
        WorkflowAbi::InvokeHostImports,
        false,
        dir.path(),
        &[],
        approved(dir.path()),
        limits(),
    )
    .unwrap();
    let original = fs::read(&compiled.wasm_path).unwrap();
    let wasm = dir.path().join("runtara_agent_utils.wasm");
    let mut bytes = fs::read(&wasm).unwrap();
    // Valid, empty-name custom section changes the digest without corrupting WASM.
    bytes.extend_from_slice(&[0, 1, 0]);
    fs::write(&wasm, &bytes).unwrap();
    let sidecar = dir.path().join("runtara_agent_utils.meta.json");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    metadata["sha256"] = artifact_digest(&bytes).into();
    metadata["sizeBytes"] = bytes.len().into();
    fs::write(&sidecar, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let error = compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        dir.path(),
        &[],
        &reviewed,
        limits(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed after isolation selection"),
        "{error}"
    );
    assert_eq!(fs::read(&compiled.wasm_path).unwrap(), original);

    // A disabled policy is not permission to accept an invalid component sidecar.
    metadata["sha256"] = "0".repeat(64).into();
    fs::write(&sidecar, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let error = compile_direct_workflow_composed_with_isolation_policy(
        input(graph, &dir.path().join("disabled")),
        WorkflowAbi::InvokeHostImports,
        false,
        dir.path(),
        &[],
        AgentIsolationPolicy::default(),
        limits(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("declares sha256"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policy_executes_only_approved_packages_in_fresh_stores() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut graph = super::super::wasm_performance_baseline::random_chain(2, false);
    graph["steps"]["r1"]["agentId"] = "datetime".into();
    graph["steps"]["r1"]["capabilityId"] = "get-current-date".into();
    let compiled = compile_direct_workflow_composed_with_isolation_policy(
        input(graph, dir.path()),
        WorkflowAbi::InvokeHostImports,
        false,
        &components,
        &[],
        approved(&components),
        limits(),
    )
    .unwrap();
    assert_eq!(
        decisions(&compiled),
        [
            ("utils".into(), Reason::Isolated),
            ("datetime".into(), Reason::UnreviewedPackage)
        ]
        .into()
    );
    let (exit, starts, _) =
        run_composed(compiled, json!({"data":{},"variables":{}}), true, false, 1).await;
    let output = completed(exit);
    assert!((0.0..1.0).contains(&output["r0"].as_f64().unwrap()));
    assert!(!output["r1"].as_str().unwrap().is_empty());
    assert_eq!(starts, 1);
}

fn stage_child(id: &str, dir: &Path, components: &Path) -> runtara_dsl::agent_meta::AgentInfo {
    let mut child_input = input(
        json!({"durable":false,"entryPoint":"finish","steps":{"finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[]}),
        &dir.join(id),
    );
    child_input.agent_slug = Some(id.into());
    let mut child = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        child_input,
        WorkflowAbi::AgentCapabilities,
        false,
    )
    .unwrap();
    compose_direct_workflow(&mut child, components).unwrap();
    fs::copy(
        child.wasm_path,
        dir.join(format!("runtara_agent_{}.wasm", id.replace('-', "_"))),
    )
    .unwrap();
    let info =
        super::super::certified_workflow_agent_info(id, id, "", &HashMap::new(), &HashMap::new());
    fs::write(
        dir.join(format!("runtara_agent_{}.meta.json", id.replace('-', "_"))),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();
    info
}

#[test]
fn policy_checkpoint_conflicts_include_legacy_packages_and_preserve_unrelated_isolation() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let first = stage_child("scope-one", dir.path(), &components);
    let second = stage_child("scope-two", dir.path(), &components);
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        first, second,
    ]));
    let call = json!({"id":"call","stepType":"Agent","agentId":"scope-one","capabilityId":"run","inputMapping":{}});
    // Check both same-package aliases and an alias with an unreviewed package.
    for other_package in [false, true] {
        for duplicate in [false, true] {
            let nested_id = if duplicate { "call" } else { "nested-call" };
            let mut nested = call.clone();
            nested["id"] = nested_id.into();
            if other_package {
                nested["agentId"] = "scope-two".into();
            }
            let graph = json!({"durable":true,"entryPoint":"random","steps":{
                "random":{"id":"random","stepType":"Agent","agentId":"utils","capabilityId":"random-double","inputMapping":{}},
                "call":call,"wait":{"id":"wait","stepType":"WaitForSignal","onWait":{"durable":false,"entryPoint":nested_id,"steps":{nested_id:nested,"finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":nested_id,"toStep":"finish"}]}},
                "finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":"random","toStep":"call"},{"fromStep":"call","toStep":"wait"},{"fromStep":"wait","toStep":"finish"}]});
            let mut policy = approved(&components);
            policy.reviews.insert(
                "scope-one".into(),
                review(artifact_digest(
                    &fs::read(dir.path().join("runtara_agent_scope_one.wasm")).unwrap(),
                )),
            );
            let mut compilation_input = input(
                graph,
                &dir.path().join(format!("{other_package}-{duplicate}")),
            );
            compilation_input.agent_catalog = Some(catalog.clone());
            let compiled = compile_direct_workflow_composed_with_isolation_policy(
                compilation_input,
                WorkflowAbi::InvokeHostImports,
                false,
                &components,
                &[dir.path().to_owned()],
                policy,
                limits(),
            )
            .unwrap();
            let reasons = decisions(&compiled);
            assert_eq!(
                reasons["scope-one"],
                if duplicate {
                    Reason::CheckpointOverlap
                } else {
                    Reason::Isolated
                }
            );
            assert_eq!(reasons["utils"], Reason::Isolated);
            if other_package {
                assert_eq!(reasons["scope-two"], Reason::UnreviewedPackage);
            }
            assert!(
                compiled
                    .invocation_manifest
                    .as_ref()
                    .unwrap()
                    .checkpoint_conflicts()
                    .is_empty()
            );
            let bytes = fs::read(&compiled.wasm_path).unwrap();
            let package = parse(&bytes, limits()).unwrap().unwrap();
            assert_eq!(package.bindings().len(), if duplicate { 1 } else { 2 });
            assert_eq!(package.invocations(), compiled.invocation_manifest.as_ref());
        }
    }
}

#[test]
fn policy_rejects_grants_overlapping_inline_embed_checkpoints_without_agent_calls() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let info = stage_child("scope-one", dir.path(), &components);
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    for duplicate in [true, false] {
        let embed_id = if duplicate { "call" } else { "other-embed" };
        let graph = json!({"durable":true,"entryPoint":"call","steps":{
            "call":{"id":"call","stepType":"Agent","agentId":"scope-one","capabilityId":"run","inputMapping":{}},
            "wait":{"id":"wait","stepType":"WaitForSignal","onWait":{"durable":false,"entryPoint":embed_id,"steps":{
                embed_id:{"id":embed_id,"stepType":"EmbedWorkflow","childWorkflowId":"inline-child","childVersion":1,"inputMapping":{}},
                "finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":embed_id,"toStep":"finish"}]}},
            "finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[{"fromStep":"call","toStep":"wait"},{"fromStep":"wait","toStep":"finish"}]});
        let mut compilation_input = input(graph, &dir.path().join(format!("inline-{duplicate}")));
        compilation_input.agent_catalog = Some(catalog.clone());
        compilation_input.child_workflows.push(ChildWorkflowInput {
            step_id: embed_id.into(), workflow_id: "inline-child".into(),
            version_requested: "1".into(), version_resolved: 1,
            execution_graph: serde_json::from_value(json!({"durable":true,"entryPoint":"finish","steps":{"finish":{"id":"finish","stepType":"Finish"}},"executionPlan":[]})).unwrap(),
        });
        let mut policy = approved(&components);
        policy.reviews.insert(
            "scope-one".into(),
            review(artifact_digest(
                &fs::read(dir.path().join("runtara_agent_scope_one.wasm")).unwrap(),
            )),
        );
        let compiled = compile_direct_workflow_composed_with_isolation_policy(
            compilation_input.clone(),
            WorkflowAbi::InvokeHostImports,
            false,
            &components,
            &[dir.path().to_owned()],
            policy,
            limits(),
        )
        .unwrap();
        assert_eq!(
            decisions(&compiled)["scope-one"],
            if duplicate {
                Reason::CheckpointOverlap
            } else {
                Reason::Isolated
            }
        );
        if duplicate {
            let actual = fs::read(&compiled.wasm_path).unwrap();
            assert!(compiled.invocation_manifest.is_none());
            assert!(compiled.artifact_metadata.isolation.is_none());
            compilation_input.output_dir = dir.path().join("inline-legacy");
            let mut legacy = compile_direct_workflow(compilation_input).unwrap();
            runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
                &mut legacy,
                &components,
                &[dir.path().to_owned()],
            )
            .unwrap();
            assert_eq!(actual, fs::read(&legacy.wasm_path).unwrap());
        }
    }
}

#[test]
fn policy_falls_back_for_unsupported_root_abis_without_changing_legacy_bytes() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    for abi in [WorkflowAbi::CliRunHttp, WorkflowAbi::AgentCapabilities] {
        let graph = super::super::wasm_performance_baseline::random_chain(1, false);
        let mut compilation_input = input(graph, &dir.path().join(format!("{abi:?}-legacy")));
        compilation_input.agent_slug = Some("fallback-test".into());
        let mut legacy = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
            compilation_input.clone(),
            abi,
            false,
        )
        .unwrap();
        compose_direct_workflow(&mut legacy, &components).unwrap();
        compilation_input.output_dir = dir.path().join(format!("{abi:?}-policy"));
        let fallback = compile_direct_workflow_composed_with_isolation_policy(
            compilation_input,
            abi,
            false,
            &components,
            &[],
            approved(&components),
            limits(),
        )
        .unwrap();
        assert_eq!(
            decisions(&fallback)["utils"],
            Reason::UnsupportedRootRuntime
        );
        assert!(fallback.scoped_agents.is_empty());
        assert!(fallback.invocation_manifest.is_none());
        assert_eq!(
            fs::read(fallback.wasm_path).unwrap(),
            fs::read(legacy.wasm_path).unwrap()
        );
    }
}

#[test]
fn policy_rechecks_shared_bytes_before_replacing_a_composed_artifact() {
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let graph = super::super::wasm_performance_baseline::random_chain(1, false);
    let legacy = compile(graph.clone(), &dir.path().join("legacy"));
    for dep in legacy
        .artifact_metadata
        .shared_components
        .iter()
        .chain(&legacy.artifact_metadata.agent_components)
    {
        fs::copy(
            components.join(&dep.wasm_filename),
            dir.path().join(&dep.wasm_filename),
        )
        .unwrap();
        let meta = components.join(&dep.meta_filename);
        if meta.exists() {
            fs::copy(meta, dir.path().join(&dep.meta_filename)).unwrap();
        }
    }
    let mut compiled = compile_direct_workflow_composed_with_isolation_policy(
        input(graph, &dir.path().join("selected")),
        WorkflowAbi::InvokeHostImports,
        false,
        dir.path(),
        &[],
        approved(dir.path()),
        limits(),
    )
    .unwrap();
    let original = fs::read(&compiled.wasm_path).unwrap();
    let report = compiled
        .artifact_metadata
        .isolation_selection
        .as_ref()
        .unwrap();
    assert!(!report.shared_components.is_empty());
    let dep = compiled
        .artifact_metadata
        .shared_components
        .iter()
        .find(|d| report.shared_components.contains_key(&d.package))
        .unwrap();
    let path = dir.path().join(&dep.wasm_filename);
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&[0, 1, 0]);
    fs::write(path, &bytes).unwrap();
    let sidecar = dir.path().join(&dep.meta_filename);
    let mut metadata: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    metadata["sha256"] = artifact_digest(&bytes).into();
    metadata["sizeBytes"] = bytes.len().into();
    fs::write(sidecar, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let error = compose_direct_workflow_with_isolated_agents(
        &mut compiled,
        dir.path(),
        &[],
        &selected(dir.path()),
        limits(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("shared component")
            && error
                .to_string()
                .contains("changed after isolation selection"),
        "{error}"
    );
    assert_eq!(fs::read(&compiled.wasm_path).unwrap(), original);
}

#[test]
fn public_compile_wrapper_routes_policy_and_keeps_legacy_default() {
    use runtara_workflows::compile::{
        CompilationInput, DirectWorkflowCompileOptions, compile_workflow_direct,
    };
    let components = direct_e2e_components_dir();
    let dir = tempfile::tempdir().unwrap();
    let graph = super::super::wasm_performance_baseline::random_chain(1, false);
    for isolated in [false, true] {
        let direct_input = input(
            graph.clone(),
            &dir.path().join(format!("direct-{isolated}")),
        );
        let reference = if isolated {
            compile_direct_workflow_composed_with_isolation_policy(
                direct_input,
                WorkflowAbi::InvokeHostImports,
                false,
                &components,
                &[],
                approved(&components),
                limits(),
            )
            .unwrap()
        } else {
            let mut result = compile_direct_workflow(direct_input).unwrap();
            compose_direct_workflow(&mut result, &components).unwrap();
            result
        };
        let wrapped = compile_workflow_direct(
            CompilationInput {
                tenant_id: "test".into(),
                workflow_id: "isolated-agent-test".into(),
                version: 1,
                execution_graph: serde_json::from_value(graph.clone()).unwrap(),
                track_events: false,
                child_workflows: vec![],
                connection_service_url: None,
                agent_catalog: None,
                progress_callback: None,
                agent_slug: None,
            },
            DirectWorkflowCompileOptions {
                output_dir: dir.path().join(format!("wrapper-{isolated}")),
                components_dir: components.clone(),
                extra_component_dirs: vec![],
                source_checksum: None,
                isolation_policy: isolated.then(|| (approved(&components), limits())),
            },
        )
        .unwrap();
        let bytes = fs::read(&wrapped.binary_path).unwrap();
        assert_eq!(bytes, fs::read(reference.wasm_path).unwrap());
        assert_eq!(wrapped.binary_checksum, artifact_digest(&bytes));
        assert_eq!(parse(&bytes, limits()).unwrap().is_some(), isolated);
    }
}
