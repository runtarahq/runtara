// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rustc-based scenario compilation to native binaries
//!
//! This module provides compilation without database dependencies.
//! Child scenarios must be pre-loaded and passed to compilation functions.
//!
//! Scenarios are compiled to native binaries for the host platform.

use runtara_dsl::ExecutionGraph;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

use crate::codegen::ast;
use crate::paths::get_scenario_json_path;

// ============================================================================
// Rustc Error Parsing
// ============================================================================

/// Parse rustc stderr and provide a user-friendly error message.
///
/// This function attempts to extract meaningful information from rustc errors
/// and provide actionable suggestions.
fn parse_rustc_error(stderr: &str, target: &str) -> String {
    // Check for common errors and provide helpful suggestions

    // Missing target
    if stderr.contains("error[E0463]")
        && stderr.contains("can't find crate")
        && stderr.contains("std")
    {
        return format!(
            "Compilation failed: The Rust standard library for target '{}' is not installed.\n\n\
             To fix this, run:\n  rustup target add {}",
            target, target
        );
    }

    // Target not installed
    if stderr.contains("could not find specification for target") {
        return format!(
            "Compilation failed: Target '{}' is not installed.\n\n\
             To fix this, run:\n  rustup target add {}",
            target, target
        );
    }

    // Linker not found (common on Linux for musl)
    if stderr.contains("linker") && stderr.contains("not found") && target.contains("musl") {
        return "Compilation failed: The musl linker is not installed.\n\n\
                 To fix this on Ubuntu/Debian, run:\n  sudo apt install musl-tools\n\n\
                 To fix this on Fedora/RHEL, run:\n  sudo dnf install musl-gcc"
            .to_string();
    }

    // Can't find crate (stdlib not compiled)
    if stderr.contains("can't find crate for")
        && let Some(crate_name) = extract_pattern(stderr, "can't find crate for `", "`")
    {
        if crate_name == "runtara_workflow_stdlib" {
            return format!(
                "Compilation failed: The workflow stdlib library is not compiled.\n\n\
                     To fix this, run:\n  cargo build -p runtara-workflow-stdlib --release --target {}\n\n\
                     Or set RUNTARA_NATIVE_LIBRARY_DIR to point to a pre-compiled stdlib.",
                target
            );
        }
        return format!(
            "Compilation failed: Cannot find crate '{}'.\n\n\
                 This may indicate the workflow stdlib is not properly compiled.\n\
                 Try rebuilding: cargo build -p runtara-workflow-stdlib --release --target {}",
            crate_name, target
        );
    }

    // Unresolved import
    if stderr.contains("error[E0432]")
        && stderr.contains("unresolved import")
        && let Some(import) = extract_pattern(stderr, "unresolved import `", "`")
    {
        return format!(
            "Compilation failed: Unresolved import '{}'.\n\n\
                 This is likely a code generation bug. Please report this issue.",
            import
        );
    }

    // Type errors (usually code generation bugs)
    if stderr.contains("error[E0308]") && stderr.contains("mismatched types") {
        return "Compilation failed: Type mismatch in generated code.\n\n\
             This is likely a code generation bug. Please report this issue."
            .to_string();
    }

    // Borrow checker errors (usually code generation bugs)
    if stderr.contains("error[E0382]")
        || stderr.contains("error[E0502]")
        || stderr.contains("error[E0499]")
    {
        return "Compilation failed: Borrow checker error in generated code.\n\n\
             This is likely a code generation bug. Please report this issue."
            .to_string();
    }

    // Extract first error message for unknown errors
    if let Some(first_error) = extract_first_error(stderr) {
        return format!(
            "Compilation failed: {}\n\n\
             If this error persists, please contact support.",
            first_error
        );
    }

    // Fallback: generic message
    "Compilation failed. Please contact support if this issue persists.".to_string()
}

/// Extract a pattern from text: prefix...suffix
fn extract_pattern<'a>(text: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    let end = rest.find(suffix)?;
    Some(&rest[..end])
}

