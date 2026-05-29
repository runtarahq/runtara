use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::api::repositories::workflows::{
    CompilationSuccessRecord, WorkflowRepository, workflow_definition_checksum,
};
use crate::compiler::child_workflows::load_child_workflows;
use crate::runtime_client::RuntimeClient;
use crate::valkey::compilation_progress::{CompilationStage, ProgressReporter};
use redis::aio::ConnectionManager;
use runtara_dsl::parse_execution_graph;
use runtara_management_sdk::{ImageSummary, RegisterImageStreamOptions, RunnerType};
use runtara_workflows::compile::ProgressCallback;
use runtara_workflows::{
    ChildWorkflowInput, CompilationInput, DirectWorkflowCompileOptions, NativeCompilationResult,
    compile_workflow, compile_workflow_direct,
};

/// Global semaphore limiting concurrent compilations across all code paths.
/// Prevents OOM when multiple compilations are triggered simultaneously.
/// Configured via MAX_CONCURRENT_COMPILATIONS env var (default: 1).
static COMPILATION_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn compilation_semaphore() -> &'static Semaphore {
    COMPILATION_SEMAPHORE.get_or_init(|| {
        let max = std::env::var("MAX_CONCURRENT_COMPILATIONS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);
        info!(
            max_concurrent_compilations = max,
            "Compilation semaphore initialized"
        );
        Semaphore::new(max)
    })
}

fn image_source_checksum(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/sourceChecksum"))
        .and_then(|v| v.as_str())
}

/// `templateMajor` stored in image metadata at registration time. Used by the
/// cache check below to invalidate every workflow on a major-version bump of
/// the compiler (e.g. `5` → `6`); minor / patch bumps don't recompile.
/// Returns `None` for images that pre-date the field, which forces a recompile
/// so they pick up the components-mode pipeline.
fn image_template_major(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/templateMajor"))
        .and_then(|v| v.as_str())
}

/// Whether `image` is a cache hit for the current source + compiler major.
/// Both must match — pre-existing images lack `templateMajor` so they always
/// miss, forcing a recompile through the components path.
fn image_cache_hits(image: &ImageSummary, source_checksum: &str) -> bool {
    image_source_checksum(image) == Some(source_checksum)
        && image_template_major(image) == Some(runtara_workflows::TEMPLATE_MAJOR_VERSION)
}

/// Disabled-by-default direct WASM compilation settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectCompilationSettings {
    /// Whether the service should try direct compilation before Rust/codegen.
    pub enabled: bool,
    /// Directory containing prebuilt shared workflow and agent components.
    pub components_dir: Option<PathBuf>,
}

impl DirectCompilationSettings {
    /// Return settings that keep the existing Rust/codegen component pipeline.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            components_dir: None,
        }
    }

    /// Return settings that try direct compilation with fallback.
    pub fn enabled(components_dir: Option<PathBuf>) -> Self {
        Self {
            enabled: true,
            components_dir,
        }
    }
}

/// Build direct compilation settings from the process configuration.
pub fn direct_compilation_settings_from_config() -> DirectCompilationSettings {
    DirectCompilationSettings {
        enabled: crate::config::direct_wasm_compile_enabled(),
        components_dir: crate::config::direct_wasm_components_dir(),
    }
}

fn compile_workflow_with_direct_fallback(
    input: CompilationInput,
    source_checksum: String,
    settings: DirectCompilationSettings,
) -> std::io::Result<NativeCompilationResult> {
    if settings.enabled {
        if let Some(components_dir) = settings.components_dir {
            let options = DirectWorkflowCompileOptions {
                output_dir: direct_output_dir(&input.tenant_id),
                components_dir,
                source_checksum: Some(source_checksum),
            };
            match compile_workflow_direct(input.clone(), options) {
                Ok(result) => {
                    info!(
                        workflow_id = %input.workflow_id,
                        version = input.version,
                        binary_size = result.binary_size,
                        "Direct WASM workflow compilation succeeded"
                    );
                    return Ok(result);
                }
                Err(err) => {
                    warn!(
                        workflow_id = %input.workflow_id,
                        version = input.version,
                        error = %err,
                        "Direct WASM workflow compilation failed; falling back to Rust/codegen compiler"
                    );
                }
            }
        } else {
            warn!(
                workflow_id = %input.workflow_id,
                version = input.version,
                "Direct WASM workflow compilation enabled but no component directory is configured; falling back to Rust/codegen compiler"
            );
        }
    }

    compile_workflow(input)
}

