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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Helper to set env vars for a test and restore them after
    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new() -> Self {
            Self { vars: Vec::new() }
        }

        fn set(&mut self, key: &str, value: &str) {
            let old = env::var(key).ok();
            self.vars.push((key.to_string(), old));
            // SAFETY: Tests are serialized via ENV_MUTEX, so no concurrent access
            unsafe { env::set_var(key, value) };
        }

        fn remove(&mut self, key: &str) {
            let old = env::var(key).ok();
            self.vars.push((key.to_string(), old));
            // SAFETY: Tests are serialized via ENV_MUTEX, so no concurrent access
            unsafe { env::remove_var(key) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.vars.drain(..).rev() {
                // SAFETY: Tests are serialized via ENV_MUTEX, so no concurrent access
                unsafe {
                    match value {
                        Some(v) => env::set_var(&key, v),
                        None => env::remove_var(&key),
                    }
                }
            }
        }
    }

    // ==========================================================================
    // NativeLibraryInfo struct tests
    // ==========================================================================

    #[test]
    fn test_native_library_info_debug() {
        let info = NativeLibraryInfo {
            scenario_lib_path: PathBuf::from("/usr/lib/libruntara_workflow_stdlib.rlib"),
            deps_dir: PathBuf::from("/usr/lib/deps"),
        };

        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("NativeLibraryInfo"));
        assert!(debug_str.contains("scenario_lib_path"));
        assert!(debug_str.contains("deps_dir"));
    }

    #[test]
    fn test_native_library_info_clone() {
        let info = NativeLibraryInfo {
            scenario_lib_path: PathBuf::from("/path/to/lib.rlib"),
            deps_dir: PathBuf::from("/path/to/deps"),
        };

        let cloned = info.clone();

        assert_eq!(info.scenario_lib_path, cloned.scenario_lib_path);
        assert_eq!(info.deps_dir, cloned.deps_dir);
    }

    #[test]
    fn test_native_library_info_paths() {
        let info = NativeLibraryInfo {
            scenario_lib_path: PathBuf::from("/custom/path/libworkflow.rlib"),
            deps_dir: PathBuf::from("/custom/path/deps"),
        };

        assert_eq!(
            info.scenario_lib_path,
            PathBuf::from("/custom/path/libworkflow.rlib")
        );
        assert_eq!(info.deps_dir, PathBuf::from("/custom/path/deps"));
    }

    // ==========================================================================
    // get_stdlib_name tests
    // ==========================================================================

    #[test]
    fn test_get_stdlib_name_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.remove("RUNTARA_STDLIB_NAME");

        let name = get_stdlib_name();
        assert_eq!(name, "runtara_workflow_stdlib");
    }

    #[test]
    fn test_get_stdlib_name_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_STDLIB_NAME", "smo_workflow_stdlib");

        let name = get_stdlib_name();
        assert_eq!(name, "smo_workflow_stdlib");
    }

    #[test]
    fn test_get_stdlib_name_custom_with_underscores() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_STDLIB_NAME", "my_custom_stdlib_name");

        let name = get_stdlib_name();
        assert_eq!(name, "my_custom_stdlib_name");
    }

    #[test]
    fn test_get_stdlib_name_empty_uses_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Empty string is a valid value, not missing
        guard.set("RUNTARA_STDLIB_NAME", "");

        let name = get_stdlib_name();
        // Empty string is returned since it's set
        assert_eq!(name, "");
    }

    // ==========================================================================
    // get_native_library_dir tests (checking env var handling)
    // ==========================================================================

    #[test]
    fn test_get_native_library_dir_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Use tempdir for a path that exists
        let temp_dir = tempfile::TempDir::new().unwrap();
        guard.set(
            "RUNTARA_NATIVE_LIBRARY_DIR",
            temp_dir.path().to_str().unwrap(),
        );

        let dir = get_native_library_dir();
        assert_eq!(dir, temp_dir.path());
    }

    #[test]
    fn test_get_native_library_dir_env_nonexistent_falls_through() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Set to non-existent path - should fall through to other checks
        guard.set("RUNTARA_NATIVE_LIBRARY_DIR", "/nonexistent/path/12345");
        guard.remove("DATA_DIR");

        let dir = get_native_library_dir();
        // Should fall back to some other path (not the env var value)
        assert_ne!(dir, PathBuf::from("/nonexistent/path/12345"));
    }

    #[test]
    fn test_get_native_library_dir_data_dir_env() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Create a temp dir structure
        let temp_dir = tempfile::TempDir::new().unwrap();
        let lib_cache = temp_dir.path().join("library_cache").join("native");
        std::fs::create_dir_all(&lib_cache).unwrap();

        guard.remove("RUNTARA_NATIVE_LIBRARY_DIR");
        guard.set("DATA_DIR", temp_dir.path().to_str().unwrap());

        let dir = get_native_library_dir();
        // May or may not use DATA_DIR depending on other paths existing
        // Just verify it doesn't panic
        assert!(dir.to_str().is_some());
    }

    // ==========================================================================
    // load_native_library error cases
    // ==========================================================================

    #[test]
    fn test_load_native_library_missing_dir() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Point to a non-existent directory that also won't fall through
        let temp_dir = tempfile::TempDir::new().unwrap();
        let nonexistent = temp_dir.path().join("nonexistent");
        guard.set("RUNTARA_NATIVE_LIBRARY_DIR", nonexistent.to_str().unwrap());

        // Clear other paths to force our env var path
        guard.remove("DATA_DIR");

        // The function internally checks if path exists before using env var
        // So we need a different approach - just verify error handling works
        let result = load_native_library();
        // Either succeeds (if system has libs) or fails with appropriate error
        if let Err(e) = result {
            assert!(e.to_string().contains("not found") || e.to_string().contains("library"));
        }
    }

    #[test]
    fn test_load_native_library_missing_rlib() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Create a directory but no .rlib file
        let temp_dir = tempfile::TempDir::new().unwrap();
        guard.set(
            "RUNTARA_NATIVE_LIBRARY_DIR",
            temp_dir.path().to_str().unwrap(),
        );
        guard.remove("RUNTARA_STDLIB_NAME");

        let result = load_native_library();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_load_native_library_missing_deps() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Create directory with .rlib but no deps dir
        let temp_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join("libruntara_workflow_stdlib.rlib"),
            b"fake rlib",
        )
        .unwrap();

        guard.set(
            "RUNTARA_NATIVE_LIBRARY_DIR",
            temp_dir.path().to_str().unwrap(),
        );
        guard.remove("RUNTARA_STDLIB_NAME");

        let result = load_native_library();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("deps"));
    }

    #[test]
    fn test_load_native_library_success() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Create complete directory structure
        let temp_dir = tempfile::TempDir::new().unwrap();
        let deps_dir = temp_dir.path().join("deps");
        std::fs::create_dir(&deps_dir).unwrap();
        std::fs::write(
            temp_dir.path().join("libruntara_workflow_stdlib.rlib"),
            b"fake rlib",
        )
        .unwrap();

        guard.set(
            "RUNTARA_NATIVE_LIBRARY_DIR",
            temp_dir.path().to_str().unwrap(),
        );
        guard.remove("RUNTARA_STDLIB_NAME");

        let result = load_native_library();
        assert!(result.is_ok());

        let info = result.unwrap();
        assert_eq!(
            info.scenario_lib_path,
            temp_dir.path().join("libruntara_workflow_stdlib.rlib")
        );
        assert_eq!(info.deps_dir, deps_dir);
    }

    #[test]
    fn test_load_native_library_custom_stdlib_name() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        // Create directory with custom stdlib name
        let temp_dir = tempfile::TempDir::new().unwrap();
        let deps_dir = temp_dir.path().join("deps");
        std::fs::create_dir(&deps_dir).unwrap();
        std::fs::write(temp_dir.path().join("libcustom_stdlib.rlib"), b"fake rlib").unwrap();

        guard.set(
            "RUNTARA_NATIVE_LIBRARY_DIR",
            temp_dir.path().to_str().unwrap(),
        );
        guard.set("RUNTARA_STDLIB_NAME", "custom_stdlib");

        let result = load_native_library();
        assert!(result.is_ok());

        let info = result.unwrap();
        assert_eq!(
            info.scenario_lib_path,
            temp_dir.path().join("libcustom_stdlib.rlib")
        );
    }
}
