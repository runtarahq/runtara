// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct WebAssembly proof-of-concept for workflow DSL graphs.
//!
//! This module intentionally does not replace the production component-mode
//! compiler. It proves that the typed DSL can be lowered straight into a Wasm
//! binary without first materializing a Rust crate and invoking
//! `cargo component build`.
//!
//! Scope is deliberately small:
//! - emits a valid core Wasm module with no imports;
//! - exports simple metrics plus `run_bool(flag: i32) -> i32`;
//! - lowers `Finish`, `Log`, and a tiny `Conditional` subset;
//! - records all DSL metadata and unsupported features in a custom section.
//!
//! The exported finish code is a placeholder ABI. A production path would
//! either emit a component that implements the workflow WIT directly or define
//! host imports for JSON mapping, agent dispatch, checkpointing, and events.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use runtara_dsl::{
    ConditionArgument, ConditionExpression, ConditionOperator, ExecutionGraph, MappingValue, Step,
};
use wasm_encoder::{
    BlockType, CodeSection, CustomSection, ExportKind, ExportSection, Function, FunctionSection,
    Module, TypeSection, ValType,
};

use crate::codegen::ast::CodegenError;
use crate::codegen::ast::context::EmitContext;
use crate::codegen::components;

const CUSTOM_SECTION_NAME: &str = "runtara.direct_wasm_poc";
const TYPE_I32_RESULT: u32 = 0;
const TYPE_I32_PARAM_I32_RESULT: u32 = 1;
const FUNC_STEP_COUNT: u32 = 0;
const FUNC_UNSUPPORTED_STEP_COUNT: u32 = 1;
const FUNC_FINISH_COUNT: u32 = 2;
const FUNC_RUN_BOOL: u32 = 3;

/// A direct-Wasm generation artifact.
#[derive(Debug, Clone)]
pub struct DirectWasmArtifact {
    /// The emitted core WebAssembly module bytes.
    pub wasm: Vec<u8>,
    /// Metadata mirrored into the module's custom section.
    pub metadata: DirectWasmMetadata,
    /// Time spent lowering the DSL and encoding the Wasm module.
    pub emit_elapsed_micros: u128,
}

/// Custom-section metadata emitted alongside the PoC module.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectWasmMetadata {
    /// Metadata schema version for this PoC.
    pub poc_version: u32,
    /// Explains the ABI exposed by the generated core Wasm module.
    pub abi: String,
    /// Workflow name copied from the DSL if present.
    pub workflow_name: Option<String>,
    /// Workflow entry point step id.
    pub entry_point: String,
    /// Number of steps in the DSL graph.
    pub step_count: usize,
    /// Finish steps reachable by this PoC, keyed by returned finish code.
    pub finishes: Vec<FinishMetadata>,
    /// Steps the PoC knows how to lower.
    pub supported_steps: Vec<StepSupportMetadata>,
    /// Steps or expressions that require production compiler work.
    pub unsupported_steps: Vec<StepSupportMetadata>,
    /// Exported function names.
    pub exports: Vec<String>,
    /// Human-readable notes about the intentionally narrow scope.
    pub notes: Vec<String>,
}

/// Metadata for a finish code returned by `run_bool`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishMetadata {
    /// Positive integer returned by `run_bool` for this finish step.
    pub code: i32,
    /// DSL step id for the finish step.
    pub step_id: String,
    /// The `Finish.inputMapping` serialized from the DSL.
    pub output_mapping: serde_json::Value,
}

/// Describes whether a step was lowered by the direct-Wasm PoC.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepSupportMetadata {
    /// DSL step id.
    pub step_id: String,
    /// DSL step type.
    pub step_type: String,
    /// Support or unsupported reason.
    pub reason: String,
}

/// Metrics comparing direct Wasm emission with current Rust artifact codegen.
#[derive(Debug, Clone)]
pub struct DirectWasmComparison {
    /// Direct Wasm artifact.
    pub direct: DirectWasmArtifact,
    /// Time spent in the current Rust/component artifact codegen path.
    pub rust_codegen_elapsed_micros: u128,
    /// Combined bytes of `src/lib.rs`, `Cargo.toml`, `wit/world.wit`, and
    /// `workflow.wac` emitted by the current codegen path.
    pub rust_artifact_bytes: usize,
    /// Generated `src/lib.rs` size in bytes.
    pub rust_lib_rs_bytes: usize,
    /// Generated `wit/world.wit` size in bytes.
    pub rust_world_wit_bytes: usize,
    /// Generated `workflow.wac` size in bytes.
    pub rust_wac_bytes: usize,
    /// Agents imported by the current component-mode Rust codegen.
    pub rust_agents_required: Vec<String>,
}

