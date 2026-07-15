// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Component-facing artifacts for the direct workflow compiler.
//!
//! Generates the composition scaffolding that lets independently-built components
//! link together: `emit_world_wit` prints the `runtara:workflow` world the core
//! module is encoded against (imports stdlib/runtime + one interface per agent,
//! exports `wasi:cli/run`), and `emit_wac` prints the `wac` script that
//! instantiates and wires them. `DIRECT_SHARED_COMPONENT_REQUIREMENTS` and the
//! per-agent requirement records pin, in one typed place, the several names each
//! component is known by (wac package, WIT package, build-output filename,
//! `.meta.json`, CAS file) so the world the module is encoded against can't drift
//! from the files actually staged on disk. Contracts over coupling: the module
//! names imports it never defines, and `wac` resolves them.

use runtara_workflow_wit::{
    LIFECYCLE_INTERFACE_NAME, RUNTIME_PACKAGE, STDLIB_PACKAGE, WORKFLOW_WIT_VERSION,
};

/// Package name used by direct-emitted workflow logic components.
pub const DIRECT_WORKFLOW_LOGIC_PACKAGE: &str = "runtara:workflow-logic@0.1.0";
/// Version used by generated per-agent component imports.
pub const DIRECT_AGENT_WIT_VERSION: &str = "0.3.0";

/// How the `runtara:workflow-runtime/runtime` interface is satisfied in the
/// composed `workflow.wasm`.
///
/// The workflow-logic module always *imports* the interface (see
/// [`emit_world_wit`]); this only decides who provides it:
///
/// - [`Composed`](Self::Composed): the prebuilt `runtara-workflow-runtime`
///   guest component is instantiated and spread into the workflow instance, so
///   the composed artifact satisfies the interface internally and the guest
///   reaches core over `wasi:http` (the legacy loopback). Retained for the
///   wasmtime-CLI A/B reference axis and for already-compiled artifacts;
///   the in-process runner supports both bindings side by side.
/// - [`HostImport`](Self::HostImport): the interface is left unbound and
///   surfaces as a component-level import of the composed artifact — exactly
///   like the WASI interfaces already do — for the embedding host to satisfy
///   natively via `add_to_linker` (no HTTP loopback). The production default
///   since Phase 2 of docs/unify-agents-workflows-plan.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeBinding {
    /// Compose the prebuilt runtime component in (guest does HTTP to core).
    Composed,
    /// Surface `runtara:workflow-runtime/runtime` as a host-satisfied import.
    #[default]
    HostImport,
}

/// The workflow's top-level export shape (Phase 3 of
/// docs/unify-agents-workflows-plan.md).
///
/// - [`CliRunHttp`](Self::CliRunHttp): the legacy shape — export
///   `wasi:cli/run`, input pulled via `runtime.load-input`, terminal status
///   pushed via `runtime.complete`/`runtime.fail`.
/// - [`InvokeHostImports`](Self::InvokeHostImports): the unified agent shape —
///   export `runtara:workflow-lifecycle/lifecycle.invoke(input) ->
///   result<outcome, error-info>`: input is the call argument, the terminal
///   result is the return value. The runtime interface stays imported (and
///   `complete`/`fail` still fire for host-side status recording — the return
///   value is additive during the migration; the imports are retired in a
///   later phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkflowAbi {
    /// Legacy: export `wasi:cli/run`, lifecycle over the runtime interface.
    /// Retained for already-compiled artifacts (the runner dispatches by
    /// artifact shape) and as the `RUNTARA_DIRECT_WORKFLOW_ABI=cli-run`
    /// rollback lever.
    CliRunHttp,
    /// Unified: export `lifecycle.invoke`, input/result at the call boundary.
    /// The production default since Phase 5 of
    /// docs/unify-agents-workflows-plan.md.
    #[default]
    InvokeHostImports,
}

/// One prebuilt shared component needed by direct workflow composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectSharedComponentRequirement {
    /// Package name used by `wac -d`, matching `[package.metadata.component]`.
    pub package: &'static str,
    /// Versioned WIT package name imported by direct workflow logic.
    pub package_with_version: &'static str,
    /// Filename emitted by `cargo component build` into the bundle directory.
    pub bundle_wasm_filename: &'static str,
    /// Metadata filename staged beside the bundle `.wasm`.
    pub bundle_meta_filename: &'static str,
    /// Stable filename used if copied into a direct component CAS.
    pub cas_wasm_filename: &'static str,
}

