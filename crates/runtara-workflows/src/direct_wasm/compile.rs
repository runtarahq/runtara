// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in direct workflow compilation entry point.
//!
//! This is the first production-shaped entry point, not the PoC ABI. It emits
//! a deterministic component artifact for finish-only graphs and writes the
//! manifest/support sidecars that later graph-lowering work will consume.

use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use runtara_dsl::ExecutionGraph;
use runtara_workflow_wit::{RUNTIME_WIT, STDLIB_WIT, WORKFLOW_WIT_VERSION};
use sha2::{Digest, Sha256};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, CustomSection, DataSection, Encode, EntityType, ExportKind,
    ExportSection, Function as WasmFunction, FunctionSection, GlobalSection, GlobalType, Ieee32,
    Ieee64, ImportSection, Instruction, MemArg, MemorySection, MemoryType, Module, Section,
    TypeSection, ValType,
};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::abi::WasmType;
use wit_parser::{
    Function as WitFunction, ManglingAndAbi, Resolve, WasmExport, WasmExportKind, WasmImport,
    WorldId, WorldItem, WorldKey,
};

use super::component::{DirectComponentArtifacts, emit_direct_component_artifacts};
use super::manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectManifestError, DirectWorkflowManifest,
    build_direct_workflow_manifest,
};
use super::support::{
    DirectWorkflowSupportReport, UnsupportedWorkflowFeature, analyze_direct_wasm_support,
};

/// Direct workflow artifact ABI version.
pub const DIRECT_WORKFLOW_ABI_VERSION: u32 = 1;
/// Custom section containing [`DirectWorkflowManifest`] JSON.
pub const DIRECT_WORKFLOW_MANIFEST_SECTION: &str = "runtara.direct_workflow.manifest";
/// Custom section containing [`DirectWorkflowSupportReport`] JSON.
pub const DIRECT_WORKFLOW_SUPPORT_SECTION: &str = "runtara.direct_workflow.support";
/// Custom section containing direct artifact ABI metadata JSON.
pub const DIRECT_WORKFLOW_ABI_SECTION: &str = "runtara.direct_workflow.abi";

const WASI_CLI_RUN_WIT: &str = r#"
package wasi:cli@0.2.3;

interface run {
    run: func() -> result;
}

world command {
    export run;
}
"#;

const DIRECT_RUN_RETPTR_OFFSET: i32 = 0;
const DIRECT_STATIC_DATA_OFFSET: i32 = 64;
const DIRECT_EMPTY_STEPS_CONTEXT: &[u8] = b"{}";
const WASM_PAGE_SIZE: i32 = 65_536;

/// Input for the opt-in direct compiler.
#[derive(Debug, Clone)]
pub struct DirectCompilationInput {
    /// Unique workflow identifier.
    pub workflow_id: String,
    /// Workflow version number.
    pub version: u32,
    /// Parsed workflow execution graph.
    pub execution_graph: ExecutionGraph,
    /// Directory where the direct artifact directory should be created.
    pub output_dir: PathBuf,
}

/// Result of opt-in direct workflow compilation.
#[derive(Debug, Clone)]
pub struct DirectCompilationResult {
    /// Path to the primary emitted Wasm artifact.
    ///
    /// Before static composition this is the directly emitted
    /// `workflow-logic.wasm`; after [`compose_direct_workflow`] it is the final
    /// runnable `workflow.wasm`.
    pub wasm_path: PathBuf,
    /// Path to the directly emitted workflow-logic component.
    pub workflow_logic_wasm_path: PathBuf,
    /// Path to the emitted manifest sidecar.
    pub manifest_path: PathBuf,
    /// Path to the emitted support-report sidecar.
    pub support_report_path: PathBuf,
    /// Path to the generated component world WIT.
    pub world_wit_path: PathBuf,
    /// Path to the generated static composition script.
    pub wac_path: PathBuf,
    /// Path to the per-workflow direct build directory.
    pub build_dir: PathBuf,
    /// Size of the primary emitted Wasm artifact in bytes.
    pub wasm_size: usize,
    /// SHA-256 checksum of the primary emitted Wasm artifact.
    pub wasm_checksum: String,
    /// Size of the workflow-logic component in bytes.
    pub workflow_logic_wasm_size: usize,
    /// SHA-256 checksum of the workflow-logic component.
    pub workflow_logic_wasm_checksum: String,
    /// Path to the final statically composed `workflow.wasm`, when composed.
    pub composed_wasm_path: Option<PathBuf>,
    /// Size of the final statically composed artifact in bytes.
    pub composed_wasm_size: Option<usize>,
    /// SHA-256 checksum of the final statically composed artifact.
    pub composed_wasm_checksum: Option<String>,
    /// SHA-256 checksum embedded in the manifest.
    pub manifest_checksum: String,
    /// Deterministic support report produced before emission.
    pub support_report: DirectWorkflowSupportReport,
    /// Component-facing scaffolding emitted beside the direct artifact.
    pub component_artifacts: DirectComponentArtifacts,
}

/// Errors returned by the opt-in direct compiler.
#[derive(Debug)]
pub enum DirectCompileError {
    /// Manifest construction failed.
    Manifest(DirectManifestError),
    /// Support report serialization failed.
    Serialize(serde_json::Error),
    /// The current direct compiler cannot emit this workflow yet.
    Unsupported {
        /// Deterministic support report with exact unsupported features.
        report: Box<DirectWorkflowSupportReport>,
    },
    /// Filesystem write or metadata read failed.
    Io(std::io::Error),
    /// Component-model artifact emission failed.
    Component(String),
}

impl fmt::Display for DirectCompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectCompileError::Manifest(err) => write!(f, "{err}"),
            DirectCompileError::Serialize(err) => {
                write!(
                    f,
                    "failed to serialize direct workflow artifact metadata: {err}"
                )
            }
            DirectCompileError::Unsupported { report } => write!(
                f,
                "direct workflow compiler does not support this graph yet: {}",
                unsupported_summary(&report.unsupported)
            ),
            DirectCompileError::Io(err) => {
                write!(f, "direct workflow artifact write failed: {err}")
            }
            DirectCompileError::Component(err) => {
                write!(f, "direct workflow component emission failed: {err}")
            }
        }
    }
}