/// Errors returned by the direct-Wasm proof-of-concept.
#[derive(Debug)]
pub enum DirectWasmError {
    /// The graph refers to a missing step.
    MissingStep {
        /// Missing step id.
        step_id: String,
    },
    /// The graph contains a cycle in the subset this PoC lowers inline.
    Cycle {
        /// Step where the cycle was detected.
        step_id: String,
    },
    /// Metadata could not be serialized into the custom section.
    MetadataSerialization(serde_json::Error),
    /// The current Rust codegen path failed during comparison.
    RustCodegen(CodegenError),
}

impl fmt::Display for DirectWasmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectWasmError::MissingStep { step_id } => {
                write!(f, "workflow references missing step `{step_id}`")
            }
            DirectWasmError::Cycle { step_id } => {
                write!(f, "direct Wasm PoC cannot inline cycle at step `{step_id}`")
            }
            DirectWasmError::MetadataSerialization(err) => {
                write!(f, "failed to serialize direct Wasm metadata: {err}")
            }
            DirectWasmError::RustCodegen(err) => {
                write!(f, "current Rust codegen failed during comparison: {err}")
            }
        }
    }
}

impl std::error::Error for DirectWasmError {}

impl From<serde_json::Error> for DirectWasmError {
    fn from(value: serde_json::Error) -> Self {
        Self::MetadataSerialization(value)
    }
}

impl From<CodegenError> for DirectWasmError {
    fn from(value: CodegenError) -> Self {
        Self::RustCodegen(value)
    }
}

/// Emit a valid core WebAssembly module directly from an execution graph.
///
/// The generated module is intentionally a PoC ABI, not the production workflow
/// component ABI. See [`DirectWasmMetadata::notes`] for the embedded caveats.
pub fn emit_direct_wasm_poc(graph: &ExecutionGraph) -> Result<DirectWasmArtifact, DirectWasmError> {
    let start = Instant::now();
    let lowering = Lowering::new(graph);
    let metadata = lowering.metadata();
    let metadata_bytes = serde_json::to_vec(&metadata)?;

    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    types.ty().function([ValType::I32], [ValType::I32]);

    let mut functions = FunctionSection::new();
    functions.function(TYPE_I32_RESULT);
    functions.function(TYPE_I32_RESULT);
    functions.function(TYPE_I32_RESULT);
    functions.function(TYPE_I32_PARAM_I32_RESULT);

    let mut exports = ExportSection::new();
    exports.export(
        "__runtara_poc_step_count",
        ExportKind::Func,
        FUNC_STEP_COUNT,
    );
    exports.export(
        "__runtara_poc_unsupported_step_count",
        ExportKind::Func,
        FUNC_UNSUPPORTED_STEP_COUNT,
    );
    exports.export(
        "__runtara_poc_finish_count",
        ExportKind::Func,
        FUNC_FINISH_COUNT,
    );
    exports.export("run_bool", ExportKind::Func, FUNC_RUN_BOOL);

    let mut code = CodeSection::new();
    code.function(&const_i32_function(graph.steps.len() as i32));
    code.function(&const_i32_function(metadata.unsupported_steps.len() as i32));
    code.function(&const_i32_function(metadata.finishes.len() as i32));
    code.function(&lowering.run_bool_function()?);

    module
        .section(&types)
        .section(&functions)
        .section(&exports)
        .section(&code)
        .section(&CustomSection {
            name: Cow::Borrowed(CUSTOM_SECTION_NAME),
            data: Cow::Owned(metadata_bytes),
        });

    Ok(DirectWasmArtifact {
        wasm: module.finish(),
        metadata,
        emit_elapsed_micros: start.elapsed().as_micros(),
    })
}