/// One prebuilt agent component needed by direct workflow composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectAgentComponentRequirement {
    /// Canonical DSL/component agent id.
    pub agent_id: String,
    /// Package name used by `wac -d`.
    pub package: String,
    /// Versioned WIT package name imported by direct workflow logic.
    pub package_with_version: String,
    /// Filename emitted by `cargo component build` into the bundle directory.
    pub bundle_wasm_filename: String,
    /// Metadata filename staged beside the bundle `.wasm`.
    pub bundle_meta_filename: String,
    /// Stable filename used if copied into a direct component CAS.
    pub cas_wasm_filename: String,
}

/// Shared components every direct workflow logic component imports.
pub const DIRECT_SHARED_COMPONENT_REQUIREMENTS: &[DirectSharedComponentRequirement] = &[
    DirectSharedComponentRequirement {
        package: "runtara:workflow-stdlib",
        package_with_version: STDLIB_PACKAGE,
        bundle_wasm_filename: "runtara_workflow_stdlib.wasm",
        bundle_meta_filename: "runtara_workflow_stdlib.meta.json",
        cas_wasm_filename: "runtara-workflow-stdlib.wasm",
    },
    DirectSharedComponentRequirement {
        package: "runtara:workflow-runtime",
        package_with_version: RUNTIME_PACKAGE,
        bundle_wasm_filename: "runtara_workflow_runtime.wasm",
        bundle_meta_filename: "runtara_workflow_runtime.meta.json",
        cas_wasm_filename: "runtara-workflow-runtime.wasm",
    },
];

/// Direct component composition scaffolding emitted beside direct artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectComponentArtifacts {
    /// `wit/world.wit` for the workflow-logic component.
    pub world_wit: String,
    /// `workflow.wac` static composition script.
    pub wac_source: String,
    /// Stdlib component package to bind during static composition.
    pub stdlib_package: String,
    /// Runtime component package to bind during static composition.
    pub runtime_package: String,
    /// How the runtime interface is satisfied in the composed artifact.
    pub runtime_binding: RuntimeBinding,
    /// Shared components required for static composition.
    pub shared_components: Vec<DirectSharedComponentRequirement>,
    /// Agent components required for static composition.
    pub agent_components: Vec<DirectAgentComponentRequirement>,
}

/// Emit the direct workflow component scaffolding.
///
/// The current direct compiler writes a component-format workflow-logic
/// artifact and composes it to the runtime-facing `workflow.wasm`. These
/// artifacts define the WIT/WAC contract the runtime completion dispatcher will
/// continue to implement without changing the output directory contract.
pub fn emit_direct_component_artifacts(agents: &[String]) -> DirectComponentArtifacts {
    emit_direct_component_artifacts_with_binding(agents, RuntimeBinding::default())
}

/// Emit the direct workflow component scaffolding with an explicit
/// [`RuntimeBinding`].
///
/// Under [`RuntimeBinding::HostImport`] the emitted `workflow.wac` neither
/// instantiates nor spreads the `runtara:workflow-runtime` component, and the
/// shared-component requirements exclude it, so composition needs no runtime
/// `.wasm` on disk and the interface bubbles up as an import of the composed
/// artifact (surfaced by the trailing `...` in the `wf` instantiation).
pub fn emit_direct_component_artifacts_with_binding(
    agents: &[String],
    runtime_binding: RuntimeBinding,
) -> DirectComponentArtifacts {
    emit_direct_component_artifacts_configured(
        agents,
        runtime_binding,
        WorkflowAbi::default(),
        false,
    )
}

