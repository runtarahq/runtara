use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::api::repositories::workflows::{
    CompilationSuccessRecord, WorkflowRepository, workflow_definition_checksum,
};
use crate::compiler::child_workflows::load_child_workflows;
use crate::runtime_client::RuntimeClient;
use crate::valkey::compilation_progress::{CompilationStage, ProgressReporter};
use opentelemetry::KeyValue;
use redis::aio::ConnectionManager;
use runtara_dsl::parse_execution_graph;
use runtara_management_sdk::{ImageSummary, RegisterImageStreamOptions, RunnerType};
use runtara_workflows::compile::ProgressCallback;
use runtara_workflows::direct_wasm::{
    DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME, DirectArtifactMetadata,
};
use runtara_workflows::{
    ChildWorkflowInput, CompilationInput, DirectWorkflowCompileOptions, NativeCompilationResult,
    ValidationError, WorkflowCompilerMode, compile_workflow_direct,
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

fn image_compiler_mode(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/compilerMode"))
        .and_then(|v| v.as_str())
}

/// Whether `image` is a cache hit for the current source, compiler major, and
/// desired compiler mode. Older images that lack either provenance field miss
/// once and are refreshed through the selected compile path.
fn image_cache_hits(
    image: &ImageSummary,
    source_checksum: &str,
    compiler_mode: WorkflowCompilerMode,
) -> bool {
    image_source_checksum(image) == Some(source_checksum)
        && image_template_major(image) == Some(runtara_workflows::TEMPLATE_MAJOR_VERSION)
        && image_compiler_mode(image) == Some(compiler_mode.as_str())
}

fn workflow_image_metadata(
    compilation_result: &NativeCompilationResult,
    workflow_id: &str,
    version: u32,
    source_checksum: &str,
    direct_artifact: Option<&DirectArtifactMetadata>,
) -> serde_json::Value {
    let mut workflow = serde_json::json!({
        "workflowId": workflow_id,
        "version": version,
        "sourceChecksum": source_checksum,
        // Major version of `runtara-workflows`. Cache miss on major
        // bump invalidates every workflow on next deploy.
        "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
        "compilerMode": compilation_result.compiler_mode.as_str(),
        "directWasm": {
            "enabled": true,
            "outcome": "success",
            "reason": "none",
        },
    });

    if let Some(direct_artifact) = direct_artifact {
        workflow["directArtifact"] = serde_json::to_value(direct_artifact)
            .expect("direct artifact metadata should serialize");
    }

    serde_json::json!({
        "variables": compilation_result.default_variables,
        "workflow": workflow
    })
}

async fn direct_artifact_metadata_for_image(
    compilation_result: &NativeCompilationResult,
) -> Option<DirectArtifactMetadata> {
    if compilation_result.compiler_mode != WorkflowCompilerMode::DirectWasm {
        return None;
    }

    let path = compilation_result
        .build_dir
        .join(DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME);
    match tokio::fs::read(&path).await {
        Ok(bytes) => match serde_json::from_slice::<DirectArtifactMetadata>(&bytes) {
            Ok(metadata) => Some(metadata),
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to parse direct workflow artifact metadata; image registration will omit direct artifact provenance"
                );
                None
            }
        },
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "Failed to read direct workflow artifact metadata; image registration will omit direct artifact provenance"
            );
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkflowImageRegistration<'a> {
    tenant_id: &'a str,
    workflow_id: &'a str,
    version: u32,
    source_checksum: &'a str,
}

/// Direct WASM compilation settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectCompilationSettings {
    /// Directory containing prebuilt shared workflow and agent components.
    pub components_dir: Option<PathBuf>,
    /// Additional agent-component search dirs (per-tenant workflow-agent
    /// staging) consulted after `components_dir` during composition.
    pub extra_component_dirs: Vec<PathBuf>,
}

/// Build direct compilation settings from the process configuration.
pub fn direct_compilation_settings_from_config() -> DirectCompilationSettings {
    DirectCompilationSettings {
        components_dir: crate::config::direct_wasm_components_dir(),
        extra_component_dirs: Vec::new(),
    }
}

