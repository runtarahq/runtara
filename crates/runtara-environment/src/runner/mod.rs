// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner module - instance execution backends.
//!
//! This module is moved from runtara-core.

mod common;
pub mod embedded;
pub mod mock;
mod traits;

pub use common::WorkflowRunnerConfig;
pub use embedded::{
    EmbeddedWasmRunner, LaunchLifecycle, RootLease, ScopedAgentRunnerConfig, publish_terminal,
    run_launch_lifecycle,
};
pub use mock::MockRunner;
pub use traits::*;

/// Build the workflow runner: the in-process embedded wasmtime engine,
/// reading workflow artifacts from `artifacts`.
pub fn build_runner(
    persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
    event_observer: Option<
        std::sync::Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>,
    >,
    artifacts: std::sync::Arc<dyn runtara_artifact_store::ArtifactStore>,
) -> Result<std::sync::Arc<dyn Runner>> {
    build_embedded_runner(RunnerParts {
        persistence,
        event_observer,
        scoped_agents: None,
        services: HostServices::default(),
        artifacts,
    })
}

/// Native services shared across runs; each invocation supplies its own identity.
#[derive(Default)]
pub struct HostServices {
    /// Approved isolated built-in capabilities.
    pub trusted: Option<std::sync::Arc<runtara_component_host::trusted::TrustedExecutor>>,
    /// Safe connection metadata and resource resolution.
    pub connections: Option<std::sync::Arc<dyn runtara_component_host::ConnectionResolverHost>>,
    /// Native SQL execution against connection-owned databases.
    pub database: Option<std::sync::Arc<dyn runtara_component_host::DatabaseHost>>,
    /// Credential-aware outbound HTTP.
    pub outbound_http: Option<std::sync::Arc<dyn runtara_component_host::OutboundHttpHost>>,
    /// The control service behind every run's `/v1/instances` calls.
    pub control_host: Option<std::sync::Arc<dyn runtara_component_host::control_host::ControlHost>>,
    /// Durable instance waits of every run.
    pub instance_waits: Option<std::sync::Arc<dyn runtara_component_host::InstanceWaitHost>>,
}

/// What a workflow runner is built from: the persistence runs record into,
/// the instance-event observer, the operator isolation policy and the native
/// services every run reaches.
pub struct RunnerParts {
    /// Durable state: instances, checkpoints, events.
    pub persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
    /// Observes instance events (step counts and the like).
    pub event_observer:
        Option<std::sync::Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>>,
    /// The operator's isolation policy for scoped agent invocations.
    pub scoped_agents: Option<ScopedAgentRunnerConfig>,
    /// The native services every run reaches.
    pub services: HostServices,
    /// Where workflow artifacts are kept: a launch names its workflow by
    /// digest and the runner reads it from here.
    pub artifacts: std::sync::Arc<dyn runtara_artifact_store::ArtifactStore>,
}

/// Builds the workflow runner from its parts. A host that runs workflows
/// elsewhere (another process, a VM) supplies its own; the default is
/// [`build_embedded_runner`].
pub type RunnerFactory =
    std::sync::Arc<dyn Fn(RunnerParts) -> Result<std::sync::Arc<dyn Runner>> + Send + Sync>;

/// The in-process embedded wasmtime engine, built from its parts.
pub fn build_embedded_runner(parts: RunnerParts) -> Result<std::sync::Arc<dyn Runner>> {
    let RunnerParts {
        persistence,
        event_observer,
        scoped_agents,
        services,
        artifacts,
    } = parts;
    let mut runner =
        EmbeddedWasmRunner::new(WorkflowRunnerConfig::from_env(), persistence, artifacts)?;
    if let Some(config) = scoped_agents {
        runner = runner.with_scoped_agents(config)?;
    }
    if let Some(service) = services.outbound_http {
        runner = runner.with_outbound_http(service)?;
    }
    if let Some(database) = services.database {
        runner = runner.with_database(database)?;
    }
    if let Some(connections) = services.connections {
        runner = runner.with_connection_resolver(connections)?;
    }
    if let Some(host) = services.control_host {
        runner = runner.with_control_host(host)?;
    }
    if let Some(waits) = services.instance_waits {
        runner = runner.with_instance_wait_host(waits)?;
    }
    if let Some(trusted) = services.trusted {
        runner = runner.with_trusted_executor(trusted)?;
    }
    if let Some(observer) = event_observer {
        runner = runner.with_event_observer(observer);
    }
    tracing::info!("Using EmbeddedWasmRunner (in-process wasmtime) for workflow execution");
    Ok(std::sync::Arc::new(runner))
}
