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

use runtara_wit::stdlib::{JSON as STDLIB_JSON_INTERFACE, PACKAGE as STDLIB_PACKAGE};
use runtara_wit::workflow::{
    LIFECYCLE as LIFECYCLE_INTERFACE_NAME, RUNTIME as RUNTIME_INTERFACE_NAME,
};

/// Package of the world direct-emitted workflow logic is encoded against, and
/// of the logic component in the composition.
pub const DIRECT_WORKFLOW_LOGIC_PACKAGE: &str = "runtara:workflow-logic@1.0.0";
/// Version used by generated per-agent component imports.
pub const DIRECT_AGENT_WIT_VERSION: &str = runtara_wit::VERSION;

/// The workflow's top-level export shape (Phase 3 of the agent/workflow
/// unification).
///
/// - [`CliRunHttp`](Self::CliRunHttp): the legacy shape — export
///   `wasi:cli/run`, input pulled via `runtime.load-input`, terminal status
///   pushed via `runtime.complete`/`runtime.fail`.
/// - [`InvokeHostImports`](Self::InvokeHostImports): the unified agent shape —
///   export `runtara:workflow/lifecycle.invoke(input) ->
///   result<outcome, error-info>`: input is the call argument, the terminal
///   result is the return value. The runtime interface stays imported (and
///   `complete`/`fail` still fire for host-side status recording — the return
///   value is additive during the migration; the imports are retired in a
///   later phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkflowAbi {
    /// Legacy: export `wasi:cli/run`, lifecycle over the runtime interface.
    /// Retained only for compiler differential tests and artifact migration
    /// tooling. Production direct compilation and generated-workflow image
    /// registration reject this shape because it cannot return a durable
    /// suspension outcome.
    CliRunHttp,
    /// Unified: export `lifecycle.invoke`, input/result at the call boundary.
    /// The production default since Phase 5 of the agent/workflow
    /// unification.
    #[default]
    InvokeHostImports,
    /// Workflow-as-agent: export `runtara:agent-<slug>/capabilities.invoke(
    /// capability-id, input) -> result<list<u8>, error-info>` — the exact
    /// agent capability shape, so a compiled workflow drops into the existing
    /// agent-composition path and is invocable AS an agent. A connection is
    /// never an out-of-band argument: it rides inside `input` (under
    /// `_connection`, or as an ordinary connection-typed input field).
    /// Production publication proves the full graph closure is
    /// free of durable suspension before selecting this ABI. Non-durable
    /// Agent waits use cancellable waitable sets without a root runtime import.
    /// The lower-level compiler retains the historic
    /// runtime-importing shape only for differential tests and migration
    /// tooling; it is never authorized by the production publisher.
    AgentCapabilities,
}

impl WorkflowAbi {
    /// Both invoke-shaped ABIs carry the terminal result in-band (return value)
    /// with a structured `error-info` error arm at the same result-area offset,
    /// and neither uses the `wasi:cli/run` exit-tag convention. The two differ
    /// only in the export declaration, the success-arm layout, and (for
    /// `AgentCapabilities`) the extra ignored params — handled at those sites.
    pub fn is_invoke_export(self) -> bool {
        matches!(self, Self::InvokeHostImports | Self::AgentCapabilities)
    }
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
///
/// The `runtara:workflow/runtime` interface is not a shared component:
/// it is always left unbound and satisfied natively by the embedding host.
pub const DIRECT_SHARED_COMPONENT_REQUIREMENTS: &[DirectSharedComponentRequirement] =
    &[DirectSharedComponentRequirement {
        package: "runtara:workflow-stdlib",
        package_with_version: STDLIB_PACKAGE,
        bundle_wasm_filename: "runtara_workflow_stdlib.wasm",
        bundle_meta_filename: "runtara_workflow_stdlib.meta.json",
        cas_wasm_filename: "runtara-workflow-stdlib.wasm",
    }];

/// Direct component composition scaffolding emitted beside direct artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectComponentArtifacts {
    /// `wit/world.wit` for the workflow-logic component.
    pub world_wit: String,
    /// `workflow.wac` static composition script.
    pub wac_source: String,
    /// Stdlib component package to bind during static composition.
    pub stdlib_package: String,
    /// Whether the workflow imports the host connection resolver.
    pub has_connections: bool,
    /// Whether emitted execution requires the host-I/O timer interface.
    pub has_timers: bool,
    /// Derived from deadline scopes; requires the standard WASI clock.
    pub needs_monotonic_clock: bool,
    /// Shared components required for static composition.
    pub shared_components: Vec<DirectSharedComponentRequirement>,
    /// Agent components required for static composition.
    pub agent_components: Vec<DirectAgentComponentRequirement>,
    /// Agents imported through `suspendable` as well as `capabilities`.
    pub suspending_agents: std::collections::BTreeSet<String>,
    /// Whether an operation-scoped site imports
    /// `runtara:workflow/operation`.
    pub operation_scope: bool,
    /// Whether a WaitForInstances step imports
    /// `runtara:workflow/waits`.
    pub wait_instances: bool,
}

