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
#[cfg(feature = "compiler")]
mod error;
pub mod manifest;
#[cfg(feature = "compiler")]
mod plan;
pub mod support;

#[cfg(feature = "compiler")]
pub use compile::{
    DIRECT_WORKFLOW_ABI_SECTION, DIRECT_WORKFLOW_ABI_VERSION,
    DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME, DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION,
    DIRECT_WORKFLOW_MANIFEST_SECTION, DIRECT_WORKFLOW_SUPPORT_SECTION, DirectArtifactFileMetadata,
    DirectArtifactMetadata, DirectCompilationInput, DirectCompilationResult,
    DirectComponentDependencyMetadata, DirectComponentSidecarMetadata, compile_direct_workflow,
    compile_direct_workflow_composed, compose_direct_workflow,
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
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectConditionManifest, DirectEdgeManifest,
    DirectGraphManifest, DirectManifestError, DirectNestedGraphManifest, DirectStepManifest,
    DirectWorkflowManifest, build_direct_workflow_manifest,
};
pub use support::{
    DirectWorkflowSupportReport, UnsupportedWorkflowFeature, analyze_direct_wasm_support,
};