impl std::error::Error for DirectCompileError {}

impl From<DirectManifestError> for DirectCompileError {
    fn from(value: DirectManifestError) -> Self {
        Self::Manifest(value)
    }
}

impl From<serde_json::Error> for DirectCompileError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialize(value)
    }
}

impl From<std::io::Error> for DirectCompileError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Compose a direct workflow logic component with prebuilt shared components.
///
/// `compile_direct_workflow` intentionally stops after direct workflow logic
/// emission. This explicit step performs the static composition that will
/// eventually produce the runnable `workflow.wasm` artifact for direct mode.
pub fn compose_direct_workflow(
    result: &mut DirectCompilationResult,
    components_dir: impl AsRef<Path>,
) -> Result<PathBuf, DirectCompileError> {
    let components_dir = components_dir.as_ref();
    let composed_path = result.build_dir.join("workflow.wasm");

    let mut cmd = Command::new("wac");
    cmd.arg("compose")
        .arg(&result.wac_path)
        .arg("-d")
        .arg(format!(
            "runtara:workflow-logic={}",
            result.workflow_logic_wasm_path.display()
        ));

    for component in &result.component_artifacts.shared_components {
        let wasm = components_dir.join(component.bundle_wasm_filename);
        if !wasm.exists() {
            return Err(DirectCompileError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "direct shared component `{}` missing at {}",
                    component.package,
                    wasm.display()
                ),
            )));
        }
        cmd.arg("-d")
            .arg(format!("{}={}", component.package, wasm.display()));
    }

    cmd.arg("-o").arg(&composed_path);
    let status = cmd.status().map_err(|err| {
        DirectCompileError::Component(format!(
            "wac compose failed to launch for direct workflow (is wac-cli installed?): {err}"
        ))
    })?;
    if !status.success() {
        return Err(DirectCompileError::Component(format!(
            "wac compose returned non-zero status {} for direct workflow (wac script: {})",
            status,
            result.wac_path.display()
        )));
    }
    if !composed_path.exists() {
        return Err(DirectCompileError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "wac compose succeeded but direct composed artifact was not written at {}",
                composed_path.display()
            ),
        )));
    }

    let composed_wasm = fs::read(&composed_path)?;
    let composed_wasm_size = composed_wasm.len();
    let composed_wasm_checksum = sha256_hex(&composed_wasm);

    result.wasm_path = composed_path.clone();
    result.wasm_size = composed_wasm_size;
    result.wasm_checksum = composed_wasm_checksum.clone();
    result.composed_wasm_path = Some(composed_path.clone());
    result.composed_wasm_size = Some(composed_wasm_size);
    result.composed_wasm_checksum = Some(composed_wasm_checksum);

    Ok(composed_path)
}

/// Compile and statically compose a direct workflow into the final
/// `workflow.wasm` artifact shape used by the runtime.
pub fn compile_direct_workflow_composed(
    input: DirectCompilationInput,
    components_dir: impl AsRef<Path>,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let mut result = compile_direct_workflow(input)?;
    compose_direct_workflow(&mut result, components_dir)?;
    Ok(result)
}

/// Compile a finish-only workflow through the direct path.
///
/// This does not replace [`crate::compile_workflow`]. It is intentionally
/// opt-in and currently supports only graphs accepted by
/// [`analyze_direct_wasm_support`]. The emitted component-format artifact is a
/// stable direct pipeline artifact with a canonical `wasi:cli/run` export and
/// a first runtime completion call. Full `Finish` output lowering will replace
/// the current static output payload in the next phases.
pub fn compile_direct_workflow(
    input: DirectCompilationInput,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let manifest = build_direct_workflow_manifest(&input.execution_graph)?;
    let support_report = analyze_direct_wasm_support(&input.execution_graph);
    if !support_report.supported {
        return Err(DirectCompileError::Unsupported {
            report: Box::new(support_report),
        });
    }

    let manifest_json = manifest.to_canonical_json()?;
    let support_json = serde_json::to_vec(&support_report)?;
    let wasm = emit_finish_only_artifact(&manifest, &manifest_json, &support_json)?;
    let component_artifacts = emit_direct_component_artifacts(&manifest.feature_summary.agent_ids);

    let build_dir = input.output_dir.join(format!(
        "{}-v{}-direct",
        sanitize_path_segment(&input.workflow_id),
        input.version
    ));
    fs::create_dir_all(&build_dir)?;
    fs::create_dir_all(build_dir.join("wit"))?;

    let wasm_path = build_dir.join("workflow-logic.wasm");
    let manifest_path = build_dir.join("manifest.json");
    let support_report_path = build_dir.join("support-report.json");
    let world_wit_path = build_dir.join("wit/world.wit");
    let wac_path = build_dir.join("workflow.wac");

    fs::write(&wasm_path, &wasm)?;
    fs::write(&manifest_path, &manifest_json)?;
    fs::write(&support_report_path, &support_json)?;
    fs::write(&world_wit_path, &component_artifacts.world_wit)?;
    fs::write(&wac_path, &component_artifacts.wac_source)?;

    Ok(DirectCompilationResult {
        wasm_path,
        workflow_logic_wasm_path: build_dir.join("workflow-logic.wasm"),
        manifest_path,
        support_report_path,
        world_wit_path,
        wac_path,
        build_dir,
        wasm_size: wasm.len(),
        wasm_checksum: sha256_hex(&wasm),
        workflow_logic_wasm_size: wasm.len(),
        workflow_logic_wasm_checksum: sha256_hex(&wasm),
        composed_wasm_path: None,
        composed_wasm_size: None,
        composed_wasm_checksum: None,
        manifest_checksum: manifest.checksum().to_string(),
        support_report,
        component_artifacts,
    })
}

