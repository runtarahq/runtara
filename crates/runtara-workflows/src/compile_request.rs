// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A compile handed to a compiler elsewhere: a WASI command in a VM that
//! grants it no filesystem, say, so a tenant's workflow is never compiled in
//! a process that serves tenants.
//!
//! The host turns a [`CompilationInput`] into a [`CompileRequest`] with
//! [`request`]: the workflow and its children, and every component the
//! compile may read, by file name and SHA-256 rather than bytes. The compiler
//! runs it with [`compile`], fetching each component by its SHA-256 only when
//! the compile first reads it, and hands back a [`CompileOutcome`] and the
//! composed `workflow.wasm`. The host writes them into its build directory
//! with [`apply`], answering as [`compile_workflow_direct`] would have.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use runtara_dsl::ExecutionGraph;
use runtara_dsl::agent_meta::AgentCatalog;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::compile::{
    ChildDependency, ChildWorkflowInput, CompilationInput, DirectWorkflowCompileOptions,
    NativeCompilationResult, WorkflowCompilerMode, compile_workflow_direct,
};
use crate::compile_fs::{MemoryFs, in_memory};
use crate::direct_wasm::DirectCompileError;

/// Where the compiler sees the shared and agent components.
const COMPONENTS: &str = "/components";
/// Where it sees the `n`th extra components directory: `/extra/{n}`.
const EXTRA: &str = "/extra";
/// Where it builds.
const BUILD: &str = "/build";
/// The composed component, handed back on its own.
const WORKFLOW_WASM: &str = "workflow.wasm";

/// A child workflow the compiled one embeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestChild {
    /// The step in the parent that embeds it.
    pub step_id: String,
    /// Its workflow id.
    pub workflow_id: String,
    /// The version the parent asked for.
    pub version_requested: String,
    /// The version that was resolved.
    pub version_resolved: i32,
    /// Its execution graph.
    pub execution_graph: ExecutionGraph,
}

/// What a compile is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompileRole {
    /// A workflow, as [`compile_workflow_direct`] compiles it.
    #[default]
    Workflow,
    /// A workflow published as an agent, as its publish compiles it
    /// ([`crate::direct_wasm::WorkflowRole::PublishedAgent`], the runtime
    /// omitted where the graph allows).
    PublishedAgent,
}

/// A compile, for a compiler elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileRequest {
    /// What it is for.
    #[serde(default)]
    pub role: CompileRole,
    /// The tenant the workflow is.
    pub tenant_id: String,
    /// The workflow.
    pub workflow_id: String,
    /// Its version.
    pub version: u32,
    /// Its execution graph.
    pub execution_graph: ExecutionGraph,
    /// Whether it records step events.
    pub track_events: bool,
    /// The child workflows it embeds.
    pub child_workflows: Vec<RequestChild>,
    /// The agent catalog it compiles against.
    pub agent_catalog: Option<AgentCatalog>,
    /// Its slug, for an agent-shaped compile.
    pub agent_slug: Option<String>,
    /// The checksum of its source.
    pub source_checksum: Option<String>,
    /// The shared and agent components, by file name: each file's SHA-256.
    pub components: BTreeMap<String, String>,
    /// The extra component directories, in order (a tenant's staged
    /// workflow-agents), each by file name: each file's SHA-256.
    pub extra_components: Vec<BTreeMap<String, String>>,
}

/// A child dependency the compile recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeChild {
    /// The step that embeds it.
    pub step_id: String,
    /// Its workflow id.
    pub child_workflow_id: String,
    /// The version asked for.
    pub child_version_requested: String,
    /// The version resolved.
    pub child_version_resolved: i32,
}

/// What a compile made, but its composed component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileOutcome {
    /// The build directory's other files, by path relative to it, base64.
    pub files: BTreeMap<String, String>,
    /// The composed component's size.
    pub binary_size: usize,
    /// The composed component's SHA-256.
    pub binary_checksum: String,
    /// See [`NativeCompilationResult::package_size`].
    pub package_size: usize,
    /// The child workflows it depends on.
    pub child_dependencies: Vec<OutcomeChild>,
    /// Its variables' defaults.
    pub default_variables: Value,
    /// The trusted built-ins it pins.
    pub trusted_pins: Vec<String>,
}

