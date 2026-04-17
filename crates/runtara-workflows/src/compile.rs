// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Rustc-based scenario compilation to native binaries
//!
//! This module provides compilation without database dependencies.
//! Child scenarios must be pre-loaded and passed to compilation functions.
//!
//! Scenarios are compiled to native binaries for the host platform.

use runtara_dsl::ExecutionGraph;
use serde::Deserialize;
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

/// A single rustc diagnostic parsed from `--error-format=json` output.
///
/// Only the fields we actually inspect are modeled; everything uses
/// `#[serde(default)]` so the struct tolerates schema additions in newer
/// toolchains without failing to deserialize.
#[derive(Debug, Deserialize)]
struct RustcDiagnostic {
    #[serde(rename = "$message_type", default)]
    message_type: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: Option<RustcCode>,
    #[serde(default)]
    level: String,
    #[serde(default)]
    rendered: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RustcCode {
    code: String,
}

/// Parse newline-delimited rustc JSON diagnostics from stderr.
///
/// Lines that do not start with `{` or fail to deserialize are silently
/// dropped. Returns only entries at `level == "error"` and of kind
/// `diagnostic` (or unspecified, for forward-compat).
fn parse_json_diagnostics(stderr: &str) -> Vec<RustcDiagnostic> {
    stderr
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('{') {
                return None;
            }
            serde_json::from_str::<RustcDiagnostic>(trimmed).ok()
        })
        .filter(|d| {
            d.level == "error" && matches!(d.message_type.as_deref(), Some("diagnostic") | None)
        })
        .collect()
}

/// Maximum characters of rendered rustc output to include in the fallback
/// user-facing error. Keeps API/CLI payloads bounded.
const FALLBACK_RENDER_CAP: usize = 500;

