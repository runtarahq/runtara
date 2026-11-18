// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow's compile, placed where workflows run
//! (docs/compiler-on-an-executor.md).
//!
//! With executors as the runner, a workflow is compiled by a guest placed on
//! one: the compiler as a WASI command that asks its host for its request
//! and for each component it reads, in a `runtara-run` that can open no file
//! and make no socket, on a host with no database. This process does not
//! load the compiler at all.
//!
//! There is no compiling anywhere else instead. A compile no executor takes,
//! or whose compiler ends without an outcome, is the compile's failure: the
//! property would otherwise hold only while executors are up.
//!
//! What comes back is input ([`crate::compile_executor::Untrusted`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use runtara_environment::runner::{CompileElsewhere, CompiledElsewhere, Runner};
use runtara_workflows::compile_request::{CompileRequest, GuestOutcome};

use crate::compile_executor::{CompileExecutor, Compiler, Shown, Untrusted};

/// The compiler placed on an executor, beside the components.
pub const COMPILE_GUEST: &str = "runtara-compile-guest.wasm";
/// The setting that names it, for a deployment that keeps it elsewhere.
pub const COMPILE_GUEST_ENV: &str = "RUNTARA_COMPILE_GUEST_WASM";

/// The compiler's memory: as the sandbox in this process gives it.
const MEMORY_BYTES: u64 = 3 * 1024 * 1024 * 1024;
/// How long a compile may run: as in the sandbox.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Where the compiler to place is: what [`COMPILE_GUEST_ENV`] names, or
/// [`COMPILE_GUEST`] in the components directory.
pub fn configured() -> Option<PathBuf> {
    let named = std::env::var(COMPILE_GUEST_ENV)
        .ok()
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    named.or_else(|| {
        std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
            .map(|dir| Path::new(&dir).join(COMPILE_GUEST))
    })
}

/// What places a compile: the runner, once it is up.
#[async_trait]
pub(crate) trait Places: Send + Sync {
    async fn place(&self, compile: CompileElsewhere) -> Option<Result<CompiledElsewhere, String>>;
}

#[async_trait]
impl Places for Arc<dyn Runner> {
    async fn place(&self, compile: CompileElsewhere) -> Option<Result<CompiledElsewhere, String>> {
        self.compile_elsewhere(compile).await
    }
}

/// Where compiles are placed, and the compiler placed there.
struct Placement {
    places: Arc<dyn Places>,
    /// The compiler in the artifact store, which a compile's runner fetches
    /// it from.
    compiler: runtara_artifact_store::Digest,
    runtime: tokio::runtime::Handle,
}

/// The compiler as a guest placed with the runner.
#[derive(Default)]
pub(crate) struct PlacedCompiler {
    /// Set when the runner is up: the compile executor is chosen before it.
    placement: OnceLock<Placement>,
}

static PLACED: OnceLock<Arc<PlacedCompiler>> = OnceLock::new();

/// Compile workflows by placing the compiler with the runner, from now on.
/// The runner is given later ([`runner_is_up`]); a compile before that fails.
pub fn install() -> Arc<dyn CompileExecutor> {
    let placed = PLACED.get_or_init(Arc::default).clone();
    Arc::new(Untrusted(placed))
}

/// Whether compiles are placed with the runner.
pub fn installed() -> bool {
    PLACED.get().is_some()
}

/// The runner is up: compiles are placed with it. `compiler` is the WASI
/// command to place, which is put in the artifact store for its runner to
/// fetch. A no-op unless [`install`] was called.
pub async fn runner_is_up(
    runner: Arc<dyn Runner>,
    artifacts: &dyn runtara_artifact_store::ArtifactStore,
    compiler: &[u8],
) -> Result<(), String> {
    let Some(placed) = PLACED.get() else {
        return Ok(());
    };
    let compiler = artifacts
        .put(compiler)
        .await
        .map_err(|error| format!("keeping the compiler in the artifact store: {error}"))?;
    let _ = placed.placement.set(Placement {
        places: Arc::new(runner),
        compiler,
        runtime: tokio::runtime::Handle::current(),
    });
    Ok(())
}

fn failed(why: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("the compiler on an executor: {why}"))
}

