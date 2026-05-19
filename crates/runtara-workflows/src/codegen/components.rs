// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Components-mode codegen — emit a workflow-logic crate that will be built
//! by `cargo component build` and then composed with the required agent
//! components via `wac compose`.
//!
//! The output isn't a single Rust file (as in rustc-legacy mode); it's a
//! self-contained crate skeleton plus a WAC composition script. See
//! `docs/wasm-components-migration-plan.md` § 7 for the design rationale.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use runtara_dsl::ExecutionGraph;

use crate::codegen::ast::context::EmitContext;
use crate::codegen::ast::{CodegenError, program};

/// Per-workflow artifacts emitted in components mode. Materialized to disk by
/// the compile pipeline; the four files together form a buildable
/// cargo-component crate plus a `wac compose` script.
#[derive(Debug, Clone)]
pub struct CodegenArtifacts {
    /// `src/lib.rs` — the workflow-logic component's Rust source.
    pub lib_rs: String,
    /// `Cargo.toml` — `cdylib` crate with `[package.metadata.component]`
    /// pointing at the per-workflow `wit/world.wit`.
    pub cargo_toml: String,
    /// `wit/world.wit` — declares one named import per used agent, plus
    /// `include wasi:cli/command@0.2.0` so the workflow can do outbound
    /// HTTP and read env vars.
    pub world_wit: String,
    /// `workflow.wac` — composition script that links workflow-logic with
    /// the agent components from the CAS.
    pub wac_source: String,
    /// Ordered list of agents this workflow imports — used by the compile
    /// pipeline to populate the `wac -d` directory.
    pub agents_required: Vec<AgentRequirement>,
}

/// One row in `CodegenArtifacts::agents_required`. The compile pipeline uses
/// this to populate the WAC `-d` lookup directory: each required agent's
/// `.wasm` is symlinked or copied in by `agent_id` + `package`.
#[derive(Debug, Clone)]
pub struct AgentRequirement {
    /// Kebab-case agent id (e.g. `"crypto"`, `"object-model"`). Matches
    /// `runtara_agent_<snake>.meta.json::id` and the WIT import name in
    /// world.wit.
    pub agent_id: String,
    /// Package name as the agent crate's `[package.metadata.component] package`
    /// (e.g. `"runtara:agent-crypto"`).
    pub package: String,
}

/// Build the four artifacts for the given workflow graph. Returns
/// `CodegenError` if the workflow itself fails codegen (same surface as
/// `ast::compile_with_children`).
pub fn emit_components_artifacts(
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
) -> Result<CodegenArtifacts, CodegenError> {
    let mut agents: Vec<String> = program::collect_used_agents(graph, ctx)
        .into_iter()
        .collect();
    agents.sort();

    let lib_rs = emit_lib_rs(graph, ctx, &agents)?;
    let cargo_toml = emit_cargo_toml(&agents);
    let world_wit = emit_world_wit(&agents);
    let wac_source = emit_wac(&agents);
    let agents_required = agents
        .iter()
        .map(|a| AgentRequirement {
            agent_id: a.clone(),
            package: format!("runtara:agent-{}", a),
        })
        .collect();

    Ok(CodegenArtifacts {
        lib_rs,
        cargo_toml,
        world_wit,
        wac_source,
        agents_required,
    })
}

// ---------------------------------------------------------------------------
// lib.rs
// ---------------------------------------------------------------------------

fn emit_lib_rs(
    graph: &ExecutionGraph,
    ctx: &mut EmitContext,
    agents: &[String],
) -> Result<String, CodegenError> {
    // Reuse the heavy emitters from the existing AST path.
    let constants = program::emit_constants(ctx);
    let input_structs = program::emit_input_structs();
    let main_fn = program::emit_main(graph);
    let execute_workflow = program::emit_execute_workflow(graph, ctx)?;

    // Components-specific pieces.
    let components_header = emit_components_header(agents);
    let dispatch = emit_components_dispatch(agents);
    let guest_wrapper = emit_guest_wrapper();

    let combined: TokenStream = quote! {
        #components_header
        #constants
        #input_structs
        #dispatch
        #main_fn
        #execute_workflow
        #guest_wrapper
    };

    Ok(combined.to_string())
}