fn emit_finish_only_artifact(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
    support_json: &[u8],
) -> Result<Vec<u8>, DirectCompileError> {
    let abi_json = serde_json::to_vec(&serde_json::json!({
        "abiVersion": DIRECT_WORKFLOW_ABI_VERSION,
        "artifactKind": "finish-only-run-component",
        "componentRunExport": "wasi:cli/run@0.2.3",
        "entryPointExecutable": true,
        "runtimeExecutable": true,
        "finishOutputMode": "stdlib-apply-mapping",
        "manifestVersion": DIRECT_WORKFLOW_MANIFEST_VERSION,
        "stepCount": manifest.feature_summary.total_steps,
        "note": "direct compiler component with canonical run export, stdlib Finish mapping, and runtime.complete call"
    }))?;

    let mut component = emit_finish_only_component(manifest, manifest_json)?;
    append_component_custom_section(&mut component, DIRECT_WORKFLOW_ABI_SECTION, &abi_json);
    append_component_custom_section(
        &mut component,
        DIRECT_WORKFLOW_MANIFEST_SECTION,
        manifest_json,
    );
    append_component_custom_section(
        &mut component,
        DIRECT_WORKFLOW_SUPPORT_SECTION,
        support_json,
    );

    Ok(component)
}

fn emit_finish_only_component(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
) -> Result<Vec<u8>, DirectCompileError> {
    let (resolve, world) = build_direct_component_resolve()?;
    let core_config = DirectCoreConfig::new(manifest, manifest_json)?;
    let mut core_module = emit_direct_core_module(&resolve, world, &core_config)?;
    embed_component_metadata(&mut core_module, &resolve, world, StringEncoding::UTF8)
        .map_err(component_error)?;

    ComponentEncoder::default()
        .module(&core_module)
        .map_err(component_error)?
        .validate(true)
        .encode()
        .map_err(component_error)
}

fn build_direct_component_resolve() -> Result<(Resolve, WorldId), DirectCompileError> {
    let mut resolve = Resolve::default();
    resolve
        .push_str("runtara-workflow-stdlib.wit", STDLIB_WIT)
        .map_err(component_error)?;
    resolve
        .push_str("runtara-workflow-runtime.wit", RUNTIME_WIT)
        .map_err(component_error)?;
    resolve
        .push_str("wasi-cli-run.wit", WASI_CLI_RUN_WIT)
        .map_err(component_error)?;

    let workflow_wit = format!(
        "package runtara:workflow@{WORKFLOW_WIT_VERSION};\n\
         \n\
         world workflow {{\n\
             import runtara:workflow-stdlib/json@{WORKFLOW_WIT_VERSION};\n\
             import runtara:workflow-runtime/runtime@{WORKFLOW_WIT_VERSION};\n\
             export wasi:cli/run@0.2.3;\n\
         }}\n"
    );
    let package = resolve
        .push_str("runtara-workflow.wit", &workflow_wit)
        .map_err(component_error)?;
    let world = resolve
        .select_world(&[package], Some("workflow"))
        .map_err(component_error)?;

    Ok((resolve, world))
}

#[derive(Debug, Clone)]
struct DirectCoreConfig {
    finish_mapping_id: u32,
    static_data: DirectCoreStaticData,
}

impl DirectCoreConfig {
    fn new(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
    ) -> Result<Self, DirectCompileError> {
        let variables_json = serde_json::to_vec(&manifest.graph.variables)?;
        Ok(Self {
            finish_mapping_id: entry_finish_mapping_id(manifest)?,
            static_data: DirectCoreStaticData::new(
                manifest_json,
                &variables_json,
                DIRECT_EMPTY_STEPS_CONTEXT,
            )?,
        })
    }
}

#[derive(Debug, Clone)]
struct DirectCoreStaticData {
    manifest: DirectDataSegment,
    variables: DirectDataSegment,
    steps: DirectDataSegment,
    heap_base: i32,
    memory_min_pages: u64,
}

impl DirectCoreStaticData {
    fn new(
        manifest_json: &[u8],
        variables_json: &[u8],
        steps_json: &[u8],
    ) -> Result<Self, DirectCompileError> {
        let mut offset = DIRECT_STATIC_DATA_OFFSET;
        let manifest = DirectDataSegment::new(offset, manifest_json);
        offset = align_i32(checked_offset_add(offset, manifest_json.len())?, 4);

        let variables = DirectDataSegment::new(offset, variables_json);
        offset = align_i32(checked_offset_add(offset, variables_json.len())?, 4);

        let steps = DirectDataSegment::new(offset, steps_json);
        offset = align_i32(checked_offset_add(offset, steps_json.len())?, 16);

        let memory_min_pages = wasm_pages_for_bytes(offset)?;
        Ok(Self {
            manifest,
            variables,
            steps,
            heap_base: offset,
            memory_min_pages,
        })
    }
}

#[derive(Debug, Clone)]
struct DirectDataSegment {
    offset: i32,
    data: Vec<u8>,
}

impl DirectDataSegment {
    fn new(offset: i32, data: &[u8]) -> Self {
        Self {
            offset,
            data: data.to_vec(),
        }
    }

    fn len_i32(&self) -> i32 {
        i32::try_from(self.data.len()).expect("direct data length already checked")
    }
}

fn entry_finish_mapping_id(manifest: &DirectWorkflowManifest) -> Result<u32, DirectCompileError> {
    manifest
        .graph
        .mappings
        .iter()
        .find(|mapping| {
            mapping.step_id == manifest.graph.entry_point
                && mapping.purpose == "finish.inputMapping"
        })
        .map(|mapping| mapping.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing Finish input mapping for entry point '{}'",
                manifest.graph.entry_point
            ))
        })
}

fn checked_offset_add(offset: i32, len: usize) -> Result<i32, DirectCompileError> {
    let len = i32::try_from(len).map_err(|_| {
        DirectCompileError::Component(
            "direct workflow static data exceeds i32 address space".into(),
        )
    })?;
    offset.checked_add(len).ok_or_else(|| {
        DirectCompileError::Component("direct workflow static data offset overflow".into())
    })
}