/// What a compiler elsewhere posts as its run's outcome.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOutcome {
    /// It compiled: what it made, and the SHA-256 of the composed component
    /// it handed back.
    Compiled {
        /// What the compile made, but the composed component.
        outcome: Box<CompileOutcome>,
        /// The SHA-256 the host answered for the handed-back component.
        workflow_wasm: String,
    },
    /// It did not.
    Failed(CompileFailure),
}

/// A compile that failed: what [`apply_failure`] turns back into the error
/// the compile would have returned in process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileFailure {
    /// The error's kind, as [`io::ErrorKind`]'s debug name.
    pub kind: String,
    /// Its message.
    pub message: String,
    /// A published workflow-agent built against a built-in no longer
    /// installed.
    pub stale: Option<Box<StaleDependency>>,
}

/// A published workflow-agent built against a built-in no longer installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleDependency {
    /// The workflow-agent to republish.
    pub dependency: String,
    /// The built-in it pins.
    pub agent: String,
    /// Its stale pins.
    pub pins: Vec<String>,
    /// The staged file, named as the compiler saw it.
    pub wasm_path: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The regular files directly in `dir`, each by name: its SHA-256, and where
/// it is.
fn listed(
    dir: &Path,
    paths: &mut BTreeMap<String, PathBuf>,
) -> io::Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // An extra directory that does not exist yet holds nothing.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let digest = sha256_hex(&std::fs::read(entry.path())?);
        paths.insert(digest.clone(), entry.path());
        files.insert(name, digest);
    }
    Ok(files)
}

/// The request for compiling `input` with `options`, and where each file it
/// names is, by SHA-256: what the compiler may fetch.
pub fn request(
    input: &CompilationInput,
    options: &DirectWorkflowCompileOptions,
) -> io::Result<(CompileRequest, BTreeMap<String, PathBuf>)> {
    let mut paths = BTreeMap::new();
    let components = listed(&options.components_dir, &mut paths)?;
    let extra_components = options
        .extra_component_dirs
        .iter()
        .map(|dir| listed(dir, &mut paths))
        .collect::<io::Result<_>>()?;
    Ok((
        CompileRequest {
            role: CompileRole::Workflow,
            tenant_id: input.tenant_id.clone(),
            workflow_id: input.workflow_id.clone(),
            version: input.version,
            execution_graph: input.execution_graph.clone(),
            track_events: input.track_events,
            child_workflows: input
                .child_workflows
                .iter()
                .map(|child| RequestChild {
                    step_id: child.step_id.clone(),
                    workflow_id: child.workflow_id.clone(),
                    version_requested: child.version_requested.clone(),
                    version_resolved: child.version_resolved,
                    execution_graph: child.execution_graph.clone(),
                })
                .collect(),
            agent_catalog: input.agent_catalog.as_deref().cloned(),
            agent_slug: input.agent_slug.clone(),
            source_checksum: options.source_checksum.clone(),
            components,
            extra_components,
        },
        paths,
    ))
}

/// The request for publishing `input`, `tenant`'s workflow, as an agent
/// with the components in `components_dir` and `extra_component_dirs`, and
/// where each file it names is, by SHA-256.
pub fn request_published_agent(
    tenant: &str,
    input: &crate::direct_wasm::DirectCompilationInput,
    components_dir: &Path,
    extra_component_dirs: &[PathBuf],
) -> io::Result<(CompileRequest, BTreeMap<String, PathBuf>)> {
    let mut paths = BTreeMap::new();
    let components = listed(components_dir, &mut paths)?;
    let extra_components = extra_component_dirs
        .iter()
        .map(|dir| listed(dir, &mut paths))
        .collect::<io::Result<_>>()?;
    Ok((
        CompileRequest {
            role: CompileRole::PublishedAgent,
            tenant_id: tenant.to_owned(),
            workflow_id: input.workflow_id.clone(),
            version: input.version,
            execution_graph: input.execution_graph.clone(),
            track_events: input.track_events,
            child_workflows: input
                .child_workflows
                .iter()
                .map(|child| RequestChild {
                    step_id: child.step_id.clone(),
                    workflow_id: child.workflow_id.clone(),
                    version_requested: child.version_requested.clone(),
                    version_resolved: child.version_resolved,
                    execution_graph: child.execution_graph.clone(),
                })
                .collect(),
            agent_catalog: input.agent_catalog.as_deref().cloned(),
            agent_slug: input.agent_slug.clone(),
            source_checksum: input.source_checksum.clone(),
            components,
            extra_components,
        },
        paths,
    ))
}

