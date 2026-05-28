// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Workflows - Workflow Compilation to Native Binaries
//!
//! This crate compiles workflow definitions (DSL workflows) into native Linux binaries.
//! The compiled binaries are standalone executables that communicate with runtara-core
//! via the SDK for durability, checkpointing, and signal handling.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        Workflow Compilation Pipeline                     │
//! └─────────────────────────────────────────────────────────────────────────┘
//!
//!     ┌─────────────┐      ┌─────────────┐      ┌─────────────┐
//!     │    DSL      │      │    Rust     │      │   Native    │
//!     │  Workflow   │─────▶│    AST      │─────▶│   Binary    │
//!     │  (JSON)     │      │  (codegen)  │      │  (rustc)    │
//!     └─────────────┘      └─────────────┘      └─────────────┘
//!           │                                         │
//!           ▼                                         ▼
//!     ┌─────────────┐                          ┌─────────────┐
//!     │ Dependency  │                          │ OCI Image   │
//!     │  Analysis   │                          │ (optional)  │
//!     └─────────────┘                          └─────────────┘
//! ```
//!
//! # Compilation Pipeline
//!
//! 1. **Parse**: Load the DSL workflow from JSON
//! 2. **Analyze Dependencies**: Identify child workflows and agent dependencies
//! 3. **Generate AST**: Convert the execution graph to Rust AST using `codegen`
//! 4. **Write Source**: Write generated Rust code to temp directory
//! 5. **Invoke rustc**: Compile with musl target for static linking
//! 6. **Package**: Optionally create OCI image for containerized execution
//!
//! # Usage
//!
//! ```ignore
//! use runtara_workflows::{compile_workflow, CompilationInput};
//!
//! // Load workflow from JSON
//! let workflow: Workflow = serde_json::from_str(&json)?;
//!
//! // Compile to native binary
//! let input = CompilationInput {
//!     workflow: &workflow,
//!     tenant_id: "tenant-1",
//!     workflow_id: "workflow-1",
//!     version: 1,
//!     output_dir: PathBuf::from("./output"),
//!     child_workflows: vec![],
//! };
//!
//! let result = compile_workflow(&input).await?;
//! println!("Binary at: {:?}", result.binary_path);
//! ```
//!
//! # Important Notes
//!
//! - This crate has **NO database dependencies**. Child workflows must be loaded
//!   by the caller and passed to compilation functions.
//! - Compilation requires `rustc` and `musl-tools` to be installed.
//! - The generated binary is statically linked for maximum portability.
//!
//! # Modules
//!
//! - [`codegen`]: AST code generation from execution graphs
//! - [`compile`]: Public compile entry point (routes to components-mode)
//! - [`components_compile`]: components-mode pipeline (cargo-component + wac compose)
//! - [`dependency_analysis`]: Dependency resolution for child workflows
//! - [`paths`]: File path utilities for workflows and data

#![deny(missing_docs)]

/// AST code generation from execution graphs.
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub mod codegen;

/// Compile entry point (routes to components-mode).
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub mod compile;

/// Phase 3 components-mode compile pipeline (cargo component + wac compose).
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub mod components_compile;

/// Proof-of-concept direct WebAssembly emitter for workflow DSL graphs.
#[cfg(all(
    feature = "compiler",
    not(all(target_family = "wasm", not(target_os = "wasi")))
))]
pub mod direct_wasm_poc;

/// Dependency analysis for child workflows.
pub mod dependency_analysis;

/// Production direct WebAssembly compiler scaffolding.
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub mod direct_wasm;

/// Workflow start input validation.
pub mod input_validation;

/// File path utilities for workflows and data.
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub mod paths;

/// Validation for editable workflow schema field rows.
pub mod schema_fields_validation;

/// Workflow validation for security and correctness.
pub mod validation;

/// Workflow feature analysis for direct-emitter planning and gating.
pub mod workflow_features;

// Re-export main types
#[cfg(not(all(target_family = "wasm", not(target_os = "wasi"))))]
pub use compile::{
    ChildDependency, ChildWorkflowInput, CompilationInput, NativeCompilationResult,
    TEMPLATE_MAJOR_VERSION, compile_workflow, workflow_has_side_effects,
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
    MissingInputField, ValidationError, ValidationResult, validate_workflow,
    validate_workflow_with_children,
};
pub use workflow_features::{
    ChildWorkflowReference, WorkflowFeature, WorkflowFeatureSummary, analyze_workflow_features,
};

// Re-export DSL types for convenience
pub use runtara_dsl::{ExecutionGraph, Workflow};
