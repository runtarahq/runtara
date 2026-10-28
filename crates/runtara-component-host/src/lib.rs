//! Embedded Wasmtime host for Runtara agent and workflow components.
//!
//! Loads approved component bundles and binds runtime, connection, database,
//! and outbound HTTP services. Invocation identity comes from host-owned context.
//! Raw WASI HTTP is denied; trusted isolated capabilities also deny outbound calls.

#[cfg(test)]
extern crate self as runtara_component_host;
#[cfg(test)]
#[path = "../tests/common/outbound.rs"]
mod outbound_test_fixture;

pub mod api_launches;
pub mod bindings;
mod cleanup_alarm;
pub mod connection_resolver_host;
pub mod control_host;
pub mod database_host;
pub mod event_budget;
pub use database_host::DatabaseHost;
pub mod outbound_http;
pub use outbound_http::OutboundHttpHost;
pub mod dispatcher;
pub mod engine;
pub mod execution_host;
pub mod host_state;
pub mod instance_wait_host;
pub use instance_wait_host::InstanceWaitHost;
pub mod run_state_host;
pub use run_state_host::RunStateHost;
pub mod isolated_tasks;
pub mod lifecycle;
pub mod operation_scope_host;
pub mod precompile;
pub mod registry;
mod removed_abi;
pub mod runtime_host;
pub mod trusted;
pub mod workflow;
pub mod workflow_api;
pub use workflow_api::{WorkflowApiFuture, WorkflowApiTransport};
pub mod workflow_manifest;

pub use bindings::exports::runtara::agent::capabilities::ErrorInfo;
pub use connection_resolver_host::ConnectionResolverHost;
pub use dispatcher::{
    ComponentDispatcherService, ResolvedConnection, TestCapabilityRequest, TestError, TestResult,
};
pub use engine::{EPOCH_TICK, EngineConfig, build_engine, spawn_epoch_ticker};
pub use host_state::{CallContext, HostState};
pub use registry::{LoadedAgent, build_linker, instantiate, load_agent};
pub use workflow::{
    CapabilityInvocation, ChildInvocationScope, ChildInvocationSpec, InvocationScopeFactory,
    InvokeExit, InvokeRunResult, PreparedChildCatalog, PreparedInvocationLauncher,
    PreparedWorkflow, RemoteGuestExit, RootExecutionCoordinator, RootLifecycleDecision,
    WorkflowExecutor, WorkflowLimits, WorkflowRunSpec, WorkflowStartConfirmation, WorkflowState,
    remote_invoke_result,
};

/// Agent metadata loaded from a sidecar `<agent>.meta.json` next to the
/// component `.wasm`. Re-exported here so server code can call
/// `dispatcher.agent_info_of("crypto")` and receive the canonical
/// `runtara_dsl::agent_meta::AgentInfo` shape directly.
pub use runtara_dsl::agent_meta::AgentInfo;

/// The canonical WIT source this host is designed against.
pub const AGENT_WIT: &str = runtara_wit::agent::WIT;