/// Top-of-file: bindings module, the same external-crate uses the legacy
/// `emit_imports` would produce minus the agent-stdlib re-exports (those
/// are now WIT imports through `bindings::<agent>`).
fn emit_components_header(_agents: &[String]) -> TokenStream {
    // The cargo-component build step generates `src/bindings.rs` from the
    // per-workflow `wit/world.wit`. The bindings carry one module per
    // imported agent (named after the import alias) plus the standard
    // wasi:cli/run export.
    quote! {
        #![allow(non_snake_case, unused_imports, unused_variables, dead_code)]

        #[allow(warnings)]
        mod bindings;

        use std::process::ExitCode;
        use bindings::exports::wasi::cli::run::Guest as __WorkflowGuest;

        // Same external pieces the legacy codegen pulls in.
        extern crate runtara_workflow_stdlib;
        extern crate runtara_sdk;
        use runtara_workflow_stdlib::prelude::*;
        use runtara_sdk::RuntaraSdk;
    }
}

/// `__workflow_dispatch` for components mode — match-arms call into the
/// wit-bindgen-generated agent imports. Same signature as the legacy
/// dispatch so the rest of the codegen (which calls it via
/// `__workflow_dispatch(...)`) is untouched.
fn emit_components_dispatch(agents: &[String]) -> TokenStream {
    if agents.is_empty() {
        return quote! {
            #[allow(dead_code)]
            fn __workflow_dispatch(
                module: &str,
                capability_id: &str,
                input: serde_json::Value,
            ) -> std::result::Result<serde_json::Value, String> {
                Err(format!("Unknown capability: {}:{}", module, capability_id))
            }
        };
    }

    let arms: Vec<TokenStream> = agents
        .iter()
        .map(|agent_id| {
            let agent_pkg = agent_package_ident(agent_id);
            let agent_str = agent_id.as_str();
            quote! {
                #agent_str => {
                    let bytes = serde_json::to_vec(&input)
                        .map_err(|e| e.to_string())?;
                    let conn = input
                        .get("_connection")
                        .and_then(__connection_from_value);
                    let result = bindings::runtara::#agent_pkg::capabilities::invoke(
                        capability_id,
                        &bytes,
                        conn.as_ref(),
                    );
                    match result {
                        Ok(out_bytes) => serde_json::from_slice(&out_bytes)
                            .map_err(|e| e.to_string()),
                        Err(err) => Err(__error_info_to_envelope(&err)),
                    }
                }
            }
        })
        .collect();

    quote! {
        /// Format a WIT `error-info` as the same JSON envelope the legacy
        /// dispatch path emits, so `#[resilient]` can parse `category` for
        /// retry decisions.
        fn __error_info_to_envelope(err: &bindings::runtara::agent::types::ErrorInfo) -> String {
            let mut obj = serde_json::Map::new();
            obj.insert("code".into(), serde_json::Value::String(err.code.clone()));
            obj.insert("message".into(), serde_json::Value::String(err.message.clone()));
            obj.insert("category".into(), serde_json::Value::String(err.category.clone()));
            obj.insert("severity".into(), serde_json::Value::String(err.severity.clone()));
            obj.insert("retryable".into(), serde_json::Value::Bool(err.retryable));
            if let Some(ms) = err.retry_after_ms {
                obj.insert("retry_after_ms".into(), serde_json::Value::from(ms));
            }
            if let Some(ref attrs) = err.attributes {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(attrs) {
                    obj.insert("attributes".into(), parsed);
                }
            }
            serde_json::Value::Object(obj).to_string()
        }

        /// Convert the `_connection` field (injected by the legacy codegen
        /// from `emit_connection_fetch`) into the WIT `ConnectionInfo`
        /// record imported via the bindings.
        fn __connection_from_value(
            v: &serde_json::Value,
        ) -> Option<bindings::runtara::agent::types::ConnectionInfo> {
            let obj = v.as_object()?;
            Some(bindings::runtara::agent::types::ConnectionInfo {
                connection_id: obj.get("connection_id")?.as_str()?.to_string(),
                integration_id: obj.get("integration_id")?.as_str()?.to_string(),
                connection_subtype: obj
                    .get("connection_subtype")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                parameters: obj
                    .get("parameters")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string()),
                rate_limit_config: obj
                    .get("rate_limit_config")
                    .filter(|v| !v.is_null())
                    .map(|v| v.to_string()),
            })
        }

        #[allow(dead_code)]
        fn __workflow_dispatch(
            module: &str,
            capability_id: &str,
            input: serde_json::Value,
        ) -> std::result::Result<serde_json::Value, String> {
            let module_lower = module.to_lowercase();
            match module_lower.as_str() {
                #(#arms)*
                _ => Err(format!("Unknown capability: {}:{}", module, capability_id)),
            }
        }
    }
}

