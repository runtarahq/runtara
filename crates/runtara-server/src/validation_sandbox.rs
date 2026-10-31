// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The save-time validation of a workflow's closure, in a sandbox (M7.15).
//!
//! With `RUNTARA_VALIDATOR_WASM` naming the `runtara-validate-guest` WASI
//! command, a closure is validated by it in a Wasmtime store of its own, with
//! no filesystem, network or environment, bounded fuel, memory and time (ten
//! seconds), a few at once, rather than natively in this process: a tenant's graph reaches only the validator's
//! sandbox. A closure the validator cannot finish within those bounds, or any
//! failure of the sandbox, is refused as an unsupported graph (E138), never
//! validated natively instead. Without it, validation runs natively, as
//! before.

use std::sync::OnceLock;

use runtara_dsl::ExecutionGraph;
use runtara_dsl::agent_meta::AgentCatalog;
use runtara_workflows::{
    ClosureChildGraph, ClosureValidationReport, ClosureValidationRequest, ValidationError,
    ValidationResult,
};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

/// The validator's WASM memory.
const MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// The instructions it may run: far beyond what validating a large graph
/// takes, and a bound on a graph that would take more.
const FUEL: u64 = 10_000_000_000;
/// How long it may run, in epoch ticks of a second each.
const DEADLINE_TICKS: u64 = 10;
/// How many validations run in the sandbox at once, process-wide: each
/// holds a thread and up to [`MEMORY_BYTES`], so one tenant's many saves
/// cannot take them all.
const CONCURRENT: usize = 2;
/// The report it may write.
const REPORT_BYTES: usize = 16 * 1024 * 1024;

struct Sandbox {
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

/// The sandbox, when one is configured: `Err` when it is, but could not be
/// loaded, so nothing is validated natively instead.
fn sandbox() -> Option<&'static Result<Sandbox, String>> {
    static SANDBOX: OnceLock<Option<Result<Sandbox, String>>> = OnceLock::new();
    SANDBOX
        .get_or_init(|| {
            let path = std::env::var("RUNTARA_VALIDATOR_WASM").ok()?;
            Some(load(&path).map_err(|error| {
                tracing::error!(path = %path, error = %error, "The validator sandbox could not be loaded; saves are refused");
                error
            }))
        })
        .as_ref()
}

fn load(path: &str) -> Result<Sandbox, String> {
    let mut config = Config::new();
    config.consume_fuel(true).epoch_interruption(true);
    let engine = Engine::new(&config).map_err(|error| error.to_string())?;
    // The clock a validation's deadline is counted in.
    let ticking = engine.weak();
    std::thread::Builder::new()
        .name("validator-epoch".into())
        .spawn(move || {
            while let Some(engine) = ticking.upgrade() {
                engine.increment_epoch();
                drop(engine);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        })
        .map_err(|error| error.to_string())?;
    let component =
        Component::from_file(&engine, path).map_err(|error| format!("{path}: {error}"))?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|error| error.to_string())?;
    Ok(Sandbox {
        engine,
        component,
        linker,
    })
}