impl DirectComponentArtifacts {
    /// Import `runtara:workflow/waits` in the world when a
    /// WaitForInstances step needs it.
    pub(super) fn with_wait_instances(mut self, wait_instances: bool) -> Self {
        if wait_instances && !self.wait_instances {
            let stdlib = format!("    import {STDLIB_JSON_INTERFACE};\n");
            let import = format!("{stdlib}    import {};\n", runtara_wit::workflow::WAITS);
            self.world_wit = self.world_wit.replacen(&stdlib, &import, 1);
        }
        self.wait_instances |= wait_instances;
        self
    }
}

/// Emit the direct workflow component scaffolding.
///
/// The current direct compiler writes a component-format workflow-logic
/// artifact and composes it to the runtime-facing `workflow.wasm`. These
/// artifacts define the WIT/WAC contract the runtime completion dispatcher will
/// continue to implement without changing the output directory contract.
///
/// The emitted `workflow.wac` never instantiates a runtime component: the
/// `runtara:workflow/runtime` interface bubbles up as an import of the
/// composed artifact (surfaced by the trailing `...` in the `wf`
/// instantiation) for the embedding host to satisfy natively.
pub fn emit_direct_component_artifacts(agents: &[String]) -> DirectComponentArtifacts {
    emit_direct_component_artifacts_configured(agents, WorkflowAbi::default(), false, None)
}

/// Fully-configured scaffolding emission: explicit [`WorkflowAbi`]. The ABI changes only the world's export line; the wac is
/// export-agnostic (`export wf...;` re-exports whatever the logic component
/// exports). `export_agent_id` is the workflow's slug — the package id an
/// `AgentCapabilities` export uses (`runtara:agent-<slug>`); ignored for the
/// other ABIs, falls back to [`CAPABILITIES_EXPORT_AGENT_ID`] when `None`.
pub fn emit_direct_component_artifacts_configured(
    agents: &[String],
    abi: WorkflowAbi,
    omit_runtime: bool,
    export_agent_id: Option<&str>,
) -> DirectComponentArtifacts {
    emit_direct_component_artifacts_with_pools(
        agents,
        abi,
        omit_runtime,
        export_agent_id,
        &std::collections::BTreeMap::new(),
    )
}

/// [`emit_direct_component_artifacts_configured`] plus the parallel-Split
/// instance pools: each pooled agent adds
/// phantom `…-par<n>` world imports and extra wac instantiations of the SAME
/// package, wired by explicit argument name.
pub fn emit_direct_component_artifacts_with_pools(
    agents: &[String],
    abi: WorkflowAbi,
    omit_runtime: bool,
    export_agent_id: Option<&str>,
    parallel_pools: &std::collections::BTreeMap<String, u32>,
) -> DirectComponentArtifacts {
    emit_direct_component_artifacts_with_pools_and_connections(
        agents,
        abi,
        omit_runtime,
        export_agent_id,
        parallel_pools,
        false,
    )
}