fn align_i32(value: i32, align: i32) -> i32 {
    debug_assert!(align > 0 && (align & (align - 1)) == 0);
    (value + align - 1) & !(align - 1)
}

fn wasm_pages_for_bytes(bytes: i32) -> Result<u64, DirectCompileError> {
    let bytes = u64::try_from(bytes)
        .map_err(|_| DirectCompileError::Component("negative direct memory size".into()))?;
    Ok(bytes.div_ceil(WASM_PAGE_SIZE as u64).max(1))
}

fn emit_direct_core_module(
    resolve: &Resolve,
    world: WorldId,
    config: &DirectCoreConfig,
) -> Result<Vec<u8>, DirectCompileError> {
    let mangling = ManglingAndAbi::Standard32;
    let world = &resolve.worlds[world];

    let mut types = TypeSection::new();
    let mut type_count = 0;
    let mut imports = ImportSection::new();
    let mut imported_function_count = 0;
    let mut import_indices = DirectCoreImportIndices::default();
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();
    let mut next_defined_function = 0;

    for (name, import) in &world.imports {
        match import {
            WorldItem::Function(function) => {
                import_core_function(
                    resolve,
                    mangling,
                    None,
                    function,
                    imported_function_count,
                    &mut types,
                    &mut type_count,
                    &mut imports,
                    &mut import_indices,
                );
                imported_function_count += 1;
            }
            WorldItem::Interface { id, .. } => {
                for function in resolve.interfaces[*id].functions.values() {
                    import_core_function(
                        resolve,
                        mangling,
                        Some(name),
                        function,
                        imported_function_count,
                        &mut types,
                        &mut type_count,
                        &mut imports,
                        &mut import_indices,
                    );
                    imported_function_count += 1;
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let import_indices = import_indices.require_all()?;

    for (name, export) in &world.exports {
        match export {
            WorldItem::Function(function) => {
                export_core_function(
                    resolve,
                    mangling,
                    None,
                    function,
                    &mut types,
                    &mut type_count,
                    &mut functions,
                    &mut exports,
                    &mut code,
                    imported_function_count,
                    &mut next_defined_function,
                    &import_indices,
                    config,
                );
            }
            WorldItem::Interface { id, .. } => {
                for function in resolve.interfaces[*id].functions.values() {
                    export_core_function(
                        resolve,
                        mangling,
                        Some(name),
                        function,
                        &mut types,
                        &mut type_count,
                        &mut functions,
                        &mut exports,
                        &mut code,
                        imported_function_count,
                        &mut next_defined_function,
                        &import_indices,
                        config,
                    );
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: config.static_data.memory_min_pages,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let memory_name = resolve.wasm_export_name(mangling, WasmExport::Memory);
    exports.export(&memory_name, ExportKind::Memory, 0);

    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(config.static_data.heap_base),
    );

    export_realloc(
        resolve,
        mangling,
        &mut types,
        &mut type_count,
        &mut functions,
        &mut exports,
        &mut code,
        imported_function_count,
        &mut next_defined_function,
    );
    export_initialize(
        resolve,
        mangling,
        &mut types,
        &mut type_count,
        &mut functions,
        &mut exports,
        &mut code,
        imported_function_count,
        &mut next_defined_function,
    );

    let mut data = DataSection::new();
    for segment in [
        &config.static_data.manifest,
        &config.static_data.variables,
        &config.static_data.steps,
    ] {
        data.active(
            0,
            &ConstExpr::i32_const(segment.offset),
            segment.data.iter().copied(),
        );
    }

    let mut module = Module::new();
    module.section(&types);
    if !imports.is_empty() {
        module.section(&imports);
    }
    module.section(&functions);
    module.section(&memories);
    module.section(&globals);
    module.section(&exports);
    module.section(&code);
    module.section(&data);
    Ok(module.finish())
}

#[derive(Debug, Default)]
struct DirectCoreImportIndices {
    runtime_load_input: Option<u32>,
    runtime_complete: Option<u32>,
    stdlib_init_manifest: Option<u32>,
    stdlib_build_source: Option<u32>,
    stdlib_apply_mapping: Option<u32>,
}

impl DirectCoreImportIndices {
    fn require_all(self) -> Result<DirectCoreFunctionIndices, DirectCompileError> {
        Ok(DirectCoreFunctionIndices {
            runtime_load_input: require_import(self.runtime_load_input, "runtime.load-input")?,
            runtime_complete: require_import(self.runtime_complete, "runtime.complete")?,
            stdlib_init_manifest: require_import(
                self.stdlib_init_manifest,
                "stdlib.init-manifest",
            )?,
            stdlib_build_source: require_import(self.stdlib_build_source, "stdlib.build-source")?,
            stdlib_apply_mapping: require_import(
                self.stdlib_apply_mapping,
                "stdlib.apply-mapping",
            )?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct DirectCoreFunctionIndices {
    runtime_load_input: u32,
    runtime_complete: u32,
    stdlib_init_manifest: u32,
    stdlib_build_source: u32,
    stdlib_apply_mapping: u32,
}

fn require_import(value: Option<u32>, name: &str) -> Result<u32, DirectCompileError> {
    value.ok_or_else(|| {
        DirectCompileError::Component(format!("missing {name} import in direct world"))
    })
}

#[allow(clippy::too_many_arguments)]
fn import_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_index: u32,
    types: &mut TypeSection,
    type_count: &mut u32,
    imports: &mut ImportSection,
    import_indices: &mut DirectCoreImportIndices,
) {
    let signature = resolve.wasm_signature(mangling.import_variant(), function);
    let type_index = push_core_type(types, type_count, &signature.params, &signature.results);
    let (module, name) = resolve.wasm_import_name(
        mangling,
        WasmImport::Func {
            interface,
            func: function,
        },
    );
    imports.import(&module, &name, EntityType::Function(type_index));

    if is_runtime_import(resolve, interface, function, "load-input") {
        import_indices.runtime_load_input = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "complete") {
        import_indices.runtime_complete = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "init-manifest") {
        import_indices.stdlib_init_manifest = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "build-source") {
        import_indices.stdlib_build_source = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "apply-mapping") {
        import_indices.stdlib_apply_mapping = Some(function_index);
    }
}

#[allow(clippy::too_many_arguments)]
fn export_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
    import_indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
) {
    let signature = resolve.wasm_signature(mangling.export_variant(), function);
    let type_index = push_core_type(types, type_count, &signature.params, &signature.results);
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let export_name = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface,
            func: function,
            kind: WasmExportKind::Normal,
        },
    );
    exports.export(&export_name, ExportKind::Func, function_index);

    let body = if is_wasi_cli_run_export(resolve, interface, function) {
        finish_mapping_run_function(import_indices, config)
    } else {
        zero_return_function(&signature.results)
    };
    code.function(&body);

    let post_return_type = push_core_type(types, type_count, &signature.results, &[]);
    functions.function(post_return_type);
    let post_return_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let post_return_name = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface,
            func: function,
            kind: WasmExportKind::PostReturn,
        },
    );
    exports.export(&post_return_name, ExportKind::Func, post_return_index);

    let mut post_return = WasmFunction::new([]);
    post_return.instruction(&Instruction::End);
    code.function(&post_return);
}

#[allow(clippy::too_many_arguments)]
fn export_realloc(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
) {
    let type_index = push_core_type(
        types,
        type_count,
        &[WasmType::I32, WasmType::I32, WasmType::I32, WasmType::I32],
        &[WasmType::I32],
    );
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let realloc_name = resolve.wasm_export_name(mangling, WasmExport::Realloc);
    exports.export(&realloc_name, ExportKind::Func, function_index);

    let mut body = WasmFunction::new([(3, ValType::I32)]);
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalSet(4));
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(5));
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::MemorySize(0));
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::MemorySize(0));
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE - 1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32DivU);
    body.instruction(&Instruction::MemoryGrow(0));
    body.instruction(&Instruction::Drop);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::GlobalSet(0));
    body.instruction(&Instruction::LocalGet(4));
    body.instruction(&Instruction::End);
    code.function(&body);
}