/// Extract the first error message from rustc output.
fn extract_first_error(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        let line = line.trim();
        if line.starts_with("error[E") {
            // Extract the error message after the code
            if let Some(msg_start) = line.find("]: ") {
                let msg = &line[msg_start + 3..];
                return Some(msg.to_string());
            }
        } else if line.starts_with("error:") {
            let msg = line.trim_start_matches("error:").trim();
            if !msg.is_empty() {
                return Some(msg.to_string());
            }
        }
    }
    None
}

/// Get the native target triple for the current host platform
///
/// This must match the target used in build.rs when precompiling libraries.
/// We use musl on Linux for static linking (scenarios run in minimal containers).
fn get_host_target() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        // Use musl for static linking - scenarios run in minimal containers
        "x86_64-unknown-linux-musl"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        // Use musl for static linking - scenarios run in minimal containers
        "aarch64-unknown-linux-musl"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    {
        compile_error!("Unsupported platform for scenario compilation")
    }
}

/// List of operator+operation combinations that have side effects (non-deterministic or external I/O)
const SIDE_EFFECT_OPERATIONS: &[(&str, &str)] = &[
    // Utils operator - random/timing operations
    ("utils", "random-double"),
    ("utils", "random-array"),
    ("utils", "get-current-unix-timestamp"),
    ("utils", "get-current-iso-datetime"),
    ("utils", "get-current-formatted-datetime"),
    ("utils", "delay-in-ms"),
    // HTTP operator - external network I/O
    ("http", "http-request"),
    // SFTP operator - external file I/O
    ("sftp", "sftp-list-files"),
    ("sftp", "sftp-download-file"),
    ("sftp", "sftp-upload-file"),
    ("sftp", "sftp-delete-file"),
];

