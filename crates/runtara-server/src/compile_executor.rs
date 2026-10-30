// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where a workflow is compiled (M7.15): in this process, by default, or by
//! an executor a deployment sets, which compiles elsewhere, so a tenant's
//! workflow is never compiled in a process that serves tenants.
//!
//! An executor is handed exactly what the in-process compile would be, and
//! answers as it would: the build in `options.output_dir`, or the error
//! (`runtara_workflows::compile_request` turns a compile elsewhere's outcome
//! and failure back into both).

use std::sync::{Arc, OnceLock};

use runtara_workflows::{CompilationInput, DirectWorkflowCompileOptions, NativeCompilationResult};

/// Compiles a workflow. Called on a blocking thread.
pub trait CompileExecutor: Send + Sync {
    /// Compiles `input` with `options`, as
    /// [`runtara_workflows::compile_workflow_direct`] does.
    fn compile(
        &self,
        input: CompilationInput,
        options: DirectWorkflowCompileOptions,
    ) -> std::io::Result<NativeCompilationResult>;
}

static EXECUTOR: OnceLock<Arc<dyn CompileExecutor>> = OnceLock::new();

/// Compile workflows with `executor` from now on. Set once, at boot.
pub fn set_compile_executor(executor: Arc<dyn CompileExecutor>) {
    let _ = EXECUTOR.set(executor);
}

/// Compiles `input` with `options` where this process compiles.
pub(crate) fn compile(
    input: CompilationInput,
    options: DirectWorkflowCompileOptions,
) -> std::io::Result<NativeCompilationResult> {
    compile_with(EXECUTOR.get().map(Arc::as_ref), input, options)
}

fn compile_with(
    executor: Option<&dyn CompileExecutor>,
    input: CompilationInput,
    options: DirectWorkflowCompileOptions,
) -> std::io::Result<NativeCompilationResult> {
    match executor {
        Some(executor) => executor.compile(input, options),
        None => runtara_workflows::compile_workflow_direct(input, options),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Elsewhere(Mutex<Vec<(String, std::path::PathBuf)>>);

    impl CompileExecutor for Elsewhere {
        fn compile(
            &self,
            input: CompilationInput,
            options: DirectWorkflowCompileOptions,
        ) -> std::io::Result<NativeCompilationResult> {
            self.0
                .lock()
                .unwrap()
                .push((input.workflow_id, options.output_dir));
            Err(std::io::Error::other("compiled elsewhere"))
        }
    }

    #[test]
    fn a_set_executor_compiles_instead_of_this_process() {
        let input = || CompilationInput {
            tenant_id: "acme".into(),
            workflow_id: "wf".into(),
            version: 1,
            execution_graph: serde_json::from_value(serde_json::json!({
                "entryPoint": "finish",
                "steps": {"finish": {"id": "finish", "stepType": "Finish"}},
                "executionPlan": []
            }))
            .unwrap(),
            track_events: false,
            child_workflows: vec![],
            agent_catalog: None,
            progress_callback: None,
            agent_slug: None,
        };
        let output = tempfile::tempdir().unwrap();
        let options = || DirectWorkflowCompileOptions {
            output_dir: output.path().to_path_buf(),
            components_dir: output.path().join("no-components"),
            extra_component_dirs: vec![],
            source_checksum: None,
        };
        let elsewhere = Elsewhere(Mutex::default());
        let error = compile_with(Some(&elsewhere), input(), options()).unwrap_err();
        assert_eq!(error.to_string(), "compiled elsewhere");
        assert_eq!(
            *elsewhere.0.lock().unwrap(),
            [("wf".to_string(), output.path().to_path_buf())]
        );
        // Without one, this process compiles (and finds no components here).
        let error = compile_with(None, input(), options()).unwrap_err();
        assert_ne!(error.to_string(), "compiled elsewhere");
    }
}