impl Compiler for Arc<PlacedCompiler> {
    fn compile(
        &self,
        tenant: &str,
        request: &CompileRequest,
        shown: &Shown<'_>,
    ) -> std::io::Result<(GuestOutcome, Vec<u8>)> {
        let placement = self
            .placement
            .get()
            .ok_or_else(|| failed("the runner that places it is not up yet"))?;
        let compile = CompileElsewhere {
            tenant_id: tenant.to_owned(),
            compiler: placement.compiler.clone(),
            request: serde_json::to_vec(request)?,
            components: shown.by_digest.clone(),
            memory_bytes: MEMORY_BYTES,
            timeout: TIMEOUT,
        };
        // A compile is asked for from a blocking thread or from inside the
        // runtime; a thread of its own waits for it either way.
        let places = placement.places.clone();
        let runtime = placement.runtime.clone();
        let answered = std::thread::scope(|scope| {
            scope
                .spawn(move || runtime.block_on(places.place(compile)))
                .join()
        })
        .map_err(|_| failed("placing it panicked"))?
        .ok_or_else(|| failed("this runner places no compiles"))?
        .map_err(failed)?;

        let outcome: GuestOutcome = serde_json::from_slice(&answered.outcome).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("the compiler posted an outcome that is not one: {error}"),
            )
        })?;
        let made = match (&outcome, answered.made) {
            (_, Some(made)) => made,
            (GuestOutcome::Failed(_), None) => Vec::new(),
            (GuestOutcome::Compiled { .. }, None) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the compiler said it compiled and handed back no component",
                ));
            }
        };
        Ok((outcome, made))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A runner that answers a compile as the test says, and keeps what it
    /// was asked.
    struct StandIn {
        answer: Mutex<Option<Option<Result<CompiledElsewhere, String>>>>,
        asked: Mutex<Vec<(String, usize, u64)>>,
    }

    #[async_trait]
    impl Places for StandIn {
        async fn place(
            &self,
            compile: CompileElsewhere,
        ) -> Option<Result<CompiledElsewhere, String>> {
            self.asked.lock().unwrap().push((
                compile.tenant_id,
                compile.components.len(),
                compile.memory_bytes,
            ));
            self.answer.lock().unwrap().take().expect("one answer")
        }
    }

    fn placed(
        answer: Option<Result<CompiledElsewhere, String>>,
        runtime: &tokio::runtime::Runtime,
    ) -> (Arc<PlacedCompiler>, Arc<StandIn>) {
        let runner = Arc::new(StandIn {
            answer: Mutex::new(Some(answer)),
            asked: Mutex::default(),
        });
        let compiler = PlacedCompiler::default();
        let _ = compiler.placement.set(Placement {
            places: runner.clone(),
            compiler: runtara_artifact_store::Digest::of(b"the compiler"),
            runtime: runtime.handle().clone(),
        });
        (Arc::new(compiler), runner)
    }

    fn request() -> CompileRequest {
        let nowhere = tempfile::tempdir().unwrap();
        let input = runtara_workflows::CompilationInput {
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
        let options = runtara_workflows::DirectWorkflowCompileOptions {
            output_dir: nowhere.path().to_path_buf(),
            components_dir: nowhere.path().to_path_buf(),
            extra_component_dirs: vec![],
            source_checksum: None,
            stdlib: None,
        };
        runtara_workflows::compile_request::request(&input, &options)
            .expect("a compile request")
            .0
    }

    fn compile(compiler: &Arc<PlacedCompiler>) -> std::io::Result<(GuestOutcome, Vec<u8>)> {
        let by_digest = BTreeMap::from([("a".repeat(64), PathBuf::from("/components/a.wasm"))]);
        compiler.compile(
            "acme",
            &request(),
            &Shown {
                components_dir: Path::new("/components"),
                extra_component_dirs: &[],
                by_digest,
            },
        )
    }

    const FAILED: &[u8] = br#"{"failed":{"kind":"Other","message":"no","stale":null}}"#;

    #[test]
    fn a_compile_is_placed_for_its_tenant_with_the_files_it_may_fetch() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (compiler, runner) = placed(
            Some(Ok(CompiledElsewhere {
                outcome: FAILED.to_vec(),
                made: None,
            })),
            &runtime,
        );
        let (outcome, made) = compile(&compiler).expect("the compiler answered");
        assert!(matches!(outcome, GuestOutcome::Failed(failure) if failure.message == "no"));
        assert!(made.is_empty());
        assert_eq!(
            *runner.asked.lock().unwrap(),
            [("acme".to_owned(), 1, MEMORY_BYTES)]
        );
    }

    #[test]
    fn a_compile_no_executor_takes_fails_and_is_not_done_here() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (compiler, _) = placed(
            Some(Err("executor did not take the compile: no room".into())),
            &runtime,
        );
        let error = compile(&compiler).unwrap_err();
        assert!(
            error.to_string().contains("did not take the compile"),
            "{error}"
        );

        // A runner that places no compiles is not compiled around either.
        let (compiler, _) = placed(None, &runtime);
        let error = compile(&compiler).unwrap_err();
        assert!(error.to_string().contains("places no compiles"), "{error}");
    }

    #[test]
    fn a_compiler_that_ended_without_an_outcome_is_the_compiles_failure() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (compiler, _) = placed(
            Some(Err("the compiler ended with 1 and no outcome".into())),
            &runtime,
        );
        let error = compile(&compiler).unwrap_err();
        assert!(error.to_string().contains("no outcome"), "{error}");
    }

    #[test]
    fn what_a_compiler_posted_is_refused_when_it_is_not_an_outcome() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (outcome, made) in [
            (b"not json".to_vec(), None),
            // It says it compiled, and handed nothing back.
            (
                br#"{"compiled":{"outcome":null,"workflow_wasm":"00"}}"#.to_vec(),
                None,
            ),
        ] {
            let (compiler, _) = placed(Some(Ok(CompiledElsewhere { outcome, made })), &runtime);
            let error = compile(&compiler).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
        }
    }

    #[test]
    fn a_compile_before_the_runner_is_up_fails() {
        let error = compile(&Arc::new(PlacedCompiler::default())).unwrap_err();
        assert!(error.to_string().contains("not up yet"), "{error}");
    }

    #[test]
    fn a_compile_asked_for_from_inside_the_runtime_is_waited_for() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (compiler, _) = placed(
            Some(Ok(CompiledElsewhere {
                outcome: FAILED.to_vec(),
                made: None,
            })),
            &runtime,
        );
        // From a task of the runtime itself, where blocking on it directly
        // would panic.
        let answered = runtime.block_on(async move { compile(&compiler) });
        assert!(answered.is_ok());
    }

    #[test]
    fn a_component_that_is_not_the_one_named_is_refused() {
        use crate::compile_executor::made_as_named;
        let named = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(made_as_named(named, b"abc".to_vec()).unwrap(), b"abc");
        let error = made_as_named(named, b"abd".to_vec()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
