// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production direct WebAssembly compiler.
//!
//! Separate from `direct_wasm_poc`: the PoC proves core Wasm can be emitted, while
//! this module owns the production compiler that turns a workflow DSL graph into a
//! runnable WASI component *without* `rustc` — emitting the core module byte-by-byte
//! and composing it with prebuilt shared + per-agent components.
//!
//! Pipeline (see `docs/direct-compilation-architecture.md` for the overview):
//! `manifest` flattens the DSL graph into a normalized, integer-addressable IR;
//! `support` gates whether that graph is fully lowerable (else the caller falls
//! back to the generated compiler); `plan` shapes it into a structured run-plan
//! tree; `static_data` lays the constants into linear memory; and `compile` emits
//! the core Wasm and composes it (via `wac`) into the final `workflow.wasm`.
//! `component` supplies the WIT world + `wac` recipe those depend on, while
//! `child_workflows` and `error` are supporting concerns.

#[cfg(feature = "compiler")]
mod child_workflows;
#[cfg(feature = "compiler")]
pub mod compile;
#[cfg(feature = "compiler")]
pub mod component;
#[cfg(feature = "compiler")]
mod error;
mod graph_order;
pub mod manifest;
#[cfg(feature = "compiler")]
mod plan;
#[cfg(feature = "compiler")]
mod static_data;
pub mod support;

#[cfg(feature = "compiler")]
pub use compile::{
    DIRECT_WORKFLOW_ABI_SECTION, DIRECT_WORKFLOW_ABI_VERSION,
    DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME, DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION,
    DIRECT_WORKFLOW_MANIFEST_SECTION, DIRECT_WORKFLOW_SUPPORT_SECTION, DirectArtifactFileMetadata,
    DirectArtifactMetadata, DirectChildWorkflowDependencyMetadata, DirectCompilationInput,
    DirectCompilationResult, DirectComponentDependencyMetadata, DirectComponentSidecarMetadata,
    compile_direct_workflow, compile_direct_workflow_composed, compose_direct_workflow,
};
#[cfg(feature = "compiler")]
pub use component::{
    DIRECT_SHARED_COMPONENT_REQUIREMENTS, DIRECT_WORKFLOW_LOGIC_PACKAGE,
    DirectAgentComponentRequirement, DirectComponentArtifacts, DirectSharedComponentRequirement,
    emit_direct_component_artifacts,
};
#[cfg(feature = "compiler")]
pub use error::DirectCompileError;
pub use manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectChildWorkflowGraphManifest, DirectConditionManifest,
    DirectEdgeManifest, DirectGraphManifest, DirectManifestChildWorkflowInput, DirectManifestError,
    DirectNestedGraphManifest, DirectStepManifest, DirectWorkflowManifest,
    build_direct_workflow_manifest,
    build_direct_workflow_manifest_with_child_workflows_and_agent_catalog,
};
pub use support::{
    DirectWorkflowSupportReport, UnsupportedWorkflowFeature, analyze_direct_wasm_support,
};