#[allow(clippy::too_many_arguments)]
fn export_initialize(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
) {
    let type_index = push_core_type(types, type_count, &[], &[]);
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let initialize_name = resolve.wasm_export_name(mangling, WasmExport::Initialize);
    exports.export(&initialize_name, ExportKind::Func, function_index);

    let mut body = WasmFunction::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
}

fn finish_mapping_run_function(
    indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
) -> WasmFunction {
    const DATA_PTR_LOCAL: u32 = 0;
    const DATA_LEN_LOCAL: u32 = 1;
    const SOURCE_PTR_LOCAL: u32 = 2;
    const SOURCE_LEN_LOCAL: u32 = 3;
    const OUTPUT_PTR_LOCAL: u32 = 4;
    const OUTPUT_LEN_LOCAL: u32 = 5;

    let mut body = WasmFunction::new([(6, ValType::I32)]);

    push_segment_args(&mut body, &config.static_data.manifest);
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.stdlib_init_manifest));
    return_if_retptr_error(&mut body);

    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.runtime_load_input));
    return_if_retptr_error(&mut body);
    load_retptr_list(&mut body, DATA_PTR_LOCAL, DATA_LEN_LOCAL);

    body.instruction(&Instruction::LocalGet(DATA_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DATA_LEN_LOCAL));
    push_segment_args(&mut body, &config.static_data.variables);
    push_segment_args(&mut body, &config.static_data.steps);
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.stdlib_build_source));
    return_if_retptr_error(&mut body);
    load_retptr_list(&mut body, SOURCE_PTR_LOCAL, SOURCE_LEN_LOCAL);

    body.instruction(&Instruction::I32Const(config.finish_mapping_id as i32));
    body.instruction(&Instruction::LocalGet(SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(SOURCE_LEN_LOCAL));
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    return_if_retptr_error(&mut body);
    load_retptr_list(&mut body, OUTPUT_PTR_LOCAL, OUTPUT_LEN_LOCAL);

    body.instruction(&Instruction::LocalGet(OUTPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(OUTPUT_LEN_LOCAL));
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.runtime_complete));
    load_retptr_tag(&mut body);
    body.instruction(&Instruction::End);
    body
}

fn push_segment_args(function: &mut WasmFunction, segment: &DirectDataSegment) {
    function.instruction(&Instruction::I32Const(segment.offset));
    function.instruction(&Instruction::I32Const(segment.len_i32()));
}

fn push_retptr_arg(function: &mut WasmFunction) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
}

fn return_if_retptr_error(function: &mut WasmFunction) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
}

fn load_retptr_tag(function: &mut WasmFunction) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
}

fn load_retptr_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(ptr_local));
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(len_local));
}

fn zero_return_function(results: &[WasmType]) -> WasmFunction {
    let mut body = WasmFunction::new([]);
    for result in results {
        push_zero_value(&mut body, result);
    }
    body.instruction(&Instruction::End);
    body
}

fn push_core_type(
    types: &mut TypeSection,
    type_count: &mut u32,
    params: &[WasmType],
    results: &[WasmType],
) -> u32 {
    let index = *type_count;
    *type_count += 1;
    types.ty().function(
        params.iter().map(core_val_type),
        results.iter().map(core_val_type),
    );
    index
}

fn core_val_type(ty: &WasmType) -> ValType {
    match ty {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => ValType::I32,
        WasmType::I64 | WasmType::PointerOrI64 => ValType::I64,
        WasmType::F32 => ValType::F32,
        WasmType::F64 => ValType::F64,
    }
}

