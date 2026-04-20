//! Dispatcher Service
//!
//! Compiles and manages the universal agent dispatcher binary.
//! The dispatcher allows testing individual agents in isolation without
//! creating a full workflow.

use runtara_management_sdk::{RegisterImageStreamOptions, RunnerType};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::runtime_client::RuntimeClient;

/// The dispatcher source code template
///
/// This is a minimal binary that:
/// 1. Loads input from runtara-core via SDK (same as compiled workflows)
/// 2. Optionally injects connection as _connection field (same as test harness)
/// 3. Executes the requested agent capability via the registry
/// 4. Reports the result via SDK protocol
const DISPATCHER_SOURCE: &str = r#"//! Universal Agent Dispatcher
//!
//! A binary that accepts agent/capability/input dynamically and dispatches
//! to the appropriate agent via the static dispatch table.
//!
//! Uses the runtara SDK protocol to report completion/failure back to
//! runtara-core (same as compiled workflows).
//!
//! Connection is injected as `_connection` field in the input, which agents
//! extract directly during execution (no thread-local storage needed).

extern crate runtara_workflow_stdlib;

use runtara_workflow_stdlib::prelude::*;
use runtara_workflow_stdlib::dispatch;
use std::process::ExitCode;

/// Input structure for the dispatcher
#[derive(serde::Deserialize)]
struct DispatcherInput {
    /// Agent module name (e.g., "utils", "http", "shopify")
    agent_id: String,
    /// Capability ID (e.g., "random-double", "http-request")
    capability_id: String,
    /// Agent-specific input
    agent_input: serde_json::Value,
    /// Optional connection data for agents that require external connections
    /// This is injected as _connection field in agent_input
    #[serde(default)]
    connection: Option<serde_json::Value>,
}

fn main() -> ExitCode {
    // Initialize SDK from environment variables (injected by OCI runner)
    // Required env vars: RUNTARA_INSTANCE_ID, RUNTARA_TENANT_ID
    let mut sdk_instance = match RuntaraSdk::from_env() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: Failed to initialize SDK: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Connect to runtara-core
    if let Err(e) = sdk_instance.connect() {
        eprintln!("ERROR: Failed to connect to runtara-core: {}", e);
        return ExitCode::FAILURE;
    }

    // Register the instance
    if let Err(e) = sdk_instance.register(None) {
        eprintln!("ERROR: Failed to register instance: {}", e);
        return ExitCode::FAILURE;
    }

    // Register SDK globally
    register_sdk(sdk_instance);

    // Load input from runtara-core via SDK (same as compiled workflows)
    let raw_input: serde_json::Value = {
        let sdk_guard = sdk().lock().unwrap();
        match sdk_guard.load_input() {
            Ok(Some(bytes)) => match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    let msg = format!("Failed to parse input from Core: {}", e);
                    eprintln!("ERROR: {}", msg);
                    let _ = sdk_guard.failed(&msg);
                    return ExitCode::FAILURE;
                }
            },
            Ok(None) => {
                let msg = "No input provided";
                eprintln!("ERROR: {}", msg);
                let _ = sdk_guard.failed(msg);
                return ExitCode::FAILURE;
            },
            Err(e) => {
                let msg = format!("Failed to load input from Core: {}", e);
                eprintln!("ERROR: {}", msg);
                let _ = sdk_guard.failed(&msg);
                return ExitCode::FAILURE;
            }
        }
    };

    // Parse dispatcher input
    let input: DispatcherInput = match serde_json::from_value(raw_input) {
        Ok(input) => input,
        Err(e) => {
            let output = serde_json::json!({
                "success": false,
                "error": format!("Invalid dispatcher input: {}", e)
            });
            let output_bytes = serde_json::to_vec(&output).unwrap_or_default();
            let sdk_guard = sdk().lock().unwrap();
            let _ = sdk_guard.completed(&output_bytes);
            return ExitCode::SUCCESS;
        }
    };

    // Prepare agent input - inject _connection if provided
    let mut agent_input_value = input.agent_input;

    if let Some(conn) = input.connection {
        if let Some(obj) = agent_input_value.as_object_mut() {
            obj.insert("_connection".to_string(), conn);
        }
    }

    // Execute the agent capability via static dispatch table (WASM-compatible)
    let result = dispatch::execute_capability(
        &input.agent_id,
        &input.capability_id,
        agent_input_value,
    );

    // Build output
    let output = match result {
        Ok(value) => serde_json::json!({
            "success": true,
            "output": value
        }),
        Err(err) => serde_json::json!({
            "success": false,
            "error": err
        }),
    };

    // Report completion via SDK (same as compiled workflows)
    let output_bytes = serde_json::to_vec(&output).unwrap_or_default();
    let sdk_guard = sdk().lock().unwrap();
    match sdk_guard.completed(&output_bytes) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: Failed to report completion: {}", e);
            ExitCode::FAILURE
        }
    }
}
"#;

