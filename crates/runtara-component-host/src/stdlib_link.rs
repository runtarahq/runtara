//! The workflow stdlib, linked into a run instead of composed into it.
//!
//! Workflow logic compiled in linked mode imports
//! `runtara:workflow-stdlib-link/json`: the stdlib's functions, typed `async`.
//! The host compiles the stdlib once and instantiates it in each run's Store
//! that needs it; every import is served by a typed forwarder that calls the
//! same function on that instance. Typed, because a dynamic (`Val`) forwarder
//! boxes every byte of a `list<u8>` and is 5-30x slower on large scopes.
//!
//! The table below is the stdlib WIT's function list; a test checks it
//! against the WIT, and linking checks every type.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use wasmtime::Store;
use wasmtime::component::{
    Component, ComponentNamedList, ComponentType, Func, InstancePre, Lift, LinkerInstance, Lower,
};

use crate::workflow::WorkflowState;

/// The interface linked workflow logic imports.
pub const LINK_INTERFACE: &str = runtara_wit::stdlib_link::JSON;
/// The interface the stdlib exports.
const STDLIB_INTERFACE: &str = runtara_wit::stdlib::JSON;

#[derive(ComponentType, Lift, Lower, Clone, Debug)]
#[component(record)]
pub struct ErrorInfo {
    code: String,
    message: String,
    category: String,
    severity: String,
    retryable: bool,
    #[component(name = "retry-after-ms")]
    retry_after_ms: Option<u64>,
    attributes: Option<String>,
    details: Option<String>,
}

#[derive(ComponentType, Lift, Lower, Clone, Debug)]
#[component(record)]
pub struct InvokeError {
    code: String,
    message: String,
    category: String,
    severity: String,
    retryable: bool,
    #[component(name = "retry-after-ms")]
    retry_after_ms: Option<u64>,
    attributes: Option<String>,
    details: Option<String>,
}

#[derive(ComponentType, Lift, Lower, Clone, Debug)]
#[component(record)]
pub struct AgentRetryError {
    payload: Vec<u8>,
    retryable: bool,
    #[component(name = "rate-limited")]
    rate_limited: bool,
}

#[derive(ComponentType, Lift, Lower, Clone, Debug)]
#[component(record)]
pub struct WaitInstancesProgress {
    pending: bool,
    #[component(name = "deadline-ms")]
    deadline_ms: Option<u64>,
}

/// One run's stdlib instance: its functions in [`FUNCTIONS`] order.
pub struct LinkedStdlib {
    functions: Vec<Func>,
}

macro_rules! stdlib_functions {
    ($($name:literal $params:ty => $result:ty;)*) => {
        /// Every stdlib function, in the order of [`LinkedStdlib`]'s functions.
        pub const FUNCTIONS: &[&str] = &[$($name),*];

        /// Serve every `runtara:workflow-stdlib-link/json` import from the
        /// run's stdlib instance.
        pub(crate) fn add_forwarders(
            linker: &mut wasmtime::component::Linker<WorkflowState>,
        ) -> Result<()> {
            let mut instance = linker.instance(LINK_INTERFACE)?;
            let mut position = 0;
            $(
                forward::<$params, ($result,)>(&mut instance, $name, position)?;
                position += 1;
            )*
            debug_assert_eq!(position, FUNCTIONS.len());
            Ok(())
        }
    };
}

