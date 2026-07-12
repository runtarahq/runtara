// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Workflows - Workflow Compilation to WebAssembly Components
//!
//! This crate compiles workflow definitions (DSL workflows) into WebAssembly
//! component-model modules. The composed `workflow.wasm` communicates with
//! runtara-core via the SDK for durability, checkpointing, and signal handling.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        Workflow Compilation Pipeline                     │
//! └─────────────────────────────────────────────────────────────────────────┘
//!
//!     ┌─────────────┐      ┌─────────────┐      ┌─────────────┐
//!     │    DSL      │      │  workflow-  │      │  workflow   │
//!     │  Workflow   │─────▶│ logic.wasm  │─────▶│   .wasm     │
//!     │  (JSON)     │      │ (emitter)   │      │ (wac-graph) │
//!     └─────────────┘      └─────────────┘      └─────────────┘
//!           │                                         │
//!           ▼                                         ▼
//!     ┌─────────────┐                          ┌─────────────┐
//!     │ Dependency  │                          │  Composed   │
//!     │  Analysis   │                          │ w/ agents   │
//!     └─────────────┘                          └─────────────┘
//! ```
//!
//! # Compilation Pipeline
//!
//! 1. **Parse**: Load the DSL workflow from JSON
//! 2. **Analyze Dependencies**: Identify child workflows and agent dependencies
//! 3. **Emit**: Byte-emit the `workflow-logic` component directly from the
//!    execution graph (the direct WebAssembly emitter — no Rust source)
//! 4. **Compose**: Statically link the emitted logic with the shared and agent
//!    components into the final `workflow.wasm` via in-process `wac-graph`
//!
//! # Usage
//!
//! ```ignore
//! use runtara_workflows::{compile_workflow_direct, CompilationInput, DirectWorkflowCompileOptions};
//!
//! let result = compile_workflow_direct(input, options)?;
//! println!("Composed component at: {:?}", result.binary_path);
//! ```
//!
//! # Important Notes
//!
//! - This crate has **NO database dependencies**. Child workflows must be loaded
//!   by the caller and passed to compilation functions.
//! - Compilation is fully in-process: the direct emitter byte-emits the
//!   workflow-logic module and composes the final `workflow.wasm` via
//!   `wac-graph`. No `rustc`, `cargo`, or external toolchain is invoked.
//!
//! # Modules
//!
//! - [`compile`]: Public compile entry point (direct WebAssembly emitter)
//! - [`direct_wasm`]: Direct WebAssembly emitter
//! - [`dependency_analysis`]: Dependency resolution for child workflows
//! - [`paths`]: File path utilities for workflows and data

#![deny(missing_docs)]

/// Compile entry point (direct WebAssembly emitter).
#[cfg(all(
    feature = "compiler",
    not(all(target_family = "wasm", not(target_os = "wasi")))
))]
pub mod compile;

/// Dependency analysis for child workflows.
pub mod dependency_analysis;

/// Production direct WebAssembly compiler scaffolding.
#[cfg(all(
    feature = "compiler",
    not(all(target_family = "wasm", not(target_os = "wasi")))
))]
pub mod direct_wasm;

/// Workflow start input validation.
pub mod input_validation;

/// File path utilities for workflows and data.
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub mod paths;

/// Validation for editable workflow schema field rows.
pub mod schema_fields_validation;

/// Server-less child-workflow resolution for standalone compilation
/// (the `runtara-compile` CLI).
#[cfg(all(
    feature = "compiler",
    not(all(target_family = "wasm", not(target_os = "wasi")))
))]
pub mod standalone;

/// Workflow validation for security and correctness.
pub mod validation;

/// Workflow feature analysis for direct-emitter planning and gating.
pub mod workflow_features;

// Re-export main types
#[cfg(all(
    feature = "compiler",
    not(all(target_family = "wasm", not(target_os = "wasi")))
))]
pub use compile::{
    ChildDependency, ChildWorkflowInput, CompilationInput, DirectWorkflowCompileOptions,
    NativeCompilationResult, TEMPLATE_MAJOR_VERSION, WorkflowCompilerMode, compile_workflow_direct,
};
pub use dependency_analysis::{DependencyGraph, WorkflowReference};
pub use input_validation::{
    WorkflowInputValidationError, is_empty_schema, validate_inputs, validate_workflow_inputs,
    validate_workflow_start_inputs,
};
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub use paths::{get_data_dir, get_workflow_dir, get_workflow_json_path};
pub use schema_fields_validation::{
    EditableSchemaField, SchemaFieldValidationIssue, validate_schema_fields,
};
pub use validation::{
    ChildValidationReport, ClosureChildGraph, ClosureValidationReport, MissingInputField,
    ValidationError, ValidationResult, validate_workflow, validate_workflow_closure,
    validate_workflow_with_children,
};
pub use workflow_features::{
    ChildWorkflowReference, WorkflowFeature, WorkflowFeatureSummary, analyze_workflow_features,
};

// Re-export DSL types for convenience
pub use runtara_dsl::{ExecutionGraph, Workflow};