/// Special workflow ID used for the dispatcher
pub const DISPATCHER_WORKFLOW_ID: &str = "__agent_dispatcher__";

/// Dispatcher version - increment when changing DISPATCHER_SOURCE
/// v10: Fixed bundle_path to use absolute paths for OCI runner
/// v11: Fixed async/await - execute_capability now returns a Future
/// v12: Added datetime agent support from runtara-agents
/// v14: Changed from INPUT_JSON env var to /data/input.json file (runtara input size fix)
/// v17: Added compression and file agents from runtara-agents 1.3.1
/// v19: Added fallback connection injection for object_model agent from OBJECT_STORE_DATABASE_URL
/// v21: Switched from write_completed/output.json to SDK protocol (QUIC to runtara-core)
/// v22: Compile to WASM (same mechanism as workflows) instead of native host binary
/// v23: Load input from runtara-core via SDK instead of /data/input.json (WASM has no filesystem)
/// v24: Use static dispatch table instead of inventory registry (inventory unavailable in WASM)
pub const DISPATCHER_VERSION: u32 = 27;

/// Service for managing the agent dispatcher binary
pub struct DispatcherService {
    runtime_client: Arc<RuntimeClient>,
    /// Cache of registered dispatcher image IDs per tenant
    image_cache: Arc<RwLock<HashMap<String, String>>>,
}

