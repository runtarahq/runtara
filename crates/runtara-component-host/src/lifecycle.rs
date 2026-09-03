// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side mirror of `runtara:workflow-lifecycle/lifecycle` — the unified
//! invoke export a workflow compiled with the invoke ABI exposes instead of
//! `wasi:cli/run` (Phase 3 of the agent/workflow unification).
//!
//! Field order and kebab names must match the WIT exactly; wasmtime
//! type-checks them against the component's export when the typed function is
//! looked up.

use std::path::Path;

/// Fully-qualified component export name of the lifecycle interface —
/// re-exported from the canonical WIT crate so the host and the compiler
/// cannot drift apart.
pub use runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME;

/// WIT mirror of `lifecycle.error-info` (field-for-field the agent error).
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(record)]
pub struct WorkflowErrorInfo {
    pub code: String,
    pub message: String,
    pub category: String,
    pub severity: String,
    pub retryable: bool,
    #[component(name = "retry-after-ms")]
    pub retry_after_ms: Option<u64>,
    pub attributes: Option<String>,
}

/// WIT mirror of `lifecycle.signal-wait`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(record)]
pub struct SignalWait {
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: String,
    #[component(name = "deadline-ms")]
    pub deadline_ms: Option<u64>,
}

/// WIT mirror of `lifecycle.wake`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(variant)]
pub enum WorkflowWake {
    /// Re-invoke at (or after) this wall-clock ms-since-epoch.
    #[component(name = "at")]
    At(u64),
    /// Re-invoke when the signal arrives, or at its deadline.
    #[component(name = "on-signal")]
    OnSignal(SignalWait),
    /// Lifecycle pause/drain: re-invoke on relaunch.
    #[component(name = "on-resume")]
    OnResume,
}

/// WIT mirror of `lifecycle.outcome` — the invoke success arm. `suspended`
/// carries a wake-SET (re-invoke on ANY; sequential lowering emits
/// singletons).
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(variant)]
pub enum WorkflowOutcome {
    #[component(name = "completed")]
    Completed(Vec<u8>),
    #[component(name = "suspended")]
    Suspended(Vec<WorkflowWake>),
}

/// The top-level execution export discovered in a workflow component.
///
/// This deliberately describes only workflow entrypoints. A generic agent can
/// have neither export and remains valid in the component dispatcher; callers
/// that require [`LifecycleInvoke`](Self::LifecycleInvoke) decide that from
/// their image kind rather than treating all components as workflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowEntrypoint {
    /// The component exports `runtara:workflow-lifecycle/lifecycle.invoke`.
    LifecycleInvoke,
    /// The component exports the retired direct-workflow `wasi:cli/run` entry.
    LegacyCliRun,
    /// Neither known workflow entrypoint is exported.
    Other,
}

/// Inspect the *actual* top-level exports of a component without instantiating
/// it. This is intentionally cheap enough to run before a queued Environment
/// launch takes a runner permit.
///
/// A component can contain nested components with their own exports, so only
/// depth-zero sections count. A nested legacy component inside a correctly
/// composed invoke artifact does not make the final artifact legacy.
pub fn inspect_workflow_entrypoint_file(
    path: impl AsRef<Path>,
) -> anyhow::Result<WorkflowEntrypoint> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|error| anyhow::anyhow!("read workflow component {}: {error}", path.display()))?;
    inspect_workflow_entrypoint(&bytes)
        .map_err(|error| anyhow::anyhow!("inspect workflow component {}: {error}", path.display()))
}