impl Sandbox {
    fn validate(&self, request: &[u8]) -> Result<ClosureValidationReport, String> {
        let stdout = MemoryOutputPipe::new(REPORT_BYTES);
        let stderr = MemoryOutputPipe::new(64 * 1024);
        let wasi = WasiCtx::builder()
            .stdin(MemoryInputPipe::new(request.to_vec()))
            .stdout(stdout.clone())
            .stderr(stderr.clone())
            .build();
        let mut store = Store::new(
            &self.engine,
            State {
                wasi,
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new().memory_size(MEMORY_BYTES).build(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_fuel(FUEL).map_err(|error| error.to_string())?;
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
        serde_json::from_slice(&stdout.contents())
            .map_err(|error| format!("the validator's report: {error}"))
    }
}

/// Validates the closure of `root`, as
/// [`runtara_workflows::validate_workflow_closure`] does: in the sandbox
/// when one is configured.
pub fn validate_workflow_closure(
    root_workflow_id: &str,
    root: &ExecutionGraph,
    catalog: &AgentCatalog,
    children: &[ClosureChildGraph],
) -> ClosureValidationReport {
    let Some(sandbox) = sandbox() else {
        return runtara_workflows::validate_workflow_closure(
            root_workflow_id,
            root,
            catalog,
            children,
        );
    };
    let validated = match sandbox {
        Ok(sandbox) => in_sandbox(sandbox, root_workflow_id, root, catalog, children),
        Err(error) => Err(format!("the validator is not available: {error}")),
    };
    validated.unwrap_or_else(refused)
}

fn in_sandbox(
    sandbox: &'static Sandbox,
    root_workflow_id: &str,
    root: &ExecutionGraph,
    catalog: &AgentCatalog,
    children: &[ClosureChildGraph],
) -> Result<ClosureValidationReport, String> {
    let request = serde_json::to_vec(&ClosureValidationRequest {
        root_workflow_id: root_workflow_id.to_owned(),
        root: root.clone(),
        catalog: catalog.clone(),
        children: children.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    let run = || {
        let _turn = Turn::take();
        // A thread of its own: WASI's synchronous calls run their own
        // runtime, which a caller's async context would not allow.
        std::thread::scope(|scope| {
            scope
                .spawn(|| sandbox.validate(&request))
                .join()
                .map_err(|_| "the validator panicked".to_owned())?
        })
    };
    // Off the async worker while it waits, where the runtime allows.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(run)
        }
        _ => run(),
    }
}

/// One of the sandbox's [`CONCURRENT`] turns, waited for and held while a
/// validation runs.
struct Turn;

static TURNS: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

impl Turn {
    fn take() -> Self {
        let (running, freed) = &TURNS;
        let mut running = running.lock().expect("validator turns poisoned");
        while *running >= CONCURRENT {
            running = freed.wait(running).expect("validator turns poisoned");
        }
        *running += 1;
        Turn
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (running, freed) = &TURNS;
        *running.lock().expect("validator turns poisoned") -= 1;
        freed.notify_one();
    }
}

/// A closure the sandbox could not validate: refused, as a graph the
/// compiler does not support.
fn refused(reason: String) -> ClosureValidationReport {
    tracing::warn!(reason = %reason, "A workflow could not be validated in the sandbox");
    let mut root = ValidationResult::default();
    root.errors.push(ValidationError::UnsupportedGraphShape {
        step_id: None,
        feature: "validation".into(),
        reason: format!("the workflow could not be validated within its limits: {reason}"),
    });
    ClosureValidationReport {
        root,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "validator-sandbox-tests")]
    fn graph(steps: serde_json::Value, entry: &str) -> ExecutionGraph {
        serde_json::from_value(serde_json::json!({
            "entryPoint": entry,
            "steps": steps,
            "executionPlan": []
        }))
        .unwrap()
    }

    #[test]
    fn a_closure_the_sandbox_cannot_validate_is_refused_as_unsupported() {
        let report = refused("all fuel consumed".into());
        assert!(!report.is_ok());
        let error = &report.root.errors[0];
        assert!(matches!(
            error,
            ValidationError::UnsupportedGraphShape { .. }
        ));
        assert!(error.to_string().contains("all fuel consumed"), "{error}");
    }

    /// Requires `RUNTARA_VALIDATOR_WASM`'s command, built with
    /// `cargo build -p runtara-compile-guest --release --target wasm32-wasip2`.
    #[cfg(feature = "validator-sandbox-tests")]
    #[test]
    fn the_sandbox_reports_what_native_validation_does() {
        let path = std::env::var("RUNTARA_VALIDATOR_WASM")
            .expect("validator-sandbox-tests requires RUNTARA_VALIDATOR_WASM");
        let sandbox = Box::leak(Box::new(load(&path).expect("validator loads")));
        let catalog = AgentCatalog::default();
        for root in [
            graph(
                serde_json::json!({"finish": {"id": "finish", "stepType": "Finish"}}),
                "finish",
            ),
            // An entry point that is not a step, and an unknown agent.
            graph(
                serde_json::json!({"a": {"id": "a", "stepType": "Agent", "agentId": "nope", "capabilityId": "x"}}),
                "missing",
            ),
        ] {
            let native = runtara_workflows::validate_workflow_closure("wf", &root, &catalog, &[]);
            let sandboxed = in_sandbox(sandbox, "wf", &root, &catalog, &[]).expect("validated");
            assert_eq!(
                serde_json::to_value(&sandboxed).unwrap(),
                serde_json::to_value(&native).unwrap()
            );
        }
    }
}