pub(super) fn emit_direct_component_artifacts_with_pools_and_connections(
    agents: &[String],
    abi: WorkflowAbi,
    omit_runtime: bool,
    export_agent_id: Option<&str>,
    parallel_pools: &std::collections::BTreeMap<String, u32>,
    has_connections: bool,
) -> DirectComponentArtifacts {
    emit_direct_component_artifacts_scoped(
        agents,
        abi,
        omit_runtime,
        export_agent_id,
        parallel_pools,
        has_connections,
        &Default::default(),
        &Default::default(),
        false,
        false,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_direct_component_artifacts_scoped(
    agents: &[String],
    abi: WorkflowAbi,
    omit_runtime: bool,
    export_agent_id: Option<&str>,
    parallel_pools: &std::collections::BTreeMap<String, u32>,
    has_connections: bool,
    scoped_agents: &std::collections::BTreeSet<String>,
    suspending_agents: &std::collections::BTreeSet<String>,
    operation_scope: bool,
    needs_timers: bool,
    needs_monotonic_clock: bool,
) -> DirectComponentArtifacts {
    let shared_components = DIRECT_SHARED_COMPONENT_REQUIREMENTS.to_vec();
    DirectComponentArtifacts {
        world_wit: emit_world_wit(
            agents,
            abi,
            omit_runtime,
            export_agent_id,
            parallel_pools,
            has_connections,
            scoped_agents,
            suspending_agents,
            operation_scope,
            needs_timers,
            needs_monotonic_clock,
        ),
        wac_source: emit_wac(agents, parallel_pools, scoped_agents),
        stdlib_package: STDLIB_PACKAGE.to_string(),
        has_connections,
        has_timers: needs_timers || !parallel_pools.is_empty() || !agents.is_empty(),
        needs_monotonic_clock,
        shared_components,
        agent_components: agents.iter().map(|agent| agent_component(agent)).collect(),
        suspending_agents: suspending_agents.clone(),
        operation_scope,
        wait_instances: false,
    }
}

pub(super) fn agent_component(agent: &str) -> DirectAgentComponentRequirement {
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

/// FALLBACK package id a workflow-as-agent exports its capabilities under when
/// no per-workflow slug is supplied (tests, legacy paths). Production passes
/// the workflow's own slug so every workflow-agent gets a distinct
/// `runtara:agent-<slug>` package; this placeholder is also a RESERVED slug
/// (the server rejects it) so a user workflow can never collide with it.
pub const CAPABILITIES_EXPORT_AGENT_ID: &str = "workflow-agent";

#[allow(clippy::too_many_arguments)]
fn emit_world_wit(
    agents: &[String],
    abi: WorkflowAbi,
    omit_runtime: bool,
    export_agent_id: Option<&str>,
    parallel_pools: &std::collections::BTreeMap<String, u32>,
    has_connections: bool,
    scoped_agents: &std::collections::BTreeSet<String>,
    suspending_agents: &std::collections::BTreeSet<String>,
    operation_scope: bool,
    needs_timers: bool,
    needs_monotonic_clock: bool,
) -> String {
    let mut out = format!(
        "// Generated by runtara-workflows direct component scaffold.\n\
         package {DIRECT_WORKFLOW_LOGIC_PACKAGE};\n\
         \n\
         world workflow {{\n\
         \x20   import {STDLIB_JSON_INTERFACE};\n",
    );
    if !omit_runtime {
        out.push_str(&format!("    import {RUNTIME_INTERFACE_NAME};\n"));
    }
    if has_connections {
        out.push_str(&format!("    import {};\n", runtara_wit::host::CONNECTIONS));
    }
    if needs_timers || !parallel_pools.is_empty() || !agents.is_empty() {
        out.push_str(&format!("    import {};\n", runtara_wit::host::TIMERS));
    }
    if needs_monotonic_clock {
        out.push_str(&format!(
            "    import {};\n",
            runtara_wit::wasi::MONOTONIC_CLOCK
        ));
    }
    if operation_scope || !suspending_agents.is_empty() {
        out.push_str(&format!(
            "    import {};\n",
            runtara_wit::workflow::OPERATION
        ));
    }
    for agent in agents {
        let interface = if scoped_agents.contains(agent) {
            "scoped-capabilities-v3"
        } else {
            "capabilities"
        };
        out.push_str(&format!(
            "    import runtara:agent-{agent}/{interface}@{DIRECT_AGENT_WIT_VERSION};\n"
        ));
        // A suspending agent is also imported through `suspendable`; the one
        // `...agent-<id>` spread in the wac wires both exports of the same
        // instance.
        if suspending_agents.contains(agent) {
            out.push_str(&format!(
                "    import runtara:agent-{agent}/{}@{DIRECT_AGENT_WIT_VERSION};\n",
                runtara_agent_suspension::SUSPENDABLE_INTERFACE
            ));
        }
        if let Some(pool) = parallel_pools.get(agent) {
            for member in 1..*pool {
                let phantom =
                    crate::direct_wasm::split_parallel_pool_member_component_id(agent, member);
                out.push_str(&format!(
                    "    import runtara:agent-{phantom}/{interface}@{DIRECT_AGENT_WIT_VERSION};\n"
                ));
            }
        }
    }
    match abi {
        WorkflowAbi::CliRunHttp => out.push_str("    export wasi:cli/run@0.2.3;\n"),
        WorkflowAbi::InvokeHostImports => {
            out.push_str(&format!("    export {LIFECYCLE_INTERFACE_NAME};\n"))
        }
        WorkflowAbi::AgentCapabilities => {
            let id = export_agent_id.unwrap_or(CAPABILITIES_EXPORT_AGENT_ID);
            out.push_str(&format!(
                "    export runtara:agent-{id}/capabilities@{DIRECT_AGENT_WIT_VERSION};\n"
            ))
        }
    }
    out.push_str("}\n");
    out
}

fn emit_wac(
    agents: &[String],
    parallel_pools: &std::collections::BTreeMap<String, u32>,
    scoped_agents: &std::collections::BTreeSet<String>,
) -> String {
    let mut out = format!(
        "// Generated by runtara-workflows direct component scaffold.\n\
         package runtara:workflow-instance@{DIRECT_AGENT_WIT_VERSION};\n\
         \n\
         let workflow-stdlib = new runtara:workflow-stdlib {{ ... }};\n",
    );

    for agent in agents {
        out.push_str(&format!(
            "let agent-{id} = new runtara:agent-{id} {{ ... }};\n",
            id = agent
        ));
        // Extra instantiations of the SAME package for the parallel pool —
        // sync-lifted instances serialize concurrent entries, so K-way
        // overlap needs K instances.
        if let Some(pool) = parallel_pools.get(agent) {
            for member in 1..*pool {
                let phantom =
                    crate::direct_wasm::split_parallel_pool_member_component_id(agent, member);
                out.push_str(&format!(
                    "let agent-{phantom} = new runtara:agent-{id} {{ ... }};\n",
                    id = agent
                ));
            }
        }
    }

    out.push_str("\nlet wf = new runtara:workflow-logic {");
    out.push_str(" ...workflow-stdlib,");
    for agent in agents {
        let interface = if scoped_agents.contains(agent) {
            "scoped-capabilities-v3"
        } else {
            "capabilities"
        };
        out.push_str(&format!(" ...agent-{id},", id = agent));
        if let Some(pool) = parallel_pools.get(agent) {
            for member in 1..*pool {
                let phantom =
                    crate::direct_wasm::split_parallel_pool_member_component_id(agent, member);
                out.push_str(&format!(
                    " \"runtara:agent-{phantom}/{interface}@{DIRECT_AGENT_WIT_VERSION}\": \
                     agent-{phantom}[\"runtara:agent-{id}/{interface}@{DIRECT_AGENT_WIT_VERSION}\"],",
                    id = agent
                ));
            }
        }
    }
    // The trailing bare `...` leaves every remaining workflow-logic import
    // unsatisfied so it bubbles to the composed component's imports. That is
    // already how the WASI interfaces reach the host; the runtime interface
    // rides the same path.
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
                .contains("package runtara:workflow-logic@1.0.0;")
        );
        assert!(
            artifacts
                .world_wit
                .contains("import runtara:workflow-stdlib/json@1.0.0;")
        );
        assert!(
            artifacts
                .world_wit
                .contains("import runtara:workflow/runtime@1.0.0;")
        );
        // The Phase-5 default exports the invoke lifecycle; the legacy run
        // export remains reachable via the explicit CliRunHttp ABI.
        assert!(
            artifacts
                .world_wit
                .contains("export runtara:workflow/lifecycle@1.0.0;")
        );
        let legacy =
            emit_direct_component_artifacts_configured(&[], WorkflowAbi::CliRunHttp, false, None);
        assert!(legacy.world_wit.contains("export wasi:cli/run@0.2.3;"));
    }

    #[test]
    fn connection_resolver_import_is_emitted_only_for_connection_workflows() {
        let without = emit_direct_component_artifacts(&["utils".to_string()]);
        assert!(!without.has_connections);
        assert!(!without.world_wit.contains("connection-resolver"));

        let with = emit_direct_component_artifacts_with_pools_and_connections(
            &["ai-tools".to_string()],
            WorkflowAbi::InvokeHostImports,
            false,
            None,
            &std::collections::BTreeMap::new(),
            true,
        );
        assert!(with.has_connections);
        assert!(
            with.world_wit
                .contains("import runtara:host/connections@1.0.0;")
        );
        assert!(
            with.wac_source.contains(" ... }"),
            "the trailing spread must bubble the host resolver import"
        );
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
            WorkflowAbi::InvokeHostImports,
            false,
            None,
        );
        let expected = "// Generated by runtara-workflows direct component scaffold.
package runtara:workflow-logic@1.0.0;

world workflow {
    import runtara:workflow-stdlib/json@1.0.0;
    import runtara:workflow/runtime@1.0.0;
    import runtara:host/timers@1.0.0;
    import runtara:agent-crypto/capabilities@1.0.0;
    import runtara:agent-object-model/capabilities@1.0.0;
    export runtara:workflow/lifecycle@1.0.0;
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
                .contains(runtara_wit::workflow::LIFECYCLE)
        );
    }

    #[test]
    fn agent_capabilities_world_exports_under_the_workflow_slug() {
        // The slug parameterizes the export package so every workflow-agent
        // gets a distinct `runtara:agent-<slug>` (two workflow-agents with the
        // fixed placeholder would collide when composed into one parent).
        let artifacts = emit_direct_component_artifacts_configured(
            &[],
            WorkflowAbi::AgentCapabilities,
            true,
            Some("order-sync"),
        );
        assert!(
            artifacts
                .world_wit
                .contains("export runtara:agent-order-sync/capabilities@1.0.0;"),
            "{}",
            artifacts.world_wit
        );
        // No slug → the legacy placeholder keeps tests/back-compat working.
        let fallback = emit_direct_component_artifacts_configured(
            &[],
            WorkflowAbi::AgentCapabilities,
            true,
            None,
        );
        assert!(
            fallback
                .world_wit
                .contains("export runtara:agent-workflow-agent/capabilities@1.0.0;"),
            "{}",
            fallback.world_wit
        );
    }

    #[test]
    fn direct_wac_composes_stdlib_and_agents_leaving_runtime_to_host() {
        let artifacts =
            emit_direct_component_artifacts(&["crypto".to_string(), "object-model".to_string()]);

        // The wac neither instantiates nor spreads the runtime component…
        assert!(!artifacts.wac_source.contains("workflow-runtime"));
        // …but still composes stdlib + agents and keeps the trailing `...`
        // that bubbles unsatisfied imports (runtime + WASI) to the top level.
        assert!(
            artifacts
                .wac_source
                .contains("let workflow-stdlib = new runtara:workflow-stdlib")
        );
        assert!(
            artifacts
                .wac_source
                .contains("let agent-object-model = new runtara:agent-object-model")
        );
        assert!(artifacts.wac_source.contains("...agent-crypto,"));
        assert!(artifacts.wac_source.contains("...agent-object-model,"));
        assert!(artifacts.wac_source.contains(" ... };"));
        assert!(artifacts.wac_source.contains("export wf...;"));
        assert_eq!(
            artifacts.agent_components,
            vec![
                DirectAgentComponentRequirement {
                    agent_id: "crypto".to_string(),
                    package: "runtara:agent-crypto".to_string(),
                    package_with_version: "runtara:agent-crypto@1.0.0".to_string(),
                    bundle_wasm_filename: "runtara_agent_crypto.wasm".to_string(),
                    bundle_meta_filename: "runtara_agent_crypto.meta.json".to_string(),
                    cas_wasm_filename: "runtara-agent-crypto.wasm".to_string(),
                },
                DirectAgentComponentRequirement {
                    agent_id: "object-model".to_string(),
                    package: "runtara:agent-object-model".to_string(),
                    package_with_version: "runtara:agent-object-model@1.0.0".to_string(),
                    bundle_wasm_filename: "runtara_agent_object_model.wasm".to_string(),
                    bundle_meta_filename: "runtara_agent_object_model.meta.json".to_string(),
                    cas_wasm_filename: "runtara-agent-object-model.wasm".to_string(),
                },
            ]
        );

        // Composition must not require the runtime .wasm on disk.
        assert_eq!(
            artifacts
                .shared_components
                .iter()
                .map(|component| component.package)
                .collect::<Vec<_>>(),
            vec!["runtara:workflow-stdlib"],
        );

        // The logic module still imports the runtime interface; the host
        // satisfies it.
        assert!(
            artifacts
                .world_wit
                .contains("import runtara:workflow/runtime@1.0.0;")
        );
    }

    #[test]
    fn direct_shared_component_requirements_match_bundle_outputs() {
        // Only the stdlib is composed in; the runtime interface is host-bound.
        let artifacts = emit_direct_component_artifacts(&[]);

        assert_eq!(
            artifacts.shared_components,
            DIRECT_SHARED_COMPONENT_REQUIREMENTS
        );
        assert!(artifacts.agent_components.is_empty());
        assert_eq!(
            artifacts.shared_components,
            vec![DirectSharedComponentRequirement {
                package: "runtara:workflow-stdlib",
                package_with_version: "runtara:workflow-stdlib@1.0.0",
                bundle_wasm_filename: "runtara_workflow_stdlib.wasm",
                bundle_meta_filename: "runtara_workflow_stdlib.meta.json",
                cas_wasm_filename: "runtara-workflow-stdlib.wasm",
            }]
        );
    }
}
