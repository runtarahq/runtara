// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Workflows - Workflow Compilation to Native Binaries
//!
//! This crate compiles workflow definitions (DSL scenarios) into native Linux binaries.
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
//!     │  Scenario   │─────▶│    AST      │─────▶│   Binary    │
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
//! 1. **Parse**: Load the DSL scenario from JSON
//! 2. **Analyze Dependencies**: Identify child scenarios and agent dependencies
//! 3. **Generate AST**: Convert the execution graph to Rust AST using `codegen`
//! 4. **Write Source**: Write generated Rust code to temp directory
//! 5. **Invoke rustc**: Compile with musl target for static linking
//! 6. **Package**: Optionally create OCI image for containerized execution
//!
//! # Usage
//!
//! ```ignore
//! use runtara_workflows::{compile_scenario, CompilationInput};
//!
//! // Load scenario from JSON
//! let scenario: Scenario = serde_json::from_str(&json)?;
//!
//! // Compile to native binary
//! let input = CompilationInput {
//!     scenario: &scenario,
//!     tenant_id: "tenant-1",
//!     scenario_id: "scenario-1",
//!     version: 1,
//!     output_dir: PathBuf::from("./output"),
//!     child_scenarios: vec![],
//! };
//!
//! let result = compile_scenario(&input).await?;
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
//! - [`agents_library`]: Pre-compiled agents library management
//! - [`codegen`]: AST code generation from execution graphs
//! - [`compile`]: Compilation orchestration and rustc invocation
//! - [`dependency_analysis`]: Dependency resolution for child scenarios
//! - [`paths`]: File path utilities for scenarios and data

#![deny(missing_docs)]

/// Pre-compiled agents library management.
pub mod agents_library;

/// AST code generation from execution graphs.
pub mod codegen;

/// Compilation orchestration and rustc invocation.
pub mod compile;

/// Dependency analysis for child scenarios.
pub mod dependency_analysis;

/// File path utilities for scenarios and data.
pub mod paths;

/// Workflow validation for security and correctness.
pub mod validation;

// Re-export main types
pub use agents_library::{NativeLibraryInfo, get_native_library, get_stdlib_name};
pub use compile::{
    ChildDependency, ChildScenarioInput, CompilationInput, NativeCompilationResult,
    compile_scenario, translate_scenario, workflow_has_side_effects,
};
pub use dependency_analysis::{DependencyGraph, ScenarioReference};
pub use paths::{get_data_dir, get_scenario_dir, get_scenario_json_path};
pub use validation::{
    MissingInputField, ValidationError, ValidationResult, validate_workflow,
    validate_workflow_with_children,
};

// Re-export DSL types for convenience
pub use runtara_dsl::{ExecutionGraph, Scenario};