/// Fully-configured scaffolding emission: explicit [`RuntimeBinding`] and
/// [`WorkflowAbi`]. The ABI changes only the world's export line; the wac is
/// export-agnostic (`export wf...;` re-exports whatever the logic component
/// exports).
pub fn emit_direct_component_artifacts_configured(
    agents: &[String],
    runtime_binding: RuntimeBinding,
    abi: WorkflowAbi,
    omit_runtime: bool,
) -> DirectComponentArtifacts {
    let shared_components = DIRECT_SHARED_COMPONENT_REQUIREMENTS
        .iter()
        .filter(|component| {
            // The runtime component is composed only under the Composed binding
            // AND when the runtime is not omitted (agent-shaped); every other
            // shared component is always required.
            component.package != "runtara:workflow-runtime"
                || (runtime_binding == RuntimeBinding::Composed && !omit_runtime)
        })
        .copied()
        .collect();
    DirectComponentArtifacts {
        world_wit: emit_world_wit(agents, abi, omit_runtime),
        wac_source: emit_wac(agents, runtime_binding),
        stdlib_package: STDLIB_PACKAGE.to_string(),
        runtime_package: RUNTIME_PACKAGE.to_string(),
        runtime_binding,
        shared_components,
        agent_components: agents.iter().map(|agent| agent_component(agent)).collect(),
    }
}

fn agent_component(agent: &str) -> DirectAgentComponentRequirement {
    let snake = agent.replace('-', "_");
    let package = format!("runtara:agent-{agent}");
    DirectAgentComponentRequirement {
        agent_id: agent.to_string(),
        package: package.clone(),
        package_with_version: format!("{package}@{DIRECT_AGENT_WIT_VERSION}"),
        bundle_wasm_filename: format!("runtara_agent_{snake}.wasm"),
        bundle_meta_filename: format!("runtara_agent_{snake}.meta.json"),
        cas_wasm_filename: format!("{}.wasm", package.replace(':', "-")),
    }
}

fn emit_world_wit(agents: &[String], abi: WorkflowAbi, omit_runtime: bool) -> String {
    let mut out = format!(
        "// Generated by runtara-workflows direct component scaffold.\n\
         package runtara:workflow@{WORKFLOW_WIT_VERSION};\n\
         \n\
         world workflow {{\n\
         \x20   import runtara:workflow-stdlib/json@{WORKFLOW_WIT_VERSION};\n",
    );
    if !omit_runtime {
        out.push_str(&format!(
            "    import runtara:workflow-runtime/runtime@{WORKFLOW_WIT_VERSION};\n"
        ));
    }
    for agent in agents {
        out.push_str(&format!(
            "    import runtara:agent-{agent}/capabilities@{DIRECT_AGENT_WIT_VERSION};\n"
        ));
    }
    match abi {
        WorkflowAbi::CliRunHttp => out.push_str("    export wasi:cli/run@0.2.3;\n"),
        WorkflowAbi::InvokeHostImports => {
            out.push_str(&format!("    export {LIFECYCLE_INTERFACE_NAME};\n"))
        }
    }
    out.push_str("}\n");
    out
}

