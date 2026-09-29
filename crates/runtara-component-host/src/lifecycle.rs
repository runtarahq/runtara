// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side mirror of the `runtara:agent/types` outcome every workflow entry
//! returns: a compiled workflow exports `runtara:agent-<id>/capabilities` and
//! answers `invoke("run", input)` like an agent.
//!
//! Field order and kebab names must match the WIT exactly; wasmtime
//! type-checks them against the component's export when the typed function is
//! looked up.

use std::path::Path;

/// Fully-qualified component export name of a top-level workflow's entry —
/// re-exported from the canonical WIT crate so the host and the compiler
/// cannot drift apart.
pub use runtara_wit::workflow::ENTRY as ENTRY_INTERFACE_NAME;
/// The capability id a workflow entry answers.
pub use runtara_wit::workflow::ENTRY_CAPABILITY;

/// WIT mirror of `runtara:agent/types.error-info`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
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
    /// The workflow's full structured error (JSON), persisted verbatim.
    pub details: Option<String>,
}

/// WIT mirror of `runtara:agent/types.signal-wait`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(record)]
pub struct SignalWait {
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: String,
    #[component(name = "deadline-ms")]
    pub deadline_ms: Option<u64>,
}

/// WIT mirror of `runtara:agent/types.wake`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
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
    /// Re-invoke when the host-owned instance wait with this id settles.
    #[component(name = "instances")]
    Instances(String),
}

/// WIT mirror of `runtara:agent/types.suspension`. A workflow's `state` is
/// always empty (its state lives in checkpoints); an agent's is its
/// continuation.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(record)]
pub struct WorkflowSuspension {
    pub wakes: Vec<WorkflowWake>,
    pub state: Vec<u8>,
}

/// WIT mirror of `runtara:agent/types.outcome` — the invoke success arm.
/// `suspended` carries a wake-SET (re-invoke on ANY).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum WorkflowOutcome {
    #[component(name = "completed")]
    Completed(Vec<u8>),
    #[component(name = "suspended")]
    Suspended(WorkflowSuspension),
}

/// The top-level execution export discovered in a workflow component.
///
/// An agent component does not export the workflow entry and remains valid in
/// the component dispatcher; only launches and registrations of workflows
/// require [`LifecycleInvoke`](Self::LifecycleInvoke).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowEntrypoint {
    /// The component exports the workflow entry's `capabilities.invoke`.
    LifecycleInvoke,
    /// The workflow entry is not exported.
    Other,
}

/// Inspect the *actual* top-level exports of a component without instantiating
/// it. This is intentionally cheap enough to run before a queued Environment
/// launch takes a runner permit.
///
/// A component can contain nested components with their own exports, so only
/// depth-zero sections count: a nested workflow entry does not make the final
/// artifact a workflow.
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

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentExportSection(reader) if depth == 0 => {
                for export in reader {
                    let export = export?;
                    if export.name.0 == ENTRY_INTERFACE_NAME {
                        lifecycle = true;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(if lifecycle {
        WorkflowEntrypoint::LifecycleInvoke
    } else {
        WorkflowEntrypoint::Other
    })
}

/// Require the workflow entry in the component at `path`, before registration
/// or launch.
pub fn require_workflow_entry_file(path: impl AsRef<Path>) -> anyhow::Result<()> {
    match inspect_workflow_entrypoint_file(path)? {
        WorkflowEntrypoint::LifecycleInvoke => Ok(()),
        WorkflowEntrypoint::Other => Err(anyhow::anyhow!(
            "compiled workflow does not export the workflow entry `{ENTRY_INTERFACE_NAME}`; \
             rebuild or republish this workflow"
        )),
    }
}

/// True when the compiled component exports the workflow entry. Workflow
/// preparation refuses a component for which this is false.
pub fn component_exports_workflow_entry(
    component: &wasmtime::component::Component,
    engine: &wasmtime::Engine,
) -> bool {
    component
        .component_type()
        .exports(engine)
        .any(|(name, _)| name == ENTRY_INTERFACE_NAME)
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
            (export "runtara:agent-workflow-agent/capabilities@1.0.0" (instance $lifecycle))
        )
    "#;

    #[test]
    fn recognizes_the_workflow_entry_from_actual_component_exports() {
        let wasm = wat::parse_str(INVOKE_COMPONENT).expect("valid component fixture");
        assert_eq!(
            inspect_workflow_entrypoint(&wasm).expect("inspect component"),
            WorkflowEntrypoint::LifecycleInvoke
        );
        let fixture = write_fixture(&wasm);
        require_workflow_entry_file(fixture.path()).expect("invoke component accepted");
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
        let fixture = write_fixture(&wasm);
        let error = require_workflow_entry_file(fixture.path())
            .expect_err("a component without the workflow entry must be rejected");
        assert!(
            error
                .to_string()
                .contains("does not export the workflow entry")
        );
    }

    fn write_fixture(wasm: &[u8]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("create component fixture");
        std::fs::write(file.path(), wasm).expect("write component fixture");
        file
    }
}