impl DispatcherService {
    pub fn new(runtime_client: Arc<RuntimeClient>) -> Self {
        Self {
            runtime_client,
            image_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initialize the dispatcher for a tenant at startup.
    ///
    /// This should be called once during server startup. It checks if the dispatcher
    /// image already exists and only compiles/registers if needed.
    ///
    /// Returns the image ID that can be used to execute the dispatcher.
    pub async fn initialize(&self, tenant_id: &str) -> Result<String, DispatcherError> {
        let image_name = format!("{}:{}", DISPATCHER_WORKFLOW_ID, DISPATCHER_VERSION);

        // Check if image already exists in runtara-environment by listing and searching by name
        let existing_image = self.find_image_by_name(tenant_id, &image_name).await?;

        let image_id = if let Some(img) = existing_image {
            info!(
                tenant_id = %tenant_id,
                image_id = %img.image_id,
                "Dispatcher image already registered, reusing"
            );
            img.image_id
        } else {
            // Image doesn't exist, compile and register
            self.compile_and_register(tenant_id).await?
        };

        // Cache the result
        {
            let mut cache = self.image_cache.write().await;
            cache.insert(tenant_id.to_string(), image_id.clone());
        }

        Ok(image_id)
    }

    /// Find an image by name in the tenant's image list
    async fn find_image_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<runtara_management_sdk::ImageSummary>, DispatcherError> {
        let result = self
            .runtime_client
            .list_images(tenant_id, 1000)
            .await
            .map_err(|e| {
                DispatcherError::RegistrationError(format!("Failed to list images: {}", e))
            })?;

        Ok(result.images.into_iter().find(|img| img.name == name))
    }

    /// Check if an image exists in runtara-environment by ID
    async fn image_exists(&self, tenant_id: &str, image_id: &str) -> Result<bool, DispatcherError> {
        match self.runtime_client.get_image(image_id, tenant_id).await {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => {
                warn!(
                    tenant_id = %tenant_id,
                    image_id = %image_id,
                    error = %e,
                    "Failed to check if image exists"
                );
                // Treat connection errors as "image doesn't exist" to trigger re-registration
                Ok(false)
            }
        }
    }

    /// Get the dispatcher image ID for a tenant.
    ///
    /// This validates that the cached image still exists in runtara-environment.
    /// If the image was deleted, it will automatically re-register the dispatcher.
    /// Returns an error if the dispatcher hasn't been initialized.
    pub async fn get_dispatcher_image(&self, tenant_id: &str) -> Result<String, DispatcherError> {
        // Check cache first
        let cached_id = {
            let cache = self.image_cache.read().await;
            cache.get(tenant_id).cloned()
        };

        match cached_id {
            Some(image_id) => {
                // Verify image still exists in runtara-environment
                if self.image_exists(tenant_id, &image_id).await? {
                    return Ok(image_id);
                }
                // Image was deleted from runtara-environment, re-register
                warn!(
                    tenant_id = %tenant_id,
                    image_id = %image_id,
                    "Cached dispatcher image no longer exists, re-registering"
                );
                self.clear_cache(tenant_id).await;
                self.initialize(tenant_id).await
            }
            None => Err(DispatcherError::ExecutionError(format!(
                "Dispatcher not initialized for tenant '{}'. Call initialize() first.",
                tenant_id
            ))),
        }
    }

    /// Compile the dispatcher binary and register it with runtara-environment
    async fn compile_and_register(&self, tenant_id: &str) -> Result<String, DispatcherError> {
        info!(
            tenant_id = %tenant_id,
            "Compiling agent dispatcher"
        );

        // Compile the dispatcher
        let compile_result = self.compile_dispatcher(tenant_id)?;

        // Register with runtara-environment
        let image_id = self.register_image(tenant_id, &compile_result).await?;

        info!(
            tenant_id = %tenant_id,
            image_id = %image_id,
            binary_size = compile_result.binary_size,
            "Agent dispatcher registered successfully"
        );

        Ok(image_id)
    }

    /// Compile the dispatcher to a WASM binary (same mechanism as workflows)
    fn compile_dispatcher(&self, tenant_id: &str) -> Result<CompilationResult, DispatcherError> {
        // Use WASM library paths (same as workflow compilation)
        let native_libs = runtara_workflows::get_wasm_native_library().map_err(|e| {
            DispatcherError::CompilationError(format!("Failed to get WASM libraries: {}", e))
        })?;

        // Create build directory
        let build_dir = get_dispatcher_build_dir(tenant_id);
        fs::create_dir_all(&build_dir).map_err(|e| {
            DispatcherError::CompilationError(format!("Failed to create build directory: {}", e))
        })?;

        // Write the dispatcher source
        let main_rs_path = build_dir.join("main.rs");
        fs::write(&main_rs_path, DISPATCHER_SOURCE).map_err(|e| {
            DispatcherError::CompilationError(format!("Failed to write main.rs: {}", e))
        })?;

        // Determine binary output path (.wasm extension for WASM target)
        let binary_path = build_dir.join("dispatcher.wasm");

        // Use WASM target (same as workflows)
        let target =
            std::env::var("RUNTARA_COMPILE_TARGET").unwrap_or_else(|_| "wasm32-wasip2".to_string());

        // Build rustc command (same approach as runtara-workflows::compile_workflow for WASM)
        let mut cmd = Command::new("rustc");
        cmd.arg(format!("--target={}", target))
            .arg("--crate-type=bin")
            .arg("--edition=2024")
            .arg("-C")
            .arg("opt-level=s")
            .arg("-C")
            .arg("codegen-units=1")
            .arg("-C")
            .arg("lto=fat");

        // Add library search paths
        let deps_dir = &native_libs.deps_dir;
        if deps_dir.exists() {
            cmd.arg("-L")
                .arg(format!("dependency={}", deps_dir.display()));
            // Also add as native search path for .a files (e.g., wit-bindgen-rt's cabi_realloc)
            cmd.arg("-L").arg(format!("native={}", deps_dir.display()));
        }

        // Add native library path (parent of workflow lib)
        if let Some(lib_dir) = native_libs.workflow_lib_path.parent() {
            cmd.arg("-L").arg(format!("native={}", lib_dir.display()));
        }

        // Add extern crate for the unified workflow stdlib library
        let stdlib_name = runtara_workflows::get_stdlib_name();
        cmd.arg("--extern").arg(format!(
            "{}={}",
            stdlib_name,
            native_libs.workflow_lib_path.display()
        ));

        // Dylib extension for proc-macros (compiled for the host, not WASM)
        let dylib_ext = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        };

        // Add ALL dependency rlibs AND dylibs as extern crates
        // Skip the stdlib itself (already added explicitly above) to avoid
        // E0464 "multiple candidates" when deps_dir contains extra copies.
        //
        // Deduplicate by crate name: when multiple versions of the same crate exist,
        // keep only the most recently modified one to avoid E0464.
        if deps_dir.exists()
            && let Ok(entries) = fs::read_dir(deps_dir)
        {
            let mut extern_crates: HashMap<String, std::path::PathBuf> = HashMap::new();

            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|s| s.to_str());

                if ext != Some("rlib") && ext != Some(dylib_ext) {
                    continue;
                }

                if let Some(filename) = path.file_name().and_then(|n| n.to_str())
                    && let Some(crate_name_part) = filename.strip_prefix("lib")
                    && let Some(crate_name) = crate_name_part.split('-').next()
                {
                    let extern_name = crate_name.replace('-', "_");
                    // Skip stdlib — it's already added explicitly via workflow_lib_path
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

        // Output and input
        cmd.arg("-o").arg(&binary_path);
        cmd.arg(&main_rs_path);

        // Execute rustc
        let output = cmd.output().map_err(|e| {
            DispatcherError::CompilationError(format!(
                "Failed to execute rustc: {}. Make sure rustc is installed with {} target.",
                e, target
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            warn!(
                stderr = %stderr,
                stdout = %stdout,
                "Dispatcher compilation failed"
            );
            return Err(DispatcherError::CompilationError(format!(
                "Rustc compilation failed: {}",
                stderr
            )));
        }

        // Verify binary was created
        if !binary_path.exists() {
            return Err(DispatcherError::CompilationError(
                "Compilation succeeded but binary was not found".to_string(),
            ));
        }

        // Get binary size and calculate checksum
        let metadata = fs::metadata(&binary_path).map_err(|e| {
            DispatcherError::CompilationError(format!("Failed to stat binary: {}", e))
        })?;
        let binary_size = metadata.len() as usize;

        // Calculate SHA256 checksum of the binary
        let file_content = fs::read(&binary_path).map_err(|e| {
            DispatcherError::CompilationError(format!("Failed to read binary for checksum: {}", e))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(&file_content);
        let binary_checksum = format!("{:x}", hasher.finalize());

        Ok(CompilationResult {
            binary_path,
            binary_size,
            binary_checksum,
            build_dir,
        })
    }

    /// Register the compiled dispatcher with runtara-environment
    async fn register_image(
        &self,
        tenant_id: &str,
        compile_result: &CompilationResult,
    ) -> Result<String, DispatcherError> {
        let image_name = format!("{}:{}", DISPATCHER_WORKFLOW_ID, DISPATCHER_VERSION);

        let options = RegisterImageStreamOptions::new(
            tenant_id,
            &image_name,
            compile_result.binary_size as u64,
        )
        .with_description("Universal agent dispatcher for testing".to_string())
        .with_runner_type(
            if compile_result
                .binary_path
                .extension()
                .is_some_and(|ext| ext == "wasm")
            {
                RunnerType::Wasm
            } else {
                RunnerType::Oci
            },
        )
        .with_sha256(&compile_result.binary_checksum);

        let file = tokio::fs::File::open(&compile_result.binary_path)
            .await
            .map_err(|e| {
                DispatcherError::RegistrationError(format!("Failed to open binary: {}", e))
            })?;

        let result = self
            .runtime_client
            .register_image_stream(options, file)
            .await
            .map_err(|e| {
                DispatcherError::RegistrationError(format!("Registration failed: {}", e))
            })?;

        info!(
            tenant_id = %tenant_id,
            success = result.success,
            image_id = %result.image_id,
            error = ?result.error,
            "Dispatcher image registration result"
        );

        if !result.success {
            return Err(DispatcherError::RegistrationError(
                result.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }

        Ok(result.image_id)
    }

    /// Clear cached image ID for a tenant (useful if dispatcher needs recompilation)
    #[allow(dead_code)]
    pub async fn clear_cache(&self, tenant_id: &str) {
        let mut cache = self.image_cache.write().await;
        cache.remove(tenant_id);
    }

    /// Get the runtime client for executing the dispatcher
    pub fn runtime_client(&self) -> &Arc<RuntimeClient> {
        &self.runtime_client
    }
}

/// Result of dispatcher compilation
struct CompilationResult {
    binary_path: PathBuf,
    binary_size: usize,
    binary_checksum: String,
    #[allow(dead_code)]
    build_dir: PathBuf,
}

/// Get the build directory for the dispatcher
fn get_dispatcher_build_dir(tenant_id: &str) -> PathBuf {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string());
    PathBuf::from(data_dir)
        .join(tenant_id)
        .join("dispatcher")
        .join(format!("version_{}", DISPATCHER_VERSION))
}

/// Errors that can occur in the dispatcher service
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum DispatcherError {
    CompilationError(String),
    RegistrationError(String),
    ExecutionError(String),
}

impl std::fmt::Display for DispatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatcherError::CompilationError(msg) => write!(f, "Compilation error: {}", msg),
            DispatcherError::RegistrationError(msg) => write!(f, "Registration error: {}", msg),
            DispatcherError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
        }
    }
}

impl std::error::Error for DispatcherError {}
