// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in direct workflow compilation entry point.
//!
//! This is the first production-shaped entry point, not the PoC ABI. It emits
//! a deterministic artifact envelope for finish-only graphs and writes the
//! manifest/support sidecars that later component-model work will consume.

use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use runtara_dsl::ExecutionGraph;
use runtara_workflow_wit::{RUNTIME_WIT, STDLIB_WIT, WORKFLOW_WIT_VERSION};
use sha2::{Digest, Sha256};
use wasm_encoder::{
    CodeSection, CustomSection, Encode, EntityType, ExportKind, ExportSection,
    Function as WasmFunction, FunctionSection, Ieee32, Ieee64, ImportSection, Instruction,
    MemorySection, MemoryType, Module, Section, TypeSection, ValType,
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
    /// Path to the emitted `workflow.wasm` artifact.
    pub wasm_path: PathBuf,
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
    /// Size of the emitted Wasm artifact in bytes.
    pub wasm_size: usize,
    /// SHA-256 checksum of the emitted Wasm artifact.
    pub wasm_checksum: String,
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

/// Compile a finish-only workflow through the direct path.
///
/// This does not replace [`crate::compile_workflow`]. It is intentionally
/// opt-in and currently supports only graphs accepted by
/// [`analyze_direct_wasm_support`]. The emitted component-format artifact is a
/// stable metadata envelope for the direct pipeline; executable component-model
/// runtime dispatch will replace this envelope in the next phases.
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

    let wasm_path = build_dir.join("workflow.wasm");
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
        manifest_path,
        support_report_path,
        world_wit_path,
        wac_path,
        build_dir,
        wasm_size: wasm.len(),
        wasm_checksum: sha256_hex(&wasm),
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
        "runtimeExecutable": false,
        "manifestVersion": DIRECT_WORKFLOW_MANIFEST_VERSION,
        "stepCount": manifest.feature_summary.total_steps,
        "note": "direct compiler component with canonical run export; runtime Finish completion comes next"
    }))?;

    let mut component = emit_finish_only_component()?;
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

fn emit_finish_only_component() -> Result<Vec<u8>, DirectCompileError> {
    let (resolve, world) = build_direct_component_resolve()?;
    let mut core_module = emit_direct_core_module(&resolve, world);
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

fn emit_direct_core_module(resolve: &Resolve, world: WorldId) -> Vec<u8> {
    let mangling = ManglingAndAbi::Standard32;
    let world = &resolve.worlds[world];

    let mut types = TypeSection::new();
    let mut type_count = 0;
    let mut imports = ImportSection::new();
    let mut imported_function_count = 0;
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
                    &mut types,
                    &mut type_count,
                    &mut imports,
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
                        &mut types,
                        &mut type_count,
                        &mut imports,
                    );
                    imported_function_count += 1;
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

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
                    );
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 0,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let memory_name = resolve.wasm_export_name(mangling, WasmExport::Memory);
    exports.export(&memory_name, ExportKind::Memory, 0);

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

    let mut module = Module::new();
    module.section(&types);
    if !imports.is_empty() {
        module.section(&imports);
    }
    module.section(&functions);
    module.section(&memories);
    module.section(&exports);
    module.section(&code);
    module.finish()
}

fn import_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    types: &mut TypeSection,
    type_count: &mut u32,
    imports: &mut ImportSection,
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

    let mut body = WasmFunction::new([]);
    for result in &signature.results {
        push_zero_value(&mut body, result);
    }
    body.instruction(&Instruction::End);
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

    let mut body = WasmFunction::new([]);
    body.instruction(&Instruction::I32Const(0));
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

    use super::*;
    use wasmparser::{ComponentExternalKind, Encoding, Parser, Payload, Validator};

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../../tests/fixtures/conditional_workflow.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
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

        assert_eq!(result.wasm_size, wasm.len());
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
                    assert_eq!(abi["runtimeExecutable"].as_bool(), Some(false));
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