fn direct_output_dir(tenant_id: &str) -> PathBuf {
    data_dir().join("workflow-builds-direct").join(tenant_id)
}

fn data_dir() -> PathBuf {
    let raw = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));
    if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&raw))
            .unwrap_or(raw)
    }
}

/// Service for workflow compilation operations
pub struct CompilationService {
    repository: Arc<WorkflowRepository>,
    connection_service_url: Option<String>,
    /// Runtime client for registering images with runtara-environment
    runtime_client: Option<Arc<RuntimeClient>>,
    /// Runtime agent metadata catalog (snapshot of every `<agent>.meta.json`
    /// staged at `$RUNTARA_AGENT_COMPONENTS_DIR`). When set, the compile
    /// pipeline uses it instead of the statically-linked
    /// `runtara_agents::registry`, making the server's compiled view of
    /// agents match what the runtime dispatcher can actually invoke.
    agent_catalog: Option<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    /// Optional Redis manager for streaming compilation progress. When set,
    /// each compile_workflow call publishes stage transitions to Redis under
    /// `runtara:compilation:progress:*` for the frontend's progress UI.
    /// `None` (e.g. CLI / Valkey-disabled paths) is a no-op.
    redis_manager: Option<ConnectionManager>,
    /// Optional connections facade used to pre-resolve each agent step's
    /// `connection_id` → `integration_id` mapping at compile time. Baked into
    /// the synthetic `_connection` literal by `emit_connection_fetch` so
    /// component-backed agents that dispatch on `integration_id` (e.g.
    /// `ai-tools::text-completion`) see a populated value rather than the
    /// empty stub the workflow runner cannot fill in from inside the WASM.
    connections_facade: Option<Arc<runtara_connections::ConnectionsFacade>>,
    /// Optional direct WASM compiler gate. Disabled by default.
    direct_compilation: DirectCompilationSettings,
}

impl CompilationService {
    pub fn new(
        repository: Arc<WorkflowRepository>,
        connection_service_url: Option<String>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        Self {
            repository,
            connection_service_url,
            runtime_client,
            agent_catalog: None,
            redis_manager: None,
            connections_facade: None,
            direct_compilation: DirectCompilationSettings::disabled(),
        }
    }

    /// Plug in the connections facade so the compile pipeline can pre-resolve
    /// each agent step's `connection_id → integration_id` and bake it into the
    /// generated workflow binary. Without it, the map is empty and component
    /// agents that dispatch on `integration_id` fall back to the empty-string
    /// behavior (broken for `ai-tools`, irrelevant for everything else).
    pub fn with_connections_facade(
        mut self,
        facade: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        self.connections_facade = Some(facade);
        self
    }

    /// Plug in the runtime agent catalog. Wired up at server boot from the
    /// `ComponentDispatcherService` so every compile sees the same agent
    /// set the dispatcher can route to.
    pub fn with_agent_catalog(
        mut self,
        catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    ) -> Self {
        self.agent_catalog = Some(catalog);
        self
    }

    /// Plug in a Redis manager for streaming compilation progress events.
    /// Without it, compile_workflow runs as before but writes no progress
    /// state — the frontend will see `unknown` until the DB row lands.
    pub fn with_redis_manager(mut self, manager: ConnectionManager) -> Self {
        self.redis_manager = Some(manager);
        self
    }

    /// Plug in direct WASM compilation settings. When enabled, the service
    /// tries direct compilation and falls back to the existing compiler on
    /// unsupported graphs or direct infrastructure errors.
    pub fn with_direct_compilation(mut self, settings: DirectCompilationSettings) -> Self {
        self.direct_compilation = settings;
        self
    }

