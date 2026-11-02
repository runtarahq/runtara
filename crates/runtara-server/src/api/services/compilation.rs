use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::api::repositories::workflows::{
    CompilationSuccessRecord, RegisteredImageRecord, WorkflowRepository,
    workflow_definition_checksum,
};
use crate::compiler::child_workflows::load_child_workflows;
use crate::runtime_client::RuntimeClient;
use crate::runtime_types::{ImageSummary, RegisterImageStreamOptions};
use crate::valkey::compilation_progress::{CompilationStage, ProgressReporter};
use opentelemetry::KeyValue;
use redis::aio::ConnectionManager;
use runtara_dsl::parse_execution_graph;
use runtara_workflows::compile::ProgressCallback;
use runtara_workflows::direct_wasm::{
    DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME, DirectArtifactMetadata,
};
use runtara_workflows::{
    ChildWorkflowInput, CompilationInput, DirectWorkflowCompileOptions, NativeCompilationResult,
    ValidationError, WorkflowCompilerMode,
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

/// The agents an image was composed with, from the direct artifact metadata
/// stored at registration. A launch checks them against the allowlist, since
/// an image keeps the children it was built with. `None` when the image
/// carries no such record (registered before it existed, or without its
/// artifact metadata), or when the record is malformed.
pub(crate) fn image_agent_ids(image: &ImageSummary) -> Option<Vec<String>> {
    image
        .metadata
        .as_ref()?
        .pointer("/workflow/directArtifact/agentComponents")?
        .as_array()?
        .iter()
        .map(|component| {
            component
                .get("agentId")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .collect()
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

/// The lowering options the image was built with, when it records them.
///
/// Absent on images compiled before lowering became part of cache identity,
/// which reads as a miss and rebuilds them once.
fn image_lowering_mode(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/loweringMode"))
        .and_then(|v| v.as_str())
}

/// The digest of the release index an image was composed from.
fn image_component_release(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/componentRelease/index"))
        .and_then(|v| v.as_str())
}

fn image_compiler_mode(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/compilerMode"))
        .and_then(|v| v.as_str())
}

/// Whether the image was built with step-event instrumentation enabled.
///
/// This is deliberately a required cache-key field rather than a display
/// detail. `trackEvents` changes the generated workflow component, so an image
/// built with the opposite value cannot attest that it can (or cannot) report
/// steps for a deployment.
fn image_track_events(image: &ImageSummary) -> Option<bool> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/trackEvents"))
        .and_then(|v| v.as_bool())
}

/// The memory setting an image was registered with, in bytes. A launch
/// enforces it; an image without one runs under the runner's ceiling only.
fn image_memory_limit_bytes(image: &ImageSummary) -> Option<u64> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/memoryLimitBytes"))
        .and_then(|v| v.as_u64())
}

/// Refuse a memory setting below what the composed workflow allocates when
/// it is instantiated: such a workflow could never start.
fn check_memory_floor(memory_mb: u32, floor_bytes: Option<u64>) -> Result<(), ServiceError> {
    let limit = u64::from(memory_mb) * 1024 * 1024;
    match floor_bytes {
        Some(floor) if floor > limit => Err(ServiceError::WorkflowAuthoringError(format!(
            "Memory of {memory_mb} MiB is below this workflow's memory floor of {} MiB; \
             raise its memory",
            floor.div_ceil(1024 * 1024),
        ))),
        _ => Ok(()),
    }
}

/// SHA-256 of the exact uploaded workflow binary.
///
/// This is separate from source provenance: resolved child workflows and
/// staged workflow-agent components can change the composed bytes even when
/// the parent definition is unchanged.
fn image_binary_checksum(image: &ImageSummary) -> Option<&str> {
    image
        .metadata
        .as_ref()
        .and_then(|m| m.pointer("/workflow/binaryChecksum"))
        .and_then(|v| v.as_str())
}

/// Whether `image` is a cache hit for the current source, compiler major,
/// compiler mode, lowering options, and step-event instrumentation. Older
/// images that lack a provenance field miss once and are refreshed through the
/// selected compile path.
///
/// Lowering belongs in the key because it changes the emitted artifact for an
/// unchanged definition: a workflow compiled before store-freeing sleep was
/// turned on would otherwise keep serving its blocking-sleep artifact forever,
/// since nothing else about it changed. The component release belongs there
/// for the same reason: new agent or stdlib bytes compose a different artifact.
fn image_cache_hits(
    image: &ImageSummary,
    source_checksum: &str,
    compiler_mode: WorkflowCompilerMode,
    track_events: bool,
) -> bool {
    image_source_checksum(image) == Some(source_checksum)
        && image_template_major(image) == Some(runtara_workflows::TEMPLATE_MAJOR_VERSION)
        && image_compiler_mode(image) == Some(compiler_mode.as_str())
        && image_lowering_mode(image) == Some(crate::config::workflow_lowering_tag().as_str())
        && image_track_events(image) == Some(track_events)
        && image_component_release(image)
            == crate::config::compile_component_release().map(|release| release.index.as_str())
}

/// Whether `image` is the exact compiled artifact about to be registered.
///
/// The provenance check makes a same-name image semantically eligible; the
/// binary checksum makes it safe to reuse as an immutable artifact even when
/// resolved child graphs or staged agent components changed during a forced
/// rebuild.
fn image_matches_compiled_artifact(
    image: &ImageSummary,
    source_checksum: &str,
    compiler_mode: WorkflowCompilerMode,
    track_events: bool,
    binary_checksum: &str,
) -> bool {
    image_cache_hits(image, source_checksum, compiler_mode, track_events)
        && image_binary_checksum(image) == Some(binary_checksum)
}

/// Fingerprint the declared compilation provenance used for cache identity.
fn workflow_compilation_fingerprint(
    source_checksum: &str,
    compiler_mode: WorkflowCompilerMode,
    track_events: bool,
) -> String {
    let lowering_mode = crate::config::workflow_lowering_tag();
    let mut fingerprint = Sha256::new();

    let component_release = crate::config::compile_component_release()
        .map(|release| release.index.as_str())
        .unwrap_or_default();
    for field in [
        source_checksum,
        runtara_workflows::TEMPLATE_MAJOR_VERSION,
        compiler_mode.as_str(),
        lowering_mode.as_str(),
        component_release,
    ] {
        fingerprint.update(field.as_bytes());
        fingerprint.update([0]);
    }
    fingerprint.update([u8::from(track_events)]);

    hex::encode(fingerprint.finalize())
}

/// Build the Environment name for one immutable compiled artifact.
///
/// Environment deliberately treats a repeated `(tenant, name)` upload as an
/// in-place replacement. A mutable `{workflow_id}:{version}` name therefore
/// lets a launch which already holds an image UUID observe a different binary
/// after a concurrent recompile. The name combines the declared provenance
/// fingerprint with the final binary checksum, so different output is always
/// a distinct image row even when a forced retry has the same parent source.
///
/// The readable prefix preserves the existing workflow-instance lookup
/// convention. The `@` suffix is intentionally opaque and filesystem-safe.
fn workflow_image_name(
    workflow_id: &str,
    version: u32,
    source_checksum: &str,
    compiler_mode: WorkflowCompilerMode,
    track_events: bool,
    binary_checksum: &str,
) -> String {
    format!(
        "{workflow_id}:{version}@{}-{binary_checksum}",
        workflow_compilation_fingerprint(source_checksum, compiler_mode, track_events)
    )
}

fn workflow_image_metadata(
    compilation_result: &NativeCompilationResult,
    workflow_id: &str,
    version: u32,
    source_checksum: &str,
    track_events: bool,
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
        // Part of cache identity: see `image_cache_hits`.
        "loweringMode": crate::config::workflow_lowering_tag(),
        // Part of cache identity: step-debug instrumentation changes the
        // generated workflow component.
        "trackEvents": track_events,
        // Immutable registration identity includes the exact composed bytes.
        "binaryChecksum": compilation_result.binary_checksum,
        "directWasm": {
            "enabled": true,
            "outcome": "success",
            "reason": "none",
        },
    });

    // Which platform release the agent and shared components came from; the
    // digests of each are in `directArtifact`.
    if let Some(release) = crate::config::compile_component_release() {
        workflow["componentRelease"] = serde_json::json!({
            "release": release.release,
            "index": release.index,
        });
    }

    if let Some(direct_artifact) = direct_artifact {
        workflow["directWasm"]["entryAbi"] = serde_json::json!(direct_artifact.entry_abi);
        workflow["directArtifact"] = serde_json::to_value(direct_artifact)
            .expect("direct artifact metadata should serialize");
    }

    serde_json::json!({
        "variables": compilation_result.default_variables,
        // Images registered before this marker store `variables` as
        // `{type, value}` declarations; the environment flattens those.
        "variablesFormat": "values",
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
    track_events: bool,
    /// The version's memory setting, recorded on the image for its launches.
    memory_limit_bytes: u64,
}

/// The exact workflow source a compilation attempt consumes.
///
/// A failed attempt needs this provenance just as much as a successful one:
/// rereading a definition after a toggle can stamp a failure with a mode the
/// compiler never saw and incorrectly make it terminal.
#[derive(Debug, Clone)]
pub(crate) struct CompilationSource {
    pub(crate) definition: serde_json::Value,
    pub(crate) source_checksum: String,
    pub(crate) track_events: bool,
    /// The version's memory, in MiB.
    pub(crate) memory_mb: u32,
}

/// Direct WASM compilation settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectCompilationSettings {
    /// Directory containing prebuilt shared workflow and agent components.
    pub components_dir: Option<PathBuf>,
    /// Additional agent-component search dirs (per-tenant workflow-agent
    /// staging) consulted after `components_dir` during composition.
    pub extra_component_dirs: Vec<PathBuf>,
    /// How workflows reach the stdlib: the mode this process's runner runs
    /// artifacts in.
    pub stdlib: runtara_workflows::direct_wasm::DirectStdlibMode,
}

/// Build direct compilation settings from the process configuration.
pub fn direct_compilation_settings_from_config() -> DirectCompilationSettings {
    DirectCompilationSettings {
        components_dir: crate::config::direct_wasm_components_dir(),
        extra_component_dirs: Vec::new(),
        stdlib: crate::compile_executor::workflow_stdlib(),
    }
}

/// Refuse a workflow that cannot be published as an agent, before any
/// artifact or sidecar is staged.
///
/// Parking is not a refusal: a wait, a sleep, a suspending capability or a
/// durable retry backoff returns the `suspended` outcome, which the caller
/// forwards, so no runner slot is held. Control calls and WaitForInstances run
/// under the caller's instance. What still refuses is what cannot be shown
/// sound: an AiAgent model-call retry, or a child closure the compiler cannot
/// see.
fn require_publishable_workflow_agent(
    execution_graph: &runtara_dsl::ExecutionGraph,
    child_workflows: &[ChildWorkflowInput],
    catalog: Option<&runtara_dsl::agent_meta::AgentCatalog>,
) -> Result<runtara_workflows::direct_wasm::WorkflowAgentSafetyReport, ServiceError> {
    let workflow_agent_safety = runtara_workflows::direct_wasm::analyze_workflow_agent_safety(
        execution_graph,
        child_workflows,
        catalog,
    );
    if let Some(violation) = workflow_agent_safety.violations.first() {
        return Err(ServiceError::WorkflowAuthoringError(format!(
            "workflow cannot be published as an agent at {} ({}/{}): {}; run it as a top-level workflow or remove that path",
            violation.path, violation.step_type, violation.feature, violation.reason,
        )));
    }
    Ok(workflow_agent_safety)
}

fn compile_workflow_direct_only(
    input: CompilationInput,
    source_checksum: String,
    components_dir: Option<PathBuf>,
    extra_component_dirs: Vec<PathBuf>,
    stdlib: runtara_workflows::direct_wasm::DirectStdlibMode,
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
        stdlib: Some(stdlib),
    };

    match crate::compile_executor::compile(input.clone(), options) {
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

/// Why a fresh artifact that pins `uninstalled` cannot run here. The compiler
/// already refuses a published workflow-agent built against another version,
/// so what remains is a compiler bundle whose trusted built-ins differ from
/// the ones the server loaded at startup.
fn trusted_dependency_unavailable_message(uninstalled: &[String]) -> String {
    let agents: std::collections::BTreeSet<&str> = uninstalled
        .iter()
        .map(|pin| {
            runtara_dsl::agent_meta::trusted_artifact_import_agent_id(pin).unwrap_or(pin.as_str())
        })
        .collect();
    format!(
        "the compiled artifact pins a version of trusted built-in {} that this server does not \
         run: the component bundle the compiler read differs from the one the server loaded at \
         startup. Either the bundle was replaced on disk without a restart (restart the server), \
         or RUNTARA_DIRECT_WASM_COMPONENTS_DIR names a different bundle than \
         RUNTARA_AGENT_COMPONENTS_DIR (point both at the same bundle and restart); then recompile",
        agents
            .iter()
            .map(|agent| format!("`{agent}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Trusted built-ins the server runs (`installed_pins`) whose version in the
/// compiler's bundle `compile_dir` differs or is missing. Every workflow that
/// uses one fails to compile with `TrustedDependencyUnavailable` until the two
/// bundles agree, so startup warns about it.
pub fn trusted_builtins_differing_from<'a>(
    compile_dir: &std::path::Path,
    installed_pins: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    installed_pins
        .into_iter()
        .filter_map(|pin| {
            let agent = runtara_dsl::agent_meta::trusted_artifact_import_agent_id(pin)?;
            (runtara_workflows::direct_wasm::bundled_trusted_pin(compile_dir, agent).as_deref()
                != Some(pin))
            .then(|| agent.to_owned())
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Refuse a fresh artifact that pins a trusted built-in version this server
/// does not run. It is never ready, so recording it as a success would make
/// every launch recompile it forever; the failure instead carries all of the
/// artifact's pins and stays terminal until they are installed.
pub fn reject_uninstalled_trusted_pins(trusted_pins: &[String]) -> Result<(), ServiceError> {
    let uninstalled = crate::api::repositories::workflows::uninstalled_trusted_pins(trusted_pins);
    if uninstalled.is_empty() {
        return Ok(());
    }
    Err(ServiceError::TrustedDependencyUnavailable {
        message: trusted_dependency_unavailable_message(&uninstalled),
        trusted_pins: trusted_pins.to_vec(),
    })
}

/// Why [`check_compile_agent_allowlist`] refused a compile.
#[derive(Debug)]
pub enum CompileAgentGateError {
    /// A step in the version's closure calls an agent the tenant's allowlist
    /// leaves out.
    Denied(crate::entitlement_error::EntitlementDenial),
    /// The database could not load the closure, so it was not checked.
    Unchecked(String),
    /// The closure as stored cannot be checked: an EmbedWorkflow reference
    /// does not resolve, or a graph does not parse. It stays that way until
    /// the workflow changes.
    Invalid(String),
}

/// What [`check_compile_agent_allowlist`] found in a closure it allowed.
#[derive(Debug, Clone, Default)]
pub struct AllowedClosure {
    /// The embedded children the check walked, as they resolve today.
    children: Vec<crate::compiler::child_workflows::ChildWorkflowInfo>,
}

/// One embedded child as an image records it: the EmbedWorkflow step, the
/// child workflow, its resolved version, and its direct manifest checksum.
type ChildFingerprint = (String, String, i64, String);

impl AllowedClosure {
    /// The version embeds other workflows.
    pub fn embeds_workflows(&self) -> bool {
        !self.children.is_empty()
    }

    /// Whether a compile may hand back `image_id`, the version's cached
    /// image, instead of building one from the closure just checked.
    ///
    /// The check reads each embedded child as it resolves today, but a cached
    /// image keeps the children it was built with, and its freshness covers
    /// only the version's own definition. A child published since ("latest"
    /// or "current" moved) or edited in place would leave agents in the image
    /// the check never saw. So the image of a version that embeds workflows
    /// is reused only when it was built from the children just checked; see
    /// [`Self::built_from_these_children`]. Anything that cannot show that,
    /// including an image the runtime cannot read, is rebuilt.
    pub async fn may_reuse_cached_image(
        &self,
        runtime_client: Option<&RuntimeClient>,
        tenant_id: &str,
        image_id: &str,
    ) -> bool {
        if self.children.is_empty() {
            return true;
        }
        let Some(runtime_client) = runtime_client else {
            return false;
        };
        match runtime_client.get_image(image_id, tenant_id).await {
            Ok(image) => self.built_from_these_children(image.as_ref()),
            Err(error) => {
                warn!(
                    image_id,
                    error = %error,
                    "Failed to read the cached image's children; rebuilding it"
                );
                false
            }
        }
    }

    /// Whether `image` records exactly the embedded children just checked:
    /// the same EmbedWorkflow steps, child workflows, resolved versions and
    /// manifest checksums. The checksum is the child manifest the compiler
    /// recorded, so a child version edited in place counts as changed too.
    pub fn built_from_these_children(&self, image: Option<&ImageSummary>) -> bool {
        if self.children.is_empty() {
            return true;
        }
        let Some(recorded) = image.and_then(image_child_fingerprints) else {
            return false;
        };
        self.child_fingerprints()
            .is_some_and(|checked| checked == recorded)
    }

    fn child_fingerprints(&self) -> Option<Vec<ChildFingerprint>> {
        let mut fingerprints = self
            .children
            .iter()
            .map(|child| {
                let graph = parse_execution_graph(&child.execution_graph).ok()?;
                let manifest =
                    runtara_workflows::direct_wasm::manifest::build_direct_workflow_manifest(
                        &graph,
                    )
                    .ok()?;
                Some((
                    child.step_id.clone(),
                    child.workflow_ref.workflow_id.clone(),
                    i64::from(child.workflow_ref.version),
                    manifest.checksum().to_owned(),
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        fingerprints.sort();
        Some(fingerprints)
    }
}

/// The embedded children an image was built with, from the direct artifact
/// metadata stored at registration. `None` when the image records none.
fn image_child_fingerprints(image: &ImageSummary) -> Option<Vec<ChildFingerprint>> {
    let mut fingerprints = image
        .metadata
        .as_ref()?
        .pointer("/workflow/directArtifact/childWorkflows")?
        .as_array()?
        .iter()
        .map(|child| {
            Some((
                child.get("stepId")?.as_str()?.to_owned(),
                child.get("workflowId")?.as_str()?.to_owned(),
                child.get("versionResolved")?.as_i64()?,
                child.get("manifestChecksum")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    fingerprints.sort();
    Some(fingerprints)
}

/// The agent allowlist check on a stored version's graph and its whole
/// EmbedWorkflow closure, with the tenant's published workflow-agents exempt,
/// as on save.
///
/// Save checks the closure when the version is written, but the allowlist can
/// change afterwards. The REST and MCP compiles run this before their
/// compiled-image fast paths, so a version saved under an older allowlist can
/// neither compile nor keep handing out its cached image with an agent the
/// tenant may no longer use. A closure that cannot be loaded or parsed is
/// refused, as save refuses it, rather than checked in part.
///
/// The closure is loaded the way the compiler loads it, so every
/// EmbedWorkflow reference must resolve. A cached image keeps the children it
/// was built with, so a child that has since been deleted, or a child version
/// that no longer resolves, would leave agents in that image this check
/// cannot see: such a closure is refused, as a fresh compile of it would be.
/// A closure that resolves can still differ from the one a cached image was
/// built with; see [`AllowedClosure::may_reuse_cached_image`].
///
/// A version that does not exist passes: the compile reports it as not found.
pub async fn check_compile_agent_allowlist(
    pool: &sqlx::PgPool,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
) -> Result<AllowedClosure, CompileAgentGateError> {
    let definition = WorkflowRepository::new(pool.clone())
        .get_definition(tenant_id, workflow_id, version)
        .await
        .map_err(|e| {
            CompileAgentGateError::Unchecked(format!("Failed to load workflow definition: {}", e))
        })?;
    let Some(definition) = definition else {
        return Ok(AllowedClosure::default());
    };
    let children = load_child_workflows(pool, tenant_id, workflow_id, version, &definition).await;
    let checked_children = children.as_ref().ok().cloned().unwrap_or_default();
    compile_agent_allowlist_decision(
        &crate::config::entitlements_for(tenant_id),
        &definition,
        children,
        &crate::workflow_agents::published_agent_ids(tenant_id),
    )?;
    Ok(AllowedClosure {
        children: checked_children,
    })
}

/// The decision [`check_compile_agent_allowlist`] makes once the closure is
/// loaded, taking the snapshot as an argument so unit tests can restrict it.
/// `children` is the closure load's result: a failed or incomplete load, or a
/// child whose graph does not parse, refuses the compile. The loader reports
/// a database failure with a `Database error` prefix, as
/// `load_child_workflows_for_validation` relies on too; anything else it
/// reports is a reference that does not resolve.
pub fn compile_agent_allowlist_decision(
    snapshot: &crate::entitlements::EntitlementSnapshot,
    definition: &serde_json::Value,
    children: Result<Vec<crate::compiler::child_workflows::ChildWorkflowInfo>, String>,
    published_agent_ids: &std::collections::HashSet<String>,
) -> Result<(), CompileAgentGateError> {
    let root = parse_execution_graph(definition).map_err(CompileAgentGateError::Invalid)?;
    let children = children.map_err(|e| {
        let message = format!("Failed to load child workflows: {}", e);
        if e.starts_with("Database error") {
            CompileAgentGateError::Unchecked(message)
        } else {
            CompileAgentGateError::Invalid(message)
        }
    })?;
    let child_graphs = children
        .into_iter()
        .map(|info| {
            parse_execution_graph(&info.execution_graph).map_err(|e| {
                CompileAgentGateError::Invalid(format!(
                    "Failed to parse child workflow '{}': {}",
                    info.workflow_ref.workflow_id, e
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    crate::middleware::entitlement::walk_closure_for_agents(
        snapshot,
        &root,
        &child_graphs,
        published_agent_ids,
    )
    .map_err(CompileAgentGateError::Denied)
}

/// A failed direct compile as a service error. A parent refused because a
/// published workflow-agent it composes pins a stale trusted version keeps
/// those pins, so its recorded failure is released when the workflow-agent is
/// republished instead of staying terminal until a forced recompile.
///
/// `still_stale` re-reads the staged workflow-agent the compile was refused
/// for. A republish between the compile reading it and this failure being
/// recorded would release nothing (no failure was recorded yet) and then be
/// masked by it, so a dependency that is no longer stale supersedes the
/// failure: nothing is recorded and the next launch recompiles.
fn direct_compile_failure(
    error: std::io::Error,
    still_stale: impl FnOnce(&std::path::Path) -> bool,
) -> ServiceError {
    let message = format!("Compilation failed: {error}");
    match error.get_ref().and_then(|inner| {
        inner.downcast_ref::<runtara_workflows::direct_wasm::DirectCompileError>()
    }) {
        Some(runtara_workflows::direct_wasm::DirectCompileError::StaleTrustedDependency {
            dependency,
            wasm_path,
            ..
        }) if !still_stale(wasm_path) => ServiceError::Superseded(format!(
            "published workflow-agent `{dependency}` was republished while this compile ran; \
             the next launch recompiles against it"
        )),
        Some(runtara_workflows::direct_wasm::DirectCompileError::StaleTrustedDependency {
            pins,
            ..
        }) => ServiceError::TrustedDependencyUnavailable {
            message,
            trusted_pins: pins.clone(),
        },
        _ => ServiceError::CompilationError(message),
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
    /// Whether an image's artifact is on disk, for the reuse decision.
    ///
    /// A lookup failure answers `false`: the cost of re-registering an artifact
    /// that was actually present is one wasted write, and the cost of reusing
    /// one that was not is a workflow that never starts.
    async fn artifact_present_for_reuse(&self, client: &RuntimeClient, image_id: &str) -> bool {
        match client.image_artifact_present(image_id).await {
            Ok(present) => present,
            Err(error) => {
                warn!(
                    image_id = %image_id,
                    error = %error,
                    "Could not confirm the registered artifact is on disk; re-registering"
                );
                false
            }
        }
    }

    pub fn new(
        repository: Arc<WorkflowRepository>,
        runtime_client: Option<Arc<RuntimeClient>>,
    ) -> Self {
        Self {
            repository,
            runtime_client,
            agent_catalog: None,
            redis_manager: None,
            direct_compilation: DirectCompilationSettings {
                components_dir: None,
                extra_component_dirs: Vec::new(),
                stdlib: crate::compile_executor::workflow_stdlib(),
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

    /// Load the source and instrumentation mode for one compilation attempt.
    ///
    /// Kept separate so the queue worker can persist a failure against the
    /// exact input handed to the compiler rather than rereading a possibly
    /// changed definition afterward.
    pub(crate) async fn load_compilation_source(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<CompilationSource, ServiceError> {
        let (definition, memory_mb, track_events) = self
            .repository
            .get_definition_with_memory(tenant_id, workflow_id, version)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to fetch definition: {e}")))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Workflow '{workflow_id}' version {version} not found"
                ))
            })?;

        Ok(CompilationSource {
            source_checksum: workflow_definition_checksum(&definition),
            definition,
            track_events,
            memory_mb,
        })
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
        let source = self
            .load_compilation_source(tenant_id, workflow_id, version)
            .await?;
        self.compile_workflow_from_source(tenant_id, workflow_id, version, force_recompile, source)
            .await
    }

    /// Compile a workflow from an already captured source snapshot.
    ///
    /// The queue worker uses this form so a recorded failure carries the same
    /// source checksum and tracking mode that were actually compiled.
    pub(crate) async fn compile_workflow_from_source(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        force_recompile: bool,
        source: CompilationSource,
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

        let CompilationSource {
            definition,
            source_checksum,
            track_events,
            memory_mb,
        } = source;
        let memory_limit_bytes = u64::from(memory_mb) * 1024 * 1024;

        let version_u32 = version as u32;

        // 1. Parse execution graph
        let step_start = std::time::Instant::now();
        debug!("compile: step 1 - parsing execution graph");
        let execution_graph = parse_execution_graph(&definition).map_err(|e| {
            ServiceError::CompilationError(format!("Failed to parse execution graph: {}", e))
        })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 1 completed - execution graph parsed"
        );

        // A workflow with no steps cannot compile, and the downstream planner
        // would report it as a missing entry step rather than as the authoring
        // mistake it is. Fail with the validation error users are meant to see.
        if execution_graph.steps.is_empty() {
            return Err(ServiceError::WorkflowAuthoringError(
                ValidationError::EmptyWorkflow.to_string(),
            ));
        }

        // 2. Load child workflows from database
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
                    track_events,
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
            "compile: step 6 - semaphore acquired"
        );

        // Another request may have completed while this one waited for the
        // process-wide compile permit. Recheck after the wait so identical
        // normal requests reuse the registered immutable image rather than
        // compiling and registering the same artifact twice.
        if !force_recompile {
            let existing_image_id = self
                .repository
                .get_fresh_registered_image_id_for_compiler(
                    tenant_id,
                    workflow_id,
                    version,
                    desired_compiler_mode.as_str(),
                    track_events,
                )
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!(
                        "Failed to recheck existing image after waiting for compilation: {e}"
                    ))
                })?;

            if let Some(existing_id) = existing_image_id {
                info!(
                    total_duration_ms = compile_start.elapsed().as_millis(),
                    "Workflow {} version {} was compiled while this request waited; reusing image {}",
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
        }

        debug!("compile: step 6 - compiling workflow artifact");
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
        let components_dir = direct_compilation.components_dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            compile_workflow_direct_only(
                compilation_input,
                compile_source_checksum,
                direct_compilation.components_dir,
                extra_component_dirs,
                direct_compilation.stdlib,
            )
        })
        .await
        .map_err(|e| ServiceError::CompilationError(format!("Compilation task panicked: {}", e)))?
        .map_err(|error| {
            direct_compile_failure(error, |staged| {
                // No bundle to compare against: keep the recorded refusal.
                components_dir.as_deref().is_none_or(|dir| {
                    runtara_workflows::direct_wasm::staged_dependency_is_stale(dir, staged)
                })
            })
        })?;
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

        // An artifact pinning a trusted built-in version this server does not
        // run is never ready: fail rather than record a success.
        if let Err(error) = reject_uninstalled_trusted_pins(&result.trusted_pins) {
            if let Some(r) = &progress_reporter {
                r.clear().await;
            }
            return Err(error);
        }
        let floor = direct_artifact_metadata_for_image(&result)
            .await
            .and_then(|metadata| metadata.memory_floor_bytes);
        let memory_floor_bytes = floor.and_then(|floor| i64::try_from(floor).ok());
        if let Err(error) = check_memory_floor(memory_mb, floor) {
            if let Some(r) = &progress_reporter {
                r.clear().await;
            }
            return Err(error);
        }

        // 7. Lock the source through registration. Image names are immutable
        // per artifact identity, but the source lock still prevents an old
        // compile from attaching its artifact to a newer definition.
        let client = self.runtime_client.as_ref().ok_or_else(|| {
            ServiceError::RegistrationError(
                "Runtime client not configured. Compilation requires runtara-environment connection."
                    .to_string(),
            )
        })?;
        let step_start = std::time::Instant::now();
        debug!("compile: step 7 - locking current compilation source");
        let Some(mut completion) = self
            .repository
            .lock_current_compilation_source(
                tenant_id,
                workflow_id,
                version,
                &definition,
                track_events,
            )
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to lock compilation source: {e}"))
            })?
        else {
            if let Some(r) = &progress_reporter {
                r.clear().await;
            }
            return Err(ServiceError::Superseded(format!(
                "Workflow '{workflow_id}' version {version} changed while compilation was running"
            )));
        };
        completion
            .record_success(&CompilationSuccessRecord {
                tenant_id,
                workflow_id,
                version,
                build_dir: &result.build_dir,
                binary_size: result.binary_size as i32,
                package_size: result.package_size as i32,
                binary_checksum: &result.binary_checksum,
                definition: &definition,
                source_checksum: &source_checksum,
                compiler_mode: result.compiler_mode.as_str(),
                track_events,
                trusted_pins: &result.trusted_pins,
                memory_floor_bytes,
            })
            .await
            .map_err(|e| {
                warn!("Failed to record compilation success: {e}");
                ServiceError::DatabaseError(format!("Failed to record compilation: {e}"))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 7 completed - compilation source locked and success recorded"
        );

        // 8. Register with runtara-environment (REQUIRED)
        // Compilation without registration is useless - the workflow can't be executed
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
        let registration = WorkflowImageRegistration {
            tenant_id,
            workflow_id,
            version: version_u32,
            source_checksum: &source_checksum,
            track_events,
            memory_limit_bytes,
        };
        let image_name = workflow_image_name(
            registration.workflow_id,
            registration.version,
            registration.source_checksum,
            result.compiler_mode,
            registration.track_events,
            &result.binary_checksum,
        );
        let image_id = match client
            .find_image_by_name_summary(tenant_id, &image_name)
            .await
        {
            // Reuse needs the row AND the file. The row is immutable and the
            // checksums prove provenance, but a matching row whose artifact has
            // gone from disk is precisely what a forced recompile is here to
            // repair — and reusing it re-registers nothing, so the file stays
            // missing and the caller retries into the same failure until its
            // budget runs out. A row without the agents the image records is
            // the same: a launch rebuilds such an image to check them, and
            // only re-registering stores the record.
            Ok(Some(existing_image))
                if image_matches_compiled_artifact(
                    &existing_image,
                    &source_checksum,
                    result.compiler_mode,
                    track_events,
                    &result.binary_checksum,
                ) && image_memory_limit_bytes(&existing_image) == Some(memory_limit_bytes)
                    && image_agent_ids(&existing_image).is_some()
                    && self
                        .artifact_present_for_reuse(client, &existing_image.image_id)
                        .await =>
            {
                info!(
                    image_id = %existing_image.image_id,
                    "Reusing already registered immutable workflow artifact"
                );
                existing_image.image_id
            }
            Ok(Some(existing_image))
                if image_matches_compiled_artifact(
                    &existing_image,
                    &source_checksum,
                    result.compiler_mode,
                    track_events,
                    &result.binary_checksum,
                ) =>
            {
                warn!(
                    image_id = %existing_image.image_id,
                    image_name = %image_name,
                    "Registered artifact is missing from disk or records no agents; re-registering the compiled binary"
                );
                self.register_image(client, &result, registration, &image_name)
                    .await?
            }
            Ok(Some(existing_image)) => {
                warn!(
                    image_id = %existing_image.image_id,
                    image_name = %image_name,
                    "Image name is occupied by an artifact with mismatched provenance; registering the compiled binary"
                );
                self.register_image(client, &result, registration, &image_name)
                    .await?
            }
            Ok(None) => {
                self.register_image(client, &result, registration, &image_name)
                    .await?
            }
            Err(error) => {
                warn!(
                    image_name = %image_name,
                    error = %error,
                    "Failed to look up a compiled image by name; registering it"
                );
                self.register_image(client, &result, registration, &image_name)
                    .await?
            }
        };
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            image_id = %image_id,
            "compile: step 8 completed - image registered with runtara-environment"
        );

        // 8b. Record registered image ID (required for execution)
        let step_start = std::time::Instant::now();
        debug!("compile: step 8b - recording registered image ID in database");
        let registered_image = RegisteredImageRecord {
            tenant_id,
            workflow_id,
            version,
            image_id: &image_id,
            definition: &definition,
            source_checksum: &source_checksum,
            compiler_mode: Some(result.compiler_mode.as_str()),
            track_events,
            trusted_pins: &result.trusted_pins,
        };
        completion
            .record_registered_image_id(&registered_image)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to record registered image ID: {}", e))
            })?;
        debug!(
            duration_ms = step_start.elapsed().as_millis(),
            "compile: step 8b completed - image ID recorded in database"
        );
        completion.commit().await.map_err(|e| {
            ServiceError::DatabaseError(format!("Failed to commit registered image ID: {e}"))
        })?;

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

    /// Rebuild every registered workflow image of the tenant whose artifact
    /// is not in the store, from its version. The artifact store is not
    /// backed up: after a restore this puts each artifact back (compilation
    /// is deterministic, so under the same digest), and the launches that
    /// waited for one proceed. Answers how many were rebuilt and how many
    /// failed.
    pub async fn restage_missing_artifacts(&self, tenant_id: &str) -> (usize, usize) {
        let Some(client) = self.runtime_client.clone() else {
            return (0, 0);
        };
        let images = match self.repository.registered_images(tenant_id).await {
            Ok(images) => images,
            Err(error) => {
                warn!(%tenant_id, %error, "could not list registered workflow images");
                return (0, 1);
            }
        };
        let (mut rebuilt, mut failed) = (0, 0);
        for (workflow_id, version, image_id) in images {
            if self.artifact_present_for_reuse(&client, &image_id).await {
                continue;
            }
            match self
                .compile_workflow(tenant_id, &workflow_id, version, true)
                .await
            {
                Ok(_) => {
                    rebuilt += 1;
                    info!(%tenant_id, %workflow_id, version, "rebuilt a missing workflow artifact");
                }
                Err(error) => {
                    failed += 1;
                    warn!(
                        %tenant_id, %workflow_id, version, %error,
                        "could not rebuild a missing workflow artifact; compile it again"
                    );
                }
            }
        }
        (rebuilt, failed)
    }

    /// Republish the tenant's staged workflow-agents that another lowering
    /// built (see [`crate::workflow_agents::stale_agents`]), from the version
    /// each was published from, or the workflow's current version when it
    /// recorded none. A parent compiled afterwards composes the new
    /// artifact. Answers how many were republished and how many failed.
    pub async fn restage_stale_workflow_agents(&self, tenant_id: &str) -> (usize, usize) {
        let lowering = crate::config::workflow_lowering_tag();
        let (mut restaged, mut failed) = (0, 0);
        for agent in crate::workflow_agents::stale_agents(tenant_id, &lowering) {
            let target = match agent.published.clone() {
                Some((workflow_id, version)) => Ok(Some((workflow_id, version as i32))),
                None => self.current_version_of_slug(tenant_id, &agent.slug).await,
            };
            let outcome = match target {
                Ok(Some((workflow_id, version))) => self
                    .publish_workflow_agent(tenant_id, &workflow_id, version, agent.slug.clone())
                    .await
                    .map(|_| ()),
                Ok(None) => Err(ServiceError::NotFound(format!(
                    "no live workflow has the slug `{}`",
                    agent.slug
                ))),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) => {
                    restaged += 1;
                    info!(%tenant_id, slug = %agent.slug, "restaged workflow-agent for this lowering");
                }
                Err(error) => {
                    failed += 1;
                    warn!(
                        %tenant_id, slug = %agent.slug, %error,
                        "could not restage workflow-agent; republish it"
                    );
                }
            }
        }
        (restaged, failed)
    }

    async fn current_version_of_slug(
        &self,
        tenant_id: &str,
        slug: &str,
    ) -> Result<Option<(String, i32)>, ServiceError> {
        let database = |e: sqlx::Error| ServiceError::DatabaseError(e.to_string());
        let Some(workflow_id) = self
            .repository
            .find_by_slug(tenant_id, slug)
            .await
            .map_err(database)?
        else {
            return Ok(None);
        };
        Ok(self
            .repository
            .get_current_or_latest_version(tenant_id, &workflow_id)
            .await
            .map_err(database)?
            .map(|version| (workflow_id, version)))
    }

    /// Publish a workflow version AS an agent: compile it with the
    /// `PublishedAgent` ABI (exports `runtara:agent-<slug>/capabilities`),
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

        let agent_catalog = self
            .agent_catalog
            .as_ref()
            .map(|base| crate::workflow_agents::catalog_with_workflow_agents(base, tenant_id));
        // This process's own proof of whether the agent parks: a compiler
        // elsewhere reports one too, which is not taken on its word.
        let safety = require_publishable_workflow_agent(
            &execution_graph,
            &child_workflows,
            agent_catalog.as_deref(),
        )?;

        let name = execution_graph.name.clone().unwrap_or_else(|| slug.clone());
        let description = execution_graph.description.clone().unwrap_or_default();
        let info = runtara_dsl::agent_meta::workflow_agent_info(
            &slug,
            &name,
            &description,
            &execution_graph.input_schema,
            &execution_graph.output_schema,
        );

        // 2. Compile with the PublishedAgent ABI + compose. Same catalog
        //    overlay as a normal compile so a workflow-agent may itself invoke
        //    previously-published workflow-agents.
        let Some(components_dir) = self.direct_compilation.components_dir.clone() else {
            return Err(ServiceError::CompilationError(
                "direct WASM compilation requires a configured component directory".to_string(),
            ));
        };
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
        let publishing_tenant = tenant_id.to_string();
        let stdlib = self.direct_compilation.stdlib;
        let result = tokio::task::spawn_blocking(move || {
            // Where this process compiles: elsewhere, with an executor set.
            if let Some(executor) = crate::compile_executor::executor() {
                return executor
                    .compile_published_agent(
                        &publishing_tenant,
                        direct_input,
                        &components_dir,
                        &extra_dirs,
                        stdlib,
                    )
                    .map_err(|error| error.to_string());
            }
            stdlib.scope(|| {
                let mut result = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
                    direct_input,
                    runtara_workflows::direct_wasm::WorkflowRole::PublishedAgent,
                    // Omit the runtime where the graph allows; the compiler keeps
                    // it for anything that parks or runs under the caller's instance.
                    true,
                )
                .map_err(|error| error.to_string())?;
                runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
                    &mut result,
                    &components_dir,
                    &extra_dirs,
                )
                .map_err(|error| error.to_string())?;
                Ok::<_, String>(runtara_workflows::compile_request::PublishedAgent {
                    wasm_path: result.wasm_path,
                    wasm_size: result.wasm_size,
                    artifact_metadata: result.artifact_metadata,
                })
            })
        })
        .await
        .map_err(|e| ServiceError::CompilationError(format!("Publish task panicked: {e}")))?
        .map_err(|e| ServiceError::CompilationError(format!("Agent publish failed: {e}")))?;
        drop(permit);

        // A caller runs a workflow-agent that may park as a child run of its
        // own: it needs the version it runs, and that version's root image.
        let parks = safety.may_suspend_or_sleep;
        if result
            .artifact_metadata
            .workflow_agent_safety
            .may_suspend_or_sleep
            != parks
        {
            return Err(ServiceError::CompilationError(
                "Agent publish failed: the compiled agent does not match whether its workflow parks"
                    .to_string(),
            ));
        }
        let info = runtara_dsl::agent_meta::with_workflow_run(
            info,
            runtara_dsl::agent_meta::WorkflowRunTarget {
                workflow_id: workflow_id.to_string(),
                version: version as u32,
                parks,
            },
        );
        if parks {
            self.compile_workflow(tenant_id, workflow_id, version, false)
                .await
                .map_err(|error| {
                    ServiceError::CompilationError(format!(
                        "the agent parks, so it runs as a child run of its own, \
                         and compiling its run failed: {error}"
                    ))
                })?;
        }

        // 3. Stage the composed artifact + synthesized meta.
        let (wasm_path, meta_path) =
            crate::workflow_agents::stage(tenant_id, &slug, &result.wasm_path, &info)
                .await
                .map_err(|e| match e.kind() {
                    // A reserved slug or metadata only built-ins may claim.
                    std::io::ErrorKind::InvalidInput => {
                        ServiceError::WorkflowAuthoringError(e.to_string())
                    }
                    _ => ServiceError::CompilationError(format!("Staging failed: {e}")),
                })?;

        info!(
            %tenant_id, %workflow_id, version, %slug,
            wasm = %wasm_path.display(),
            "published workflow as agent"
        );

        // A parent refused because a workflow-agent it composes pinned a
        // trusted version this server no longer runs recorded that failure
        // as terminal. The republish may have cleared it, so let each such
        // failure in this tenant retry once on its next launch; one still
        // composing a stale workflow-agent fails and is recorded again.
        match self
            .repository
            .release_stale_trusted_dependency_failures(tenant_id)
            .await
        {
            Ok(released) if released > 0 => info!(
                %tenant_id, %slug, released,
                "released compile failures on stale trusted dependencies for retry"
            ),
            Ok(_) => {}
            Err(error) => warn!(
                %tenant_id, %slug, %error,
                "could not release compile failures on stale trusted dependencies; \
                 a forced recompile retries them"
            ),
        }
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
        image_name: &str,
    ) -> Result<String, ServiceError> {
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
        let artifact_path = binary_path.clone();
        tokio::task::spawn_blocking(move || {
            runtara_component_host::lifecycle::require_workflow_entry_file(&artifact_path)
        })
        .await
        .map_err(|error| {
            ServiceError::RegistrationError(format!(
                "workflow ABI inspection task panicked: {error}"
            ))
        })?
        .map_err(|error| {
            ServiceError::RegistrationError(format!(
                "compiled workflow is not launchable: {error:#}"
            ))
        })?;
        let options =
            RegisterImageStreamOptions::new(registration.tenant_id, image_name, binary_size)
                .with_description(format!(
                    "Workflow {} version {}",
                    registration.workflow_id, registration.version
                ))
                .with_sha256(&compilation_result.binary_checksum)
                .with_metadata({
                    let mut metadata = workflow_image_metadata(
                        compilation_result,
                        registration.workflow_id,
                        registration.version,
                        registration.source_checksum,
                        registration.track_events,
                        direct_artifact.as_ref(),
                    );
                    metadata["workflow"]["memoryLimitBytes"] =
                        registration.memory_limit_bytes.into();
                    metadata
                });

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
    /// The workflow cannot compile as authored - no steps, an unreachable
    /// entry point, and so on. The system is behaving correctly; the graph is
    /// the problem, so this is reported to the author rather than alerted on.
    WorkflowAuthoringError(String),
    RegistrationError(String),
    /// The compile depends on trusted built-in versions this server does not
    /// run, so it could never become ready: either the fresh artifact pins
    /// them (`trusted_pins` are all its pins), or a published workflow-agent
    /// it composes was built against them (`trusted_pins` are that child's
    /// stale pins). The failure is recorded with `trusted_pins` and retries
    /// once they are all installed or a workflow-agent republish releases it.
    TrustedDependencyUnavailable {
        message: String,
        trusted_pins: Vec<String>,
    },
    /// The definition or instrumentation mode changed after this attempt
    /// captured its input. A newer compilation owns terminal state instead.
    Superseded(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ServiceError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ServiceError::CompilationError(msg) => write!(f, "Compilation error: {}", msg),
            ServiceError::WorkflowAuthoringError(msg) => write!(f, "{}", msg),
            ServiceError::RegistrationError(msg) => write!(f, "Registration error: {}", msg),
            ServiceError::TrustedDependencyUnavailable { message, .. } => {
                write!(f, "Compilation error: {}", message)
            }
            ServiceError::Superseded(msg) => write!(f, "Compilation superseded: {}", msg),
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

    fn s3_pin(wasm: char, meta: char) -> String {
        runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &wasm.to_string().repeat(64),
            &meta.to_string().repeat(64),
        )
    }

    #[test]
    fn trusted_dependency_diagnostic_names_each_agent_once_and_both_causes() {
        let message = trusted_dependency_unavailable_message(&[
            s3_pin('a', 'b'),
            s3_pin('c', 'd'),
            "not-a-pin".to_owned(),
        ]);
        assert_eq!(message.matches("`s3-storage`").count(), 1, "{message}");
        assert!(message.contains("`not-a-pin`"), "{message}");
        assert!(message.contains("does not run"), "{message}");
        assert!(message.contains("restart the server"), "{message}");
        assert!(
            message.contains("RUNTARA_DIRECT_WASM_COMPONENTS_DIR")
                && message.contains("RUNTARA_AGENT_COMPONENTS_DIR"),
            "a restart alone does not fix differing compile and runtime bundles: {message}"
        );
        let error = ServiceError::TrustedDependencyUnavailable {
            message,
            trusted_pins: vec![s3_pin('a', 'b')],
        };
        let recorded = error.to_string();
        assert!(recorded.starts_with("Compilation error: "), "{recorded}");
        assert!(
            !crate::api::repositories::workflows::is_workflow_authoring_error(&recorded),
            "an operator-side bundle mismatch is not the author's to fix: {recorded}"
        );
    }

    #[test]
    fn a_fresh_artifact_pinning_an_uninstalled_version_is_refused_with_all_its_pins() {
        let _installed = crate::api::repositories::workflows::INSTALLED_PINS_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let installed = s3_pin('a', 'b');
        let other = runtara_dsl::agent_meta::trusted_artifact_import(
            "azure-blob-storage",
            &"e".repeat(64),
            &"f".repeat(64),
        );
        crate::api::repositories::workflows::set_installed_trusted_pins([installed.clone()]);
        assert!(reject_uninstalled_trusted_pins(&[]).is_ok());
        assert!(reject_uninstalled_trusted_pins(std::slice::from_ref(&installed)).is_ok());
        let pins = vec![installed.clone(), other];
        match reject_uninstalled_trusted_pins(&pins) {
            Err(ServiceError::TrustedDependencyUnavailable {
                message,
                trusted_pins,
            }) => {
                assert_eq!(trusted_pins, pins, "the failure records every pin");
                assert!(message.contains("`azure-blob-storage`"), "{message}");
                assert!(!message.contains("`s3-storage`"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        crate::api::repositories::workflows::set_installed_trusted_pins([]);
    }

    #[test]
    fn a_stale_published_workflow_agent_fails_with_its_pins() {
        let stale = s3_pin('a', 'b');
        let staged = std::path::PathBuf::from("/staging/runtara_agent_wrapper.wasm");
        let refused = || {
            std::io::Error::other(
                runtara_workflows::direct_wasm::DirectCompileError::StaleTrustedDependency {
                    dependency: "wrapper".into(),
                    agent: "s3-storage".into(),
                    pins: vec![stale.clone()],
                    wasm_path: staged.clone(),
                },
            )
        };
        // Republished while the compile ran: nothing to record, retry.
        let mut checked = None;
        let republished = direct_compile_failure(refused(), |path| {
            checked = Some(path.to_owned());
            false
        });
        assert_eq!(
            checked.as_ref(),
            Some(&staged),
            "the staged artifact is re-read"
        );
        match &republished {
            ServiceError::Superseded(message) => {
                assert!(message.contains("workflow-agent `wrapper`"), "{message}");
                assert!(message.contains("republished"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        let error = direct_compile_failure(refused(), |_| true);
        match &error {
            ServiceError::TrustedDependencyUnavailable {
                message,
                trusted_pins,
            } => {
                assert_eq!(trusted_pins, &vec![stale]);
                assert!(message.starts_with("Compilation failed: "), "{message}");
                assert!(message.contains("workflow-agent `wrapper`"), "{message}");
                assert!(message.contains("republish"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !crate::api::repositories::workflows::is_workflow_authoring_error(&error.to_string())
        );
        assert!(matches!(
            direct_compile_failure(std::io::Error::other("emission failed"), |_| {
                unreachable!("only a stale dependency is re-read")
            }),
            ServiceError::CompilationError(message) if message == "Compilation failed: emission failed"
        ));
    }

    #[test]
    fn startup_detects_a_compile_bundle_with_other_trusted_versions() {
        let bundle = tempfile::tempdir().unwrap();
        let (wasm, meta) = (b"s3 bytes".to_vec(), b"{}".to_vec());
        std::fs::write(bundle.path().join("runtara_agent_s3_storage.wasm"), &wasm).unwrap();
        std::fs::write(
            bundle.path().join("runtara_agent_s3_storage.meta.json"),
            &meta,
        )
        .unwrap();
        let hex = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        let same = runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &hex(&wasm),
            &hex(&meta),
        );
        assert!(trusted_builtins_differing_from(bundle.path(), [same.as_str()]).is_empty());
        let upgraded = s3_pin('a', 'b');
        let missing = runtara_dsl::agent_meta::trusted_artifact_import(
            "azure-blob-storage",
            &"e".repeat(64),
            &"f".repeat(64),
        );
        assert_eq!(
            trusted_builtins_differing_from(
                bundle.path(),
                [
                    same.as_str(),
                    upgraded.as_str(),
                    missing.as_str(),
                    "not-a-pin"
                ]
            ),
            vec!["azure-blob-storage".to_owned(), "s3-storage".to_owned()]
        );
    }

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
    fn workflow_agent_publish_preflight_accepts_a_delay_as_a_parking_agent() {
        let graph = parse_execution_graph(&serde_json::json!({
            "steps": {
                "delay": {
                    "stepType": "Delay",
                    "id": "delay",
                    "durationMs": { "valueType": "immediate", "value": 1 }
                }
            },
            "entryPoint": "delay",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        let safety = require_publishable_workflow_agent(&graph, &[], None)
            .expect("a sleeping workflow-agent parks rather than holding a runner");
        assert!(
            safety.may_suspend_or_sleep,
            "the publish runs it as a child run of its own on this process's proof"
        );
    }

    #[test]
    fn workflow_agent_publish_preflight_accepts_a_control_call() {
        let graph = parse_execution_graph(&serde_json::json!({
            "steps": {
                "get": {"stepType": "Agent", "id": "get", "agentId": "control",
                    "capabilityId": "get", "maxRetries": 0}
            },
            "entryPoint": "get",
            "executionPlan": []
        }))
        .expect("graph parses");
        require_publishable_workflow_agent(&graph, &[], None)
            .expect("a workflow-agent calls control under its caller's instance");
    }

    #[test]
    fn workflow_agent_publish_preflight_refuses_an_unloaded_child_with_a_stable_path() {
        let graph = parse_execution_graph(&serde_json::json!({
            "steps": {
                "child": {
                    "stepType": "EmbedWorkflow",
                    "id": "child",
                    "childWorkflowId": "unloaded",
                    "childVersion": 1,
                    "maxRetries": 0
                }
            },
            "entryPoint": "child",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        // A child closure the compiler cannot see can hide anything, so this
        // still refuses, and names the exact step it refused on.
        let error = require_publishable_workflow_agent(&graph, &[], None)
            .expect_err("an unseen child closure cannot be proven sound");
        // The author fixes it, so the publish handler answers 422, not 500.
        assert!(
            matches!(error, ServiceError::WorkflowAuthoringError(_)),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("root/steps/child (EmbedWorkflow/missing-child-closure)"),
            "{error}"
        );
    }

    #[test]
    fn workflow_agent_publish_preflight_accepts_agent_waits_either_way() {
        let mut graph = parse_execution_graph(&serde_json::json!({
            "durable": false, "entryPoint": "call", "steps": {
                "call": {"id":"call", "stepType":"Agent", "agentId":"http",
                    "capabilityId":"http-request", "maxRetries":3, "retryDelay":1000},
                "finish":{"id":"finish", "stepType":"Finish"}
            }, "executionPlan":[{"fromStep":"call", "toStep":"finish"}]
        }))
        .expect("graph parses");
        require_publishable_workflow_agent(&graph, &[], None)
            .expect("guest-local Agent waits publish");
        graph.durable = Some(true);
        require_publishable_workflow_agent(&graph, &[], None)
            .expect("durable Agent backoff parks rather than holding a runner");
    }

    #[test]
    fn workflow_agent_publish_preflight_accepts_a_pure_finish() {
        let graph = parse_execution_graph(&serde_json::json!({
            "steps": { "finish": { "stepType": "Finish", "id": "finish" } },
            "entryPoint": "finish",
            "executionPlan": [],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        }))
        .expect("graph parses");

        require_publishable_workflow_agent(&graph, &[], None)
            .expect("a pure finish is safe as a workflow-agent");
    }

    #[test]
    fn image_cache_hit_requires_matching_compiler_and_tracking_modes() {
        let metadata = serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
                "compilerMode": "direct-wasm",
                "loweringMode": crate::config::workflow_lowering_tag(),
                "trackEvents": true
            }
        });
        let image = image_summary_with_metadata(metadata);

        assert!(image_cache_hits(
            &image,
            "source-sha256",
            WorkflowCompilerMode::DirectWasm,
            true,
        ));
        assert!(!image_cache_hits(
            &image,
            "other-source",
            WorkflowCompilerMode::DirectWasm,
            true,
        ));
        assert!(
            !image_cache_hits(
                &image,
                "source-sha256",
                WorkflowCompilerMode::DirectWasm,
                false,
            ),
            "step instrumentation changes the artifact and must miss the cache"
        );

        let missing_mode = image_summary_with_metadata(serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION
            }
        }));
        assert!(!image_cache_hits(
            &missing_mode,
            "source-sha256",
            WorkflowCompilerMode::DirectWasm,
            true,
        ));
    }

    #[test]
    fn workflow_image_name_is_specific_to_the_compilation_identity() {
        let source = "source-sha256";
        let current = workflow_image_name(
            "workflow-a",
            7,
            source,
            WorkflowCompilerMode::DirectWasm,
            true,
            "binary-sha256-a",
        );

        assert!(current.starts_with("workflow-a:7@"));
        assert_eq!(
            current,
            workflow_image_name(
                "workflow-a",
                7,
                source,
                WorkflowCompilerMode::DirectWasm,
                true,
                "binary-sha256-a",
            ),
            "the same compiled bytes must recover the same orphaned image"
        );
        assert_ne!(
            current,
            workflow_image_name(
                "workflow-a",
                7,
                "other-source-sha256",
                WorkflowCompilerMode::DirectWasm,
                true,
                "binary-sha256-a",
            ),
            "a changed definition must receive a distinct Environment image"
        );
        assert_ne!(
            current,
            workflow_image_name(
                "workflow-a",
                7,
                source,
                WorkflowCompilerMode::DirectWasm,
                false,
                "binary-sha256-a",
            ),
            "opposite step instrumentation must never overwrite this image"
        );
        assert_ne!(
            current,
            workflow_image_name(
                "workflow-a",
                7,
                source,
                WorkflowCompilerMode::DirectWasm,
                true,
                "binary-sha256-b",
            ),
            "different composed bytes must never share an Environment image"
        );
    }

    /// An image built with different lowering is not interchangeable with one
    /// built now, even though its definition never changed.
    ///
    /// This is what makes turning store-freeing sleep on by default actually
    /// reach existing workflows: without lowering in the key, every already
    /// compiled workflow keeps serving its old blocking-sleep artifact because
    /// checksum, template major and compiler mode all still match.
    #[test]
    fn image_cache_misses_when_lowering_mode_differs() {
        let stale = image_summary_with_metadata(serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
                "compilerMode": "direct-wasm",
                "loweringMode": "store_freeing_sleep=false,omit_runtime=false",
                "trackEvents": true
            }
        }));
        assert!(
            !image_cache_hits(
                &stale,
                "source-sha256",
                WorkflowCompilerMode::DirectWasm,
                true,
            ) || crate::config::workflow_lowering_tag()
                == "store_freeing_sleep=false,omit_runtime=false",
            "an image built with other lowering must not be reused"
        );

        let current = image_summary_with_metadata(serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
                "compilerMode": "direct-wasm",
                "loweringMode": crate::config::workflow_lowering_tag(),
                "trackEvents": true
            }
        }));
        assert!(
            image_cache_hits(
                &current,
                "source-sha256",
                WorkflowCompilerMode::DirectWasm,
                true,
            ),
            "an image built with the current lowering must be reused"
        );
    }

    /// Images predating lowering provenance have no tag, so they rebuild once
    /// rather than being served forever from a stale artifact.
    #[test]
    fn image_cache_misses_when_lowering_mode_is_absent() {
        let legacy = image_summary_with_metadata(serde_json::json!({
            "workflow": {
                "sourceChecksum": "source-sha256",
                "templateMajor": runtara_workflows::TEMPLATE_MAJOR_VERSION,
                "compilerMode": "direct-wasm"
            }
        }));
        assert!(!image_cache_hits(
            &legacy,
            "source-sha256",
            WorkflowCompilerMode::DirectWasm,
            true,
        ));
    }

    #[test]
    fn memory_below_the_workflow_floor_is_refused() {
        let mib = 1024 * 1024;
        let refused = check_memory_floor(8, Some(9 * mib)).unwrap_err();
        assert!(
            matches!(&refused, ServiceError::WorkflowAuthoringError(message)
                if message.contains("Memory of 8 MiB is below this workflow's memory floor of 9 MiB")),
            "{refused:?}"
        );
        assert!(check_memory_floor(8, Some(8 * mib)).is_ok());
        assert!(check_memory_floor(9, Some(9 * mib)).is_ok());
        assert!(check_memory_floor(8, None).is_ok());
    }

    #[test]
    fn workflow_image_metadata_records_lowering_mode() {
        let result = NativeCompilationResult {
            binary_path: "/tmp/workflow.wasm".into(),
            binary_size: 123,
            binary_checksum: "abc".to_string(),
            build_dir: "/tmp/build".into(),
            package_size: 99,
            child_dependencies: vec![],
            default_variables: serde_json::json!({}),
            compiler_mode: WorkflowCompilerMode::DirectWasm,
            trusted_pins: vec![],
        };
        let metadata =
            workflow_image_metadata(&result, "workflow-a", 7, "source-sha256", true, None);
        assert_eq!(
            metadata["workflow"]["loweringMode"],
            serde_json::json!(crate::config::workflow_lowering_tag()),
            "provenance must record the lowering the artifact was built with"
        );
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
            trusted_pins: vec![],
        };

        let metadata =
            workflow_image_metadata(&result, "workflow-a", 7, "source-sha256", true, None);

        assert_eq!(metadata["variables"], serde_json::json!({ "limit": 5 }));
        assert_eq!(metadata["variablesFormat"], "values");
        assert_eq!(metadata["workflow"]["workflowId"], "workflow-a");
        assert_eq!(metadata["workflow"]["version"], 7);
        assert_eq!(metadata["workflow"]["sourceChecksum"], "source-sha256");
        assert_eq!(
            metadata["workflow"]["templateMajor"],
            runtara_workflows::TEMPLATE_MAJOR_VERSION
        );
        assert_eq!(metadata["workflow"]["compilerMode"], "direct-wasm");
        assert_eq!(metadata["workflow"]["trackEvents"], true);
        assert_eq!(metadata["workflow"]["binaryChecksum"], "abc");
        assert_eq!(metadata["workflow"]["directWasm"]["enabled"], true);
        assert_eq!(metadata["workflow"]["directWasm"]["outcome"], "success");
        assert_eq!(metadata["workflow"]["directWasm"]["reason"], "none");
    }

    #[test]
    fn workflow_image_metadata_records_direct_artifact_provenance() {
        let result = native_result_with_mode(WorkflowCompilerMode::DirectWasm, "/tmp/build".into());
        let artifact = direct_artifact_metadata_fixture();

        let metadata = workflow_image_metadata(
            &result,
            "workflow-a",
            7,
            "source-sha256",
            true,
            Some(&artifact),
        );

        let direct_artifact = &metadata["workflow"]["directArtifact"];
        assert_eq!(
            direct_artifact["schemaVersion"],
            serde_json::json!(
                runtara_workflows::direct_wasm::DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION
            )
        );
        assert_eq!(
            direct_artifact["directAbiVersion"],
            runtara_workflows::direct_wasm::compile::DIRECT_WORKFLOW_INVOKE_ABI_VERSION
        );
        assert_eq!(direct_artifact["entryAbi"], "invoke");
        assert_eq!(metadata["workflow"]["directWasm"]["entryAbi"], "invoke");
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
            trusted_pins: vec![],
        }
    }

    fn image_summary_with_metadata(metadata: serde_json::Value) -> ImageSummary {
        ImageSummary {
            image_id: "image-a".to_string(),
            tenant_id: "tenant-a".to_string(),
            name: "workflow-a:7".to_string(),
            description: None,
            created_at: chrono::Utc::now(),
            metadata: Some(metadata),
        }
    }

    #[test]
    fn an_image_records_the_agents_it_was_composed_with() {
        let image = image_summary_with_metadata(serde_json::json!({
            "workflow": {
                "directArtifact": {
                    "agentComponents": [
                        {"kind": "agent", "agentId": "csv", "package": "runtara:agent-csv"},
                        {"kind": "agent", "agentId": "http", "package": "runtara:agent-http"}
                    ]
                }
            }
        }));
        assert_eq!(
            image_agent_ids(&image),
            Some(vec!["csv".to_string(), "http".to_string()])
        );

        // No record to check: an image registered before it existed, or
        // without its artifact metadata, or one whose record is malformed.
        let without_metadata = ImageSummary {
            metadata: None,
            ..image_summary_with_metadata(serde_json::json!({}))
        };
        assert_eq!(image_agent_ids(&without_metadata), None);
        for metadata in [
            serde_json::json!({"workflow": {"executionTimeoutSeconds": 30}}),
            serde_json::json!({
                "workflow": {"directArtifact": {"agentComponents": [{"kind": "agent"}]}}
            }),
        ] {
            assert_eq!(
                image_agent_ids(&image_summary_with_metadata(metadata)),
                None
            );
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
            direct_abi_version:
                runtara_workflows::direct_wasm::compile::DIRECT_WORKFLOW_INVOKE_ABI_VERSION,
            entry_abi: "invoke".to_string(),
            workflow_agent_safety:
                runtara_workflows::direct_wasm::WorkflowAgentSafetyReport::default(),
            manifest_version: runtara_workflows::direct_wasm::DIRECT_WORKFLOW_MANIFEST_VERSION,
            template_major_version: runtara_workflows::TEMPLATE_MAJOR_VERSION.to_string(),
            manifest_checksum: "manifest-sha256".to_string(),
            support_report_checksum: "support-sha256".to_string(),
            workflow_logic_wasm: wasm("workflow-logic.wasm", "logic-sha256"),
            composed_wasm: Some(wasm("workflow.wasm", "composed-sha256")),
            memory_floor_bytes: None,
            isolation: None,
            isolation_selection: None,
            shared_components: vec![DirectComponentDependencyMetadata {
                kind: "shared".to_string(),
                agent_id: None,
                package: "runtara:workflow-stdlib".to_string(),
                package_with_version: "runtara:workflow-stdlib@1.0.0".to_string(),
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

    fn http_only_snapshot() -> crate::entitlements::EntitlementSnapshot {
        crate::entitlements::EntitlementSnapshot::parse_entitlements(
            "tenant-test",
            None,
            Some(r#"{"agents":["http"]}"#),
            None,
            &crate::entitlements::parse_agents(&["http", "csv"]),
        )
        .expect("snapshot parses")
    }

    fn graph_calling(agent: &str) -> serde_json::Value {
        serde_json::json!({
            "entryPoint": "call",
            "steps": {
                "call": {"stepType": "Agent", "id": "call", "agentId": agent, "capabilityId": "run"}
            }
        })
    }

    fn graph_embedding_child() -> serde_json::Value {
        serde_json::json!({
            "entryPoint": "embed",
            "steps": {
                "embed": {"stepType": "EmbedWorkflow", "id": "embed", "childWorkflowId": "child-wf", "childVersion": "latest"}
            }
        })
    }

    fn child_with_graph(
        execution_graph: serde_json::Value,
    ) -> crate::compiler::child_workflows::ChildWorkflowInfo {
        crate::compiler::child_workflows::ChildWorkflowInfo {
            step_id: "embed".to_string(),
            workflow_ref: runtara_workflows::dependency_analysis::WorkflowReference {
                workflow_id: "child-wf".to_string(),
                version: 1,
            },
            execution_graph,
            version_requested: "latest".to_string(),
        }
    }

    fn expect_denied_agent(result: Result<(), CompileAgentGateError>, agent: &str) {
        match result {
            Err(CompileAgentGateError::Denied(denial)) => {
                assert_eq!(
                    denial.code(),
                    crate::entitlement_error::codes::AGENT_NOT_ENABLED
                );
                assert_eq!(denial.json_body()["agent"], agent);
            }
            other => panic!("expected AGENT_NOT_ENABLED for '{agent}', got {other:?}"),
        }
    }

    fn expect_unchecked(result: Result<(), CompileAgentGateError>, needle: &str) {
        match result {
            Err(CompileAgentGateError::Unchecked(message)) => {
                assert!(message.contains(needle), "{message}");
            }
            other => panic!("expected an unchecked refusal mentioning '{needle}', got {other:?}"),
        }
    }

    fn expect_invalid(result: Result<(), CompileAgentGateError>, needle: &str) {
        match result {
            Err(CompileAgentGateError::Invalid(message)) => {
                assert!(message.contains(needle), "{message}");
            }
            other => {
                panic!("expected an invalid-closure refusal mentioning '{needle}', got {other:?}")
            }
        }
    }

    #[test]
    fn compile_gate_denies_an_agent_outside_the_allowlist_in_the_root_or_a_child() {
        let snapshot = http_only_snapshot();
        let none_published = std::collections::HashSet::new();

        expect_denied_agent(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_calling("csv"),
                Ok(Vec::new()),
                &none_published,
            ),
            "csv",
        );
        expect_denied_agent(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_embedding_child(),
                Ok(vec![child_with_graph(graph_calling("csv"))]),
                &none_published,
            ),
            "csv",
        );

        assert!(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_embedding_child(),
                Ok(vec![child_with_graph(graph_calling("http"))]),
                &none_published,
            )
            .is_ok()
        );
        // The tenant's published workflow-agents are exempt, as on save.
        let published = std::collections::HashSet::from(["shout-echo".to_string()]);
        assert!(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_calling("shout-echo"),
                Ok(Vec::new()),
                &published,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_cached_image_is_reused_only_when_built_from_the_checked_children() {
        let child_graph = graph_calling("http");
        let closure = AllowedClosure {
            children: vec![child_with_graph(child_graph.clone())],
        };
        let checksum = runtara_workflows::direct_wasm::manifest::build_direct_workflow_manifest(
            &parse_execution_graph(&child_graph).expect("child parses"),
        )
        .expect("manifest builds")
        .checksum()
        .to_owned();
        let image_recording = |version: i64, checksum: &str| {
            image_summary_with_metadata(serde_json::json!({
                "workflow": {"directArtifact": {"childWorkflows": [{
                    "stepId": "embed",
                    "workflowId": "child-wf",
                    "versionRequested": "latest",
                    "versionResolved": version,
                    "manifestChecksum": checksum,
                }]}}
            }))
        };

        assert!(closure.built_from_these_children(Some(&image_recording(1, &checksum))));
        // Built from another child version, or the same version edited in
        // place, or with no record of its children at all.
        assert!(!closure.built_from_these_children(Some(&image_recording(2, &checksum))));
        assert!(!closure.built_from_these_children(Some(&image_recording(1, "edited"))));
        assert!(
            !closure.built_from_these_children(Some(&image_summary_with_metadata(
                serde_json::json!({"workflow": {}})
            )))
        );
        assert!(!closure.built_from_these_children(None));

        // A version that embeds nothing has no children to differ.
        assert!(AllowedClosure::default().built_from_these_children(None));
    }

    #[test]
    fn compile_gate_refuses_a_closure_it_cannot_load_or_parse() {
        let snapshot = http_only_snapshot();
        let none_published = std::collections::HashSet::new();

        // The root only calls an allowed agent, so walking it alone would pass.
        expect_unchecked(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_calling("http"),
                Err("Database error: connection reset".to_string()),
                &none_published,
            ),
            "Failed to load child workflows: Database error: connection reset",
        );
        // A reference that does not resolve stays that way until the
        // workflow changes, unlike a database failure.
        expect_invalid(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_embedding_child(),
                Err("EmbedWorkflow step 'embed': child workflow 'child-wf' not found".to_string()),
                &none_published,
            ),
            "child workflow 'child-wf' not found",
        );
        expect_invalid(
            compile_agent_allowlist_decision(
                &snapshot,
                &graph_embedding_child(),
                Ok(vec![
                    child_with_graph(graph_calling("http")),
                    child_with_graph(serde_json::json!({"steps": "not a map"})),
                ]),
                &none_published,
            ),
            "Failed to parse child workflow 'child-wf'",
        );
        expect_invalid(
            compile_agent_allowlist_decision(
                &snapshot,
                &serde_json::json!({"steps": "not a map"}),
                Ok(Vec::new()),
                &none_published,
            ),
            "Failed to parse execution graph",
        );
    }
}