fn compile_workflow_direct_only(
    input: CompilationInput,
    source_checksum: String,
    components_dir: Option<PathBuf>,
    extra_component_dirs: Vec<PathBuf>,
) -> std::io::Result<NativeCompilationResult> {
    let Some(components_dir) = components_dir else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "direct WASM compilation requires a configured component directory (RUNTARA_AGENT_COMPONENTS_DIR / RUNTARA_DIRECT_WASM_COMPONENTS_DIR)",
        ));
    };

    let direct_start = Instant::now();
    let options = DirectWorkflowCompileOptions {
        output_dir: direct_output_dir(&input.tenant_id),
        components_dir,
        extra_component_dirs,
        source_checksum: Some(source_checksum),
    };

    match compile_workflow_direct(input.clone(), options) {
        Ok(result) => {
            record_direct_compilation_outcome("success", "none", direct_start.elapsed());
            info!(
                workflow_id = %input.workflow_id,
                version = input.version,
                binary_size = result.binary_size,
                "Direct WASM workflow compilation succeeded"
            );
            Ok(result)
        }
        Err(err) => {
            record_direct_compilation_outcome("failed", "direct-error", direct_start.elapsed());
            warn!(
                workflow_id = %input.workflow_id,
                version = input.version,
                error = %err,
                "Direct WASM workflow compilation failed"
            );
            Err(err)
        }
    }
}

