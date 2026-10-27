// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The asyncify pass accepts what the emitter writes for published
//! workflow-agents: every function that reaches a cooperative wait or yield
//! calls it in a shape the pass can rewind.

use std::fs;

use super::*;
use crate::direct_wasm::asyncify::{AsyncifyConfig, asyncify};

/// The waits of a root workflow's module (no callback lift applies).
const WAITS: [&str; 1] = ["[waitable-set-wait]"];

/// The logic core module of a published-agent compile of `fixture`.
fn published_agent_core(fixture: &str) -> Vec<u8> {
    core_of(
        fixture,
        crate::direct_wasm::component::WorkflowRole::PublishedAgent,
    )
}

/// The logic core module of a root compile of `fixture`.
fn root_core(fixture: &str) -> Vec<u8> {
    core_of(fixture, crate::direct_wasm::component::WorkflowRole::Root)
}

fn core_of(fixture: &str, role: crate::direct_wasm::component::WorkflowRole) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/{fixture}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let graph = serde_json::from_str(&fs::read_to_string(&path).expect("fixture")).expect("graph");
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: fixture.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("shape-probe".into()),
        },
        role,
        false,
    )
    .unwrap_or_else(|error| panic!("{fixture} compiles: {error}"));
    let component = fs::read(&result.workflow_logic_wasm_path).expect("logic component");
    // The emitted module is the largest; the encoder adds small shims.
    let mut largest: Option<std::ops::Range<usize>> = None;
    for payload in wasmparser::Parser::new(0).parse_all(&component) {
        if let wasmparser::Payload::ModuleSection {
            unchecked_range, ..
        } = payload.expect("component parses")
            && largest
                .as_ref()
                .is_none_or(|r| r.len() < unchecked_range.len())
        {
            largest = Some(unchecked_range);
        }
    }
    component[largest.unwrap_or_else(|| panic!("{fixture}: no core module"))].to_vec()
}

/// `core` with two more mutable i32 globals, for the pass's state and buffer,
/// and their indices.
fn with_state_globals(core: &[u8]) -> (Vec<u8>, u32, u32) {
    use wasm_encoder::reencode::{Reencode, utils};
    struct AddGlobals {
        first: Option<u32>,
    }
    impl Reencode for AddGlobals {
        type Error = std::convert::Infallible;
        fn parse_global_section(
            &mut self,
            globals: &mut wasm_encoder::GlobalSection,
            section: wasmparser::GlobalSectionReader<'_>,
        ) -> Result<(), wasm_encoder::reencode::Error<Self::Error>> {
            utils::parse_global_section(self, globals, section)?;
            self.first = Some(globals.len());
            for _ in 0..2 {
                globals.global(
                    wasm_encoder::GlobalType {
                        val_type: wasm_encoder::ValType::I32,
                        mutable: true,
                        shared: false,
                    },
                    &wasm_encoder::ConstExpr::i32_const(0),
                );
            }
            Ok(())
        }
    }
    let mut imported = 0;
    for payload in wasmparser::Parser::new(0).parse_all(core) {
        if let wasmparser::Payload::ImportSection(reader) = payload.expect("core parses") {
            for import in reader.into_imports() {
                if matches!(import.expect("import").ty, wasmparser::TypeRef::Global(_)) {
                    imported += 1;
                }
            }
        }
    }
    let mut reencoder = AddGlobals { first: None };
    let mut out = wasm_encoder::Module::new();
    reencoder
        .parse_core_module(&mut out, wasmparser::Parser::new(0), core)
        .expect("reencodes");
    let first = imported + reencoder.first.expect("the module has a global section");
    (out.finish(), first, first + 1)
}

/// The imported wait and yield builtins as leaves.
fn probe_config(core: &[u8], state_global: u32, data_global: u32) -> AsyncifyConfig {
    let mut leaves = std::collections::BTreeSet::new();
    let mut function = 0;
    for payload in wasmparser::Parser::new(0).parse_all(core) {
        if let wasmparser::Payload::ImportSection(reader) = payload.expect("core parses") {
            for import in reader.into_imports() {
                let import = import.expect("import");
                if let wasmparser::TypeRef::Func(_) = import.ty {
                    if WAITS.contains(&import.name) {
                        leaves.insert(function);
                    }
                    function += 1;
                }
            }
        }
    }
    AsyncifyConfig {
        leaves,
        drivers: Default::default(),
        state_global,
        data_global,
    }
}

/// Published-agent fixtures, from AI loops to parallel Splits and branches.
const FIXTURES: [&str; 19] = [
    "agent_edge_condition",
    "ai_agent_memory",
    "ai_agent_mcp",
    "ai_agent_memory_compaction",
    "ai_agent_multi_tool",
    "ai_agent_tool_loop",
    "ai_agent_tool_error",
    "error_retry_exhausted",
    "error_in_loop",
    "error_transient",
    "fanout_diamond",
    "http_structured_errors",
    "log_in_loop",
    "split_parallel_workflow",
    "split_workflow",
    "split_with_schemas",
    "transform_workflow",
    "while_nested_condition",
    "while_with_previous_outputs",
];

/// Every published agent compiles with the callback lift: the component
/// encoder accepts its `[async-lift]` export, its callback and its
/// `task.return`, and no `cancellable` builtin is left.
#[test]
fn published_agents_are_lifted_with_a_callback() {
    for fixture in FIXTURES {
        let core = published_agent_core(fixture);
        let mut lifted = false;
        for payload in wasmparser::Parser::new(0).parse_all(&core) {
            match payload.expect("core parses") {
                wasmparser::Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("import");
                        assert!(
                            !import.name.contains("[cancellable]"),
                            "{fixture} imports {}",
                            import.name
                        );
                    }
                }
                wasmparser::Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.expect("export");
                        lifted |= export.name.starts_with("[callback][async-lift]");
                    }
                }
                _ => {}
            }
        }
        assert!(lifted, "{fixture} exports a callback");
    }
}

/// The pass alone, on the module before the lift: the builtins as leaves,
/// two fresh globals for its state, and a valid result.
#[test]
fn published_agent_waits_have_shapes_the_pass_rewinds() {
    let mut probed = 0;
    for fixture in FIXTURES {
        let (core, state, data) = with_state_globals(&root_core(fixture));
        let config = probe_config(&core, state, data);
        if config.leaves.is_empty() {
            continue;
        }
        let (instrumented_module, instrumented) =
            asyncify(&core, &config).unwrap_or_else(|error| panic!("{fixture}: {error}"));
        assert!(!instrumented.is_empty(), "{fixture} waits somewhere");
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(&instrumented_module)
            .unwrap_or_else(|e| panic!("{fixture}: the instrumented module validates: {e}"));
        probed += 1;
    }
    assert!(probed >= 10, "only {probed} fixtures wait");
}