/// Compare direct Wasm emission against the current Rust/component artifact
/// codegen path.
///
/// This intentionally stops before `cargo component build` and `wac compose`,
/// so it can run quickly in unit tests and developer loops. The heavy external
/// compiler step remains covered by existing ignored component tests.
pub fn compare_direct_wasm_to_rust_codegen(
    graph: &ExecutionGraph,
    track_events: bool,
    catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
) -> Result<DirectWasmComparison, DirectWasmError> {
    let direct = emit_direct_wasm_poc(graph)?;

    let mut ctx =
        EmitContext::with_child_workflows(track_events, HashMap::new(), HashMap::new(), None, None);
    ctx.set_catalog(catalog);
    ctx.rate_limit_budget_ms = graph.rate_limit_budget_ms;
    ctx.durable = graph.durable.unwrap_or(true);

    let rust_start = Instant::now();
    let artifacts = components::emit_components_artifacts(graph, &mut ctx)?;
    let rust_codegen_elapsed_micros = rust_start.elapsed().as_micros();
    let rust_artifact_bytes = artifacts.lib_rs.len()
        + artifacts.cargo_toml.len()
        + artifacts.world_wit.len()
        + artifacts.wac_source.len();
    let rust_agents_required = artifacts
        .agents_required
        .iter()
        .map(|agent| agent.agent_id.clone())
        .collect();

    Ok(DirectWasmComparison {
        direct,
        rust_codegen_elapsed_micros,
        rust_artifact_bytes,
        rust_lib_rs_bytes: artifacts.lib_rs.len(),
        rust_world_wit_bytes: artifacts.world_wit.len(),
        rust_wac_bytes: artifacts.wac_source.len(),
        rust_agents_required,
    })
}

fn const_i32_function(value: i32) -> Function {
    let mut func = Function::new([]);
    func.instructions().i32_const(value).end();
    func
}

struct Lowering<'a> {
    graph: &'a ExecutionGraph,
    finish_codes: HashMap<String, i32>,
    outgoing: HashMap<String, Vec<&'a runtara_dsl::ExecutionPlanEdge>>,
}

impl<'a> Lowering<'a> {
    fn new(graph: &'a ExecutionGraph) -> Self {
        let mut finish_ids: Vec<String> = graph
            .steps
            .iter()
            .filter_map(|(step_id, step)| {
                matches!(step, Step::Finish(_)).then_some(step_id.clone())
            })
            .collect();
        finish_ids.sort();
        let finish_codes = finish_ids
            .into_iter()
            .enumerate()
            .map(|(idx, step_id)| (step_id, (idx + 1) as i32))
            .collect();

        let mut outgoing: HashMap<String, Vec<&runtara_dsl::ExecutionPlanEdge>> = HashMap::new();
        for edge in &graph.execution_plan {
            outgoing
                .entry(edge.from_step.clone())
                .or_default()
                .push(edge);
        }

        Self {
            graph,
            finish_codes,
            outgoing,
        }
    }

    fn metadata(&self) -> DirectWasmMetadata {
        let mut supported_steps = Vec::new();
        let mut unsupported_steps = Vec::new();
        let mut finishes = Vec::new();

        let mut sorted_steps: Vec<_> = self.graph.steps.iter().collect();
        sorted_steps.sort_by(|a, b| a.0.cmp(b.0));

        for (step_id, step) in sorted_steps {
            match step {
                Step::Finish(finish) => {
                    supported_steps.push(StepSupportMetadata {
                        step_id: step_id.clone(),
                        step_type: step_type_name(step).to_string(),
                        reason: "returns a PoC finish code; output mapping is stored in metadata"
                            .to_string(),
                    });
                    if let Some(code) = self.finish_codes.get(step_id) {
                        finishes.push(FinishMetadata {
                            code: *code,
                            step_id: step_id.clone(),
                            output_mapping: finish
                                .input_mapping
                                .as_ref()
                                .and_then(|mapping| serde_json::to_value(mapping).ok())
                                .unwrap_or(serde_json::Value::Null),
                        });
                    }
                }
                Step::Conditional(conditional) => {
                    let maybe_predicate = BoolPredicate::from_condition(&conditional.condition);
                    let reason = if maybe_predicate.is_some() {
                        "lowers EQ/NE against data.flag into Wasm if/else".to_string()
                    } else {
                        "only EQ/NE conditions against data.flag are lowered in this PoC"
                            .to_string()
                    };
                    let target = if maybe_predicate.is_some() {
                        &mut supported_steps
                    } else {
                        &mut unsupported_steps
                    };
                    target.push(StepSupportMetadata {
                        step_id: step_id.clone(),
                        step_type: step_type_name(step).to_string(),
                        reason,
                    });
                }
                Step::Log(_) => supported_steps.push(StepSupportMetadata {
                    step_id: step_id.clone(),
                    step_type: step_type_name(step).to_string(),
                    reason: "lowered as a no-op before following the normal edge".to_string(),
                }),
                _ => unsupported_steps.push(StepSupportMetadata {
                    step_id: step_id.clone(),
                    step_type: step_type_name(step).to_string(),
                    reason: "not implemented by the direct Wasm PoC".to_string(),
                }),
            }
        }

        DirectWasmMetadata {
            poc_version: 1,
            abi: "core wasm: run_bool(flag: i32) -> finish_code; 0 means unsupported path"
                .to_string(),
            workflow_name: self.graph.name.clone(),
            entry_point: self.graph.entry_point.clone(),
            step_count: self.graph.steps.len(),
            finishes,
            supported_steps,
            unsupported_steps,
            exports: vec![
                "__runtara_poc_step_count".to_string(),
                "__runtara_poc_unsupported_step_count".to_string(),
                "__runtara_poc_finish_count".to_string(),
                "run_bool".to_string(),
            ],
            notes: vec![
                "This PoC emits core Wasm directly with wasm-encoder.".to_string(),
                "It does not yet emit a component-model workflow ABI.".to_string(),
                "JSON mapping, agent dispatch, durability, signals, and events are metadata or unsupported paths.".to_string(),
                "Use it to compare direct emission cost against Rust artifact generation before designing the production ABI.".to_string(),
            ],
        }
    }