fn record_direct_compilation_outcome(
    outcome: &'static str,
    reason: &'static str,
    duration: Duration,
) {
    if let Some(metrics) = crate::observability::metrics() {
        let attrs = [
            KeyValue::new("outcome", outcome),
            KeyValue::new("reason", reason),
        ];
        metrics.direct_compilations_total.add(1, &attrs);
        metrics
            .direct_compilation_duration
            .record(duration.as_secs_f64(), &attrs);
    }
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
    /// Direct WASM compiler settings (component directory). Direct compilation
    /// is the only path; a missing component directory fails the compile.
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
            direct_compilation: DirectCompilationSettings {
                components_dir: None,
                extra_component_dirs: Vec::new(),
            },
        }
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

    /// Plug in direct WASM compilation settings (the component directory).
    /// Direct compilation is the only path; if it fails, the compile fails.
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

        // A workflow with no steps cannot compile, and the downstream planner
        // would report it as a missing entry step rather than as the authoring
        // mistake it is. Fail with the validation error users are meant to see.
        if execution_graph.steps.is_empty() {
            return Err(ServiceError::CompilationError(
                ValidationError::EmptyWorkflow.to_string(),
            ));
        }

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
            // When configured, the compile uses the runtime catalog from
            // the component dispatcher so the compiled view of agents
            // matches what the runtime can actually invoke — merged with the
            // tenant's PUBLISHED workflow-agents so a parent can target them.
            agent_catalog: self
                .agent_catalog
                .as_ref()
                .map(|base| crate::workflow_agents::catalog_with_workflow_agents(base, tenant_id)),
            agent_slug: None,
            progress_callback,
        };
        let desired_compiler_mode = WorkflowCompilerMode::DirectWasm;

        // 5. Check if already registered BEFORE compiling, unless a rebuild was requested.
        // This prevents FK constraint violations when re-compiling workflows that are already registered.
        let step_start = std::time::Instant::now();
        debug!("compile: step 5 - checking if already registered in database");
        let existing_image_id = if force_recompile {
            None
        } else {
            self.repository
                .get_fresh_registered_image_id_for_compiler(
                    tenant_id,
                    workflow_id,
                    version,
                    desired_compiler_mode.as_str(),
                )
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to check existing image: {}", e))
                })?
        };
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            found = existing_image_id.is_some(),
            desired_compiler_mode = desired_compiler_mode.as_str(),
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
                    if image_cache_hits(
                        &existing_image,
                        source_checksum.as_str(),
                        desired_compiler_mode,
                    ) =>
                {
                    let compiler_mode = image_compiler_mode(&existing_image).map(str::to_string);
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
                            compiler_mode.as_deref(),
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
                        desired_compiler_mode = desired_compiler_mode.as_str(),
                        "compile: step 5b found image name but source checksum, template major, or compiler mode differed or was absent; rebuilding"
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
            "compile: step 6 - semaphore acquired, compiling workflow artifact"
        );
        let compile_start_time = std::time::Instant::now();
        let direct_compilation = self.direct_compilation.clone();
        let compile_source_checksum = source_checksum.clone();
        // A parent composing a published workflow-agent finds its staged
        // `.wasm` in the tenant staging dir (searched after the primary
        // components dir).
        let mut extra_component_dirs = direct_compilation.extra_component_dirs.clone();
        let tenant_staging = crate::workflow_agents::staging_dir(tenant_id);
        if tenant_staging.is_dir() {
            extra_component_dirs.push(tenant_staging);
        }
        let result = tokio::task::spawn_blocking(move || {
            compile_workflow_direct_only(
                compilation_input,
                compile_source_checksum,
                direct_compilation.components_dir,
                extra_component_dirs,
            )
        })
        .await
        .map_err(|e| ServiceError::CompilationError(format!("Compilation task panicked: {}", e)))?
        .map_err(|e| ServiceError::CompilationError(format!("Compilation failed: {}", e)))?;
        debug!(
            duration_ms = compile_start_time.elapsed().as_millis(),
            binary_size = result.binary_size,
            compiler_mode = result.compiler_mode.as_str(),
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
                compiler_mode: result.compiler_mode.as_str(),
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
                &result,
                WorkflowImageRegistration {
                    tenant_id,
                    workflow_id,
                    version: version_u32,
                    source_checksum: &source_checksum,
                },
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
                Some(result.compiler_mode.as_str()),
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

    /// Publish a workflow version AS an agent: compile it with the
    /// `AgentCapabilities` ABI (exports `runtara:agent-<slug>/capabilities`),
    /// synthesize the catalog `AgentInfo` from its input/output schemas, and
    /// stage both into the tenant's workflow-agent dir — after which any
    /// parent workflow can target it as `agentId: <slug>, capabilityId: "run"`
    /// (validation sees it through the catalog overlay; composition finds the
    /// `.wasm` through the extra search dir).
    ///
    /// The agent artifact is compiled with `track_events: false` — a child's
    /// step-debug events inside a parent's instance would misattribute.
    pub async fn publish_workflow_agent(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        slug: String,
    ) -> Result<serde_json::Value, ServiceError> {
        // 1. Load the definition + graph + children (same assembly as compile).
        let (definition, _track_events) = self
            .repository
            .get_definition_with_track_events(tenant_id, workflow_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to fetch definition: {e}")))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Workflow '{workflow_id}' version {version} not found"
                ))
            })?;
        let source_checksum = workflow_definition_checksum(&definition);
        let execution_graph = parse_execution_graph(&definition).map_err(|e| {
            ServiceError::CompilationError(format!("Failed to parse execution graph: {e}"))
        })?;
        let child_workflows = self
            .load_child_workflows_as_input(tenant_id, workflow_id, version, &definition)
            .await?;

        let name = execution_graph.name.clone().unwrap_or_else(|| slug.clone());
        let description = execution_graph.description.clone().unwrap_or_default();
        let info = runtara_dsl::agent_meta::workflow_agent_info(
            &slug,
            &name,
            &description,
            &execution_graph.input_schema,
            &execution_graph.output_schema,
        );

        // 2. Compile with the AgentCapabilities ABI + compose. Same catalog
        //    overlay as a normal compile so a workflow-agent may itself invoke
        //    previously-published workflow-agents.
        let Some(components_dir) = self.direct_compilation.components_dir.clone() else {
            return Err(ServiceError::CompilationError(
                "direct WASM compilation requires a configured component directory".to_string(),
            ));
        };
        let agent_catalog = self
            .agent_catalog
            .as_ref()
            .map(|base| crate::workflow_agents::catalog_with_workflow_agents(base, tenant_id));
        let direct_input = runtara_workflows::direct_wasm::DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: version as u32,
            source_checksum: Some(source_checksum),
            execution_graph,
            child_workflows,
            output_dir: direct_output_dir(tenant_id).join("agent-publish"),
            track_events: false,
            agent_catalog,
            agent_slug: Some(slug.clone()),
        };
        let mut extra_dirs = self.direct_compilation.extra_component_dirs.clone();
        let tenant_staging = crate::workflow_agents::staging_dir(tenant_id);
        if tenant_staging.is_dir() {
            extra_dirs.push(tenant_staging);
        }
        let permit = compilation_semaphore().acquire().await.map_err(|e| {
            ServiceError::CompilationError(format!("Compilation queue closed: {e}"))
        })?;
        let result = tokio::task::spawn_blocking(move || {
            let mut result = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
                direct_input,
                runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
                // Blocking durable-sleep (store-freeing suspend stays gated) and
                // omit-runtime "requested" — the AgentCapabilities arm decides
                // the effective shape.
                false,
                true,
            )?;
            runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
                &mut result,
                &components_dir,
                &extra_dirs,
            )?;
            Ok::<_, runtara_workflows::direct_wasm::DirectCompileError>(result)
        })
        .await
        .map_err(|e| ServiceError::CompilationError(format!("Publish task panicked: {e}")))?
        .map_err(|e| ServiceError::CompilationError(format!("Agent publish failed: {e}")))?;
        drop(permit);

        // 3. Stage the composed artifact + synthesized meta.
        let (wasm_path, meta_path) =
            crate::workflow_agents::stage(tenant_id, &slug, &result.wasm_path, &info)
                .map_err(|e| ServiceError::CompilationError(format!("Staging failed: {e}")))?;

        info!(
            %tenant_id, %workflow_id, version, %slug,
            wasm = %wasm_path.display(),
            "published workflow as agent"
        );
        Ok(serde_json::json!({
            "slug": slug,
            "agentId": info.id,
            "capabilityId": runtara_dsl::agent_meta::WORKFLOW_AGENT_CAPABILITY_ID,
            "workflowId": workflow_id,
            "version": version,
            "wasmSizeBytes": result.wasm_size,
            "stagedWasm": wasm_path.display().to_string(),
            "stagedMeta": meta_path.display().to_string(),
        }))
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

    /// Register a compiled binary with runtara-environment using streaming upload
    async fn register_image(
        &self,
        client: &RuntimeClient,
        compilation_result: &runtara_workflows::NativeCompilationResult,
        registration: WorkflowImageRegistration<'_>,
    ) -> Result<String, ServiceError> {
        // Build the image name: {workflow_id}:{version}
        let image_name = format!("{}:{}", registration.workflow_id, registration.version);

        // Get binary path and size (use binary_path from compilation result,
        // which is target-aware: "workflow" for native, "workflow.wasm" for WASM)
        let binary_path = &compilation_result.binary_path;
        let metadata = tokio::fs::metadata(&binary_path).await.map_err(|e| {
            ServiceError::RegistrationError(format!("Failed to read binary metadata: {}", e))
        })?;
        let binary_size = metadata.len();

        info!(
            "Registering image {} for tenant {} ({} bytes)",
            image_name, registration.tenant_id, binary_size
        );

        // Create registration options with workflow variables as metadata.
        // Every compile produces a components-mode `workflow.wasm`, so the
        // runner type is always `Wasm` now.
        let direct_artifact = direct_artifact_metadata_for_image(compilation_result).await;
        let options =
            RegisterImageStreamOptions::new(registration.tenant_id, &image_name, binary_size)
                .with_description(format!(
                    "Workflow {} version {}",
                    registration.workflow_id, registration.version
                ))
                .with_runner_type(RunnerType::Wasm)
                .with_sha256(&compilation_result.binary_checksum)
                .with_metadata(workflow_image_metadata(
                    compilation_result,
                    registration.workflow_id,
                    registration.version,
                    registration.source_checksum,
                    direct_artifact.as_ref(),
                ));

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
    use runtara_workflows::direct_wasm::{
        DirectArtifactFileMetadata, DirectComponentDependencyMetadata,
    };

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
    fn image_cache_hit_requires_matching_compiler_mode() {
        let metadata = serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
                "compilerMode": "direct-wasm"
            }
        });
        let image = image_summary_with_metadata(metadata);

        assert!(image_cache_hits(
            &image,
            "source-sha256",
            WorkflowCompilerMode::DirectWasm
        ));
        assert!(!image_cache_hits(
            &image,
            "other-source",
            WorkflowCompilerMode::DirectWasm
        ));

        let missing_mode = image_summary_with_metadata(serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION
            }
        }));
        assert!(!image_cache_hits(
            &missing_mode,
            "source-sha256",
            WorkflowCompilerMode::DirectWasm
        ));
    }

    #[test]
    fn workflow_image_metadata_records_compiler_mode_and_direct_wasm_block() {
        let result = NativeCompilationResult {
            binary_path: "/tmp/workflow.wasm".into(),
            binary_size: 123,
            binary_checksum: "abc".to_string(),
            build_dir: "/tmp/build".into(),
            package_size: 99,
            child_dependencies: vec![],
            default_variables: serde_json::json!({ "limit": 5 }),
            compiler_mode: WorkflowCompilerMode::DirectWasm,
        };

        let metadata = workflow_image_metadata(&result, "workflow-a", 7, "source-sha256", None);

        assert_eq!(metadata["variables"], serde_json::json!({ "limit": 5 }));
        assert_eq!(metadata["workflow"]["workflowId"], "workflow-a");
        assert_eq!(metadata["workflow"]["version"], 7);
        assert_eq!(metadata["workflow"]["sourceChecksum"], "source-sha256");
        assert_eq!(
            metadata["workflow"]["templateMajor"],
            runtara_workflows::TEMPLATE_MAJOR_VERSION
        );
        assert_eq!(metadata["workflow"]["compilerMode"], "direct-wasm");
        assert_eq!(metadata["workflow"]["directWasm"]["enabled"], true);
        assert_eq!(metadata["workflow"]["directWasm"]["outcome"], "success");
        assert_eq!(metadata["workflow"]["directWasm"]["reason"], "none");
    }

    #[test]
    fn workflow_image_metadata_records_direct_artifact_provenance() {
        let result = native_result_with_mode(WorkflowCompilerMode::DirectWasm, "/tmp/build".into());
        let artifact = direct_artifact_metadata_fixture();

        let metadata =
            workflow_image_metadata(&result, "workflow-a", 7, "source-sha256", Some(&artifact));

        let direct_artifact = &metadata["workflow"]["directArtifact"];
        assert_eq!(
            direct_artifact["schemaVersion"],
            serde_json::json!(
                runtara_workflows::direct_wasm::DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION
            )
        );
        assert_eq!(direct_artifact["directAbiVersion"], 1);
        assert_eq!(
            direct_artifact["manifestVersion"],
            serde_json::json!(runtara_workflows::direct_wasm::DIRECT_WORKFLOW_MANIFEST_VERSION)
        );
        assert_eq!(direct_artifact["manifestChecksum"], "manifest-sha256");
        assert_eq!(
            direct_artifact["sharedComponents"][0]["package"],
            "runtara:workflow-stdlib"
        );
        assert_eq!(
            direct_artifact["sharedComponents"][0]["wasm"]["sha256"],
            "stdlib-sha256"
        );
        assert_eq!(
            direct_artifact["agentComponents"][0]["agentId"],
            "transform"
        );
    }

    #[tokio::test]
    async fn direct_artifact_metadata_for_image_reads_only_direct_sidecars() {
        let build_dir = unique_test_dir("direct-artifact-metadata");
        std::fs::create_dir_all(&build_dir).expect("create build dir");
        let artifact = direct_artifact_metadata_fixture();
        std::fs::write(
            build_dir.join(DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME),
            serde_json::to_vec(&artifact).expect("serialize artifact metadata"),
        )
        .expect("write artifact metadata");

        let direct_result =
            native_result_with_mode(WorkflowCompilerMode::DirectWasm, build_dir.clone());
        let loaded = direct_artifact_metadata_for_image(&direct_result)
            .await
            .expect("direct artifact metadata should load");
        assert_eq!(loaded, artifact);

        let _ = std::fs::remove_dir_all(build_dir);
    }

    fn native_result_with_mode(
        compiler_mode: WorkflowCompilerMode,
        build_dir: std::path::PathBuf,
    ) -> NativeCompilationResult {
        NativeCompilationResult {
            binary_path: build_dir.join("workflow.wasm"),
            binary_size: 123,
            binary_checksum: "abc".to_string(),
            build_dir,
            package_size: 99,
            child_dependencies: vec![],
            default_variables: serde_json::json!({}),
            compiler_mode,
        }
    }

    fn image_summary_with_metadata(metadata: serde_json::Value) -> ImageSummary {
        ImageSummary {
            image_id: "image-a".to_string(),
            tenant_id: "tenant-a".to_string(),
            name: "workflow-a:7".to_string(),
            description: None,
            runner_type: RunnerType::Wasm,
            created_at: chrono::Utc::now(),
            metadata: Some(metadata),
        }
    }

    fn direct_artifact_metadata_fixture() -> DirectArtifactMetadata {
        let wasm = |filename: &str, sha256: &str| DirectArtifactFileMetadata {
            filename: filename.to_string(),
            sha256: sha256.to_string(),
            size_bytes: 42,
        };

        DirectArtifactMetadata {
            schema_version:
                runtara_workflows::direct_wasm::DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION,
            artifact_kind: "direct-workflow-component".to_string(),
            workflow_id: "workflow-a".to_string(),
            workflow_version: 7,
            source_checksum: Some("source-sha256".to_string()),
            direct_abi_version: 1,
            manifest_version: runtara_workflows::direct_wasm::DIRECT_WORKFLOW_MANIFEST_VERSION,
            template_major_version: runtara_workflows::TEMPLATE_MAJOR_VERSION.to_string(),
            manifest_checksum: "manifest-sha256".to_string(),
            support_report_checksum: "support-sha256".to_string(),
            workflow_logic_wasm: wasm("workflow-logic.wasm", "logic-sha256"),
            composed_wasm: Some(wasm("workflow.wasm", "composed-sha256")),
            shared_components: vec![DirectComponentDependencyMetadata {
                kind: "shared".to_string(),
                agent_id: None,
                package: "runtara:workflow-stdlib".to_string(),
                package_with_version: "runtara:workflow-stdlib@0.1.0".to_string(),
                wasm_filename: "runtara_workflow_stdlib.wasm".to_string(),
                wasm: Some(wasm("runtara_workflow_stdlib.wasm", "stdlib-sha256")),
                meta_filename: "runtara_workflow_stdlib.meta.json".to_string(),
                meta: None,
            }],
            agent_components: vec![DirectComponentDependencyMetadata {
                kind: "agent".to_string(),
                agent_id: Some("transform".to_string()),
                package: "runtara:agent-transform".to_string(),
                package_with_version: "runtara:agent-transform@0.3.0".to_string(),
                wasm_filename: "runtara_agent_transform.wasm".to_string(),
                wasm: Some(wasm("runtara_agent_transform.wasm", "agent-sha256")),
                meta_filename: "runtara_agent_transform.meta.json".to_string(),
                meta: None,
            }],
            child_workflows: vec![],
        }
    }

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("runtara-{label}-{nanos}"))
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