/// Compiles `request` with no filesystem: each component is fetched by its
/// SHA-256 through `fetch` the first time the compile reads it, and checked
/// against it. Answers what the compile made and the composed component, or
/// why it failed.
pub fn compile(
    request: CompileRequest,
    fetch: impl Fn(&str) -> io::Result<Vec<u8>> + Send + Sync + 'static,
) -> Result<(CompileOutcome, Vec<u8>), CompileFailure> {
    let mut by_path = BTreeMap::new();
    for (name, digest) in &request.components {
        by_path.insert(Path::new(COMPONENTS).join(name), digest.clone());
    }
    let extra_dirs: Vec<PathBuf> = (0..request.extra_components.len())
        .map(|index| Path::new(EXTRA).join(index.to_string()))
        .collect();
    for (dir, files) in extra_dirs.iter().zip(&request.extra_components) {
        for (name, digest) in files {
            by_path.insert(dir.join(name), digest.clone());
        }
    }
    let known: Vec<PathBuf> = by_path.keys().cloned().collect();
    let memory = MemoryFs::with_loader(
        known,
        Box::new(move |path| {
            let digest = by_path.get(path).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, path.display().to_string())
            })?;
            let bytes = fetch(digest)?;
            if sha256_hex(&bytes) != *digest {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not the file it was named for", path.display()),
                ));
            }
            Ok(bytes)
        }),
    );
    if request.role == CompileRole::PublishedAgent {
        return compile_published_agent(request, extra_dirs, memory);
    }
    let input = CompilationInput {
        tenant_id: request.tenant_id,
        workflow_id: request.workflow_id,
        version: request.version,
        execution_graph: request.execution_graph,
        track_events: request.track_events,
        child_workflows: request
            .child_workflows
            .into_iter()
            .map(|child| ChildWorkflowInput {
                step_id: child.step_id,
                workflow_id: child.workflow_id,
                version_requested: child.version_requested,
                version_resolved: child.version_resolved,
                execution_graph: child.execution_graph,
            })
            .collect(),
        agent_catalog: request.agent_catalog.map(Arc::new),
        progress_callback: None,
        agent_slug: request.agent_slug,
    };
    let options = DirectWorkflowCompileOptions {
        output_dir: PathBuf::from(BUILD),
        components_dir: PathBuf::from(COMPONENTS),
        extra_component_dirs: extra_dirs,
        source_checksum: request.source_checksum,
    };
    let result =
        in_memory(memory.clone(), || compile_workflow_direct(input, options)).map_err(failure)?;
    let (files, wasm) = handed_back(&memory, &result.build_dir)?;
    Ok((
        CompileOutcome {
            files,
            binary_size: result.binary_size,
            binary_checksum: result.binary_checksum,
            package_size: result.package_size,
            child_dependencies: result
                .child_dependencies
                .into_iter()
                .map(|child| OutcomeChild {
                    step_id: child.step_id,
                    child_workflow_id: child.child_workflow_id,
                    child_version_requested: child.child_version_requested,
                    child_version_resolved: child.child_version_resolved,
                })
                .collect(),
            default_variables: result.default_variables,
            trusted_pins: result.trusted_pins,
        },
        wasm,
    ))
}

