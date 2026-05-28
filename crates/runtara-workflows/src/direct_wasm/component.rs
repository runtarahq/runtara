// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Component-facing artifacts for the direct workflow compiler.

use runtara_workflow_wit::{RUNTIME_PACKAGE, STDLIB_PACKAGE, WORKFLOW_WIT_VERSION};

/// Package name used by direct-emitted workflow logic components.
pub const DIRECT_WORKFLOW_LOGIC_PACKAGE: &str = "runtara:workflow-logic@0.1.0";
/// Version used by generated per-agent component imports.
pub const DIRECT_AGENT_WIT_VERSION: &str = "0.3.0";

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
    DirectComponentArtifacts {
        world_wit: emit_world_wit(agents),
        wac_source: emit_wac(agents),
        stdlib_package: STDLIB_PACKAGE.to_string(),
        runtime_package: RUNTIME_PACKAGE.to_string(),
        shared_components: DIRECT_SHARED_COMPONENT_REQUIREMENTS.to_vec(),
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

fn emit_world_wit(agents: &[String]) -> String {
    let mut out = format!(
        "// Generated by runtara-workflows direct component scaffold.\n\
         package runtara:workflow@{WORKFLOW_WIT_VERSION};\n\
         \n\
         world workflow {{\n\
             import runtara:workflow-stdlib/json@{WORKFLOW_WIT_VERSION};\n\
             import runtara:workflow-runtime/runtime@{WORKFLOW_WIT_VERSION};\n",
    );
    for agent in agents {
        out.push_str(&format!(
            "    import runtara:agent-{agent}/capabilities@{DIRECT_AGENT_WIT_VERSION};\n"
        ));
    }
    out.push_str("    export wasi:cli/run@0.2.3;\n");
    out.push_str("}\n");
    out
}

fn emit_wac(agents: &[String]) -> String {
    let mut out = format!(
        "// Generated by runtara-workflows direct component scaffold.\n\
         package runtara:workflow-instance@{WORKFLOW_WIT_VERSION};\n\
         \n\
         let workflow-stdlib = new runtara:workflow-stdlib {{ ... }};\n\
         let workflow-runtime = new runtara:workflow-runtime {{ ... }};\n",
    );

    for agent in agents {
        out.push_str(&format!(
            "let agent-{id} = new runtara:agent-{id} {{ ... }};\n",
            id = agent
        ));
    }

    out.push_str("\nlet wf = new runtara:workflow-logic {");
    out.push_str(" ...workflow-stdlib, ...workflow-runtime,");
    for agent in agents {
        out.push_str(&format!(" ...agent-{id},", id = agent));
    }
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
        assert!(artifacts.world_wit.contains("export wasi:cli/run@0.2.3;"));
    }

    #[test]
    fn direct_wac_statically_composes_stdlib_runtime_and_agents() {
        let artifacts =
            emit_direct_component_artifacts(&["crypto".to_string(), "object-model".to_string()]);

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
    fn direct_shared_component_requirements_match_bundle_outputs() {
        let artifacts = emit_direct_component_artifacts(&[]);

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
    }
}
