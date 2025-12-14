// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native library management
//!
//! This module handles loading the pre-compiled runtara-workflow-stdlib
//! that workflows link against.

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing::info;

/// Information about the compiled native library
#[derive(Debug, Clone)]
pub struct NativeLibraryInfo {
    /// Path to the runtara_workflow_stdlib .rlib file (unified library)
    pub scenario_lib_path: PathBuf,
    /// Path to the directory containing dependency .rlib files
    pub deps_dir: PathBuf,
}

/// Global cache for the compiled native library info
static NATIVE_LIBRARY: OnceLock<NativeLibraryInfo> = OnceLock::new();

/// Get the stdlib crate name from environment or default.
///
/// Products extending runtara can set `RUNTARA_STDLIB_NAME` to use their own
/// workflow stdlib crate that re-exports runtara-workflow-stdlib with additional agents.
///
/// # Default
/// `runtara_workflow_stdlib`
///
/// # Example
/// ```bash
/// export RUNTARA_STDLIB_NAME=smo_workflow_stdlib
/// ```
pub fn get_stdlib_name() -> String {
    std::env::var("RUNTARA_STDLIB_NAME").unwrap_or_else(|_| "runtara_workflow_stdlib".to_string())
}

/// Get the pre-compiled native library directory
fn get_native_library_dir() -> PathBuf {
    // First check if explicitly set via environment variable
    if let Ok(cache_dir) = std::env::var("RUNTARA_NATIVE_LIBRARY_DIR") {
        let path = PathBuf::from(cache_dir);
        if path.exists() {
            return path;
        }
    }

    // Try installed location (for deb packages)
    let installed_path = PathBuf::from("/usr/share/runtara/library_cache/native");
    if installed_path.exists() {
        return installed_path;
    }

    // For release builds, use the deduplicated copy in target/native_cache
    let deduplicated_path = PathBuf::from("target/native_cache");
    if deduplicated_path.exists() {
        return deduplicated_path;
    }

    // For development builds, search in target/debug/build or target/release/build
    // The build.rs outputs to OUT_DIR/native_cache/native
    for profile in &["debug", "release"] {
        let build_dir = PathBuf::from(format!("target/{}/build", profile));
        if build_dir.exists() {
            // Find the runtara-* directory
            if let Ok(entries) = std::fs::read_dir(&build_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("runtara-") {
                        let native_path = entry.path().join("out/native_cache/native");
                        if native_path.exists() {
                            return native_path;
                        }
                    }
                }
            }
        }
    }

    // For development, check DATA_DIR
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    let data_path = PathBuf::from(data_dir).join("library_cache").join("native");
    if data_path.exists() {
        return data_path;
    }

    // Final fallback
    PathBuf::from(".data/library_cache/native")
}

/// Load the pre-compiled native library
fn load_native_library() -> io::Result<NativeLibraryInfo> {
    let lib_dir = get_native_library_dir();

    if !lib_dir.exists() {
        return Err(io::Error::other(format!(
            "Pre-compiled native library not found. Expected at: {:?}",
            lib_dir
        )));
    }

    // Find the unified workflow stdlib library .rlib file
    let stdlib_name = get_stdlib_name();
    let scenario_lib_path = lib_dir.join(format!("lib{}.rlib", stdlib_name));

    if !scenario_lib_path.exists() {
        return Err(io::Error::other(format!(
            "{} library not found at: {:?}",
            stdlib_name, scenario_lib_path
        )));
    }

    // Native deps directory
    let deps_dir = lib_dir.join("deps");

    if !deps_dir.exists() {
        return Err(io::Error::other(format!(
            "Native deps directory not found at: {:?}",
            deps_dir
        )));
    }

    tracing::debug!(
        scenario_lib = %scenario_lib_path.display(),
        deps_dir = %deps_dir.display(),
        "Loaded pre-compiled native library"
    );

    Ok(NativeLibraryInfo {
        scenario_lib_path,
        deps_dir,
    })
}

/// Get the compiled native library information
///
/// This loads the pre-compiled library that was built during `cargo build`.
/// Subsequent calls return the cached information.
///
/// # Returns
/// Library information including path to .rlib file and dependencies directory
pub fn get_native_library() -> io::Result<NativeLibraryInfo> {
    // Check if already initialized
    if let Some(info) = NATIVE_LIBRARY.get() {
        return Ok(info.clone());
    }

    // Load the pre-compiled library
    let info = load_native_library()?;

    // Cache it
    let _ = NATIVE_LIBRARY.set(info.clone());

    info!(
        scenario_lib = %info.scenario_lib_path.display(),
        deps_dir = %info.deps_dir.display(),
        "Native library ready (pre-compiled during build)"
    );

    Ok(info)
}
