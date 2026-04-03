use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::api::repositories::scenarios::ScenarioRepository;
use crate::compiler::child_scenarios::load_child_scenarios;
use crate::runtime_client::RuntimeClient;
use runtara_dsl::parse_execution_graph;
use runtara_management_sdk::{RegisterImageStreamOptions, RunnerType};
use runtara_workflows::{ChildScenarioInput, CompilationInput, compile_scenario};

/// Service for scenario compilation operations
pub struct CompilationService {
    repository: Arc<ScenarioRepository>,
    connection_service_url: Option<String>,
    /// Runtime client for registering images with runtara-environment
    runtime_client: Option<Arc<RuntimeClient>>,
}

impl CompilationService {
    pub fn new(
        repository: Arc<ScenarioRepository>,
        connection_service_url: Option<String>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        Self {
            repository,
            connection_service_url,
            runtime_client,
        }
    }

    /// Compile a scenario to binary and optionally register with runtara-environment
    ///
    /// This orchestrates the full compilation pipeline:
    /// 1. Fetch scenario definition from database
    /// 2. Load child scenarios from database
    /// 3. Compile to binary (native or WASM, depending on target)
    /// 4. Record compilation result in database
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant identifier
    /// * `scenario_id` - The scenario identifier
    /// * `version` - The version number
    ///
    /// # Returns
    /// Result with compilation metadata or a ServiceError
    pub async fn compile_scenario(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
    ) -> Result<CompilationResultDto, ServiceError> {
        let compile_start = std::time::Instant::now();
        info!(
            "Starting compilation for scenario {} version {}",
            scenario_id, version
        );

        // 1. Fetch scenario definition and track-events mode
        let step_start = std::time::Instant::now();
        debug!("compile: step 1 - fetching definition from database");
        let (definition, track_events) = self
            .repository
            .get_definition_with_track_events(tenant_id, scenario_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to fetch definition: {}", e)))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Scenario '{}' version {} not found",
                    scenario_id, version
                ))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 1 completed - definition fetched"
        );

        let version_u32 = version as u32;

        // 2. Parse execution graph
        let step_start = std::time::Instant::now();
        debug!("compile: step 2 - parsing execution graph");
        let execution_graph = parse_execution_graph(&definition).map_err(|e| {
            ServiceError::CompilationError(format!("Failed to parse execution graph: {}", e))
        })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 2 completed - execution graph parsed"
        );

        // 3. Load child scenarios from database
        let step_start = std::time::Instant::now();
        debug!("compile: step 3 - loading child scenarios from database");
        let child_scenarios = self
            .load_child_scenarios_as_input(tenant_id, scenario_id, version, &definition)
            .await?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            child_count = child_scenarios.len(),
            "compile: step 3 completed - child scenarios loaded"
        );

        // 4. Build compilation input
        let compilation_input = CompilationInput {
            tenant_id: tenant_id.to_string(),
            scenario_id: scenario_id.to_string(),
            version: version_u32,
            execution_graph,
            track_events,
            child_scenarios,
            connection_service_url: self.connection_service_url.clone(),
        };

        // 5. Check if already registered BEFORE compiling
        // This prevents FK constraint violations when re-compiling scenarios that are already registered
        let step_start = std::time::Instant::now();
        debug!("compile: step 5 - checking if already registered in database");
        let existing_image_id = self
            .repository
            .get_registered_image_id(tenant_id, scenario_id, version)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to check existing image: {}", e))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            found = existing_image_id.is_some(),
            "compile: step 5 completed - database check done"
        );

        if let Some(existing_id) = existing_image_id {
            info!(
                total_duration_ms = compile_start.elapsed().as_millis(),
                "Scenario {} version {} already registered with image {}, skipping compilation",
                scenario_id,
                version,
                existing_id
            );
            return Ok(CompilationResultDto {
                scenario_id: scenario_id.to_string(),
                version,
                build_dir: String::new(),
                binary_size: 0,
                binary_checksum: String::new(),
                image_id: Some(existing_id),
            });
        }

        // 5b. Also check runtara-environment directly in case we have an orphaned image
        // (image exists in runtara but no local record due to failed registration save)
        if let Some(client) = &self.runtime_client {
            let image_name = format!("{}:{}", scenario_id, version);
            let step_start = std::time::Instant::now();
            debug!("compile: step 5b - checking runtara-environment for existing image");
            match client.find_image_by_name(tenant_id, &image_name).await {
                Ok(Some(existing_id)) => {
                    info!(
                        duration_ms = step_start.elapsed().as_millis(),
                        total_duration_ms = compile_start.elapsed().as_millis(),
                        "Found existing image {} in runtara-environment for scenario {} version {}, recording locally",
                        existing_id,
                        scenario_id,
                        version
                    );
                    // Record this in our DB so we don't check again
                    let _ = self
                        .repository
                        .record_registered_image_id(tenant_id, scenario_id, version, &existing_id)
                        .await;
                    return Ok(CompilationResultDto {
                        scenario_id: scenario_id.to_string(),
                        version,
                        build_dir: String::new(),
                        binary_size: 0,
                        binary_checksum: String::new(),
                        image_id: Some(existing_id),
                    });
                }
                Ok(None) => {
                    debug!(
                        duration_ms = step_start.elapsed().as_millis(),
                        "compile: step 5b completed - no existing image found, proceeding with compilation"
                    );
                }
                Err(e) => {
                    warn!(
                        duration_ms = step_start.elapsed().as_millis(),
                        "Failed to check runtara-environment for existing image: {}", e
                    );
                    // Continue with compilation attempt
                }
            }
        }

        // 6. Compile to native binary
        // IMPORTANT: compile_scenario is a synchronous blocking function that runs cargo build.
        // We MUST use spawn_blocking to prevent blocking the tokio runtime, which would
        // starve all other async tasks (API handlers, database queries, etc.) during compilation.
        let step_start = std::time::Instant::now();
        debug!("compile: step 6 - compiling to native binary");
        let result = tokio::task::spawn_blocking(move || compile_scenario(compilation_input))
            .await
            .map_err(|e| {
                ServiceError::CompilationError(format!("Compilation task panicked: {}", e))
            })?
            .map_err(|e| ServiceError::CompilationError(format!("Compilation failed: {}", e)))?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            binary_size = result.binary_size,
            "compile: step 6 completed - native binary compiled"
        );

        // 7. Record compilation success in database FIRST (before registration)
        // This ensures we have a record even if registration fails, preventing
        // orphaned images in runtara-environment with no local record
        let step_start = std::time::Instant::now();
        debug!("compile: step 7 - recording compilation success in database");
        self.repository
            .record_compilation_success(
                tenant_id,
                scenario_id,
                version,
                &result.build_dir,
                result.binary_size as i32,
                &result.binary_checksum,
            )
            .await
            .map_err(|e| {
                warn!("Failed to record compilation success: {}", e);
                ServiceError::DatabaseError(format!("Failed to record compilation: {}", e))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 7 completed - compilation success recorded in database"
        );

        // 8. Register with runtara-environment (REQUIRED)
        // Compilation without registration is useless - the scenario can't be executed
        let client = self.runtime_client.as_ref().ok_or_else(|| {
            ServiceError::RegistrationError(
                "Runtime client not configured. Compilation requires runtara-environment connection.".to_string()
            )
        })?;

        let step_start = std::time::Instant::now();
        debug!(
            binary_size = result.binary_size,
            "compile: step 8 - registering image with runtara-environment"
        );
        let image_id = self
            .register_image(client, tenant_id, scenario_id, version_u32, &result)
            .await?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            image_id = %image_id,
            "compile: step 8 completed - image registered with runtara-environment"
        );

        // 8b. Record registered image ID (required for execution)
        let step_start = std::time::Instant::now();
        debug!("compile: step 8b - recording registered image ID in database");
        self.repository
            .record_registered_image_id(tenant_id, scenario_id, version, &image_id)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to record registered image ID: {}", e))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 8b completed - image ID recorded in database"
        );

        // 9. Record child scenario dependencies
        if !result.child_dependencies.is_empty() {
            let step_start = std::time::Instant::now();
            debug!(
                dependency_count = result.child_dependencies.len(),
                "compile: step 9 - recording child scenario dependencies"
            );
            for dep in &result.child_dependencies {
                let insert_result = sqlx::query!(
                    r#"
                    INSERT INTO scenario_dependencies
                        (parent_tenant_id, parent_scenario_id, parent_version, child_scenario_id,
                         child_version_requested, child_version_resolved, step_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (parent_tenant_id, parent_scenario_id, parent_version, step_id)
                    DO UPDATE SET
                        child_scenario_id = $4,
                        child_version_requested = $5,
                        child_version_resolved = $6
                    "#,
                    tenant_id,
                    scenario_id,
                    version,
                    dep.child_scenario_id,
                    dep.child_version_requested,
                    dep.child_version_resolved,
                    dep.step_id
                )
                .execute(self.repository.pool())
                .await;

                if let Err(e) = insert_result {
                    warn!(
                        "Failed to record dependency for step {}: {}",
                        dep.step_id, e
                    );
                }
            }

            debug!(
                duration_ms = step_start.elapsed().as_millis(),
                dependency_count = result.child_dependencies.len(),
                "compile: step 9 completed - child scenario dependencies recorded"
            );
        }

        info!(
            total_duration_ms = compile_start.elapsed().as_millis(),
            "Compilation successful for scenario {} version {} ({} bytes) [registered: {}]",
            scenario_id,
            version,
            result.binary_size,
            image_id
        );

        Ok(CompilationResultDto {
            scenario_id: scenario_id.to_string(),
            version,
            build_dir: result.build_dir.to_string_lossy().to_string(),
            binary_size: result.binary_size,
            binary_checksum: result.binary_checksum,
            image_id: Some(image_id),
        })
    }

    /// Load child scenarios from database and convert to ChildScenarioInput
    async fn load_child_scenarios_as_input(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
        definition: &serde_json::Value,
    ) -> Result<Vec<ChildScenarioInput>, ServiceError> {
        let child_scenarios_list = load_child_scenarios(
            self.repository.pool(),
            tenant_id,
            scenario_id,
            version,
            definition,
        )
        .await
        .map_err(|e| {
            ServiceError::CompilationError(format!("Failed to load child scenarios: {}", e))
        })?;

        if !child_scenarios_list.is_empty() {
            debug!(
                tenant_id = %tenant_id,
                scenario_id = %scenario_id,
                version = version,
                child_scenario_count = child_scenarios_list.len(),
                "Loaded child scenarios for compilation"
            );
        }

        // Convert to ChildScenarioInput
        let mut child_scenarios = Vec::new();
        for info in child_scenarios_list {
            let graph = parse_execution_graph(&info.execution_graph).map_err(|e| {
                ServiceError::CompilationError(format!(
                    "Failed to parse child scenario '{}': {}",
                    info.scenario_ref.scenario_id, e
                ))
            })?;

            child_scenarios.push(ChildScenarioInput {
                step_id: info.step_id,
                scenario_id: info.scenario_ref.scenario_id,
                version_requested: info.version_requested,
                version_resolved: info.scenario_ref.version,
                execution_graph: graph,
            });
        }

        Ok(child_scenarios)
    }

    /// Register a compiled binary with runtara-environment using streaming upload
    async fn register_image(
        &self,
        client: &RuntimeClient,
        tenant_id: &str,
        scenario_id: &str,
        version: u32,
        compilation_result: &runtara_workflows::NativeCompilationResult,
    ) -> Result<String, ServiceError> {
        // Build the image name: {scenario_id}:{version}
        let image_name = format!("{}:{}", scenario_id, version);

        // Get binary path and size (use binary_path from compilation result,
        // which is target-aware: "scenario" for native, "scenario.wasm" for WASM)
        let binary_path = &compilation_result.binary_path;
        let metadata = tokio::fs::metadata(&binary_path).await.map_err(|e| {
            ServiceError::RegistrationError(format!("Failed to read binary metadata: {}", e))
        })?;
        let binary_size = metadata.len();

        info!(
            "Registering image {} for tenant {} ({} bytes)",
            image_name, tenant_id, binary_size
        );

        // Create registration options with scenario variables as metadata
        let options = RegisterImageStreamOptions::new(tenant_id, &image_name, binary_size)
            .with_description(format!("Scenario {} version {}", scenario_id, version))
            .with_runner_type(
                if compilation_result
                    .binary_path
                    .extension()
                    .is_some_and(|ext| ext == "wasm")
                {
                    RunnerType::Wasm
                } else {
                    RunnerType::Oci
                },
            )
            .with_sha256(&compilation_result.binary_checksum)
            .with_metadata(serde_json::json!({"variables": compilation_result.default_variables}));

        // Open the binary file for streaming
        let file = tokio::fs::File::open(&binary_path).await.map_err(|e| {
            ServiceError::RegistrationError(format!("Failed to open binary: {}", e))
        })?;

        // Register via streaming upload
        let result = client
            .register_image_stream(options, file)
            .await
            .map_err(|e| ServiceError::RegistrationError(format!("Registration failed: {}", e)))?;

        if !result.success {
            return Err(ServiceError::RegistrationError(
                result.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }

        info!(
            "Successfully registered image {} with ID {}",
            image_name, result.image_id
        );

        Ok(result.image_id)
    }
}

/// DTO for compilation result
#[derive(Debug)]
pub struct CompilationResultDto {
    pub scenario_id: String,
    pub version: i32,
    pub build_dir: String,
    pub binary_size: usize,
    pub binary_checksum: String,
    /// Image ID returned from runtara-environment registration (if enabled)
    pub image_id: Option<String>,
}

/// Service-level errors for compilation operations
#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    NotFound(String),
    DatabaseError(String),
    CompilationError(String),
    RegistrationError(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ServiceError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ServiceError::CompilationError(msg) => write!(f, "Compilation error: {}", msg),
            ServiceError::RegistrationError(msg) => write!(f, "Registration error: {}", msg),
        }
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // ServiceError Display tests
    // =========================================================================

    #[test]
    fn test_service_error_not_found_display() {
        let error = ServiceError::NotFound("Scenario 'test' version 5 not found".to_string());
        assert_eq!(
            error.to_string(),
            "Not found: Scenario 'test' version 5 not found"
        );
    }

    #[test]
    fn test_service_error_database_display() {
        let error = ServiceError::DatabaseError("Connection refused".to_string());
        assert_eq!(error.to_string(), "Database error: Connection refused");
    }

    #[test]
    fn test_service_error_compilation_display() {
        let error =
            ServiceError::CompilationError("cargo build failed with exit code 101".to_string());
        assert_eq!(
            error.to_string(),
            "Compilation error: cargo build failed with exit code 101"
        );
    }

    #[test]
    fn test_service_error_registration_display() {
        let error = ServiceError::RegistrationError("runtara-environment unreachable".to_string());
        assert_eq!(
            error.to_string(),
            "Registration error: runtara-environment unreachable"
        );
    }

    #[test]
    fn test_service_error_is_std_error() {
        // Verify ServiceError implements std::error::Error trait
        let error: Box<dyn std::error::Error> =
            Box::new(ServiceError::CompilationError("test".to_string()));
        assert!(error.to_string().contains("Compilation error"));
    }

    // =========================================================================
    // ServiceError Debug tests
    // =========================================================================

    #[test]
    fn test_service_error_debug_format() {
        let error = ServiceError::NotFound("test".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("NotFound"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_service_error_compilation_debug() {
        let error = ServiceError::CompilationError("linker error".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("CompilationError"));
        assert!(debug_str.contains("linker error"));
    }

    #[test]
    fn test_service_error_registration_debug() {
        let error = ServiceError::RegistrationError("timeout".to_string());
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("RegistrationError"));
    }

    // =========================================================================
    // CompilationResultDto tests
    // =========================================================================

    #[test]
    fn test_compilation_result_dto_fields() {
        let result = CompilationResultDto {
            scenario_id: "my-scenario".to_string(),
            version: 7,
            build_dir: "/tmp/builds/abc123".to_string(),
            binary_size: 5_242_880, // 5MB
            binary_checksum: "sha256:abc123def456".to_string(),
            image_id: Some("img-uuid-12345".to_string()),
        };

        assert_eq!(result.scenario_id, "my-scenario");
        assert_eq!(result.version, 7);
        assert_eq!(result.build_dir, "/tmp/builds/abc123");
        assert_eq!(result.binary_size, 5_242_880);
        assert_eq!(result.binary_checksum, "sha256:abc123def456");
        assert_eq!(result.image_id, Some("img-uuid-12345".to_string()));
    }

    #[test]
    fn test_compilation_result_dto_without_image_id() {
        let result = CompilationResultDto {
            scenario_id: "local-only".to_string(),
            version: 1,
            build_dir: "/data/scenarios/local-only/build".to_string(),
            binary_size: 1024,
            binary_checksum: "sha256:1234".to_string(),
            image_id: None,
        };

        assert!(result.image_id.is_none());
    }

    #[test]
    fn test_compilation_result_dto_debug_format() {
        let result = CompilationResultDto {
            scenario_id: "test".to_string(),
            version: 1,
            build_dir: "/tmp".to_string(),
            binary_size: 100,
            binary_checksum: "checksum".to_string(),
            image_id: None,
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("scenario_id"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("version"));
        assert!(debug_str.contains("binary_size"));
    }

    #[test]
    fn test_compilation_result_dto_large_binary() {
        // Test with realistic large binary size (100MB)
        let result = CompilationResultDto {
            scenario_id: "large-scenario".to_string(),
            version: 1,
            build_dir: "/data/builds".to_string(),
            binary_size: 104_857_600,
            binary_checksum: "sha256:largechecksum".to_string(),
            image_id: Some("img-large".to_string()),
        };

        assert_eq!(result.binary_size, 104_857_600);
    }
}