/// Checks if a workflow has any operations with side effects
///
/// # Arguments
/// * `workflow` - The workflow JSON definition
///
/// # Returns
/// `true` if the workflow contains any operator+operation combination that has side effects
pub fn workflow_has_side_effects(workflow: &Value) -> bool {
    // Get the steps object
    let steps = match workflow.get("steps") {
        Some(Value::Object(steps)) => steps,
        _ => return false,
    };

    // Check each step for side effect operations
    for (_step_id, step) in steps {
        // Only check Agent steps (other step types don't execute operators)
        if let Some(Value::String(step_type)) = step.get("stepType")
            && step_type != "Agent"
        {
            continue;
        }

        // Get operator and operation IDs (case-insensitive comparison)
        let operator_id = step
            .get("operatorId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());
        let operation_id = step
            .get("operationId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());

        if let (Some(operator), Some(operation)) = (operator_id, operation_id) {
            // Check if this operator+operation combination has side effects
            for (side_effect_op, side_effect_operation) in SIDE_EFFECT_OPERATIONS {
                if operator == side_effect_op.to_lowercase()
                    && operation == side_effect_operation.to_lowercase()
                {
                    return true;
                }
            }
        }
    }

    false
}

/// Dependency information for a child scenario.
///
/// When a workflow contains `StartScenario` steps, each one creates a dependency
/// on a child workflow. This struct captures the relationship.
#[derive(Debug, Clone)]
pub struct ChildDependency {
    /// The step ID in the parent workflow that starts this child.
    pub step_id: String,
    /// The scenario ID of the child workflow.
    pub child_scenario_id: String,
    /// The version requested (e.g., "latest", "current", or explicit number).
    pub child_version_requested: String,
    /// The resolved version number that will actually be used.
    pub child_version_resolved: i32,
}

/// Input for a child scenario (pre-loaded by caller).
///
/// This crate has no database dependencies, so child scenarios must be loaded
/// by the caller and passed to compilation functions.
#[derive(Debug, Clone)]
pub struct ChildScenarioInput {
    /// The step ID in the parent workflow that references this child.
    pub step_id: String,
    /// The scenario ID of the child workflow.
    pub scenario_id: String,
    /// The version requested (e.g., "latest", "current", or explicit number).
    pub version_requested: String,
    /// The resolved version number.
    pub version_resolved: i32,
    /// The child's execution graph.
    pub execution_graph: ExecutionGraph,
}

/// Input for compilation (all data pre-loaded, no DB access needed).
///
/// This struct contains everything needed to compile a workflow to a native binary.
/// The caller is responsible for loading all required data (including child scenarios)
/// before calling compilation functions.
#[derive(Debug)]
pub struct CompilationInput {
    /// Tenant ID for multi-tenant isolation.
    pub tenant_id: String,
    /// Unique scenario identifier.
    pub scenario_id: String,
    /// Version number for this scenario.
    pub version: u32,
    /// The workflow's execution graph definition.
    pub execution_graph: ExecutionGraph,
    /// Whether to enable debug mode (additional logging).
    pub debug_mode: bool,
    /// Pre-loaded child scenarios (empty if none).
    pub child_scenarios: Vec<ChildScenarioInput>,
    /// URL for fetching connections at runtime.
    /// If provided, generated code will fetch connections from this service.
    /// Expected endpoint: GET {url}/{tenant_id}/{connection_id}
    pub connection_service_url: Option<String>,
}

/// Result of native binary compilation.
///
/// Contains the compiled binary and metadata about the compilation.
#[derive(Debug)]
pub struct NativeCompilationResult {
    /// Path to the compiled binary.
    pub binary_path: PathBuf,
    /// Size of the binary in bytes.
    pub binary_size: usize,
    /// SHA-256 checksum of the binary.
    pub binary_checksum: String,
    /// Path to the build directory containing intermediate files.
    pub build_dir: PathBuf,
    /// Whether the workflow has side effects (e.g., HTTP calls, external actions).
    pub has_side_effects: bool,
    /// Child workflow dependencies.
    pub child_dependencies: Vec<ChildDependency>,
}

/// Get the rustc compilation directory for scenarios
fn get_rustc_compile_dir(tenant_id: &str, workflow_id: &str, version: u32) -> PathBuf {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    let path = PathBuf::from(data_dir)
        .join(tenant_id)
        .join("scenarios")
        .join(workflow_id)
        .join("native_build")
        .join(format!("version_{}", version));
    // Canonicalize to absolute path to avoid CWD-related issues with the linker.
    // The parent directory might not exist yet, so canonicalize the DATA_DIR portion
    // and append the rest.
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

/// Get the native library information (runtime, agents, deps)
fn get_native_libs() -> io::Result<crate::agents_library::NativeLibraryInfo> {
    crate::agents_library::get_native_library()
}

/// Compile a scenario to a native Linux binary
///
/// This is the main compilation entry point. All required data (including child scenarios)
/// must be pre-loaded and passed in the CompilationInput.
///
/// # Arguments
/// * `input` - All compilation inputs including pre-loaded child scenarios
///
/// # Returns
/// Result with native binary compilation data
pub fn compile_scenario(input: CompilationInput) -> io::Result<NativeCompilationResult> {
    let CompilationInput {
        tenant_id,
        scenario_id,
        version,
        execution_graph,
        debug_mode,
        child_scenarios,
        connection_service_url,
    } = input;

    // Validate workflow for security, correctness, and configuration
    let validation_result = crate::validation::validate_workflow(&execution_graph);

    // Log warnings (but don't fail)
    for warning in &validation_result.warnings {
        tracing::warn!(
            tenant_id = %tenant_id,
            scenario_id = %scenario_id,
            version = version,
            warning = %warning,
            "Workflow validation warning"
        );
    }

    // Fail on errors
    if validation_result.has_errors() {
        let error_messages: Vec<String> = validation_result
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect();

        let warning_note = if validation_result.has_warnings() {
            format!(
                "\n\nAdditionally, {} warning(s) were found.",
                validation_result.warnings.len()
            )
        } else {
            String::new()
        };

        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Workflow validation failed with {} error(s):\n\n{}{}",
                validation_result.errors.len(),
                error_messages.join("\n\n"),
                warning_note
            ),
        ));
    }

    // Get native library paths
    let native_libs = get_native_libs()?;

    // Create build directory
    let setup_start = std::time::Instant::now();
    let build_dir = get_rustc_compile_dir(&tenant_id, &scenario_id, version);
    fs::create_dir_all(&build_dir)?;

    // Convert child scenarios to two HashMaps:
    // 1. child_graphs: "{scenario_id}::{version_resolved}" -> ExecutionGraph
    // 2. step_to_child_ref: step_id -> (scenario_id, version_resolved)
    //
    // This prevents collisions when different parent scenarios have StartScenario steps
    // with the same step_id but referencing different child scenarios.
    let child_graphs: HashMap<String, ExecutionGraph> = child_scenarios
        .iter()
        .map(|c| {
            let scenario_ref_key = format!("{}::{}", c.scenario_id, c.version_resolved);
            (scenario_ref_key, c.execution_graph.clone())
        })
        .collect();

    let step_to_child_ref: HashMap<String, (String, i32)> = child_scenarios
        .iter()
        .map(|c| {
            (
                c.step_id.clone(),
                (c.scenario_id.clone(), c.version_resolved),
            )
        })
        .collect();

    // Generate the Rust program using AST-based code generation
    let codegen_start = std::time::Instant::now();
    let tenant_id_for_codegen = tenant_id.clone();
    let rust_code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ast::compile_with_children(
            &execution_graph,
            debug_mode,
            child_graphs,
            step_to_child_ref,
            connection_service_url,
            Some(tenant_id_for_codegen),
        )
    }))
    .map_err(|panic_info| {
        let panic_msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic during code generation".to_string()
        };

        tracing::error!(
            tenant_id = %tenant_id,
            scenario_id = %scenario_id,
            version = version,
            error = %panic_msg,
            "Code generation panicked"
        );

        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Code generation failed: {}", panic_msg),
        )
    })?
    // Handle codegen errors (e.g., missing child scenario)
    .map_err(|codegen_err| {
        tracing::error!(
            tenant_id = %tenant_id,
            scenario_id = %scenario_id,
            version = version,
            error = %codegen_err,
            "Code generation failed"
        );

        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Code generation failed: {}", codegen_err),
        )
    })?;
    let codegen_duration = codegen_start.elapsed();
    tracing::debug!(
        codegen_duration_ms = codegen_duration.as_millis() as u64,
        "Code generation completed"
    );

    let main_rs_path = build_dir.join("main.rs");
    fs::write(&main_rs_path, rust_code)?;
    let setup_duration = setup_start.elapsed();
    tracing::debug!(
        setup_duration_ms = setup_duration.as_millis() as u64,
        "Setup completed (dirs + codegen + write)"
    );

    // Determine binary output path
    let binary_output_path = build_dir.join("scenario");

    // Compile with rustc to native binary
    let compilation_start = std::time::Instant::now();

    // Log the CWD for debugging path resolution issues
    let cwd = std::env::current_dir().unwrap_or_default();
    info!(
        scenario_id = %scenario_id,
        version = version,
        mode = "native",
        cwd = %cwd.display(),
        build_dir = %build_dir.display(),
        "Starting scenario compilation"
    );

    // Build rustc command for native binary
    let target = get_host_target();
    let mut cmd = Command::new("rustc");

    // Set explicit working directory to avoid CWD issues in async contexts
    cmd.current_dir(&cwd);

    // Clear RUSTFLAGS to ensure consistent behavior across environments
    // (some production environments may have -D warnings set globally)
    cmd.env_remove("RUSTFLAGS");

    // Default opt-level=0: skips LLVM optimization passes entirely (~2x faster compilation).
    // The generated code is all function calls into pre-optimized library code, so
    // opt-level>0 only optimizes glue code with negligible runtime benefit.
    let opt_level = std::env::var("RUNTARA_OPT_LEVEL").unwrap_or_else(|_| "0".to_string());
    let codegen_units = std::env::var("RUNTARA_CODEGEN_UNITS").unwrap_or_else(|_| "1".to_string());

    cmd.arg(format!("--target={}", target))
        .arg("--crate-type=bin")
        .arg("--edition=2024")
        .arg("-C")
        .arg(format!("opt-level={}", opt_level))
        .arg("-C")
        .arg(format!("codegen-units={}", codegen_units));

    // Strip symbols to reduce binary size (skip in debug mode for stack traces)
    if !debug_mode {
        cmd.arg("-C").arg("strip=symbols");
    }

    // Use static CRT linking on Linux (musl) for fully static binaries
    #[cfg(target_os = "linux")]
    {
        cmd.arg("-C").arg("target-feature=+crt-static");
    }

    // Add library search paths
    let deps_dir = &native_libs.deps_dir;
    if deps_dir.exists() {
        cmd.arg("-L")
            .arg(format!("dependency={}", deps_dir.display()));
    }

    // Add native library path (parent of scenario lib)
    if let Some(lib_dir) = native_libs.scenario_lib_path.parent() {
        cmd.arg("-L").arg(format!("native={}", lib_dir.display()));
    }

    // Add OpenSSL library paths for macOS (Homebrew)
    #[cfg(target_os = "macos")]
    {
        // Try common Homebrew OpenSSL locations
        let openssl_paths = [
            "/opt/homebrew/opt/openssl/lib", // ARM64
            "/usr/local/opt/openssl/lib",    // Intel
            "/opt/homebrew/opt/openssl@3/lib",
            "/usr/local/opt/openssl@3/lib",
        ];
        for path in &openssl_paths {
            if std::path::Path::new(path).exists() {
                cmd.arg("-L").arg(format!("native={}", path));
                break;
            }
        }
    }

    // Add extern crate for the unified workflow stdlib library
    let stdlib_name = crate::agents_library::get_stdlib_name();
    cmd.arg("--extern").arg(format!(
        "{}={}",
        stdlib_name,
        native_libs.scenario_lib_path.display()
    ));

    // Determine the dylib extension for the current platform
    #[cfg(target_os = "macos")]
    let dylib_ext = "dylib";
    #[cfg(target_os = "linux")]
    let dylib_ext = "so";
    #[cfg(target_os = "windows")]
    let dylib_ext = "dll";

    // Add ALL dependency rlibs AND dylibs as extern crates (needed for transitive dependency resolution)
    // Proc-macro crates are compiled as dylibs, not rlibs
    if deps_dir.exists()
        && let Ok(entries) = fs::read_dir(deps_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());

            // Accept both rlibs and dylibs (proc-macros)
            if ext != Some("rlib") && ext != Some(dylib_ext) {
                continue;
            }

            if let Some(filename) = path.file_name().and_then(|n| n.to_str())
                && let Some(crate_name_part) = filename.strip_prefix("lib")
                && let Some(crate_name) = crate_name_part.split('-').next()
            {
                // Convert hyphens to underscores for crate names
                let extern_name = crate_name.replace('-', "_");
                cmd.arg("--extern")
                    .arg(format!("{}={}", extern_name, path.display()));
            }
        }
    }

    // Output
    cmd.arg("-o").arg(&binary_output_path);

    // Input
    cmd.arg(&main_rs_path);

    // Log the full command for debugging
    let cmd_str = format!("{:?}", cmd);
    tracing::info!(
        tenant_id = %tenant_id,
        scenario_id = %scenario_id,
        version = version,
        command = %cmd_str,
        "Invoking rustc for native compilation"
    );
    let rustc_start = std::time::Instant::now();
    let output = cmd.output().map_err(|e| {
        io::Error::other(format!(
            "Failed to execute rustc: {}. Make sure rustc is installed with {} target.",
            e, target
        ))
    })?;
    let rustc_duration = rustc_start.elapsed();
    tracing::info!(
        rustc_duration_ms = rustc_duration.as_millis() as u64,
        "Rustc compilation completed"
    );

    // Check if compilation was successful
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Log the full error for debugging
        tracing::error!(
            stderr = %stderr,
            stdout = %stdout,
            "Rustc compilation failed"
        );

        // Parse and provide user-friendly error message
        let user_message = parse_rustc_error(&stderr, target);
        return Err(io::Error::other(user_message));
    }

    // Verify the binary was created
    if !binary_output_path.exists() {
        return Err(io::Error::other(format!(
            "Compilation appeared to succeed but binary was not found at {:?}",
            binary_output_path
        )));
    }

    // Get binary size and calculate checksum
    let io_start = std::time::Instant::now();
    let binary_metadata = fs::metadata(&binary_output_path).map_err(|e| {
        io::Error::other(format!(
            "Failed to stat binary at {:?}: {}",
            binary_output_path, e
        ))
    })?;
    let binary_size = binary_metadata.len() as usize;

    // Calculate checksum by reading in chunks (more memory efficient)
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(&binary_output_path)?;
    std::io::copy(&mut file, &mut hasher)?;
    let binary_checksum = format!("{:x}", hasher.finalize());

    let io_duration = io_start.elapsed();
    tracing::debug!(
        io_duration_ms = io_duration.as_millis() as u64,
        binary_size_bytes = binary_size,
        "Checksum calculated"
    );

    // Detect side effects from the scenario JSON file if it exists
    let scenario_json_path = get_scenario_json_path(&tenant_id, &scenario_id, version);
    let has_side_effects = if scenario_json_path.exists() {
        let json_content = fs::read_to_string(&scenario_json_path)?;
        let workflow: Value = serde_json::from_str(&json_content).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse scenario JSON: {}", e),
            )
        })?;
        workflow_has_side_effects(&workflow)
    } else {
        false
    };

    let compilation_duration = compilation_start.elapsed();
    info!(
        tenant_id = %tenant_id,
        scenario_id = %scenario_id,
        version = version,
        binary_size_bytes = binary_size,
        compilation_duration_ms = compilation_duration.as_millis() as u64,
        has_side_effects = has_side_effects,
        "Scenario compiled successfully"
    );

    // Convert child scenarios to dependencies
    let child_dependencies: Vec<ChildDependency> = child_scenarios
        .iter()
        .map(|c| ChildDependency {
            step_id: c.step_id.clone(),
            child_scenario_id: c.scenario_id.clone(),
            child_version_requested: c.version_requested.clone(),
            child_version_resolved: c.version_resolved,
        })
        .collect();

    Ok(NativeCompilationResult {
        binary_path: binary_output_path,
        binary_size,
        binary_checksum,
        build_dir,
        has_side_effects,
        child_dependencies,
    })
}