fn emit_wac(agents: &[String], runtime_binding: RuntimeBinding) -> String {
    let mut out = format!(
        "// Generated by runtara-workflows direct component scaffold.\n\
         package runtara:workflow-instance@{WORKFLOW_WIT_VERSION};\n\
         \n\
         let workflow-stdlib = new runtara:workflow-stdlib {{ ... }};\n",
    );
    if runtime_binding == RuntimeBinding::Composed {
        out.push_str("let workflow-runtime = new runtara:workflow-runtime { ... };\n");
    }

    for agent in agents {
        out.push_str(&format!(
            "let agent-{id} = new runtara:agent-{id} {{ ... }};\n",
            id = agent
        ));
    }

    out.push_str("\nlet wf = new runtara:workflow-logic {");
    out.push_str(" ...workflow-stdlib,");
    if runtime_binding == RuntimeBinding::Composed {
        out.push_str(" ...workflow-runtime,");
    }
    for agent in agents {
        out.push_str(&format!(" ...agent-{id},", id = agent));
    }
    // The trailing bare `...` leaves every remaining workflow-logic import
    // unsatisfied so it bubbles to the composed component's imports. That is
    // already how the WASI interfaces reach the host; under
    // `RuntimeBinding::HostImport` the runtime interface rides the same path.
    out.push_str(" ... };\n\n");
    out.push_str("export wf...;\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_world_imports_stdlib_runtime_and_exports_wasi_run() {
        let artifacts = emit_direct_component_artifacts(&[]);

        assert!(
            artifacts
                .world_wit
                .contains("package runtara:workflow@0.1.0;")
        );
        assert!(
            artifacts
                .world_wit
                .contains("import runtara:workflow-stdlib/json@0.1.0;")
        );
        assert!(
            artifacts
                .world_wit
                .contains("import runtara:workflow-runtime/runtime@0.1.0;")
        );
        // The Phase-5 default exports the invoke lifecycle; the legacy run
        // export remains reachable via the explicit CliRunHttp ABI.
        assert!(
            artifacts
                .world_wit
                .contains("export runtara:workflow-lifecycle/lifecycle@0.1.0;")
        );
        let legacy = emit_direct_component_artifacts_configured(
            &[],
            RuntimeBinding::HostImport,
            WorkflowAbi::CliRunHttp,
            false,
        );
        assert!(legacy.world_wit.contains("export wasi:cli/run@0.2.3;"));
    }

    /// Golden snapshot of the invoke world (the Phase-5 default). Guards
    /// against silent drift of the export line, the runtime import, or agent
    /// imports — any of which would produce a component that composes but
    /// won't drive. The literal here is the drift tripwire; update it
    /// deliberately when the world genuinely changes.
    #[test]
    fn invoke_world_wit_matches_golden_snapshot() {
        let artifacts = emit_direct_component_artifacts_configured(
            &["crypto".to_string(), "object-model".to_string()],
            RuntimeBinding::HostImport,
            WorkflowAbi::InvokeHostImports,
            false,
        );
        let expected = "// Generated by runtara-workflows direct component scaffold.
package runtara:workflow@0.1.0;

world workflow {
    import runtara:workflow-stdlib/json@0.1.0;
    import runtara:workflow-runtime/runtime@0.1.0;
    import runtara:agent-crypto/capabilities@0.3.0;
    import runtara:agent-object-model/capabilities@0.3.0;
    export runtara:workflow-lifecycle/lifecycle@0.1.0;
}
";
        assert_eq!(
            artifacts.world_wit, expected,
            "invoke world drifted — update the golden snapshot deliberately"
        );
        // The export line is the canonical interface name (single source).
        assert!(
            artifacts
                .world_wit
                .contains(runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME)
        );
    }

    #[test]
    fn direct_wac_statically_composes_stdlib_runtime_and_agents() {
        // The Composed (legacy) binding: runtime instantiated + spread.
        let artifacts = emit_direct_component_artifacts_with_binding(
            &["crypto".to_string(), "object-model".to_string()],
            RuntimeBinding::Composed,
        );

        assert!(
            artifacts
                .wac_source
                .contains("let workflow-stdlib = new runtara:workflow-stdlib")
        );
        assert!(
            artifacts
                .wac_source
                .contains("let workflow-runtime = new runtara:workflow-runtime")
        );
        assert!(
            artifacts
                .wac_source
                .contains("let agent-crypto = new runtara:agent-crypto")
        );
        assert!(
            artifacts
                .wac_source
                .contains("let agent-object-model = new runtara:agent-object-model")
        );
        assert!(
            artifacts
                .wac_source
                .contains("...workflow-stdlib, ...workflow-runtime,")
        );
        assert!(artifacts.wac_source.contains("...agent-crypto,"));
        assert!(artifacts.wac_source.contains("...agent-object-model,"));
        assert!(artifacts.wac_source.contains("export wf...;"));
        assert_eq!(
            artifacts.agent_components,
            vec![
                DirectAgentComponentRequirement {
                    agent_id: "crypto".to_string(),
                    package: "runtara:agent-crypto".to_string(),
                    package_with_version: "runtara:agent-crypto@0.3.0".to_string(),
                    bundle_wasm_filename: "runtara_agent_crypto.wasm".to_string(),
                    bundle_meta_filename: "runtara_agent_crypto.meta.json".to_string(),
                    cas_wasm_filename: "runtara-agent-crypto.wasm".to_string(),
                },
                DirectAgentComponentRequirement {
                    agent_id: "object-model".to_string(),
                    package: "runtara:agent-object-model".to_string(),
                    package_with_version: "runtara:agent-object-model@0.3.0".to_string(),
                    bundle_wasm_filename: "runtara_agent_object_model.wasm".to_string(),
                    bundle_meta_filename: "runtara_agent_object_model.meta.json".to_string(),
                    cas_wasm_filename: "runtara-agent-object-model.wasm".to_string(),
                },
            ]
        );
    }

    #[test]
    fn host_import_binding_omits_runtime_from_wac_and_requirements() {
        let artifacts = emit_direct_component_artifacts_with_binding(
            &["crypto".to_string()],
            RuntimeBinding::HostImport,
        );

        // The wac neither instantiates nor spreads the runtime component…
        assert!(!artifacts.wac_source.contains("workflow-runtime"));
        // …but still composes stdlib + agents and keeps the trailing `...`
        // that bubbles unsatisfied imports (runtime + WASI) to the top level.
        assert!(
            artifacts
                .wac_source
                .contains("let workflow-stdlib = new runtara:workflow-stdlib")
        );
        assert!(artifacts.wac_source.contains("...agent-crypto,"));
        assert!(artifacts.wac_source.contains(" ... };"));
        assert!(artifacts.wac_source.contains("export wf...;"));

        // Composition must not require the runtime .wasm on disk.
        assert_eq!(
            artifacts
                .shared_components
                .iter()
                .map(|component| component.package)
                .collect::<Vec<_>>(),
            vec!["runtara:workflow-stdlib"],
        );
        assert_eq!(artifacts.runtime_binding, RuntimeBinding::HostImport);

        // The world is binding-independent: the logic module always imports
        // the runtime interface; the binding only decides who satisfies it.
        assert!(
            artifacts
                .world_wit
                .contains("import runtara:workflow-runtime/runtime@0.1.0;")
        );
    }

    #[test]
    fn default_binding_is_host_import() {
        // Phase 2 of docs/unify-agents-workflows-plan.md: new compiles
        // surface the runtime interface as a host-satisfied import; the
        // Composed binding stays available for the CLI A/B axis and old
        // artifacts keep running unchanged (they carry their own runtime).
        let with_default = emit_direct_component_artifacts(&["crypto".to_string()]);
        let explicit = emit_direct_component_artifacts_with_binding(
            &["crypto".to_string()],
            RuntimeBinding::HostImport,
        );
        assert_eq!(with_default, explicit);
        assert_eq!(with_default.runtime_binding, RuntimeBinding::HostImport);
    }

    #[test]
    fn direct_shared_component_requirements_match_bundle_outputs() {
        // The Composed (legacy) binding needs both bundle components on disk;
        // the HostImport default needs only the stdlib.
        let artifacts = emit_direct_component_artifacts_with_binding(&[], RuntimeBinding::Composed);

        assert_eq!(
            artifacts.shared_components,
            DIRECT_SHARED_COMPONENT_REQUIREMENTS
        );
        assert!(artifacts.agent_components.is_empty());
        assert_eq!(
            artifacts.shared_components,
            vec![
                DirectSharedComponentRequirement {
                    package: "runtara:workflow-stdlib",
                    package_with_version: "runtara:workflow-stdlib@0.1.0",
                    bundle_wasm_filename: "runtara_workflow_stdlib.wasm",
                    bundle_meta_filename: "runtara_workflow_stdlib.meta.json",
                    cas_wasm_filename: "runtara-workflow-stdlib.wasm",
                },
                DirectSharedComponentRequirement {
                    package: "runtara:workflow-runtime",
                    package_with_version: "runtara:workflow-runtime@0.1.0",
                    bundle_wasm_filename: "runtara_workflow_runtime.wasm",
                    bundle_meta_filename: "runtara_workflow_runtime.meta.json",
                    cas_wasm_filename: "runtara-workflow-runtime.wasm",
                },
            ]
        );

        let host_import = emit_direct_component_artifacts(&[]);
        assert_eq!(
            host_import
                .shared_components
                .iter()
                .map(|component| component.package)
                .collect::<Vec<_>>(),
            vec!["runtara:workflow-stdlib"],
        );
    }
}