fn truncate_for_display(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= FALLBACK_RENDER_CAP {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(FALLBACK_RENDER_CAP).collect();
    format!("{}…", head)
}

fn error_code(d: &RustcDiagnostic) -> Option<&str> {
    d.code.as_ref().map(|c| c.code.as_str())
}

/// Parse rustc stderr and provide a user-friendly error message.
///
/// Consumes JSON diagnostics produced by `rustc --error-format=json`. The
/// structured output is stable across toolchain releases, unlike the English
/// text of individual error messages.
fn parse_rustc_error(stderr: &str, target: &str) -> String {
    let diagnostics = parse_json_diagnostics(stderr);

    // Rule 1: E0463 + missing `std` -> suggest rustup target add.
    if diagnostics
        .iter()
        .any(|d| error_code(d) == Some("E0463") && d.message.contains("`std`"))
    {
        return format!(
            "Compilation failed: The Rust standard library for target '{}' is not installed.\n\n\
             To fix this, run:\n  rustup target add {}",
            target, target
        );
    }

    // Rule 2: unknown target specification -> suggest rustup target add.
    // Case-insensitive: historical rustc releases have emitted both
    // "could not find specification" and "Could not find specification".
    if diagnostics.iter().any(|d| {
        d.message
            .to_lowercase()
            .contains("could not find specification for target")
    }) {
        return format!(
            "Compilation failed: Target '{}' is not installed.\n\n\
             To fix this, run:\n  rustup target add {}",
            target, target
        );
    }

    // Rule 3: missing linker on a musl target.
    if target.contains("musl")
        && diagnostics
            .iter()
            .any(|d| d.message.contains("linker") && d.message.contains("not found"))
    {
        return "Compilation failed: The musl linker is not installed.\n\n\
                 To fix this on Ubuntu/Debian, run:\n  sudo apt install musl-tools\n\n\
                 To fix this on Fedora/RHEL, run:\n  sudo dnf install musl-gcc"
            .to_string();
    }

    // Rules 4 & 5: E0463 for any other crate -> stdlib build hint.
    if let Some(crate_name) = diagnostics.iter().find_map(|d| {
        if error_code(d) == Some("E0463") {
            extract_pattern(&d.message, "can't find crate for `", "`").map(str::to_string)
        } else {
            None
        }
    }) {
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

    // Rule 6: E0432 unresolved import -> codegen bug with import name.
    if let Some(import) = diagnostics.iter().find_map(|d| {
        if error_code(d) == Some("E0432") {
            extract_pattern(&d.message, "unresolved import `", "`").map(str::to_string)
        } else {
            None
        }
    }) {
        return format!(
            "Compilation failed: Unresolved import '{}'.\n\n\
                 This is likely a code generation bug. Please report this issue.",
            import
        );
    }

    // Rule 7: E0308 mismatched types -> codegen bug.
    if diagnostics
        .iter()
        .any(|d| error_code(d) == Some("E0308") && d.message.contains("mismatched types"))
    {
        return "Compilation failed: Type mismatch in generated code.\n\n\
             This is likely a code generation bug. Please report this issue."
            .to_string();
    }

    // Rule 8: borrow checker codegen bugs.
    if diagnostics
        .iter()
        .any(|d| matches!(error_code(d), Some("E0382") | Some("E0502") | Some("E0499")))
    {
        return "Compilation failed: Borrow checker error in generated code.\n\n\
             This is likely a code generation bug. Please report this issue."
            .to_string();
    }

    // Rule 9: generic fallback using the first parsed diagnostic.
    if let Some(first) = diagnostics.first() {
        let display = first.rendered.as_deref().unwrap_or(&first.message);
        return format!(
            "Compilation failed: {}\n\n\
             If this error persists, please contact support.",
            truncate_for_display(display)
        );
    }

    // Rule 10: no JSON diagnostics parsed. If stderr has any raw text (rare —
    // e.g. a driver panic before JSON emission), surface the head of it;
    // otherwise emit the generic message.
    let raw = stderr.trim();
    if !raw.is_empty() {
        return format!(
            "Compilation failed: {}\n\n\
             If this error persists, please contact support.",
            truncate_for_display(raw)
        );
    }
    "Compilation failed. Please contact support if this issue persists.".to_string()
}

/// Extract a pattern from text: prefix...suffix
fn extract_pattern<'a>(text: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    let end = rest.find(suffix)?;
    Some(&rest[..end])
}

/// Get the native target triple for the current host platform
///
/// This must match the target used in build.rs when precompiling libraries.
/// We use musl on Linux for static linking (scenarios run in minimal containers).
#[allow(dead_code)]
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
    pub track_events: bool,
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
    /// Default variable values from the scenario definition.
    /// Callers should include these in image metadata so the environment
    /// can enrich stored input with defaults at instance start time.
    pub default_variables: serde_json::Value,
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
fn get_native_libs(target: Option<&str>) -> io::Result<crate::agents_library::NativeLibraryInfo> {
    match target {
        Some(t) if t.contains("wasm") => crate::agents_library::get_wasm_native_library(),
        _ => crate::agents_library::get_native_library(),
    }
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
        track_events,
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

    // Resolve target triple from RUNTARA_COMPILE_TARGET env var (default: wasm32-wasip2)
    let resolved_target =
        std::env::var("RUNTARA_COMPILE_TARGET").unwrap_or_else(|_| "wasm32-wasip2".to_string());
    let is_wasm = resolved_target.contains("wasm");

    // Get native library paths (target-aware)
    let native_libs = get_native_libs(if is_wasm {
        Some(&resolved_target)
    } else {
        None
    })?;

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
            track_events,
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

    // Determine binary output path (WASM uses .wasm extension)
    let binary_name = if is_wasm { "scenario.wasm" } else { "scenario" };
    let binary_output_path = build_dir.join(binary_name);

    // Compile with rustc to native binary
    let compilation_start = std::time::Instant::now();

    // Log the CWD for debugging path resolution issues
    let cwd = std::env::current_dir().unwrap_or_default();
    let mode = if is_wasm { "wasm" } else { "native" };
    let target = &resolved_target;
    info!(
        scenario_id = %scenario_id,
        version = version,
        mode = mode,
        target = %target,
        cwd = %cwd.display(),
        build_dir = %build_dir.display(),
        "Starting scenario compilation"
    );

    // Build rustc command
    let mut cmd = Command::new("rustc");

    // Set explicit working directory to avoid CWD issues in async contexts
    cmd.current_dir(&cwd);

    // Clear RUSTFLAGS to ensure consistent behavior across environments
    // (some production environments may have -D warnings set globally)
    cmd.env_remove("RUSTFLAGS");

    // Default opt-level=0 for native: skips LLVM optimization passes entirely (~2x faster compilation).
    // The generated code is all function calls into pre-optimized library code, so
    // opt-level>0 only optimizes glue code with negligible runtime benefit.
    // For WASM: default to opt-level=s (optimize for size) since binary size is critical
    // and dead code elimination needs optimization passes to work effectively.
    let default_opt = if is_wasm { "s" } else { "0" };
    let opt_level = std::env::var("RUNTARA_OPT_LEVEL").unwrap_or_else(|_| default_opt.to_string());
    let codegen_units = std::env::var("RUNTARA_CODEGEN_UNITS").unwrap_or_else(|_| "1".to_string());

    cmd.arg(format!("--target={}", target))
        .arg("--crate-type=bin")
        .arg("--edition=2024")
        .arg("--error-format=json")
        .arg("-C")
        .arg(format!("opt-level={}", opt_level))
        .arg("-C")
        .arg(format!("codegen-units={}", codegen_units));

    // WASM: enable LTO for cross-crate dead code elimination.
    // The build script compiles rlibs with -C embed-bitcode=yes to support this.
    if is_wasm {
        let lto_level = std::env::var("RUNTARA_LTO").unwrap_or_else(|_| "fat".to_string());
        if lto_level != "off" {
            cmd.arg("-C").arg(format!("lto={}", lto_level));
        }
    }

    // Skip strip and crt-static for WASM targets
    if !is_wasm {
        // Strip symbols to reduce binary size (skip in debug mode for stack traces)
        if !track_events {
            cmd.arg("-C").arg("strip=symbols");
        }

        // Use static CRT linking on Linux (musl) for fully static binaries
        #[cfg(target_os = "linux")]
        {
            cmd.arg("-C").arg("target-feature=+crt-static");
        }
    }

    // Add library search paths
    let deps_dir = &native_libs.deps_dir;
    if deps_dir.exists() {
        cmd.arg("-L")
            .arg(format!("dependency={}", deps_dir.display()));
        // Also add as native search path for .a files (e.g., wit-bindgen-rt's cabi_realloc)
        cmd.arg("-L").arg(format!("native={}", deps_dir.display()));
    }

    // Add native library path (parent of scenario lib)
    if let Some(lib_dir) = native_libs.scenario_lib_path.parent() {
        cmd.arg("-L").arg(format!("native={}", lib_dir.display()));
    }

    // Add OpenSSL library paths for macOS (Homebrew) — not needed for WASM
    #[cfg(target_os = "macos")]
    if !is_wasm {
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

    // Determine the dylib extension for the current host platform
    // (proc-macro dylibs are always compiled for the host, even when cross-compiling)
    #[cfg(target_os = "macos")]
    let dylib_ext = "dylib";
    #[cfg(target_os = "linux")]
    let dylib_ext = "so";
    #[cfg(target_os = "windows")]
    let dylib_ext = "dll";

    // Add ALL dependency rlibs AND dylibs as extern crates (needed for transitive dependency resolution)
    // Proc-macro crates are compiled as dylibs, not rlibs
    // Skip the stdlib itself (already added explicitly above) to avoid
    // E0464 "multiple candidates" when deps_dir contains extra copies.
    //
    // Deduplicate by crate name: when multiple versions of the same crate exist
    // (e.g., libruntara_sdk-aaa.rlib and libruntara_sdk-bbb.rlib), keep only
    // the most recently modified one to avoid E0464.
    if deps_dir.exists()
        && let Ok(entries) = fs::read_dir(deps_dir)
    {
        let mut extern_crates: HashMap<String, std::path::PathBuf> = HashMap::new();

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
                // Skip stdlib — it's already added explicitly via scenario_lib_path
                if extern_name == stdlib_name {
                    continue;
                }

                // Keep the most recently modified file when duplicates exist
                let dominated = extern_crates.get(&extern_name).is_some_and(|existing| {
                    let existing_mtime = fs::metadata(existing)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    let new_mtime = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    new_mtime > existing_mtime
                });

                if !extern_crates.contains_key(&extern_name) || dominated {
                    extern_crates.insert(extern_name, path);
                }
            }
        }

        for (extern_name, path) in &extern_crates {
            cmd.arg("--extern")
                .arg(format!("{}={}", extern_name, path.display()));
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
        mode = mode,
        command = %cmd_str,
        "Invoking rustc for scenario compilation"
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

    // Extract default variable values from the execution graph for image metadata
    let default_variables = if execution_graph.variables.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::Value::Object(
            execution_graph
                .variables
                .iter()
                .map(|(name, var)| (name.clone(), var.value.clone()))
                .collect(),
        )
    };

    Ok(NativeCompilationResult {
        binary_path: binary_output_path,
        binary_size,
        binary_checksum,
        build_dir,
        has_side_effects,
        child_dependencies,
        default_variables,
    })
}

/// Translate (generate code only, no compilation)
///
/// # Arguments
/// * `tenant_id` - Tenant identifier
/// * `scenario_id` - Scenario identifier
/// * `version` - Version number
/// * `execution_graph` - The execution graph
/// * `track_events` - Whether to include debug instrumentation
///
/// # Returns
/// Path to the build directory containing generated main.rs
pub fn translate_scenario(
    tenant_id: &str,
    scenario_id: &str,
    version: u32,
    execution_graph: &ExecutionGraph,
    track_events: bool,
) -> io::Result<PathBuf> {
    // Create build directory
    let build_dir = get_rustc_compile_dir(tenant_id, scenario_id, version);
    fs::create_dir_all(&build_dir)?;

    // Generate the Rust program using AST-based code generation
    let rust_code = ast::compile(execution_graph, track_events).map_err(|codegen_err| {
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

    // ========================================================================
    // parse_rustc_error tests
    //
    // Fixtures are minimal newline-delimited JSON diagnostics mirroring
    // `rustc --error-format=json` output (rustc 1.90). Only the fields the
    // parser inspects (`$message_type`, `level`, `code`, `message`,
    // `rendered`) are included.
    // ========================================================================

    const TARGET: &str = "aarch64-unknown-linux-musl";

    const E0463_STD: &str = r#"{"$message_type":"diagnostic","message":"can't find crate for `std`","code":{"code":"E0463","explanation":null},"level":"error","rendered":"error[E0463]: can't find crate for `std`\n"}
{"$message_type":"diagnostic","message":"aborting due to 1 previous error","code":null,"level":"error","rendered":"error: aborting due to 1 previous error\n"}
"#;

    const TARGET_SPEC_MISSING: &str = r#"{"$message_type":"diagnostic","message":"Error loading target specification: Could not find specification for target \"aarch64-unknown-linux-musl\". Run `rustc --print target-list` for a list of built-in targets","code":null,"level":"error","rendered":"error: Error loading target specification: Could not find specification for target \"aarch64-unknown-linux-musl\"\n"}
"#;

    const LINKER_NOT_FOUND: &str = r#"{"$message_type":"diagnostic","message":"linker `musl-gcc` not found","code":null,"level":"error","rendered":"error: linker `musl-gcc` not found\n"}
{"$message_type":"diagnostic","message":"aborting due to 1 previous error","code":null,"level":"error","rendered":"error: aborting due to 1 previous error\n"}
"#;

    const E0463_STDLIB: &str = r#"{"$message_type":"diagnostic","message":"can't find crate for `runtara_workflow_stdlib`","code":{"code":"E0463","explanation":null},"level":"error","rendered":"error[E0463]: can't find crate for `runtara_workflow_stdlib`\n"}
"#;

    const E0463_OTHER_CRATE: &str = r#"{"$message_type":"diagnostic","message":"can't find crate for `serde`","code":{"code":"E0463","explanation":null},"level":"error","rendered":"error[E0463]: can't find crate for `serde`\n"}
"#;

    const E0432_UNRESOLVED_IMPORT: &str = r#"{"$message_type":"diagnostic","message":"unresolved import `foo::bar`","code":{"code":"E0432","explanation":null},"level":"error","rendered":"error[E0432]: unresolved import `foo::bar`\n"}
"#;

    const E0308_MISMATCH: &str = r#"{"$message_type":"diagnostic","message":"mismatched types","code":{"code":"E0308","explanation":null},"level":"error","rendered":"error[E0308]: mismatched types\n"}
"#;

    const E0382_BORROW: &str = r#"{"$message_type":"diagnostic","message":"borrow of moved value: `x`","code":{"code":"E0382","explanation":null},"level":"error","rendered":"error[E0382]: borrow of moved value: `x`\n"}
"#;

    const UNKNOWN_CODE: &str = r#"{"$message_type":"diagnostic","message":"attributes are not yet allowed on `if` expressions","code":{"code":"E0658","explanation":null},"level":"error","rendered":"error[E0658]: attributes are not yet allowed on `if` expressions\n"}
"#;

    const NON_JSON_STDERR: &str = "rustc panicked: something went terribly wrong\n";

    #[test]
    fn parse_rustc_error_e0463_std() {
        let msg = parse_rustc_error(E0463_STD, TARGET);
        assert!(
            msg.contains("rustup target add aarch64-unknown-linux-musl"),
            "msg was: {}",
            msg
        );
    }

    #[test]
    fn parse_rustc_error_target_spec_missing() {
        let msg = parse_rustc_error(TARGET_SPEC_MISSING, TARGET);
        assert!(msg.contains("rustup target add"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_linker_not_found_musl() {
        let msg = parse_rustc_error(LINKER_NOT_FOUND, TARGET);
        assert!(msg.contains("musl-tools"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_linker_not_found_non_musl_falls_through() {
        // Same linker message but target doesn't contain "musl": rule 3
        // should not fire; we fall through to rule 9 (generic wrapper).
        let msg = parse_rustc_error(LINKER_NOT_FOUND, "x86_64-apple-darwin");
        assert!(!msg.contains("musl-tools"), "msg was: {}", msg);
        assert!(msg.contains("Compilation failed"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_e0463_stdlib() {
        let msg = parse_rustc_error(E0463_STDLIB, TARGET);
        assert!(
            msg.contains("runtara-workflow-stdlib --release"),
            "msg was: {}",
            msg
        );
    }

    #[test]
    fn parse_rustc_error_e0463_other_crate() {
        let msg = parse_rustc_error(E0463_OTHER_CRATE, TARGET);
        assert!(msg.contains("serde"), "msg was: {}", msg);
        assert!(msg.contains("runtara-workflow-stdlib"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_e0432() {
        let msg = parse_rustc_error(E0432_UNRESOLVED_IMPORT, TARGET);
        assert!(msg.contains("Unresolved import"), "msg was: {}", msg);
        assert!(msg.contains("foo::bar"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_e0308() {
        let msg = parse_rustc_error(E0308_MISMATCH, TARGET);
        assert!(
            msg.contains("Type mismatch in generated code"),
            "msg was: {}",
            msg
        );
    }

    #[test]
    fn parse_rustc_error_e0382_borrow() {
        let msg = parse_rustc_error(E0382_BORROW, TARGET);
        assert!(msg.contains("Borrow checker error"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_unknown_code_falls_through_to_render() {
        let msg = parse_rustc_error(UNKNOWN_CODE, TARGET);
        assert!(msg.contains("Compilation failed:"), "msg was: {}", msg);
        assert!(
            msg.contains("attributes are not yet allowed"),
            "msg was: {}",
            msg
        );
    }

    #[test]
    fn parse_rustc_error_non_json_stderr() {
        let msg = parse_rustc_error(NON_JSON_STDERR, TARGET);
        assert!(msg.contains("contact support"), "msg was: {}", msg);
        assert!(msg.contains("rustc panicked"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_empty_stderr() {
        let msg = parse_rustc_error("", TARGET);
        assert!(msg.contains("contact support"), "msg was: {}", msg);
    }

    #[test]
    fn parse_rustc_error_ignores_artifact_and_warning_lines() {
        // An artifact message and a warning should both be ignored; the real
        // error (E0308) should still be classified.
        let stderr = format!(
            "{}{}{}",
            r#"{"$message_type":"artifact","artifact":"/tmp/foo","emit":"metadata"}
"#,
            r#"{"$message_type":"diagnostic","message":"unused variable: `x`","code":{"code":"unused_variables","explanation":null},"level":"warning","rendered":"warning: unused variable\n"}
"#,
            E0308_MISMATCH
        );
        let msg = parse_rustc_error(&stderr, TARGET);
        assert!(
            msg.contains("Type mismatch in generated code"),
            "msg was: {}",
            msg
        );
    }

    #[test]
    fn parse_json_diagnostics_filters_non_errors() {
        let stderr = r#"{"$message_type":"diagnostic","message":"warn","code":null,"level":"warning"}
{"$message_type":"artifact","artifact":"/tmp/foo"}
not json at all
{"$message_type":"diagnostic","message":"err","code":null,"level":"error"}
"#;
        let diags = parse_json_diagnostics(stderr);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "err");
    }
}