/// [`compile`] for a workflow published as an agent.
fn compile_published_agent(
    request: CompileRequest,
    extra_dirs: Vec<PathBuf>,
    memory: Arc<MemoryFs>,
) -> Result<(CompileOutcome, Vec<u8>), CompileFailure> {
    use crate::direct_wasm::{
        DirectCompilationInput, WorkflowRole, compile_direct_workflow_with_abi,
        compose_direct_workflow_with_extra_dirs,
    };
    let input = DirectCompilationInput {
        workflow_id: request.workflow_id,
        version: request.version,
        source_checksum: request.source_checksum,
        execution_graph: request.execution_graph,
        child_workflows: request
            .child_workflows
            .into_iter()
            .map(|child| ChildWorkflowInput {
                step_id: child.step_id,
                workflow_id: child.workflow_id,
                version_requested: child.version_requested,
                version_resolved: child.version_resolved,
                execution_graph: child.execution_graph,
            })
            .collect(),
        output_dir: PathBuf::from(BUILD),
        track_events: request.track_events,
        agent_catalog: request.agent_catalog.map(Arc::new),
        agent_slug: request.agent_slug,
    };
    let result = in_memory(memory.clone(), || {
        let mut result =
            compile_direct_workflow_with_abi(input, WorkflowRole::PublishedAgent, true)?;
        compose_direct_workflow_with_extra_dirs(&mut result, Path::new(COMPONENTS), &extra_dirs)?;
        Ok::<_, DirectCompileError>(result)
    })
    .map_err(|error| failure(crate::compile::direct_compile_error_to_io(error)))?;
    let (files, wasm) = handed_back(&memory, &result.build_dir)?;
    Ok((
        CompileOutcome {
            files,
            binary_size: result.wasm_size,
            binary_checksum: result.wasm_checksum,
            package_size: 0,
            child_dependencies: Vec::new(),
            default_variables: Value::Null,
            trusted_pins: Vec::new(),
        },
        wasm,
    ))
}

/// The build in `build_dir`: its files but the composed component, base64,
/// and that component.
fn handed_back(
    memory: &MemoryFs,
    build_dir: &Path,
) -> Result<(BTreeMap<String, String>, Vec<u8>), CompileFailure> {
    let mut files = BTreeMap::new();
    let mut wasm = None;
    for (relative, bytes) in memory.under(build_dir) {
        if relative == Path::new(WORKFLOW_WASM) {
            wasm = Some(bytes);
        } else {
            files.insert(
                relative.to_string_lossy().into_owned(),
                STANDARD.encode(bytes),
            );
        }
    }
    let wasm = wasm.ok_or_else(|| CompileFailure {
        kind: format!("{:?}", io::ErrorKind::Other),
        message: "the compile composed no workflow.wasm".into(),
        stale: None,
    })?;
    Ok((files, wasm))
}

fn failure(error: io::Error) -> CompileFailure {
    let stale = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<DirectCompileError>())
        .and_then(|inner| match inner {
            DirectCompileError::StaleTrustedDependency {
                dependency,
                agent,
                pins,
                wasm_path,
            } => Some(Box::new(StaleDependency {
                dependency: dependency.clone(),
                agent: agent.clone(),
                pins: pins.clone(),
                wasm_path: wasm_path.display().to_string(),
            })),
            _ => None,
        });
    CompileFailure {
        kind: format!("{:?}", error.kind()),
        message: error.to_string(),
        stale,
    }
}

/// The error a failed compile elsewhere would have returned in process: a
/// stale workflow-agent's staged file is named as the host has it, from the
/// `options` the request was made with.
pub fn apply_failure(failure: CompileFailure, options: &DirectWorkflowCompileOptions) -> io::Error {
    if let Some(stale) = failure.stale {
        let StaleDependency {
            dependency,
            agent,
            pins,
            wasm_path: seen,
        } = *stale;
        let seen = PathBuf::from(seen);
        let wasm_path = seen
            .strip_prefix(EXTRA)
            .ok()
            .and_then(|rest| {
                let mut parts = rest.components();
                let index: usize = parts.next()?.as_os_str().to_str()?.parse().ok()?;
                Some(
                    options
                        .extra_component_dirs
                        .get(index)?
                        .join(parts.as_path()),
                )
            })
            .or_else(|| {
                seen.strip_prefix(COMPONENTS)
                    .ok()
                    .map(|rest| options.components_dir.join(rest))
            })
            .unwrap_or(seen);
        return io::Error::other(DirectCompileError::StaleTrustedDependency {
            dependency,
            agent,
            pins,
            wasm_path,
        });
    }
    // Its message names the paths the compiler saw: name the host's.
    let mut message = failure.message;
    for (index, dir) in options.extra_component_dirs.iter().enumerate().rev() {
        message = message.replace(&format!("{EXTRA}/{index}/"), &format!("{}/", dir.display()));
    }
    let message = message.replace(
        &format!("{COMPONENTS}/"),
        &format!("{}/", options.components_dir.display()),
    );
    let kind = match failure.kind.as_str() {
        "Unsupported" => io::ErrorKind::Unsupported,
        "InvalidInput" => io::ErrorKind::InvalidInput,
        "InvalidData" => io::ErrorKind::InvalidData,
        "NotFound" => io::ErrorKind::NotFound,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, message)
}