/// `Component` + `Guest::run()` wrapper that delegates to the emitted
/// `fn main() -> ExitCode`.
fn emit_guest_wrapper() -> TokenStream {
    quote! {
        struct __RuntaraWorkflowComponent;

        impl __WorkflowGuest for __RuntaraWorkflowComponent {
            fn run() -> Result<(), ()> {
                if matches!(main(), ExitCode::SUCCESS) {
                    Ok(())
                } else {
                    Err(())
                }
            }
        }

        bindings::export!(__RuntaraWorkflowComponent with_types_in bindings);
    }
}

/// Snake-case identifier for the agent's bindings module. cargo-component
/// snake-cases the WIT import name (`object-model` → `object_model`).
#[allow(dead_code)]
fn agent_module_ident(agent_id: &str) -> Ident {
    let snake = agent_id.replace('-', "_");
    Ident::new(&snake, Span::call_site())
}

/// wit-bindgen module path for a per-agent package import. Per-agent WIT
/// declares `package runtara:agent-<id>@0.3.0;`; cargo-component generates
/// `bindings::runtara::agent_<snake>::capabilities` for it.
fn agent_package_ident(agent_id: &str) -> Ident {
    let snake = format!("agent_{}", agent_id.replace('-', "_"));
    Ident::new(&snake, Span::call_site())
}

// ---------------------------------------------------------------------------
// Cargo.toml
// ---------------------------------------------------------------------------

fn emit_cargo_toml(agents: &[String]) -> String {
    // One dep entry per used agent. cargo-component reads
    // `[package.metadata.component.target.dependencies]` to populate wit/deps/
    // — auto-discovery from wit/deps/ alone is not enough; the deps have to
    // be declared explicitly here.
    let mut per_agent_deps = String::new();
    for agent in agents {
        per_agent_deps.push_str(&format!(
            "\"runtara:agent-{id}\" = {{ path = \"{{{{AGENT_PER_WIT_PATH:{id}}}}}\" }}\n",
            id = agent,
        ));
    }

    format!(
        r#"# Generated by runtara-workflows components-mode codegen.
[package]
name = "workflow-logic"
version = "0.0.1"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
wit-bindgen-rt = {{ version = "0.44", features = ["bitflags"] }}
runtara-workflow-stdlib = {{ path = "{stdlib_path}", default-features = false, features = ["wasi"] }}
runtara-sdk = {{ path = "{sdk_path}" }}
tracing = "0.1"

[package.metadata.component]
package = "runtara:workflow-logic"

[package.metadata.component.target]
path = "wit"
world = "workflow"

[package.metadata.component.target.dependencies]
"runtara:agent" = {{ path = "{agent_wit_path}" }}
"wasi:cli" = {{ path = "{wasi_cli_path}" }}
"wasi:io" = {{ path = "{wasi_io_path}" }}
"wasi:clocks" = {{ path = "{wasi_clocks_path}" }}
"wasi:random" = {{ path = "{wasi_random_path}" }}
"wasi:filesystem" = {{ path = "{wasi_filesystem_path}" }}
"wasi:sockets" = {{ path = "{wasi_sockets_path}" }}
{per_agent_deps}"#,
        // The compile pipeline resolves these as absolute workspace paths
        // before writing — these placeholders are filled in there.
        stdlib_path = "{{STDLIB_PATH}}",
        sdk_path = "{{SDK_PATH}}",
        agent_wit_path = "{{AGENT_WIT_PATH}}",
        wasi_cli_path = "{{WASI_CLI_PATH}}",
        wasi_io_path = "{{WASI_IO_PATH}}",
        wasi_clocks_path = "{{WASI_CLOCKS_PATH}}",
        wasi_random_path = "{{WASI_RANDOM_PATH}}",
        wasi_filesystem_path = "{{WASI_FILESYSTEM_PATH}}",
        wasi_sockets_path = "{{WASI_SOCKETS_PATH}}",
        per_agent_deps = per_agent_deps,
    )
}

// ---------------------------------------------------------------------------
// world.wit
// ---------------------------------------------------------------------------