    fn run_bool_function(&self) -> Result<Function, DirectWasmError> {
        let mut func = Function::new([]);
        let mut visited = Vec::new();
        {
            let mut instructions = func.instructions();
            self.emit_step(&self.graph.entry_point, &mut instructions, &mut visited)?;
            instructions.end();
        }
        Ok(func)
    }

    fn emit_step(
        &self,
        step_id: &str,
        instructions: &mut wasm_encoder::InstructionSink<'_>,
        visited: &mut Vec<String>,
    ) -> Result<(), DirectWasmError> {
        if visited.iter().any(|seen| seen == step_id) {
            return Err(DirectWasmError::Cycle {
                step_id: step_id.to_string(),
            });
        }

        let Some(step) = self.graph.steps.get(step_id) else {
            return Err(DirectWasmError::MissingStep {
                step_id: step_id.to_string(),
            });
        };

        visited.push(step_id.to_string());
        match step {
            Step::Finish(_) => {
                let code = self.finish_codes.get(step_id).copied().unwrap_or(0);
                instructions.i32_const(code);
            }
            Step::Conditional(conditional) => {
                if let Some(predicate) = BoolPredicate::from_condition(&conditional.condition) {
                    predicate.emit(instructions);
                    instructions.if_(BlockType::Result(ValType::I32));
                    if let Some(true_step) = self.edge_target(step_id, "true") {
                        self.emit_step(true_step, instructions, visited)?;
                    } else {
                        instructions.i32_const(0);
                    }
                    instructions.else_();
                    if let Some(false_step) = self.edge_target(step_id, "false") {
                        self.emit_step(false_step, instructions, visited)?;
                    } else {
                        instructions.i32_const(0);
                    }
                    instructions.end();
                } else {
                    instructions.i32_const(0);
                }
            }
            Step::Log(_) => {
                if let Some(next_step) = self.normal_edge_target(step_id) {
                    self.emit_step(next_step, instructions, visited)?;
                } else {
                    instructions.i32_const(0);
                }
            }
            _ => {
                instructions.i32_const(0);
            }
        }
        visited.pop();
        Ok(())
    }

    fn edge_target(&self, step_id: &str, label: &str) -> Option<&str> {
        self.outgoing
            .get(step_id)?
            .iter()
            .find(|edge| edge.label.as_deref() == Some(label))
            .map(|edge| edge.to_step.as_str())
    }

    fn normal_edge_target(&self, step_id: &str) -> Option<&str> {
        self.outgoing
            .get(step_id)?
            .iter()
            .find(|edge| {
                edge.label
                    .as_deref()
                    .is_none_or(|label| label.is_empty() || label == "next")
            })
            .map(|edge| edge.to_step.as_str())
    }
}

#[derive(Clone, Copy, Debug)]
struct BoolPredicate {
    expect_true: bool,
}

