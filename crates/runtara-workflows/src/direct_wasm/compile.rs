// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in direct workflow compilation entry point.
//!
//! This is the first production-shaped entry point, not the PoC ABI. It emits
//! a deterministic artifact envelope for finish-only graphs and writes the
//! manifest/support sidecars that later component-model work will consume.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use runtara_dsl::ExecutionGraph;
use sha2::{Digest, Sha256};
use wasm_encoder::{
    CodeSection, CustomSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
    Module, TypeSection, ValType,
};

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

const TYPE_I32_RESULT: u32 = 0;
const FUNC_ABI_VERSION: u32 = 0;
const FUNC_MANIFEST_VERSION: u32 = 1;
const FUNC_STEP_COUNT: u32 = 2;

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
/// [`analyze_direct_wasm_support`]. The emitted core Wasm artifact is a stable
/// metadata envelope for the direct pipeline; component-model runtime execution
/// will replace this envelope in the next phases.
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

    let build_dir = input.output_dir.join(format!(
        "{}-v{}-direct",
        sanitize_path_segment(&input.workflow_id),
        input.version
    ));
    fs::create_dir_all(&build_dir)?;

    let wasm_path = build_dir.join("workflow.wasm");
    let manifest_path = build_dir.join("manifest.json");
    let support_report_path = build_dir.join("support-report.json");

    fs::write(&wasm_path, &wasm)?;
    fs::write(&manifest_path, &manifest_json)?;
    fs::write(&support_report_path, &support_json)?;

    Ok(DirectCompilationResult {
        wasm_path,
        manifest_path,
        support_report_path,
        build_dir,
        wasm_size: wasm.len(),
        wasm_checksum: sha256_hex(&wasm),
        manifest_checksum: manifest.checksum().to_string(),
        support_report,
    })
}

fn emit_finish_only_artifact(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
    support_json: &[u8],
) -> Result<Vec<u8>, DirectCompileError> {
    let abi_json = serde_json::to_vec(&serde_json::json!({
        "abiVersion": DIRECT_WORKFLOW_ABI_VERSION,
        "artifactKind": "finish-only-core-envelope",
        "runtimeExecutable": false,
        "note": "temporary direct compiler artifact envelope; component-model execution comes next"
    }))?;

    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);

    let mut functions = FunctionSection::new();
    functions.function(TYPE_I32_RESULT);
    functions.function(TYPE_I32_RESULT);
    functions.function(TYPE_I32_RESULT);

    let mut exports = ExportSection::new();
    exports.export(
        "__runtara_direct_abi_version",
        ExportKind::Func,
        FUNC_ABI_VERSION,
    );
    exports.export(
        "__runtara_direct_manifest_version",
        ExportKind::Func,
        FUNC_MANIFEST_VERSION,
    );
    exports.export(
        "__runtara_direct_step_count",
        ExportKind::Func,
        FUNC_STEP_COUNT,
    );

    let mut code = CodeSection::new();
    code.function(&const_i32_function(DIRECT_WORKFLOW_ABI_VERSION as i32));
    code.function(&const_i32_function(DIRECT_WORKFLOW_MANIFEST_VERSION as i32));
    code.function(&const_i32_function(
        manifest.feature_summary.total_steps as i32,
    ));

    module.section(&types);
    module.section(&functions);
    module.section(&exports);
    module.section(&code);
    module.section(&CustomSection {
        name: DIRECT_WORKFLOW_ABI_SECTION.into(),
        data: abi_json.into(),
    });
    module.section(&CustomSection {
        name: DIRECT_WORKFLOW_MANIFEST_SECTION.into(),
        data: manifest_json.into(),
    });
    module.section(&CustomSection {
        name: DIRECT_WORKFLOW_SUPPORT_SECTION.into(),
        data: support_json.into(),
    });

    Ok(module.finish())
}

fn const_i32_function(value: i32) -> Function {
    let mut function = Function::new([]);
    function.instruction(&Instruction::I32Const(value));
    function.instruction(&Instruction::End);
    function
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
    use wasmparser::{ExternalKind, Parser, Payload, Validator};

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
            .expect("direct artifact should validate as core Wasm");

        assert_eq!(result.wasm_size, wasm.len());
        assert_eq!(result.manifest_checksum.len(), 64);
        assert!(result.manifest_path.exists());
        assert!(result.support_report_path.exists());
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
        let mut saw_manifest = false;
        let mut saw_support = false;
        let mut saw_export = false;

        for payload in Parser::new(0).parse_all(&wasm) {
            match payload.expect("wasm payload") {
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.expect("export");
                        if export.name == "__runtara_direct_abi_version" {
                            assert_eq!(export.kind, ExternalKind::Func);
                            saw_export = true;
                        }
                    }
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

        assert!(saw_export, "direct ABI version export should exist");
        assert!(saw_manifest, "manifest custom section should exist");
        assert!(saw_support, "support-report custom section should exist");
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