stdlib_functions! {
    "init-manifest" (u64, Vec<u8>) => Result<u64, String>;
    "value-store-retain" (u64, Vec<u8>, Vec<u8>) => Result<(), String>;
    "value-store-scope" (u64,) => u64;
    "value-store-retain-scoped" (u64, Vec<u8>, Vec<u8>, u64) => Result<(), String>;
    "build-source" (u64, Vec<u8>, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "apply-mapping" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "eval-condition" (u64, u32, Vec<u8>) => Result<bool, String>;
    "conditional-output" (u64, u32, Vec<u8>, bool) => Result<Vec<u8>, String>;
    "process-switch" (u64, u32, Vec<u8>) => Result<String, String>;
    "value-switch" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "split-items" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "split-item-count" (u64, u32, Vec<u8>) => Result<u32, String>;
    "split-item" (u64, u32, Vec<u8>, u32) => Result<Vec<u8>, String>;
    "split-iteration-variables" (u64, u32, Vec<u8>, Vec<u8>, u32) => Result<Vec<u8>, String>;
    "split-validate-input" (u64, u32, Vec<u8>, u32) => Result<(), String>;
    "split-validate-output" (u64, u32, Vec<u8>, u32) => Result<(), String>;
    "split-initial-results" (u64, u32) => Result<Vec<u8>, String>;
    "split-append-output" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "split-append-error" (u64, u32, Vec<u8>, String, u32) => Result<Vec<u8>, String>;
    "split-output" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "split-cache-key" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "split-result" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "split-output-from-result" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "while-max-iterations" (u64, u32) => Result<u32, String>;
    "while-initial-state" (u64, u32) => Result<Vec<u8>, String>;
    "while-condition-source" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "while-condition" (u64, u32, Vec<u8>) => Result<bool, String>;
    "while-iteration-variables" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "while-advance-state" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "while-output" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "while-cache-key" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "while-result" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "while-output-from-result" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "filter" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "log-event" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "log" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "error-event" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "error" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "error-steps" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "group-by" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "delay-duration-ms" (u64, u32, Vec<u8>) => Result<u64, String>;
    "delay" (u64, u32, Vec<u8>, u64) => Result<Vec<u8>, String>;
    "loop-deadline-key" (u64, String, Vec<u8>, bool) => Result<Vec<u8>, String>;
    "delay-sleep-key" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "invoke-error-fields" (u64, Vec<u8>) => Result<InvokeError, String>;
    "breakpoint-key" (u64, String, Vec<u8>) => Result<String, String>;
    "breakpoint-event" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-signal-id" (u64, String, String, Vec<u8>) => Result<String, String>;
    "wait-timeout-ms" (u64, String, Vec<u8>) => Result<Option<u64>, String>;
    "wait-timeout-error" (u64, String, String, u64) => Result<Vec<u8>, String>;
    "wait-timeout-error-envelope" (u64, String, String, u64) => Result<Vec<u8>, String>;
    "wait-closed-error" (u64, String, String, String, Option<u64>) => Result<Vec<u8>, String>;
    "wait-on-wait-variables" (u64, String, String, String, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-on-wait-error" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-poll-interval-ms" (u64, String) => Result<u64, String>;
    "wait-poll-interval-ms-scoped" (u64, String, Vec<u8>) => Result<u64, String>;
    "wait-event" (u64, String, String, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-debug-start" (u64, String, String, Option<u64>, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-output" (u64, String, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-instances-key" (u64, String, Vec<u8>) => Result<String, String>;
    "wait-instances-request" (u64, String, Vec<u8>, u64) => Result<Vec<u8>, String>;
    "wait-instances-state" (u64, Vec<u8>) => Result<WaitInstancesProgress, String>;
    "wait-instances-output" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "wait-instances-error" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "state-key" (u64, String, Vec<u8>, bool) => Result<String, String>;
    "state-patch" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "state-local-set" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "state-local-get" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "state-local-restore" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "state-output" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "api-error-json" (u64, String, String, Option<u64>) => Vec<u8>;
    "agent-operation-input" (u64, Vec<u8>, Option<String>, Option<Vec<u8>>) => Result<Vec<u8>, String>;
    "state-error" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-wait-tool-signal-id" (u64, String, String, String, u32, Vec<u8>) => Result<String, String>;
    "ai-wait-tool-result" (u64, Vec<u8>) => Result<Vec<u8>, String>;
    "embed-workflow-cache-key" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "embed-workflow-variables" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "embed-workflow-result" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "embed-workflow-output-from-result" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "embed-workflow-error" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "embed-workflow-error-scoped" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "retry-sleep-key" (u64, String, u32) => Result<Vec<u8>, String>;
    "retry-delay-ms" (u64, u32, u32, u64, u64, Option<u64>) => Result<u64, String>;
    "workflow-error-retryable" (u64, Vec<u8>) => Result<bool, String>;
    "workflow-error-rate-limited" (u64, Vec<u8>) => Result<bool, String>;
    "workflow-error-retry-after-ms" (u64, Vec<u8>) => Result<Option<u64>, String>;
    "agent-output" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-agent-output" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-turn-next-input" (u64, Vec<u8>, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-turn-is-complete" (u64, Vec<u8>) => Result<bool, String>;
    "ai-turn-tool-count" (u64, Vec<u8>) => Result<u32, String>;
    "ai-turn-tool-args" (u64, Vec<u8>, u32) => Result<Vec<u8>, String>;
    "ai-turn-tool-index" (u64, Vec<u8>, u32) => Result<u32, String>;
    "ai-tool-args-with-timeout" (u64, Vec<u8>, u64) => Result<Vec<u8>, String>;
    "ai-turn-add-result" (u64, Vec<u8>, Vec<u8>, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-turn-cache-key" (u64, String, u32, Vec<u8>) => Result<String, String>;
    "ai-turn-response-key" (u64, String, u32, Vec<u8>) => Result<String, String>;
    "ai-turn-response-validate" (u64, Vec<u8>) => Result<(), String>;
    "ai-turn-snapshot" (u64, Vec<u8>, Vec<u8>, u32, bool) => Result<Vec<u8>, String>;
    "ai-turn-snapshot-part" (u64, Vec<u8>, u32) => Result<Vec<u8>, String>;
    "ai-turn-snapshot-tool-calls" (u64, Vec<u8>) => Result<u32, String>;
    "ai-turn-snapshot-complete" (u64, Vec<u8>) => Result<bool, String>;
    "ai-turn-slot-key" (u64, String, u32, Vec<u8>) => Result<String, String>;
    "ai-turn-slot-snapshot" (u64, Vec<u8>, Vec<u8>, u32, u32) => Result<Vec<u8>, String>;
    "ai-turn-snapshot-turn" (u64, Vec<u8>) => Result<u32, String>;
    "ai-turn-latest" (u64, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-agent-result-key" (u64, String, Vec<u8>) => Result<String, String>;
    "ai-agent-result" (u64, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-summary-key" (u64, String, Vec<u8>) => Result<String, String>;
    "ai-turn-output" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-tool-debug-start" (u64, u32, Vec<u8>, u32, u32, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-tool-debug-end" (u64, u32, Vec<u8>, u32, u32, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-memory-debug-start" (u64, u32, u32, Vec<u8>, Vec<u8>, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-memory-debug-end" (u64, u32, u32, Vec<u8>, Vec<u8>, Vec<u8>, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-memory-initial-state" (u64, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-memory-save-input" (u64, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "ai-memory-compact-sliding" (u64, Vec<u8>, u32) => Result<Vec<u8>, String>;
    "ai-summarize-input" (u64, Vec<u8>, Vec<u8>, u32) => Result<Vec<u8>, String>;
    "ai-summarize-output" (u64, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-validate-input" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-connection-id" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-connection-input" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-scope-input" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-tool-scope-input" (u64, String, String, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "tool-scope-source" (u64, String, String, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-aux-scope-source" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-cache-key" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-retry-sleep-key" (u64, String, u32) => Result<Vec<u8>, String>;
    "agent-attempt-result-key" (u64, String, u32) => Result<Vec<u8>, String>;
    "agent-attempt-envelope" (u64, u8, bool, bool, bool, u64, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-retry-delay-ms" (u64, u32, u32, u64, u64, Option<u64>) => Result<u64, String>;
    "agent-retry-error-info" (ErrorInfo,) => Result<AgentRetryError, String>;
    "agent-error" (u32, ErrorInfo, u64) => Result<Vec<u8>, String>;
    "agent-error-from-info" (u64, u32, Vec<u8>) => Result<Vec<u8>, String>;
    "agent-debug-error" (u64, u32, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
    "step-debug-start" (u64, String, Vec<u8>) => Result<Vec<u8>, String>;
    "step-debug-end" (u64, String, Vec<u8>, u64, u64) => Result<Vec<u8>, String>;
    "step-debug-error" (u64, String, Vec<u8>, Vec<u8>) => Result<Vec<u8>, String>;
}

fn forward<P, R>(
    instance: &mut LinkerInstance<'_, WorkflowState>,
    name: &'static str,
    position: usize,
) -> Result<()>
where
    P: ComponentNamedList + Lift + Lower + Send + Sync + 'static,
    R: ComponentNamedList + Lift + Lower + Send + Sync + 'static,
{
    instance.func_wrap_concurrent(name, move |accessor, params: P| {
        Box::pin(async move {
            let function = accessor.with(|mut access| {
                access
                    .data_mut()
                    .stdlib
                    .as_ref()
                    .map(|stdlib| stdlib.functions[position])
            });
            let function = function.ok_or_else(|| {
                wasmtime::format_err!(
                    "stdlib function `{name}` called before the run's stdlib was linked"
                )
            })?;
            let typed = accessor.with(|access| function.typed::<P, R>(&access))?;
            typed.call_concurrent(accessor, params).await
        })
    })?;
    Ok(())
}

/// Whether `component` imports the linked stdlib.
pub fn imports_linked_stdlib(component: &Component) -> bool {
    component
        .component_type()
        .imports(component.engine())
        .any(|(name, _)| name == LINK_INTERFACE)
}

/// Instantiate the stdlib in `store` and collect its functions.
pub(crate) async fn instantiate(
    pre: &InstancePre<WorkflowState>,
    store: &mut Store<WorkflowState>,
) -> Result<LinkedStdlib> {
    let instance = pre
        .instantiate_async(&mut *store)
        .await
        .map_err(|error| anyhow::anyhow!("instantiate the workflow stdlib: {error:#}"))?;
    let interface = instance
        .get_export_index(&mut *store, None, STDLIB_INTERFACE)
        .with_context(|| format!("the workflow stdlib exports no `{STDLIB_INTERFACE}`"))?;
    let mut functions = Vec::with_capacity(FUNCTIONS.len());
    for name in FUNCTIONS {
        let index = instance
            .get_export_index(&mut *store, Some(&interface), name)
            .with_context(|| format!("the workflow stdlib exports no `{name}`"))?;
        functions.push(
            instance
                .get_func(&mut *store, index)
                .with_context(|| format!("stdlib export `{name}` is not a function"))?,
        );
    }
    Ok(LinkedStdlib { functions })
}

/// The stdlib component in `dir` (`runtara_workflow_stdlib.wasm`).
pub(crate) fn load(engine: &wasmtime::Engine, dir: &std::path::Path) -> Result<Component> {
    let path = dir.join("runtara_workflow_stdlib.wasm");
    Component::from_file(engine, &path).map_err(|error| {
        anyhow::anyhow!("compile the workflow stdlib {}: {error:#}", path.display())
    })
}

/// Shared ownership for the executor's compiled stdlib.
pub(crate) type StdlibPre = Arc<InstancePre<WorkflowState>>;

#[cfg(test)]
mod tests {
    /// The table is the stdlib's function list, in the WIT's order; linking
    /// checks each function's types.
    #[test]
    fn the_table_is_the_stdlib_wit() {
        let declared: Vec<&str> = runtara_wit::stdlib::WIT
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .filter_map(|line| line.split_once(": func(").map(|(name, _)| name))
            .collect();
        assert_eq!(declared, super::FUNCTIONS);
    }
}