fn push_zero_value(function: &mut WasmFunction, ty: &WasmType) {
    match ty {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => {
            function.instruction(&Instruction::I32Const(0));
        }
        WasmType::I64 | WasmType::PointerOrI64 => {
            function.instruction(&Instruction::I64Const(0));
        }
        WasmType::F32 => {
            function.instruction(&Instruction::F32Const(Ieee32::new(0)));
        }
        WasmType::F64 => {
            function.instruction(&Instruction::F64Const(Ieee64::new(0)));
        }
    };
}

fn is_runtime_import(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_name: &str,
) -> bool {
    function.name == function_name
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-runtime/runtime"))
}

fn is_stdlib_import(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_name: &str,
) -> bool {
    function.name == function_name
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-stdlib/json"))
}

fn is_wasi_cli_run_export(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
) -> bool {
    function.name == "run"
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("wasi:cli/run"))
}

fn append_component_custom_section(bytes: &mut Vec<u8>, name: &str, data: &[u8]) {
    let section = CustomSection {
        name: Cow::Borrowed(name),
        data: Cow::Borrowed(data),
    };
    bytes.push(section.id());
    section.encode(bytes);
}

fn component_error(error: impl fmt::Display) -> DirectCompileError {
    DirectCompileError::Component(error.to_string())
}

fn sanitize_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "workflow".to_string()
    } else {
        trimmed.to_string()
    }
}

