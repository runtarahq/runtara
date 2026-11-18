// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow's compile, in a sandbox (docs/splitting-the-engine.md, Phase 6).
//!
//! With `RUNTARA_COMPILER_WASM` naming the `runtara-compile-sandboxed` WASI
//! command, a workflow is compiled by it in a Wasmtime store of its own,
//! rather than natively in this process. The store has no network and no
//! environment, a bound on its memory and its time, and of the filesystem
//! only the component files the compile may read, read-only. A tenant's
//! graph reaches only the compiler's sandbox: not this process's databases,
//! keys or seal.
//!
//! What the sandbox hands back is input, not this process's own work. The
//! composed component is checked against the outcome that names it before
//! anything is written, and what the engine records of a compile it checks
//! or derives itself (`api::services::compilation`).
//!
//! A compile the sandbox cannot finish within its bounds, or any failure of
//! the sandbox, is the compile's failure. Nothing is compiled natively
//! instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use runtara_workflows::compile_request::{self, CompileRequest, GuestOutcome};
use runtara_workflows::direct_wasm::DirectStdlibMode;
use runtara_workflows::{CompilationInput, DirectWorkflowCompileOptions, NativeCompilationResult};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxView, WasiView};

use crate::compile_executor::CompileExecutor;

/// The setting that names the compiler's WASI command.
pub const COMPILER_WASM_ENV: &str = "RUNTARA_COMPILER_WASM";

/// The compiler's WASM memory: a 32-bit guest's whole address space is 4 GiB.
const MEMORY_BYTES: usize = 3 * 1024 * 1024 * 1024;
/// How long a compile may run, in epoch ticks of a second each.
const DEADLINE_TICKS: u64 = 300;
/// The most a compile writes: its outcome and the composed component.
const OUTPUT_BYTES: usize = 160 * 1024 * 1024;
/// The most of what it says went wrong that is kept.
const STDERR_BYTES: usize = 64 * 1024;

/// The compiler in a sandbox.
pub struct CompileSandbox {
    engine: Engine,
    component: Component,
    linker: Linker<State>,
}

struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// The path `RUNTARA_COMPILER_WASM` names, if it is set.
pub fn configured() -> Option<PathBuf> {
    std::env::var(COMPILER_WASM_ENV)
        .ok()
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

impl CompileSandbox {
    /// Loads the compiler's WASI command at `path`.
    pub fn load(path: &Path) -> Result<Arc<Self>, String> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|error| error.to_string())?;
        // The clock a compile's deadline is counted in.
        let ticking = engine.weak();
        std::thread::Builder::new()
            .name("compiler-epoch".into())
            .spawn(move || {
                while let Some(engine) = ticking.upgrade() {
                    engine.increment_epoch();
                    drop(engine);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            })
            .map_err(|error| error.to_string())?;
        let component = Component::from_file(&engine, path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|error| error.to_string())?;
        Ok(Arc::new(Self {
            engine,
            component,
            linker,
        }))
    }

    /// Runs one compile: `request` on its input, `components_dir` and the
    /// `extra` directories where the compiler sees them, read-only. Answers
    /// what it wrote, or why the sandbox did not finish.
    fn run(
        &self,
        request: &[u8],
        components_dir: &Path,
        extra: &[PathBuf],
    ) -> Result<Vec<u8>, String> {
        let stdout = MemoryOutputPipe::new(OUTPUT_BYTES);
        let stderr = MemoryOutputPipe::new(STDERR_BYTES);
        let mut wasi = WasiCtx::builder();
        wasi.stdin(MemoryInputPipe::new(request.to_vec()))
            .stdout(stdout.clone())
            .stderr(stderr.clone());
        // Only the files a compile may read, and only to read. A directory
        // that is not there holds nothing, and is not shown.
        let shown = std::iter::once((components_dir.to_path_buf(), "/components".to_owned()))
            .chain(
                extra
                    .iter()
                    .enumerate()
                    .map(|(index, dir)| (dir.clone(), format!("/extra/{index}"))),
            );
        for (host, guest) in shown {
            if host.is_dir() {
                wasi.preopened_dir(&host, &guest, FsPerms::ReadOnly)
                    .map_err(|error| format!("{}: {error}", host.display()))?;
            }
        }
        let mut store = Store::new(
            &self.engine,
            State {
                wasi: wasi.build(),
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new().memory_size(MEMORY_BYTES).build(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(DEADLINE_TICKS);
        let command = wasmtime_wasi::p2::bindings::sync::Command::instantiate(
            &mut store,
            &self.component,
            &self.linker,
        )
        .map_err(|error| error.to_string())?;
        let ran = command
            .wasi_cli_run()
            .call_run(&mut store)
            .map_err(|error| format!("{error:#}"))?;
        if ran.is_err() {
            return Err(String::from_utf8_lossy(&stderr.contents()).into_owned());
        }
        Ok(stdout.contents().to_vec())
    }

    /// Compiles `request` in the sandbox, on a thread of its own: WASI's
    /// synchronous calls run their own runtime, which a caller's async
    /// context would not allow.
    fn compile(
        &self,
        request: &CompileRequest,
        components_dir: &Path,
        extra: &[PathBuf],
    ) -> std::io::Result<(GuestOutcome, Vec<u8>)> {
        let request = serde_json::to_vec(request)?;
        let written = std::thread::scope(|scope| {
            scope
                .spawn(|| self.run(&request, components_dir, extra))
                .join()
                .map_err(|_| "the compiler's sandbox panicked".to_owned())?
        })
        .map_err(|error| std::io::Error::other(format!("the compiler's sandbox: {error}")))?;
        parse(&written)
    }
}

/// What a compile wrote: its outcome, and the composed component after it.
/// See `runtara-compile-sandboxed`.
fn parse(written: &[u8]) -> std::io::Result<(GuestOutcome, Vec<u8>)> {
    let malformed = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("the compiler's sandbox wrote {what}"),
        )
    };
    let (length, rest) = written
        .split_first_chunk::<8>()
        .ok_or_else(|| malformed("no outcome"))?;
    let length = usize::try_from(u64::from_le_bytes(*length))
        .map_err(|_| malformed("an outcome too long"))?;
    if length > rest.len() {
        return Err(malformed("less than the outcome it announced"));
    }
    let (outcome, made) = rest.split_at(length);
    let outcome = serde_json::from_slice(outcome)
        .map_err(|error| malformed(&format!("an outcome that is not one: {error}")))?;
    Ok((outcome, made.to_vec()))
}

/// The component a compile handed back, if it is the one its outcome names.
fn made_as_named(named: &str, made: Vec<u8>) -> std::io::Result<Vec<u8>> {
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(&made)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != named {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the compiler's sandbox handed back another component than its outcome names",
        ));
    }
    Ok(made)
}

