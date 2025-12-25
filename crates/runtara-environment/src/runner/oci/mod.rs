// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI container runner.
//!
//! Launches instance binaries via crun OCI containers.
//! This module is pure execution logic - no database access.

mod bundle;
mod runner;

pub use bundle::{
    BundleConfig, BundleManager, NetworkMode, OciSpec, bundle_exists_at_path,
    create_bundle_at_path, generate_default_oci_config,
};
pub use runner::{OciRunner, OciRunnerConfig};