/// Byte-slice variant of [`inspect_workflow_entrypoint_file`], useful for
/// registration tests and callers that already have the artifact in memory.
pub fn inspect_workflow_entrypoint(wasm: &[u8]) -> anyhow::Result<WorkflowEntrypoint> {
    let mut depth = 0usize;
    let mut lifecycle = false;
    let mut cli_run = false;

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentExportSection(reader) if depth == 0 => {
                for export in reader {
                    let export = export?;
                    let name = export.name.0;
                    if name == LIFECYCLE_INTERFACE_NAME
                        || name == runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME_V1
                    {
                        lifecycle = true;
                    }
                    if name == "wasi:cli/run@0.2.3" || name.starts_with("wasi:cli/run@") {
                        cli_run = true;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(if lifecycle {
        WorkflowEntrypoint::LifecycleInvoke
    } else if cli_run {
        WorkflowEntrypoint::LegacyCliRun
    } else {
        WorkflowEntrypoint::Other
    })
}

/// Require the current direct-workflow entrypoint before registration. This is
/// intentionally separate from [`inspect_workflow_entrypoint_file`]: launch
/// paths use the classifier to report a useful legacy-artifact error, whereas
/// registration must reject everything that is not invoke-shaped.
pub fn require_lifecycle_invoke_file(path: impl AsRef<Path>) -> anyhow::Result<()> {
    match inspect_workflow_entrypoint_file(path)? {
        WorkflowEntrypoint::LifecycleInvoke => Ok(()),
        WorkflowEntrypoint::LegacyCliRun => Err(anyhow::anyhow!(
            "unsupported_legacy_abi: compiled workflows must export lifecycle.invoke; rebuild or republish this workflow"
        )),
        WorkflowEntrypoint::Other => Err(anyhow::anyhow!(
            "compiled workflow does not export lifecycle.invoke"
        )),
    }
}

/// True when the loaded component exports the lifecycle interface — i.e. it
/// is an invoke-shaped artifact that must run through
/// [`crate::workflow::WorkflowExecutor::execute_invoke`] rather than the
/// legacy `wasi:cli/run` path. The runner's dual-ABI dispatch keys off this.
pub fn exports_lifecycle_invoke(
    pre: &wasmtime::component::InstancePre<crate::workflow::WorkflowState>,
    engine: &wasmtime::Engine,
) -> bool {
    pre.component()
        .component_type()
        .exports(engine)
        .any(|(name, _)| {
            name == LIFECYCLE_INTERFACE_NAME
                || name == runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME_V1
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVOKE_COMPONENT: &str = r#"
        (component
            (core module $m (func (export "invoke")))
            (core instance $i (instantiate $m))
            (func $invoke (canon lift (core func $i "invoke")))
            (instance $lifecycle (export "invoke" (func $invoke)))
            (export "runtara:workflow-lifecycle/lifecycle@0.2.0" (instance $lifecycle))
        )
    "#;

    const LEGACY_COMPONENT: &str = r#"
        (component
            (core module $m
                (func (export "run") (result i32) (i32.const 0))
            )
            (core instance $i (instantiate $m))
            (func $run (result (result)) (canon lift (core func $i "run")))
            (instance $run-interface (export "run" (func $run)))
            (export "wasi:cli/run@0.2.3" (instance $run-interface))
        )
    "#;

    #[test]
    fn recognizes_lifecycle_invoke_from_actual_component_exports() {
        let wasm = wat::parse_str(INVOKE_COMPONENT).expect("valid component fixture");
        assert_eq!(
            inspect_workflow_entrypoint(&wasm).expect("inspect component"),
            WorkflowEntrypoint::LifecycleInvoke
        );
        let fixture = write_fixture(&wasm);
        require_lifecycle_invoke_file(fixture.path()).expect("invoke component accepted");
    }

    #[test]
    fn rejects_legacy_cli_run_from_actual_component_exports() {
        let wasm = wat::parse_str(LEGACY_COMPONENT).expect("valid component fixture");
        assert_eq!(
            inspect_workflow_entrypoint(&wasm).expect("inspect component"),
            WorkflowEntrypoint::LegacyCliRun
        );
        let fixture = write_fixture(&wasm);
        let error = require_lifecycle_invoke_file(fixture.path())
            .expect_err("legacy component must be rejected");
        assert!(error.to_string().contains("unsupported_legacy_abi"));
    }

    #[test]
    fn unrelated_component_is_not_misclassified_as_a_workflow() {
        let wasm = wat::parse_str(
            r#"(component
                (core module $m (func (export "run")))
                (core instance $i (instantiate $m))
                (func $run (canon lift (core func $i "run")))
                (export "run" (func $run))
            )"#,
        )
        .expect("valid component fixture");
        assert_eq!(
            inspect_workflow_entrypoint(&wasm).expect("inspect component"),
            WorkflowEntrypoint::Other
        );
    }

    fn write_fixture(wasm: &[u8]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("create component fixture");
        std::fs::write(file.path(), wasm).expect("write component fixture");
        file
    }
}