fn unsupported_summary(unsupported: &[UnsupportedWorkflowFeature]) -> String {
    if unsupported.is_empty() {
        return "no unsupported features reported".to_string();
    }

    unsupported
        .iter()
        .map(|feature| {
            let step = feature.step_id.as_deref().unwrap_or("<graph>");
            format!("{step}:{}", feature.feature)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Stdio;

    use super::*;
    use wasmparser::{
        ComponentExternalKind, Encoding, Operator, Parser, Payload, TypeRef, Validator,
    };

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../../tests/fixtures/conditional_workflow.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
    }

    fn tool_installed(tool: &str) -> bool {
        Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn shared_components_dir() -> Option<PathBuf> {
        let dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("target/wasm32-wasip2/release")
            });
        let missing: Vec<_> = super::super::component::DIRECT_SHARED_COMPONENT_REQUIREMENTS
            .iter()
            .filter_map(|component| {
                let wasm = dir.join(component.bundle_wasm_filename);
                (!wasm.exists()).then_some(wasm)
            })
            .collect();
        if missing.is_empty() {
            Some(dir)
        } else {
            eprintln!(
                "SKIP: direct shared workflow components are not staged: {:?}",
                missing
            );
            None
        }
    }

    fn imported_wit_function<'a>(
        resolve: &'a Resolve,
        world: WorldId,
        interface_prefix: &str,
        function_name: &str,
    ) -> (&'a WorldKey, &'a WitFunction) {
        resolve.worlds[world]
            .imports
            .iter()
            .find_map(|(key, item)| match item {
                WorldItem::Interface { id, .. }
                    if resolve.name_world_key(key).starts_with(interface_prefix) =>
                {
                    Some((key, &resolve.interfaces[*id].functions[function_name]))
                }
                _ => None,
            })
            .expect("imported WIT function")
    }

    #[test]
    fn direct_compile_emits_finish_only_artifact_without_rust_crate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple/workflow".to_string(),
            version: 7,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
        })
        .expect("direct compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        Validator::new()
            .validate_all(&wasm)
            .expect("direct artifact should validate as a Wasm component");

        assert_eq!(result.wasm_path, result.workflow_logic_wasm_path);
        assert_eq!(result.wasm_size, wasm.len());
        assert_eq!(result.workflow_logic_wasm_size, wasm.len());
        assert_eq!(result.wasm_checksum, result.workflow_logic_wasm_checksum);
        assert!(result.wasm_path.ends_with("workflow-logic.wasm"));
        assert!(!result.build_dir.join("workflow.wasm").exists());
        assert!(result.composed_wasm_path.is_none());
        assert!(result.composed_wasm_size.is_none());
        assert!(result.composed_wasm_checksum.is_none());
        assert_eq!(result.manifest_checksum.len(), 64);
        assert!(result.manifest_path.exists());
        assert!(result.support_report_path.exists());
        assert!(result.world_wit_path.exists());
        assert!(result.wac_path.exists());
        assert!(!result.build_dir.join("Cargo.toml").exists());
        assert!(!result.build_dir.join("src/lib.rs").exists());
    }

    #[test]
    fn direct_compile_embeds_manifest_and_support_sections() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
        })
        .expect("direct compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        let mut saw_component_header = false;
        let mut saw_abi = false;
        let mut saw_manifest = false;
        let mut saw_support = false;

        for payload in Parser::new(0).parse_all(&wasm) {
            match payload.expect("wasm payload") {
                Payload::Version { encoding, .. } if !saw_component_header => {
                    assert_eq!(encoding, Encoding::Component);
                    saw_component_header = true;
                }
                Payload::CustomSection(section)
                    if section.name() == DIRECT_WORKFLOW_ABI_SECTION =>
                {
                    let abi: serde_json::Value =
                        serde_json::from_slice(section.data()).expect("abi json");
                    assert_eq!(
                        abi["abiVersion"].as_u64(),
                        Some(u64::from(DIRECT_WORKFLOW_ABI_VERSION))
                    );
                    assert_eq!(abi["artifactKind"], "finish-only-run-component");
                    assert_eq!(abi["componentRunExport"], "wasi:cli/run@0.2.3");
                    assert_eq!(abi["entryPointExecutable"].as_bool(), Some(true));
                    assert_eq!(abi["runtimeExecutable"].as_bool(), Some(true));
                    assert_eq!(abi["finishOutputMode"], "stdlib-apply-mapping");
                    assert_eq!(
                        abi["manifestVersion"].as_u64(),
                        Some(u64::from(DIRECT_WORKFLOW_MANIFEST_VERSION))
                    );
                    saw_abi = true;
                }
                Payload::CustomSection(section)
                    if section.name() == DIRECT_WORKFLOW_MANIFEST_SECTION =>
                {
                    let manifest: DirectWorkflowManifest =
                        serde_json::from_slice(section.data()).expect("manifest json");
                    assert_eq!(manifest.checksum(), result.manifest_checksum);
                    saw_manifest = true;
                }
                Payload::CustomSection(section)
                    if section.name() == DIRECT_WORKFLOW_SUPPORT_SECTION =>
                {
                    let report: DirectWorkflowSupportReport =
                        serde_json::from_slice(section.data()).expect("support json");
                    assert!(report.supported);
                    saw_support = true;
                }
                _ => {}
            }
        }

        assert!(
            saw_component_header,
            "direct artifact should be a component"
        );
        assert!(saw_abi, "direct ABI custom section should exist");
        assert!(saw_manifest, "manifest custom section should exist");
        assert!(saw_support, "support-report custom section should exist");
    }

    #[test]
    fn direct_compile_exports_wasi_cli_run_and_imports_components() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
        })
        .expect("direct compile should succeed");

        let wasm = fs::read(&result.wasm_path).expect("wasm");
        let mut saw_stdlib_import = false;
        let mut saw_runtime_import = false;
        let mut saw_run_export = false;

        for payload in Parser::new(0).parse_all(&wasm) {
            match payload.expect("wasm payload") {
                Payload::ComponentImportSection(reader) => {
                    for import in reader {
                        let import = import.expect("component import");
                        saw_stdlib_import |=
                            import.name.0.contains("runtara:workflow-stdlib/json@0.1.0");
                        saw_runtime_import |= import
                            .name
                            .0
                            .contains("runtara:workflow-runtime/runtime@0.1.0");
                    }
                }
                Payload::ComponentExportSection(reader) => {
                    for export in reader {
                        let export = export.expect("component export");
                        if export.name.0 == "wasi:cli/run@0.2.3" {
                            assert_eq!(export.kind, ComponentExternalKind::Instance);
                            saw_run_export = true;
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(saw_stdlib_import, "stdlib interface import should exist");
        assert!(saw_runtime_import, "runtime interface import should exist");
        assert!(saw_run_export, "wasi:cli/run export should exist");
    }

    #[test]
    fn direct_core_run_lowers_finish_mapping_through_stdlib() {
        let graph = fixture("simple");
        let manifest = build_direct_workflow_manifest(&graph).expect("manifest");
        let manifest_json = manifest.to_canonical_json().expect("manifest json");
        let core_config = DirectCoreConfig::new(&manifest, &manifest_json).expect("core config");
        let variables_json = serde_json::to_vec(&manifest.graph.variables).expect("variables json");

        let (resolve, world) = build_direct_component_resolve().expect("resolve");
        let expected_imports = [
            (
                "runtime.load-input",
                "runtara:workflow-runtime/runtime",
                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                "load-input",
                vec![WasmType::Pointer],
            ),
            (
                "stdlib.init-manifest",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "init-manifest",
                vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
            ),
            (
                "stdlib.build-source",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "build-source",
                vec![
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "stdlib.apply-mapping",
                "runtara:workflow-stdlib/json",
                "cm32p2|runtara:workflow-stdlib/json@0.1",
                "apply-mapping",
                vec![
                    WasmType::I32,
                    WasmType::Pointer,
                    WasmType::Length,
                    WasmType::Pointer,
                ],
            ),
            (
                "runtime.complete",
                "runtara:workflow-runtime/runtime",
                "cm32p2|runtara:workflow-runtime/runtime@0.1",
                "complete",
                vec![WasmType::Pointer, WasmType::Length, WasmType::Pointer],
            ),
        ];

        for (label, interface_prefix, module, name, params) in &expected_imports {
            let (interface_key, function) =
                imported_wit_function(&resolve, world, interface_prefix, name);
            let signature =
                resolve.wasm_signature(ManglingAndAbi::Standard32.import_variant(), function);
            assert_eq!(&signature.params, params, "{label} params");
            assert!(signature.retptr, "{label} should use retptr");
            assert!(signature.results.is_empty(), "{label} has no core results");

            let (actual_module, actual_name) = resolve.wasm_import_name(
                ManglingAndAbi::Standard32,
                WasmImport::Func {
                    interface: Some(interface_key),
                    func: function,
                },
            );
            assert_eq!(actual_module, *module, "{label} module");
            assert_eq!(actual_name, *name, "{label} name");
        }

        let core = emit_direct_core_module(&resolve, world, &core_config).expect("core module");
        Validator::new()
            .validate_all(&core)
            .expect("core module validates");

        let mut next_function_index = 0;
        let mut init_manifest_index = None;
        let mut load_input_index = None;
        let mut build_source_index = None;
        let mut apply_mapping_index = None;
        let mut complete_index = None;
        let mut saw_manifest_data = false;
        let mut saw_variables_data = false;
        let mut saw_steps_data = false;
        let mut saw_mapping_id = false;
        let mut saw_run_retptr_tag_load = false;
        let mut run_calls = Vec::new();
        let mut code_body_index = 0;

        for payload in Parser::new(0).parse_all(&core) {
            match payload.expect("core wasm payload") {
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.expect("core import");
                        if matches!(import.ty, TypeRef::Func(_)) {
                            match (import.module, import.name) {
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "init-manifest") => {
                                    init_manifest_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "load-input") => {
                                    load_input_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "build-source") => {
                                    build_source_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-stdlib/json@0.1", "apply-mapping") => {
                                    apply_mapping_index = Some(next_function_index)
                                }
                                ("cm32p2|runtara:workflow-runtime/runtime@0.1", "complete") => {
                                    complete_index = Some(next_function_index)
                                }
                                _ => {}
                            }
                            next_function_index += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    if code_body_index == 0 {
                        for operator in body.get_operators_reader().expect("operators").into_iter()
                        {
                            match operator.expect("operator") {
                                Operator::Call { function_index } => run_calls.push(function_index),
                                Operator::I32Const { value }
                                    if value == core_config.finish_mapping_id as i32 =>
                                {
                                    saw_mapping_id = true;
                                }
                                Operator::I32Load8U { memarg }
                                    if memarg.offset == 0 && memarg.memory == 0 =>
                                {
                                    saw_run_retptr_tag_load = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    code_body_index += 1;
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.expect("data segment");
                        saw_manifest_data |= data.data == manifest_json;
                        saw_variables_data |= data.data == variables_json;
                        saw_steps_data |= data.data == DIRECT_EMPTY_STEPS_CONTEXT;
                    }
                }
                _ => {}
            }
        }

        let expected_call_order = [
            init_manifest_index.expect("init-manifest import"),
            load_input_index.expect("load-input import"),
            build_source_index.expect("build-source import"),
            apply_mapping_index.expect("apply-mapping import"),
            complete_index.expect("complete import"),
        ];
        assert_eq!(
            run_calls, expected_call_order,
            "run body should lower Finish through stdlib/runtime calls in order"
        );
        assert!(saw_manifest_data, "manifest JSON should be static data");
        assert!(saw_variables_data, "variables JSON should be static data");
        assert!(saw_steps_data, "empty steps context should be static data");
        assert!(saw_mapping_id, "run body should pass manifest mapping id");
        assert!(
            saw_run_retptr_tag_load,
            "run body should return runtime.complete result tag"
        );
    }

    #[test]
    fn direct_compile_writes_component_scaffold_sidecars() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
        })
        .expect("direct compile should succeed");

        let world_wit = fs::read_to_string(&result.world_wit_path).expect("world wit");
        let wac = fs::read_to_string(&result.wac_path).expect("wac");

        assert_eq!(world_wit, result.component_artifacts.world_wit);
        assert_eq!(wac, result.component_artifacts.wac_source);
        assert!(world_wit.contains("import runtara:workflow-stdlib/json@0.1.0;"));
        assert!(world_wit.contains("import runtara:workflow-runtime/runtime@0.1.0;"));
        assert!(world_wit.contains("export wasi:cli/run@0.2.3;"));
        assert!(wac.contains("new runtara:workflow-stdlib"));
        assert!(wac.contains("new runtara:workflow-runtime"));
        assert!(wac.contains("new runtara:workflow-logic"));
        assert!(wac.contains("export wf...;"));
    }

    #[test]
    fn direct_compile_composes_finish_with_shared_components_when_available() {
        if !tool_installed("wac") {
            eprintln!("SKIP: wac not installed. `cargo install wac-cli --locked` first.");
            return;
        }
        let Some(components_dir) = shared_components_dir() else {
            return;
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let mut result = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "simple".to_string(),
            version: 1,
            execution_graph: fixture("simple"),
            output_dir: temp.path().to_path_buf(),
        })
        .expect("direct compile should succeed");

        let composed = compose_direct_workflow(&mut result, &components_dir)
            .expect("direct workflow composition should succeed");
        let wasm = fs::read(&composed).expect("composed wasm");
        assert!(!wasm.is_empty());
        assert_eq!(composed, result.build_dir.join("workflow.wasm"));
        assert_eq!(result.wasm_path, composed);
        assert_eq!(
            result.composed_wasm_path.as_deref(),
            Some(composed.as_path())
        );
        assert_eq!(result.wasm_size, wasm.len());
        assert_eq!(result.composed_wasm_size, Some(wasm.len()));
        assert_eq!(
            result.composed_wasm_checksum.as_deref(),
            Some(result.wasm_checksum.as_str())
        );
        assert_eq!(
            result.workflow_logic_wasm_path,
            result.build_dir.join("workflow-logic.wasm")
        );
        assert!(result.workflow_logic_wasm_path.exists());
        Validator::new()
            .validate_all(&wasm)
            .expect("composed direct workflow should validate");
    }

    #[test]
    fn direct_compile_composed_returns_final_workflow_wasm_when_available() {
        if !tool_installed("wac") {
            eprintln!("SKIP: wac not installed. `cargo install wac-cli --locked` first.");
            return;
        }
        let Some(components_dir) = shared_components_dir() else {
            return;
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let result = compile_direct_workflow_composed(
            DirectCompilationInput {
                workflow_id: "simple".to_string(),
                version: 1,
                execution_graph: fixture("simple"),
                output_dir: temp.path().to_path_buf(),
            },
            &components_dir,
        )
        .expect("direct composed compile should succeed");

        assert_eq!(result.wasm_path, result.build_dir.join("workflow.wasm"));
        assert_eq!(
            result.workflow_logic_wasm_path,
            result.build_dir.join("workflow-logic.wasm")
        );
        assert_eq!(
            result.composed_wasm_path.as_deref(),
            Some(result.wasm_path.as_path())
        );
        assert!(result.wasm_path.exists());
        assert!(result.workflow_logic_wasm_path.exists());

        let wasm = fs::read(&result.wasm_path).expect("composed wasm");
        assert_eq!(result.wasm_size, wasm.len());
        Validator::new()
            .validate_all(&wasm)
            .expect("composed direct workflow should validate");
    }

    #[test]
    fn direct_compile_rejects_unsupported_graphs_before_writing_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = compile_direct_workflow(DirectCompilationInput {
            workflow_id: "conditional".to_string(),
            version: 1,
            execution_graph: fixture("conditional"),
            output_dir: temp.path().to_path_buf(),
        })
        .expect_err("conditional is not supported yet");

        let DirectCompileError::Unsupported { report } = err else {
            panic!("expected unsupported error");
        };
        assert!(!report.supported);
        assert!(
            report
                .unsupported
                .iter()
                .any(|feature| feature.step_id.as_deref() == Some("check")
                    && feature.feature == "conditional")
        );
        assert!(
            fs::read_dir(temp.path())
                .expect("temp dir")
                .next()
                .is_none(),
            "unsupported graphs should not create build output"
        );
    }
}
