// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production direct WebAssembly compiler scaffolding.
//!
//! This module is separate from `direct_wasm_poc`: the PoC proves core Wasm can
//! be emitted, while this module owns the production migration surface.

#[cfg(feature = "compiler")]
pub mod compile;
#[cfg(feature = "compiler")]
pub mod component;
pub mod manifest;
pub mod support;

#[cfg(feature = "compiler")]
pub use compile::{
    DIRECT_WORKFLOW_ABI_SECTION, DIRECT_WORKFLOW_ABI_VERSION, DIRECT_WORKFLOW_MANIFEST_SECTION,
    DIRECT_WORKFLOW_SUPPORT_SECTION, DirectCompilationInput, DirectCompilationResult,
    DirectCompileError, compile_direct_workflow, compile_direct_workflow_composed,
    compose_direct_workflow,
};
#[cfg(feature = "compiler")]
pub use component::{
    DIRECT_SHARED_COMPONENT_REQUIREMENTS, DIRECT_WORKFLOW_LOGIC_PACKAGE, DirectComponentArtifacts,
    DirectSharedComponentRequirement, emit_direct_component_artifacts,
};
pub use manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectEdgeManifest, DirectGraphManifest, DirectManifestError,
    DirectNestedGraphManifest, DirectStepManifest, DirectWorkflowManifest,
    build_direct_workflow_manifest,
};
pub use support::{
    DirectWorkflowSupportReport, UnsupportedWorkflowFeature, analyze_direct_wasm_support,
};