    /// Compile a workflow to binary and optionally register with runtara-environment
    ///
    /// This orchestrates the full compilation pipeline:
    /// 1. Fetch workflow definition from database
    /// 2. Load child workflows from database
    /// 3. Compile to binary (native or WASM, depending on target)
    /// 4. Record compilation result in database
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant identifier
    /// * `workflow_id` - The workflow identifier
    /// * `version` - The version number
    ///
    /// # Returns
    /// Result with compilation metadata or a ServiceError
    pub async fn compile_workflow(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
    ) -> Result<CompilationResultDto, ServiceError> {
        let compile_start = std::time::Instant::now();
        info!(
            force_recompile = force_recompile,
            "Starting compilation for workflow {} version {}", workflow_id, version
        );

        // Build a progress reporter scoped to this compile if Redis is wired up.
        // Every stage transition below routes through this; cache-hit short
        // circuits clear it explicitly so the frontend stops polling.
        let progress_reporter = self.redis_manager.as_ref().map(|m| {
            ProgressReporter::new(
                m.clone(),
                tenant_id.to_string(),
                workflow_id.to_string(),
                version,
            )
        });
        if let Some(r) = &progress_reporter {
            r.report(CompilationStage::Preparing, "Loading workflow definition")
                .await;
        }

        // 1. Fetch workflow definition and track-events mode
        let step_start = std::time::Instant::now();
        debug!("compile: step 1 - fetching definition from database");
        let (definition, track_events) = self
            .repository
            .get_definition_with_track_events(tenant_id, workflow_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to fetch definition: {}", e)))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Workflow '{}' version {} not found",
                    workflow_id, version
                ))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 1 completed - definition fetched"
        );
        let source_checksum = workflow_definition_checksum(&definition);

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

        // 3. Load child workflows from database
        let step_start = std::time::Instant::now();
        debug!("compile: step 3 - loading child workflows from database");
        let child_workflows = self
            .load_child_workflows_as_input(tenant_id, workflow_id, version, &definition)
            .await?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            child_count = child_workflows.len(),
            "compile: step 3 completed - child workflows loaded"
        );

        // 3a. Pre-resolve connection_id → integration_id for every Agent /
        // AiAgent step that references a connection. The codegen bakes the
        // resulting value into the synthetic `_connection` literal so
        // component-backed agents that dispatch on integration_id (e.g.
        // `ai-tools::text-completion`) see it without an in-WASM HTTP fetch.
        let connection_integration_ids = self
            .resolve_connection_integration_ids(tenant_id, &execution_graph, &child_workflows)
            .await;

        // Set up the sync→async progress bridge. The inner compile pipeline
        // runs in `spawn_blocking` and can't `.await` Redis writes directly,
        // so it fires events through a channel that a tokio task drains and
        // forwards to the reporter. When the reporter is `None` (e.g. CLI
        // path), we skip the channel entirely and pass `None` as the
        // callback — zero overhead.
        let (progress_callback, drain_handle, outer_tx) = match progress_reporter.clone() {
            Some(reporter) => {
                let (tx, mut rx) = mpsc::unbounded_channel::<(String, String)>();
                let drain = tokio::spawn(async move {
                    while let Some((stage_str, msg)) = rx.recv().await {
                        if let Some(stage) = CompilationStage::parse(&stage_str) {
                            reporter.report(stage, &msg).await;
                        }
                    }
                });
                let tx_cb = tx.clone();
                let cb: ProgressCallback = Arc::new(move |stage: &str, msg: &str| {
                    // Drop on closed channel is fine — the drain task may
                    // have exited if the compile already finished.
                    let _ = tx_cb.send((stage.to_string(), msg.to_string()));
                });
                (Some(cb), Some(drain), Some(tx))
            }
            None => (None, None, None),
        };

        // 4. Build compilation input
        let compilation_input = CompilationInput {
            tenant_id: tenant_id.to_string(),
            workflow_id: workflow_id.to_string(),
            version: version_u32,
            execution_graph,
            track_events,
            child_workflows,
            connection_service_url: self.connection_service_url.clone(),
            connection_integration_ids,
            // When configured, the compile uses the runtime catalog from
            // the component dispatcher so the compiled view of agents
            // matches what the runtime can actually invoke.
            agent_catalog: self.agent_catalog.clone(),
            progress_callback,
        };

        // 5. Check if already registered BEFORE compiling, unless a rebuild was requested.
        // This prevents FK constraint violations when re-compiling workflows that are already registered.
        let step_start = std::time::Instant::now();
        debug!("compile: step 5 - checking if already registered in database");
        let existing_image_id = if force_recompile {
            None
        } else {
            self.repository
                .get_fresh_registered_image_id(tenant_id, workflow_id, version)
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to check existing image: {}", e))
                })?
        };
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            found = existing_image_id.is_some(),
            "compile: step 5 completed - database check done"
        );

        if let Some(existing_id) = existing_image_id {
            info!(
                total_duration_ms = compile_start.elapsed().as_millis(),
                "Workflow {} version {} already registered with image {}, skipping compilation",
                workflow_id,
                version,
                existing_id
            );
            if let Some(r) = &progress_reporter {
                r.clear().await;
            }
            return Ok(CompilationResultDto {
                workflow_id: workflow_id.to_string(),
                version,
                build_dir: String::new(),
                binary_size: 0,
                binary_checksum: String::new(),
                image_id: Some(existing_id),
            });
        }

        // 5b. Also check runtara-environment directly in case we have an orphaned image
        // (image exists in runtara but no local record due to failed registration save).
        if !force_recompile && let Some(client) = &self.runtime_client {
            let image_name = format!("{}:{}", workflow_id, version);
            let step_start = std::time::Instant::now();
            debug!("compile: step 5b - checking runtara-environment for existing image");
            match client
                .find_image_by_name_summary(tenant_id, &image_name)
                .await
            {
                Ok(Some(existing_image))
                    if image_cache_hits(&existing_image, source_checksum.as_str()) =>
                {
                    let existing_id = existing_image.image_id;
                    info!(
                        duration_ms = step_start.elapsed().as_millis(),
                        total_duration_ms = compile_start.elapsed().as_millis(),
                        "Found existing image {} in runtara-environment for workflow {} version {}, recording locally",
                        existing_id,
                        workflow_id,
                        version
                    );
                    // Record this in our DB so we don't check again
                    let _ = self
                        .repository
                        .record_registered_image_id(
                            tenant_id,
                            workflow_id,
                            version,
                            &existing_id,
                            Some(&source_checksum),
                        )
                        .await;
                    if let Some(r) = &progress_reporter {
                        r.clear().await;
                    }
                    return Ok(CompilationResultDto {
                        workflow_id: workflow_id.to_string(),
                        version,
                        build_dir: String::new(),
                        binary_size: 0,
                        binary_checksum: String::new(),
                        image_id: Some(existing_id),
                    });
                }
                Ok(Some(_)) => {
                    debug!(
                        duration_ms = step_start.elapsed().as_millis(),
                        "compile: step 5b found image name but source checksum differed or was absent; rebuilding"
                    );
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

        // 6. Compile to workflow.wasm
        // IMPORTANT: compile_workflow is a synchronous blocking function that runs cargo build.
        // We MUST use spawn_blocking to prevent blocking the tokio runtime, which would
        // starve all other async tasks (API handlers, database queries, etc.) during compilation.
        //
        // The semaphore limits concurrent compilations to prevent OOM when multiple
        // compilations are triggered simultaneously (e.g., via API or execution engine).
        let step_start = std::time::Instant::now();
        debug!("compile: step 6 - acquiring compilation semaphore");
        let _permit = compilation_semaphore().acquire().await.map_err(|_| {
            ServiceError::CompilationError("Compilation semaphore closed".to_string())
        })?;
        debug!(
            wait_ms = step_start.elapsed().as_millis(),
            direct_wasm_enabled = self.direct_compilation.enabled,
            "compile: step 6 - semaphore acquired, compiling workflow artifact"
        );
        let compile_start_time = std::time::Instant::now();
        let direct_compilation = self.direct_compilation.clone();
        let compile_source_checksum = source_checksum.clone();
        let result = tokio::task::spawn_blocking(move || {
            compile_workflow_with_direct_fallback(
                compilation_input,
                compile_source_checksum,
                direct_compilation,
            )
        })
        .await
        .map_err(|e| ServiceError::CompilationError(format!("Compilation task panicked: {}", e)))?
        .map_err(|e| ServiceError::CompilationError(format!("Compilation failed: {}", e)))?;
        debug!(
            duration_ms = compile_start_time.elapsed().as_millis(),
            binary_size = result.binary_size,
            "compile: step 6 completed - workflow artifact compiled"
        );

        // spawn_blocking returned, so the build callbacks are done firing.
        // Drop the outer sender to close the channel, then wait for the
        // drain task to flush remaining events into Redis. Without this
        // flush a tail of "Compiling X" events from late in the build could
        // outlive the Registering report below.
        drop(outer_tx);
        if let Some(handle) = drain_handle {
            let _ = handle.await;
        }

        // 7. Record compilation success in database FIRST (before registration)
        // This ensures we have a record even if registration fails, preventing
        // orphaned images in runtara-environment with no local record
        let step_start = std::time::Instant::now();
        debug!("compile: step 7 - recording compilation success in database");
        self.repository
            .record_compilation_success(CompilationSuccessRecord {
                tenant_id,
                workflow_id,
                version,
                build_dir: &result.build_dir,
                binary_size: result.binary_size as i32,
                package_size: result.package_size as i32,
                binary_checksum: &result.binary_checksum,
                source_checksum: &source_checksum,
            })
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
        // Compilation without registration is useless - the workflow can't be executed
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
        if let Some(r) = &progress_reporter {
            r.report(
                CompilationStage::Registering,
                "Registering compiled workflow",
            )
            .await;
        }
        let image_id = self
            .register_image(
                client,
                tenant_id,
                workflow_id,
                version_u32,
                &result,
                &source_checksum,
            )
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
            .record_registered_image_id(
                tenant_id,
                workflow_id,
                version,
                &image_id,
                Some(&source_checksum),
            )
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to record registered image ID: {}", e))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 8b completed - image ID recorded in database"
        );

        // 9. Record child workflow dependencies
        if !result.child_dependencies.is_empty() {
            let step_start = std::time::Instant::now();
            debug!(
                dependency_count = result.child_dependencies.len(),
                "compile: step 9 - recording child workflow dependencies"
            );
            for dep in &result.child_dependencies {
                let insert_result = sqlx::query!(
                    r#"
                    INSERT INTO workflow_dependencies
                        (parent_tenant_id, parent_workflow_id, parent_version, child_workflow_id,
                         child_version_requested, child_version_resolved, step_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (parent_tenant_id, parent_workflow_id, parent_version, step_id)
                    DO UPDATE SET
                        child_workflow_id = $4,
                        child_version_requested = $5,
                        child_version_resolved = $6
                    "#,
                    tenant_id,
                    workflow_id,
                    version,
                    dep.child_workflow_id,
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
                "compile: step 9 completed - child workflow dependencies recorded"
            );
        }

        info!(
            total_duration_ms = compile_start.elapsed().as_millis(),
            "Compilation successful for workflow {} version {} ({} bytes) [registered: {}]",
            workflow_id,
            version,
            result.binary_size,
            image_id
        );

        // Terminal state (success) is now in scenario_compilations; clear
        // the Redis progress entry so polling clients fall through to the
        // DB read.
        if let Some(r) = &progress_reporter {
            r.clear().await;
        }

        Ok(CompilationResultDto {
            workflow_id: workflow_id.to_string(),
            version,
            build_dir: result.build_dir.to_string_lossy().to_string(),
            binary_size: result.binary_size,
            binary_checksum: result.binary_checksum,
            image_id: Some(image_id),
        })
    }

    /// Load child workflows from database and convert to ChildWorkflowInput
    async fn load_child_workflows_as_input(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        definition: &serde_json::Value,
    ) -> Result<Vec<ChildWorkflowInput>, ServiceError> {
        let child_workflows_list = load_child_workflows(
            self.repository.pool(),
            tenant_id,
            workflow_id,
            version,
            definition,
        )
        .await
        .map_err(|e| {
            ServiceError::CompilationError(format!("Failed to load child workflows: {}", e))
        })?;

        if !child_workflows_list.is_empty() {
            debug!(
                tenant_id = %tenant_id,
                workflow_id = %workflow_id,
                version = version,
                child_workflow_count = child_workflows_list.len(),
                "Loaded child workflows for compilation"
            );
        }

        // Convert to ChildWorkflowInput
        let mut child_workflows = Vec::new();
        for info in child_workflows_list {
            let graph = parse_execution_graph(&info.execution_graph).map_err(|e| {
                ServiceError::CompilationError(format!(
                    "Failed to parse child workflow '{}': {}",
                    info.workflow_ref.workflow_id, e
                ))
            })?;

            child_workflows.push(ChildWorkflowInput {
                step_id: info.step_id,
                workflow_id: info.workflow_ref.workflow_id,
                version_requested: info.version_requested,
                version_resolved: info.workflow_ref.version,
                execution_graph: graph,
            });
        }

        Ok(child_workflows)
    }

    /// Walk the parent graph and every child graph for Agent / AiAgent steps
    /// that reference a connection, then look up each connection's
    /// `integration_id` via the connections facade. Returns
    /// `connection_id -> integration_id` for every row that exists and has a
    /// non-empty `integration_id`. Missing rows / NULL columns are silently
    /// omitted: the codegen falls back to the empty-string behavior, which is
    /// only broken for component agents that dispatch on integration_id —
    /// and those are explicit fix candidates anyway.
    ///
    /// No-op when `connections_facade` isn't wired in (CLI / test paths).
    async fn resolve_connection_integration_ids(
        &self,
        tenant_id: &str,
        execution_graph: &runtara_dsl::ExecutionGraph,
        child_workflows: &[ChildWorkflowInput],
    ) -> HashMap<String, String> {
        let facade = match &self.connections_facade {
            Some(f) => f,
            None => return HashMap::new(),
        };

        let mut connection_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        Self::collect_connection_ids(execution_graph, &mut connection_ids);
        for child in child_workflows {
            Self::collect_connection_ids(&child.execution_graph, &mut connection_ids);
        }

        let mut out = HashMap::with_capacity(connection_ids.len());
        for conn_id in connection_ids {
            match facade.get_connection(&conn_id, tenant_id).await {
                Ok(Some(dto)) => {
                    if let Some(int_id) = dto.integration_id.filter(|s| !s.is_empty()) {
                        out.insert(conn_id, int_id);
                    }
                }
                Ok(None) => {
                    debug!(
                        connection_id = %conn_id,
                        tenant_id = %tenant_id,
                        "compile: connection referenced by step is missing; \
                         integration_id will fall back to empty string"
                    );
                }
                Err(e) => {
                    warn!(
                        connection_id = %conn_id,
                        tenant_id = %tenant_id,
                        error = %e,
                        "compile: failed to load connection for integration_id pre-resolution; \
                         continuing with empty fallback"
                    );
                }
            }
        }
        out
    }

    /// Push every `connection_id` referenced by Agent / AiAgent steps in the
    /// graph into `out`. Steps without a connection_id (or step kinds that
    /// don't take one) are skipped.
    fn collect_connection_ids(
        graph: &runtara_dsl::ExecutionGraph,
        out: &mut std::collections::HashSet<String>,
    ) {
        for step in graph.steps.values() {
            match step {
                runtara_dsl::Step::Agent(agent) => {
                    if let Some(ref id) = agent.connection_id {
                        out.insert(id.clone());
                    }
                }
                runtara_dsl::Step::AiAgent(ai) => {
                    if let Some(ref id) = ai.connection_id {
                        out.insert(id.clone());
                    }
                }
                // Recurse into subgraphs so agent steps nested inside loops /
                // splits are covered too (EmbedWorkflow children are walked
                // separately by the caller via `child_workflows`).
                runtara_dsl::Step::Split(s) => Self::collect_connection_ids(&s.subgraph, out),
                runtara_dsl::Step::While(w) => Self::collect_connection_ids(&w.subgraph, out),
                _ => {}
            }
        }
    }

    /// Register a compiled binary with runtara-environment using streaming upload
    async fn register_image(
        &self,
        client: &RuntimeClient,
        tenant_id: &str,
        workflow_id: &str,
        version: u32,
        compilation_result: &runtara_workflows::NativeCompilationResult,
        source_checksum: &str,
    ) -> Result<String, ServiceError> {
        // Build the image name: {workflow_id}:{version}
        let image_name = format!("{}:{}", workflow_id, version);

        // Get binary path and size (use binary_path from compilation result,
        // which is target-aware: "workflow" for native, "workflow.wasm" for WASM)
        let binary_path = &compilation_result.binary_path;
        let metadata = tokio::fs::metadata(&binary_path).await.map_err(|e| {
            ServiceError::RegistrationError(format!("Failed to read binary metadata: {}", e))
        })?;
        let binary_size = metadata.len();

        info!(
            "Registering image {} for tenant {} ({} bytes)",
            image_name, tenant_id, binary_size
        );

        // Create registration options with workflow variables as metadata.
        // Every compile produces a components-mode `workflow.wasm`, so the
        // runner type is always `Wasm` now.
        let options = RegisterImageStreamOptions::new(tenant_id, &image_name, binary_size)
            .with_description(format!("Workflow {} version {}", workflow_id, version))
            .with_runner_type(RunnerType::Wasm)
            .with_sha256(&compilation_result.binary_checksum)
            .with_metadata(serde_json::json!({
                "variables": compilation_result.default_variables,
                "workflow": {
                    "workflowId": workflow_id,
                    "version": version,
                    "sourceChecksum": source_checksum,
                    // Major version of `runtara-workflows`. Cache miss on major
                    // bump invalidates every workflow on next deploy.
                    "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
                }
            }));

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
    pub workflow_id: String,
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
        let error = ServiceError::NotFound("Workflow 'test' version 5 not found".to_string());
        assert_eq!(
            error.to_string(),
            "Not found: Workflow 'test' version 5 not found"
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

    #[test]
    fn direct_compilation_settings_default_to_disabled() {
        let settings = DirectCompilationSettings::disabled();

        assert!(!settings.enabled);
        assert!(settings.components_dir.is_none());
    }

    #[test]
    fn direct_compilation_settings_keep_component_dir() {
        let settings = DirectCompilationSettings::enabled(Some("/opt/runtara/agents".into()));

        assert!(settings.enabled);
        assert_eq!(
            settings.components_dir.as_deref(),
            Some(std::path::Path::new("/opt/runtara/agents"))
        );
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
            workflow_id: "my-workflow".to_string(),
            version: 7,
            build_dir: "/tmp/builds/abc123".to_string(),
            binary_size: 5_242_880, // 5MB
            binary_checksum: "sha256:abc123def456".to_string(),
            image_id: Some("img-uuid-12345".to_string()),
        };

        assert_eq!(result.workflow_id, "my-workflow");
        assert_eq!(result.version, 7);
        assert_eq!(result.build_dir, "/tmp/builds/abc123");
        assert_eq!(result.binary_size, 5_242_880);
        assert_eq!(result.binary_checksum, "sha256:abc123def456");
        assert_eq!(result.image_id, Some("img-uuid-12345".to_string()));
    }

    #[test]
    fn test_compilation_result_dto_without_image_id() {
        let result = CompilationResultDto {
            workflow_id: "local-only".to_string(),
            version: 1,
            build_dir: "/data/workflows/local-only/build".to_string(),
            binary_size: 1024,
            binary_checksum: "sha256:1234".to_string(),
            image_id: None,
        };

        assert!(result.image_id.is_none());
    }

    #[test]
    fn test_compilation_result_dto_debug_format() {
        let result = CompilationResultDto {
            workflow_id: "test".to_string(),
            version: 1,
            build_dir: "/tmp".to_string(),
            binary_size: 100,
            binary_checksum: "checksum".to_string(),
            image_id: None,
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("workflow_id"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("version"));
        assert!(debug_str.contains("binary_size"));
    }

    #[test]
    fn test_compilation_result_dto_large_binary() {
        // Test with realistic large binary size (100MB)
        let result = CompilationResultDto {
            workflow_id: "large-workflow".to_string(),
            version: 1,
            build_dir: "/data/builds".to_string(),
            binary_size: 104_857_600,
            binary_checksum: "sha256:largechecksum".to_string(),
            image_id: Some("img-large".to_string()),
        };

        assert_eq!(result.binary_size, 104_857_600);
    }
}
