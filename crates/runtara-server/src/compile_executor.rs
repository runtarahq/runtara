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

use std::sync::{Arc, OnceLock, RwLock};

use runtara_workflows::direct_wasm::DirectStdlibMode;
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

    /// Compiles `input`, `tenant`'s workflow, to publish it as an agent, with
    /// the components in `components_dir` and `extra_component_dirs`,
    /// reaching the stdlib as `stdlib` says, as its publish does in process.
    fn compile_published_agent(
        &self,
        tenant: &str,
        input: runtara_workflows::direct_wasm::DirectCompilationInput,
        components_dir: &std::path::Path,
        extra_component_dirs: &[std::path::PathBuf],
        stdlib: DirectStdlibMode,
    ) -> std::io::Result<runtara_workflows::compile_request::PublishedAgent>;
}

/// What a compiler outside this process's trust is shown: the files a
/// compile may read, as directories (for one that reads them where it is
/// shown them) and each by its SHA-256 (for one that asks for them).
pub(crate) struct Shown<'a> {
    pub components_dir: &'a std::path::Path,
    pub extra_component_dirs: &'a [std::path::PathBuf],
    pub by_digest: std::collections::BTreeMap<String, std::path::PathBuf>,
}

/// A compiler that is not this process: a sandbox in it, or a guest placed
/// elsewhere. It is handed a request and answers what the compiler said and
/// the component it handed back. Neither is taken on its word
/// ([`Untrusted`]).
pub(crate) trait Compiler: Send + Sync {
    fn compile(
        &self,
        tenant: &str,
        request: &runtara_workflows::compile_request::CompileRequest,
        shown: &Shown<'_>,
    ) -> std::io::Result<(runtara_workflows::compile_request::GuestOutcome, Vec<u8>)>;
}

/// A [`Compiler`] as where this process compiles: makes the request, and
/// applies what came back only if the component is the one the outcome
/// names. What the engine records of the build it checks or derives itself
/// afterwards (`api::services::compilation`), for any executor.
pub(crate) struct Untrusted<C>(pub C);

/// The component a compile handed back, if it is the one its outcome names.
pub(crate) fn made_as_named(named: &str, made: Vec<u8>) -> std::io::Result<Vec<u8>> {
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(&made)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != named {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the compiler handed back another component than its outcome names",
        ));
    }
    Ok(made)
}

impl<C: Compiler> CompileExecutor for Untrusted<C> {
    fn compile(
        &self,
        input: CompilationInput,
        options: DirectWorkflowCompileOptions,
    ) -> std::io::Result<NativeCompilationResult> {
        use runtara_workflows::compile_request::{self, GuestOutcome};
        let (request, by_digest) = compile_request::request(&input, &options)?;
        let (outcome, made) = self.0.compile(
            &input.tenant_id,
            &request,
            &Shown {
                components_dir: &options.components_dir,
                extra_component_dirs: &options.extra_component_dirs,
                by_digest,
            },
        )?;
        match outcome {
            GuestOutcome::Compiled {
                outcome,
                workflow_wasm,
            } => compile_request::apply(
                *outcome,
                &made_as_named(&workflow_wasm, made)?,
                &input.workflow_id,
                input.version,
                &options,
            ),
            GuestOutcome::Failed(failure) => Err(compile_request::apply_failure(failure, &options)),
        }
    }

    fn compile_published_agent(
        &self,
        tenant: &str,
        input: runtara_workflows::direct_wasm::DirectCompilationInput,
        components_dir: &std::path::Path,
        extra_component_dirs: &[std::path::PathBuf],
        stdlib: DirectStdlibMode,
    ) -> std::io::Result<runtara_workflows::compile_request::PublishedAgent> {
        use runtara_workflows::compile_request::{self, GuestOutcome};
        let (request, by_digest) = compile_request::request_published_agent(
            tenant,
            &input,
            components_dir,
            extra_component_dirs,
            stdlib,
        )?;
        let (outcome, made) = self.0.compile(
            tenant,
            &request,
            &Shown {
                components_dir,
                extra_component_dirs,
                by_digest,
            },
        )?;
        match outcome {
            GuestOutcome::Compiled {
                outcome,
                workflow_wasm,
            } => compile_request::apply_published_agent(
                *outcome,
                &made_as_named(&workflow_wasm, made)?,
                &input.workflow_id,
                input.version,
                &input.output_dir,
            ),
            GuestOutcome::Failed(failure) => {
                // The paths a failure names are the compiler's: name the
                // host's, as a workflow's compile does.
                let options = DirectWorkflowCompileOptions {
                    output_dir: input.output_dir.clone(),
                    components_dir: components_dir.to_path_buf(),
                    extra_component_dirs: extra_component_dirs.to_vec(),
                    source_checksum: input.source_checksum.clone(),
                    stdlib: Some(stdlib),
                };
                Err(compile_request::apply_failure(failure, &options))
            }
        }
    }
}

static EXECUTOR: OnceLock<Arc<dyn CompileExecutor>> = OnceLock::new();

/// Compile workflows with `executor` from now on. Set once, at boot.
pub fn set_compile_executor(executor: Arc<dyn CompileExecutor>) {
    let _ = EXECUTOR.set(executor);
}

static WORKFLOW_STDLIB: RwLock<Option<DirectStdlibMode>> = RwLock::new(None);

/// Compile workflows for a runner that does or does not link the stdlib into
/// a run ([`runtara_environment::runner::Runner::links_workflow_stdlib`]).
/// Set at boot, from the runner.
pub(crate) fn set_runner_links_stdlib(links: bool) -> DirectStdlibMode {
    let mode = stdlib_for_runner(links);
    *WORKFLOW_STDLIB.write().expect("workflow stdlib") = Some(mode);
    mode
}

fn stdlib_for_runner(links: bool) -> DirectStdlibMode {
    if links {
        DirectStdlibMode::current()
    } else {
        DirectStdlibMode::Fused
    }
}

/// How this process compiles workflows to reach the stdlib: as its runner
/// runs them, or the process's default before a runner is built.
pub(crate) fn workflow_stdlib() -> DirectStdlibMode {
    WORKFLOW_STDLIB
        .read()
        .expect("workflow stdlib")
        .unwrap_or_else(DirectStdlibMode::current)
}

/// The executor set for this process, if one is.
pub(crate) fn executor() -> Option<Arc<dyn CompileExecutor>> {
    EXECUTOR.get().cloned()
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

        fn compile_published_agent(
            &self,
            _tenant: &str,
            _input: runtara_workflows::direct_wasm::DirectCompilationInput,
            _components_dir: &std::path::Path,
            _extra_component_dirs: &[std::path::PathBuf],
            _stdlib: DirectStdlibMode,
        ) -> std::io::Result<runtara_workflows::compile_request::PublishedAgent> {
            Err(std::io::Error::other("published elsewhere"))
        }
    }

    #[test]
    fn workflows_compile_with_the_stdlib_composed_in_for_a_runner_that_links_none() {
        assert_eq!(stdlib_for_runner(false), DirectStdlibMode::Fused);
        assert_eq!(stdlib_for_runner(true), DirectStdlibMode::current());
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
            stdlib: None,
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