impl CompileExecutor for CompileSandbox {
    fn compile(
        &self,
        input: CompilationInput,
        options: DirectWorkflowCompileOptions,
    ) -> std::io::Result<NativeCompilationResult> {
        let (request, _) = compile_request::request(&input, &options)?;
        let (outcome, made) = CompileSandbox::compile(
            self,
            &request,
            &options.components_dir,
            &options.extra_component_dirs,
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
        components_dir: &Path,
        extra_component_dirs: &[PathBuf],
        stdlib: DirectStdlibMode,
    ) -> std::io::Result<compile_request::PublishedAgent> {
        let (request, _) = compile_request::request_published_agent(
            tenant,
            &input,
            components_dir,
            extra_component_dirs,
            stdlib,
        )?;
        let (outcome, made) =
            CompileSandbox::compile(self, &request, components_dir, extra_component_dirs)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn written(outcome: &[u8], made: &[u8]) -> Vec<u8> {
        let mut bytes = (outcome.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(outcome);
        bytes.extend_from_slice(made);
        bytes
    }

    #[test]
    fn what_a_compile_wrote_is_its_outcome_and_then_the_component() {
        let failed = br#"{"failed":{"kind":"Other","message":"no","stale":null}}"#;
        let (outcome, made) = parse(&written(failed, b"")).unwrap();
        assert!(matches!(outcome, GuestOutcome::Failed(failure) if failure.message == "no"));
        assert!(made.is_empty());
        let (_, made) = parse(&written(failed, b"the component")).unwrap();
        assert_eq!(made, b"the component");
    }

    #[test]
    fn what_is_not_an_outcome_is_refused() {
        // Nothing, a length with nothing after it, a length past the end,
        // and an outcome that is not one.
        for bytes in [
            Vec::new(),
            vec![1, 2, 3],
            (64u64).to_le_bytes().to_vec(),
            written(b"not json", b""),
            (u64::MAX).to_le_bytes().to_vec(),
        ] {
            let error = parse(&bytes).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
        }
    }

    #[test]
    fn a_component_that_is_not_the_one_named_is_refused() {
        let named = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(made_as_named(named, b"abc".to_vec()).unwrap(), b"abc");
        let error = made_as_named(named, b"abd".to_vec()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// Requires `RUNTARA_COMPILER_WASM`'s command, built with
    /// `cargo build -p runtara-compile-guest --bin runtara-compile-sandboxed
    /// --release --target wasm32-wasip2`, and the workflow components in
    /// `RUNTARA_AGENT_COMPONENTS_DIR`.
    #[cfg(feature = "compiler-sandbox-tests")]
    mod with_the_compiler {
        use super::*;

        fn input(steps: serde_json::Value, entry: &str) -> CompilationInput {
            CompilationInput {
                tenant_id: "acme".into(),
                workflow_id: "wf".into(),
                version: 1,
                execution_graph: serde_json::from_value(serde_json::json!({
                    "entryPoint": entry, "steps": steps, "executionPlan": []
                }))
                .unwrap(),
                track_events: false,
                child_workflows: vec![],
                agent_catalog: None,
                progress_callback: None,
                agent_slug: None,
            }
        }

        fn options(output: &Path) -> DirectWorkflowCompileOptions {
            DirectWorkflowCompileOptions {
                output_dir: output.to_path_buf(),
                components_dir: std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
                    .expect("compiler-sandbox-tests requires RUNTARA_AGENT_COMPONENTS_DIR")
                    .into(),
                extra_component_dirs: vec![],
                source_checksum: Some("a-checksum".into()),
                stdlib: None,
            }
        }

        fn sandbox() -> Arc<CompileSandbox> {
            let path = configured().expect("compiler-sandbox-tests requires RUNTARA_COMPILER_WASM");
            CompileSandbox::load(&path).expect("the compiler loads")
        }

        fn finish() -> serde_json::Value {
            serde_json::json!({"finish": {
                "id": "finish", "stepType": "Finish",
                "inputMapping": {"answer": {"valueType": "immediate", "value": 42}}
            }})
        }

        #[test]
        fn the_sandbox_makes_what_a_compile_in_this_process_does() {
            let sandbox = sandbox();
            let (native_dir, sandboxed_dir) =
                (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let native = runtara_workflows::compile_workflow_direct(
                input(finish(), "finish"),
                options(native_dir.path()),
            )
            .expect("compiles natively");
            let sandboxed = CompileExecutor::compile(
                sandbox.as_ref(),
                input(finish(), "finish"),
                options(sandboxed_dir.path()),
            )
            .expect("compiles in the sandbox");
            assert_eq!(sandboxed.binary_checksum, native.binary_checksum);
            assert_eq!(sandboxed.binary_size, native.binary_size);
            assert_eq!(sandboxed.trusted_pins, native.trusted_pins);
            assert_eq!(
                std::fs::read(&sandboxed.binary_path).unwrap(),
                std::fs::read(&native.binary_path).unwrap()
            );
            assert!(sandboxed.binary_path.starts_with(sandboxed_dir.path()));
        }

        #[test]
        fn a_graph_that_does_not_compile_fails_as_it_does_in_this_process() {
            let sandbox = sandbox();
            let output = tempfile::tempdir().unwrap();
            let broken = || input(finish(), "not-a-step");
            let native =
                runtara_workflows::compile_workflow_direct(broken(), options(output.path()))
                    .unwrap_err();
            let sandboxed =
                CompileExecutor::compile(sandbox.as_ref(), broken(), options(output.path()))
                    .unwrap_err();
            assert_eq!(sandboxed.kind(), native.kind());
            assert_eq!(sandboxed.to_string(), native.to_string());
        }

        #[test]
        fn the_compiler_is_shown_only_the_components_a_compile_reads() {
            // A components directory that is not there is not shown: the
            // compile fails for want of its components, as it does in this
            // process, and not for the sandbox.
            let sandbox = sandbox();
            let output = tempfile::tempdir().unwrap();
            let mut nowhere = options(output.path());
            nowhere.components_dir = output.path().join("no-components");
            let error =
                CompileExecutor::compile(sandbox.as_ref(), input(finish(), "finish"), nowhere)
                    .unwrap_err();
            assert!(!error.to_string().contains("sandbox"), "{error}");
        }
    }
}