/// Writes what a compile elsewhere made into `options.output_dir`, as the
/// compile would have, and answers as [`compile_workflow_direct`] would
/// have. The composed component must be the one the outcome names.
pub fn apply(
    outcome: CompileOutcome,
    workflow_wasm: &[u8],
    workflow_id: &str,
    version: u32,
    options: &DirectWorkflowCompileOptions,
) -> io::Result<NativeCompilationResult> {
    let (build_dir, binary_path) = write_build(
        &outcome,
        workflow_wasm,
        workflow_id,
        version,
        &options.output_dir,
    )?;
    Ok(NativeCompilationResult {
        binary_path,
        binary_size: outcome.binary_size,
        binary_checksum: outcome.binary_checksum,
        build_dir,
        package_size: outcome.package_size,
        child_dependencies: outcome
            .child_dependencies
            .into_iter()
            .map(|child| ChildDependency {
                step_id: child.step_id,
                child_workflow_id: child.child_workflow_id,
                child_version_requested: child.child_version_requested,
                child_version_resolved: child.child_version_resolved,
            })
            .collect(),
        default_variables: outcome.default_variables,
        compiler_mode: WorkflowCompilerMode::DirectWasm,
        trusted_pins: outcome.trusted_pins,
    })
}

/// A workflow-agent compiled for publishing elsewhere: its composed
/// component and its artifact metadata, written where its publish would
/// have written them.
#[derive(Debug)]
pub struct PublishedAgent {
    /// The composed component.
    pub wasm_path: PathBuf,
    /// Its size.
    pub wasm_size: usize,
    /// Its artifact metadata.
    pub artifact_metadata: crate::direct_wasm::DirectArtifactMetadata,
}

/// Writes what a publish compile elsewhere made into `output_dir`, as the
/// compile would have.
pub fn apply_published_agent(
    outcome: CompileOutcome,
    workflow_wasm: &[u8],
    workflow_id: &str,
    version: u32,
    output_dir: &Path,
) -> io::Result<PublishedAgent> {
    let (build_dir, wasm_path) =
        write_build(&outcome, workflow_wasm, workflow_id, version, output_dir)?;
    let metadata = std::fs::read(
        build_dir.join(crate::direct_wasm::DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME),
    )?;
    let artifact_metadata = serde_json::from_slice(&metadata)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(PublishedAgent {
        wasm_path,
        wasm_size: workflow_wasm.len(),
        artifact_metadata,
    })
}

/// Writes a compile's build into `output_dir`, as the compile would have:
/// answers its directory and composed component.
fn write_build(
    outcome: &CompileOutcome,
    workflow_wasm: &[u8],
    workflow_id: &str,
    version: u32,
    output_dir: &Path,
) -> io::Result<(PathBuf, PathBuf)> {
    if sha256_hex(workflow_wasm) != outcome.binary_checksum
        || workflow_wasm.len() != outcome.binary_size
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the composed component is not the one the compile made",
        ));
    }
    let build_dir = output_dir.join(format!(
        "{}-v{}-direct",
        crate::direct_wasm::compile::sanitize_path_segment(workflow_id),
        version
    ));
    let _ = std::fs::remove_dir_all(&build_dir);
    std::fs::create_dir_all(&build_dir)?;
    for (relative, encoded) in &outcome.files {
        let relative = Path::new(relative);
        // Only paths beneath the build directory.
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not within the build", relative.display()),
            ));
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let path = build_dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    let binary_path = build_dir.join(WORKFLOW_WASM);
    std::fs::write(&binary_path, workflow_wasm)?;
    Ok((build_dir, binary_path))
}
