// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production direct WebAssembly compiler scaffolding.
//!
//! This module is separate from `direct_wasm_poc`: the PoC proves core Wasm can
//! be emitted, while this module owns the production migration surface.

pub mod manifest;
pub mod support;

pub use manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectEdgeManifest, DirectGraphManifest, DirectManifestError,
    DirectNestedGraphManifest, DirectStepManifest, DirectWorkflowManifest,
    build_direct_workflow_manifest,
};
pub use support::{
    DirectWorkflowSupportReport, UnsupportedWorkflowFeature, analyze_direct_wasm_support,
};