fn emit_world_wit(agents: &[String]) -> String {
    let mut out = String::from(
        "// Generated by runtara-workflows components-mode codegen.\n\
         package runtara:workflow@0.0.1;\n\
         \n\
         world workflow {\n",
    );
    for agent in agents {
        // Anonymous import of the per-agent package (e.g.
        // `runtara:agent-crypto/capabilities@0.3.0`). cargo-component 0.21.1's
        // wit-parser accepts this form (where named imports of versioned
        // namespaced packages like `import crypto: runtara:agent/...` would
        // fail). Per-agent packages also let `wac compose` bind each agent
        // component by exact interface name match.
        out.push_str(&format!(
            "    import runtara:agent-{}/capabilities@0.3.0;\n",
            agent
        ));
    }
    // Match the WASI version pinned by runtara-agent-wit/wit/deps.toml. The
    // agent components import the same wasi:cli/command@0.2.3 family.
    out.push_str("    include wasi:cli/command@0.2.3;\n");
    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// workflow.wac
// ---------------------------------------------------------------------------

fn emit_wac(agents: &[String]) -> String {
    let mut out = String::from(
        "// Generated by runtara-workflows components-mode codegen.\n\
         package runtara:workflow-instance@0.0.1;\n\
         \n",
    );
    // Instantiate each agent component the workflow uses. `wac` uses snake
    // for local-variable identifiers but kebab for the package name (which
    // matches the agent crate's `[package.metadata.component] package`).
    for agent in agents {
        out.push_str(&format!(
            "let {var}_comp = new runtara:agent-{pkg} {{ ... }};\n",
            var = agent.replace('-', "_"),
            pkg = agent,
        ));
    }
    out.push('\n');

    // Instantiate workflow-logic, wiring each import to the corresponding
    // agent's `capabilities` export.
    out.push_str("let wf = new runtara:workflow-logic {\n");
    for agent in agents {
        out.push_str(&format!(
            "    {agent_kebab}: {agent_snake}_comp.capabilities,\n",
            agent_kebab = agent,
            agent_snake = agent.replace('-', "_"),
        ));
    }
    out.push_str("};\n\n");
    out.push_str("export wf...;\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(set: &[String]) -> std::collections::HashSet<String> {
        set.iter().cloned().collect()
    }

    #[test]
    fn world_wit_imports_each_agent() {
        let world = emit_world_wit(&["crypto".into(), "object-model".into()]);
        assert!(world.contains("import runtara:agent-crypto/capabilities@0.3.0;"));
        assert!(world.contains("import runtara:agent-object-model/capabilities@0.3.0;"));
        assert!(world.contains("include wasi:cli/command@0.2.0;"));
    }

    #[test]
    fn wac_instantiates_each_agent_and_workflow_logic() {
        let wac = emit_wac(&["crypto".into(), "object-model".into()]);
        assert!(wac.contains("let crypto_comp = new runtara:agent-crypto"));
        assert!(wac.contains("let object_model_comp = new runtara:agent-object-model"));
        assert!(wac.contains("let wf = new runtara:workflow-logic"));
        assert!(wac.contains("crypto: crypto_comp.capabilities"));
        assert!(wac.contains("object-model: object_model_comp.capabilities"));
        assert!(wac.contains("export wf...;"));
    }

    #[test]
    fn cargo_toml_has_cargo_component_metadata() {
        let toml = emit_cargo_toml(&["crypto".into()]);
        assert!(toml.contains("crate-type = [\"cdylib\"]"));
        assert!(toml.contains("[package.metadata.component]"));
        assert!(toml.contains("package = \"runtara:workflow-logic\""));
        assert!(toml.contains("[package.metadata.component.target.dependencies]"));
        assert!(toml.contains("\"runtara:agent\""));
    }

    #[test]
    fn agent_module_ident_snake_cases() {
        assert_eq!(agent_module_ident("crypto").to_string(), "crypto");
        assert_eq!(
            agent_module_ident("object-model").to_string(),
            "object_model"
        );
    }

    #[test]
    fn empty_agent_set_still_yields_valid_wit_and_wac() {
        let world = emit_world_wit(&[]);
        assert!(world.contains("world workflow {"));
        assert!(world.contains("include wasi:cli/command@0.2.0;"));
        let wac = emit_wac(&[]);
        assert!(wac.contains("let wf = new runtara:workflow-logic"));
    }

    /// Sanity: the agent-id set we emit world imports for matches the
    /// graph's used-agents collector. Smoke against an empty graph.
    #[test]
    fn agents_required_uses_kebab_id_and_package_prefix() {
        let req = AgentRequirement {
            agent_id: "object-model".into(),
            package: "runtara:agent-object-model".into(),
        };
        assert_eq!(req.agent_id, "object-model");
        assert_eq!(req.package, "runtara:agent-object-model");
        // Make sure the snake → kebab assumption holds for the registry too.
        let s: std::collections::HashSet<String> = ids(&["object-model".into()]);
        assert!(s.contains("object-model"));
    }
}