impl BoolPredicate {
    fn from_condition(condition: &ConditionExpression) -> Option<Self> {
        match condition {
            ConditionExpression::Value(value) if is_data_flag_reference(value) => {
                Some(Self { expect_true: true })
            }
            ConditionExpression::Operation(operation) => {
                let [left, right] = operation.arguments.as_slice() else {
                    return None;
                };
                let (reference, literal) = match (
                    argument_as_data_flag_reference(left),
                    argument_as_bool_literal(right),
                ) {
                    (true, Some(value)) => (true, value),
                    _ => match (
                        argument_as_data_flag_reference(right),
                        argument_as_bool_literal(left),
                    ) {
                        (true, Some(value)) => (true, value),
                        _ => (false, false),
                    },
                };
                if !reference {
                    return None;
                }
                match operation.op {
                    ConditionOperator::Eq => Some(Self {
                        expect_true: literal,
                    }),
                    ConditionOperator::Ne => Some(Self {
                        expect_true: !literal,
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn emit(self, instructions: &mut wasm_encoder::InstructionSink<'_>) {
        instructions.local_get(0);
        if !self.expect_true {
            instructions.i32_eqz();
        }
    }
}

fn argument_as_data_flag_reference(argument: &ConditionArgument) -> bool {
    match argument {
        ConditionArgument::Value(value) => is_data_flag_reference(value),
        ConditionArgument::Expression(expr) => {
            matches!(expr.as_ref(), ConditionExpression::Value(value) if is_data_flag_reference(value))
        }
    }
}

fn argument_as_bool_literal(argument: &ConditionArgument) -> Option<bool> {
    match argument {
        ConditionArgument::Value(value) => mapping_bool_literal(value),
        ConditionArgument::Expression(expr) => match expr.as_ref() {
            ConditionExpression::Value(value) => mapping_bool_literal(value),
            ConditionExpression::Operation(_) => None,
        },
    }
}

fn is_data_flag_reference(value: &MappingValue) -> bool {
    matches!(
        value,
        MappingValue::Reference(reference) if reference.value == "data.flag"
    )
}

fn mapping_bool_literal(value: &MappingValue) -> Option<bool> {
    match value {
        MappingValue::Immediate(immediate) => immediate.value.as_bool(),
        _ => None,
    }
}

fn step_type_name(step: &Step) -> &'static str {
    match step {
        Step::Finish(_) => "Finish",
        Step::Agent(_) => "Agent",
        Step::Conditional(_) => "Conditional",
        Step::Split(_) => "Split",
        Step::Switch(_) => "Switch",
        Step::EmbedWorkflow(_) => "EmbedWorkflow",
        Step::While(_) => "While",
        Step::Log(_) => "Log",
        Step::Error(_) => "Error",
        Step::Filter(_) => "Filter",
        Step::GroupBy(_) => "GroupBy",
        Step::Delay(_) => "Delay",
        Step::WaitForSignal(_) => "WaitForSignal",
        Step::AiAgent(_) => "AiAgent",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use wasmparser::{ExternalKind, Parser, Payload, Validator};

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../tests/fixtures/conditional_workflow.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
    }

    #[test]
    fn direct_wasm_for_simple_passthrough_validates() {
        let graph = fixture("simple");
        let artifact = emit_direct_wasm_poc(&graph).expect("direct wasm should emit");

        Validator::new()
            .validate_all(&artifact.wasm)
            .expect("direct wasm should validate");
        assert_eq!(artifact.metadata.step_count, 1);
        assert_eq!(artifact.metadata.finishes.len(), 1);
        assert!(artifact.metadata.unsupported_steps.is_empty());
    }

    #[test]
    fn direct_wasm_exports_run_bool_and_metadata_section() {
        let graph = fixture("conditional");
        let artifact = emit_direct_wasm_poc(&graph).expect("direct wasm should emit");

        let mut saw_run_bool = false;
        let mut saw_metadata = false;
        for payload in Parser::new(0).parse_all(&artifact.wasm) {
            match payload.expect("wasm parser payload") {
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.expect("export");
                        if export.name == "run_bool" {
                            assert_eq!(export.kind, ExternalKind::Func);
                            saw_run_bool = true;
                        }
                    }
                }
                Payload::CustomSection(section) if section.name() == CUSTOM_SECTION_NAME => {
                    let metadata: DirectWasmMetadata =
                        serde_json::from_slice(section.data()).expect("metadata json");
                    assert_eq!(metadata.entry_point, "check");
                    saw_metadata = true;
                }
                _ => {}
            }
        }

        assert!(saw_run_bool, "run_bool export should be present");
        assert!(saw_metadata, "custom metadata section should be present");
        assert!(artifact.metadata.unsupported_steps.is_empty());
    }

    #[test]
    fn comparison_reports_direct_and_rust_artifact_sizes() {
        let graph = fixture("simple");
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
            runtara_agents::registry::get_agents(),
        ));

        let comparison = compare_direct_wasm_to_rust_codegen(&graph, false, catalog)
            .expect("comparison should succeed");

        assert!(!comparison.direct.wasm.is_empty());
        assert!(comparison.rust_artifact_bytes > comparison.rust_world_wit_bytes);
        assert!(comparison.rust_lib_rs_bytes > 0);
    }
}
