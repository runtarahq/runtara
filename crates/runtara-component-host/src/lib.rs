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

pub mod bindings;
mod cleanup_alarm;
pub mod connection_resolver_host;
pub mod database_host;
pub use database_host::DatabaseHost;
pub mod outbound_http;
pub use outbound_http::OutboundHttpHost;
pub mod dispatcher;
pub mod engine;
pub mod execution_host;
pub(crate) mod host_io;
pub mod host_state;
pub mod isolated_tasks;
pub mod lifecycle;
pub mod precompile;
pub mod registry;
pub mod runtime_host;
pub mod trusted;
pub mod workflow;

pub use bindings::exports::runtara::agent::capabilities::ErrorInfo;
pub use connection_resolver_host::{CONNECTION_RESOLVER_INTERFACE_NAME, ConnectionResolverHost};
pub use dispatcher::{
    ComponentDispatcherService, DispatcherEnv, ResolvedConnection, TestCapabilityRequest,
    TestError, TestResult,
};
pub use engine::{EPOCH_TICK, EngineConfig, build_engine, spawn_epoch_ticker};
pub use host_state::{CallContext, HostState};
pub use registry::{LoadedAgent, build_linker, instantiate, load_agent};
pub use workflow::{
    CapabilityInvocation, ChildInvocationScope, ChildInvocationSpec, InvocationScopeFactory,
    InvokeExit, InvokeRunResult, PreparedChildCatalog, PreparedInvocationLauncher,
    PreparedWorkflow, RootExecutionCoordinator, RootLifecycleDecision, WorkflowExecutor,
    WorkflowExit, WorkflowLimits, WorkflowRunResult, WorkflowRunSpec, WorkflowStartConfirmation,
    WorkflowState,
};

/// Agent metadata loaded from a sidecar `<agent>.meta.json` next to the
/// component `.wasm`. Re-exported here so server code can call
/// `dispatcher.agent_info_of("crypto")` and receive the canonical
/// `runtara_dsl::agent_meta::AgentInfo` shape directly.
pub use runtara_dsl::agent_meta::AgentInfo;

/// The canonical WIT source this host is designed against.
pub const AGENT_WIT: &str = runtara_agent_wit::RUNTARA_AGENT_WIT;