/// Translate (generate code only, no compilation)
///
/// # Arguments
/// * `tenant_id` - Tenant identifier
/// * `scenario_id` - Scenario identifier
/// * `version` - Version number
/// * `execution_graph` - The execution graph
/// * `debug_mode` - Whether to include debug instrumentation
///
/// # Returns
/// Path to the build directory containing generated main.rs
pub fn translate_scenario(
    tenant_id: &str,
    scenario_id: &str,
    version: u32,
    execution_graph: &ExecutionGraph,
    debug_mode: bool,
) -> io::Result<PathBuf> {
    // Create build directory
    let build_dir = get_rustc_compile_dir(tenant_id, scenario_id, version);
    fs::create_dir_all(&build_dir)?;

    // Generate the Rust program using AST-based code generation
    let rust_code = ast::compile(execution_graph, debug_mode).map_err(|codegen_err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Code generation failed: {}", codegen_err),
        )
    })?;

    // Write main.rs
    let main_rs_path = build_dir.join("main.rs");
    fs::write(&main_rs_path, &rust_code)?;

    info!(
        "Generated Rust code for scenario {}/{} v{} at {:?}",
        tenant_id, scenario_id, version, main_rs_path
    );

    Ok(build_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_has_side_effects_empty() {
        let workflow: Value = serde_json::json!({
            "steps": {}
        });
        assert!(!workflow_has_side_effects(&workflow));
    }

    #[test]
    fn test_workflow_has_side_effects_http() {
        let workflow: Value = serde_json::json!({
            "steps": {
                "step1": {
                    "stepType": "Agent",
                    "operatorId": "http",
                    "operationId": "http-request"
                }
            }
        });
        assert!(workflow_has_side_effects(&workflow));
    }

    #[test]
    fn test_workflow_has_side_effects_pure() {
        let workflow: Value = serde_json::json!({
            "steps": {
                "step1": {
                    "stepType": "Agent",
                    "operatorId": "transform",
                    "operationId": "map"
                }
            }
        });
        assert!(!workflow_has_side_effects(&workflow));
    }
}
