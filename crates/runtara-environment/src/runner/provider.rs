// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The provider-neutral runner (file-less runs plan, step 5): each launch is
//! handed to a capacity provider (a fenders fleet, a container platform, a
//! local runner) with its spec, and tracked by its run channel credential.
//!
//! The durable lifecycle around the run is runtara's own
//! ([`run_launch_lifecycle`]). The runner mints the launch's credential
//! ([`ApiLaunches::mint_credential`]) and puts the run channel into the spec
//! ([`RunChannel::attach`]), places the run with its provider, and confirms
//! the durable start while the provider starts it; the run's calls wait at
//! the run channel's start gate until then. The run has started when its
//! runner first uses the credential and has ended when it posts its outcome
//! (a runner whose guest died posts a failed one with the reason). Exits and
//! losses without an outcome come from the provider ([`Provider::ended`]) or,
//! failing that, the root lease's expiry.
//!
//! A runner slot bounds runs being placed, not running ones: it is given back
//! once the run is placed and its start confirmed. What runs at once is the
//! provider's capacity and the tenant's limits.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use runtara_component_host::api_launches::ApiLaunches;
use runtara_component_host::lifecycle::WorkflowErrorInfo;
use runtara_component_host::runtime_host::{RuntimeHost, WorkflowApiControls};
use runtara_component_host::trusted::TrustedLaunch;
use runtara_component_host::{InvokeExit, InvokeRunResult, RemoteGuestExit, remote_invoke_result};
use runtara_core::persistence::Persistence;
use runtara_workflow_api::{Binding, RunInfo};
use serde_json::{Value, json};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use super::{
    ContainerMetrics, LaunchLifecycle, LaunchOptions, Result, RunExitReport, Runner, RunnerError,
    RunnerHandle, RunnerOccupancy, RunnerParts, StartGate, StartGateOutcome, publish_terminal,
    run_launch_lifecycle,
};
use crate::launch_queue::LaunchKind;
use crate::runtime_host::PersistenceRuntimeHost;

/// The run protocol version a spec speaks (fenders-shim's `protocol`).
pub const RUN_PROTOCOL: u64 = 1;

/// Where a run's runner reaches the workflow API: the run channel, served by
/// this process ([`ApiLaunches::credentialed_router`]), and how the runner
/// verifies it. Each launch gets its own credential in its spec, so specs
/// are secret-bearing: never logged.
#[derive(Clone)]
pub struct RunChannel {
    /// `https://host:port`, reachable from the runners.
    pub endpoint: String,
    /// The CA the endpoint's certificate chains to (PEM).
    pub ca_pem: String,
    /// The SHA-256 of the endpoint certificate's public key, hex.
    pub pin: Option<String>,
    /// The name the endpoint's certificate is for, when not its host.
    pub server_name: Option<String>,
}

impl RunChannel {
    /// Mints `binding`'s launch credential and puts the run protocol into
    /// `spec`: `protocol`, `run` (the launch, its root lease epoch, a trace
    /// id) and `channel` (endpoint, credential, trust). The launch must be
    /// serving; its calls wait until its start is opened.
    pub fn attach(
        &self,
        spec: &mut Value,
        launches: &ApiLaunches,
        binding: &Binding,
    ) -> std::result::Result<(), String> {
        let credential = launches.mint_credential(&binding.launch_id)?;
        let trace: String = binding
            .launch_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(128)
            .collect();
        spec["protocol"] = json!(RUN_PROTOCOL);
        spec["run"] = json!({
            "execution": binding.launch_id,
            "generation": binding.root_lease_epoch,
            "trace_id": trace,
        });
        spec["channel"] = json!({
            "endpoint": self.endpoint,
            "credential": credential,
            "trust": {"ca_pem": self.ca_pem, "pin": self.pin, "server_name": self.server_name},
        });
        Ok(())
    }
}

/// How a run ended at its provider, without regard to what it posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEnd {
    /// Its runner exited with this code.
    Exited(i64),
    /// The provider could not run it (a rejected artifact, a failed start).
    Failed(String),
    /// It is not running there (its host was lost, or refused it for now):
    /// runtara recovers it.
    Lost(String),
}

/// A capacity provider: where runs execute.
#[async_trait]
pub trait Provider: Send + Sync {
    /// The runner type a launch records (`fleet`, `local`).
    fn name(&self) -> &'static str;
    /// The provider's launch spec (without the run protocol fields, which
    /// the runner adds) for a run of `options` of a component of
    /// `component_bytes`.
    fn spec(&self, options: &LaunchOptions, component_bytes: u64) -> Value;
    /// Stages a workflow component for `tenant`; what a placement names it by.
    async fn stage(&self, tenant: &str, bytes: &[u8]) -> std::result::Result<String, String>;
    /// Places a run of `artifact` with `spec`; the provider's run id.
    async fn place(
        &self,
        spec: Value,
        artifact: &str,
        binding: &Binding,
    ) -> std::result::Result<String, String>;
    /// Stops run `run`; its runner posts its failed outcome if it can.
    async fn stop(&self, run: &str);
    /// Waits up to `timeout` for run `run` to end; `None` if it has not.
    async fn ended(&self, run: &str, timeout: Duration) -> Option<ProviderEnd>;
}

struct ProviderTask {
    cancel: Arc<AtomicBool>,
    run: Mutex<Option<String>>,
    abort_at: Mutex<Option<tokio::time::Instant>>,
    finished: AtomicBool,
    done: Notify,
}

/// A workflow artifact staged with the provider for a tenant, and what a
/// launch reads of it.
#[derive(Clone)]
struct Staged {
    artifact: String,
    component_bytes: u64,
    trusted_pins: Arc<std::collections::HashSet<String>>,
    at: Instant,
}

/// How long a staged artifact is launched without staging it again.
const STAGED_FOR: Duration = Duration::from_secs(600);
/// How often a placed run looks for a stop or a deadline while it waits.
const WAIT: Duration = Duration::from_millis(250);

/// runtara's [`Runner`] over a [`Provider`].
pub struct ProviderRunner {
    provider: Arc<dyn Provider>,
    persistence: Arc<dyn Persistence>,
    artifacts: Arc<dyn runtara_artifact_store::ArtifactStore>,
    launches: Arc<ApiLaunches>,
    tasks: Arc<Mutex<HashMap<String, Arc<ProviderTask>>>>,
    exit_reports: Arc<Mutex<HashMap<String, RunExitReport>>>,
    /// Bounds runs being placed; given back once a run is placed and its
    /// start confirmed.
    slots: Arc<Semaphore>,
    limit: usize,
    released: Arc<Notify>,
    handler_state: Arc<runtara_core::instance_handlers::InstanceHandlerState>,
    staged: Mutex<HashMap<(String, String), Staged>>,
    run_channel: Option<RunChannel>,
}

impl ProviderRunner {
    /// A runner over `provider` placing up to `limit` runs at once, serving
    /// their workflow API from `launches`.
    pub fn new(
        provider: Arc<dyn Provider>,
        parts: RunnerParts,
        launches: Arc<ApiLaunches>,
        limit: usize,
    ) -> Self {
        let services = parts.services;
        if let Some(service) = services.outbound_http {
            let _ = launches.set_outbound_http(service);
        }
        if let Some(database) = services.database {
            let _ = launches.set_database(database);
        }
        if let Some(resolver) = services.connections {
            let _ = launches.set_connection_resolver(resolver);
        }
        if let Some(host) = services.control_host {
            let _ = launches.set_control_host(host);
        }
        if let Some(executor) = services.trusted {
            let _ = launches.set_trusted_executor(executor);
        }
        if let Some(host) = services.instance_waits {
            let _ = launches.set_instance_wait_host(host);
        }
        // Published state lives with the run's checkpoints, as for a local run.
        let _ = launches.set_run_state(Arc::new(super::PersistenceRunState(
            parts.persistence.clone(),
        )));
        let mut handler_state =
            runtara_core::instance_handlers::InstanceHandlerState::new(parts.persistence.clone());
        if let Some(observer) = parts.event_observer {
            handler_state = handler_state.with_event_observer(observer);
        }
        Self {
            provider,
            persistence: parts.persistence,
            artifacts: parts.artifacts,
            launches,
            tasks: Arc::default(),
            exit_reports: Arc::default(),
            slots: Arc::new(Semaphore::new(limit)),
            limit,
            released: Arc::new(Notify::new()),
            handler_state: Arc::new(handler_state),
            staged: Mutex::default(),
            run_channel: None,
        }
    }

    /// Runs reach the workflow API over `channel`; without one, none is
    /// launched.
    pub fn with_run_channel(mut self, channel: RunChannel) -> Self {
        self.run_channel = Some(channel);
        self
    }

    fn task(&self, handle: &RunnerHandle) -> Option<Arc<ProviderTask>> {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&handle.handle_id)
            .cloned()
    }

    /// The run's workflow artifact staged with the provider: as staged for
    /// its tenant in the last [`STAGED_FOR`], else read and staged now.
    async fn staged(&self, options: &LaunchOptions) -> Result<Staged> {
        let key = (
            options.tenant_id.clone(),
            options.workflow_digest.to_string(),
        );
        if let Some(staged) = self
            .staged
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .filter(|staged| staged.at.elapsed() < STAGED_FOR)
            .cloned()
        {
            return Ok(staged);
        }
        let component = self
            .artifacts
            .get(&options.workflow_digest)
            .await
            .map_err(|error| RunnerError::StartFailed(format!("read workflow artifact: {error}")))?
            .ok_or_else(|| {
                RunnerError::BinaryNotFound(format!(
                    "workflow artifact {}",
                    options.workflow_digest
                ))
            })?;
        // Its trusted calls run here, never at the provider: the artifact's
        // pins decide which, as for an in-process run.
        let trusted_pins = Arc::new(
            runtara_component_host::trusted::artifact_pins(&component).map_err(|error| {
                RunnerError::StartFailed(format!("read workflow artifact pins: {error}"))
            })?,
        );
        let artifact = self
            .provider
            .stage(&options.tenant_id, &component)
            .await
            .map_err(|error| RunnerError::StartFailed(format!("stage artifact: {error}")))?;
        let staged = Staged {
            artifact,
            component_bytes: component.len() as u64,
            trusted_pins,
            at: Instant::now(),
        };
        let mut cache = self.staged.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, kept| kept.at.elapsed() < STAGED_FOR);
        cache.insert(key, staged.clone());
        Ok(staged)
    }

    async fn input(&self, options: &LaunchOptions) -> Result<Vec<u8>> {
        if let Some(input) = options.prepersisted_input.clone() {
            return Ok(input);
        }
        self.persistence
            .get_instance(&options.instance_id)
            .await
            .map_err(|error| RunnerError::StartFailed(format!("load instance input: {error}")))?
            .and_then(|instance| instance.input)
            .ok_or_else(|| {
                RunnerError::StartFailed(format!(
                    "instance {} has no persisted input envelope",
                    options.instance_id
                ))
            })
    }
}

/// A run's slot, which signals [`ProviderRunner::released`] once it is back.
struct SlotPermit {
    permit: Option<OwnedSemaphorePermit>,
    released: Arc<Notify>,
}

impl SlotPermit {
    fn give_back(&mut self) {
        if self.permit.take().is_some() {
            self.released.notify_one();
        }
    }
}

impl Drop for SlotPermit {
    fn drop(&mut self) {
        self.give_back();
    }
}

/// What a run needs while it is placed and tracked.
struct Placement {
    provider: Arc<dyn Provider>,
    launches: Arc<ApiLaunches>,
    host: Arc<PersistenceRuntimeHost>,
    task: Arc<ProviderTask>,
    spec: Value,
    artifact: String,
    input: Vec<u8>,
    kind: LaunchKind,
    timeout: Duration,
    gate: Option<StartGate>,
    exit_reports: Arc<Mutex<HashMap<String, RunExitReport>>>,
    owner: String,
    trusted_pins: Arc<std::collections::HashSet<String>>,
    channel: Option<RunChannel>,
}

fn lost_result(started: Instant) -> InvokeRunResult {
    InvokeRunResult {
        exit: InvokeExit::Failed(WorkflowErrorInfo {
            code: "NODE_LOST".into(),
            message: "the run was lost by its provider; it is recovered".into(),
            category: "transient".into(),
            severity: "error".into(),
            retryable: true,
            retry_after_ms: None,
            attributes: None,
            details: None,
        }),
        memory_peak_bytes: 0,
        duration: started.elapsed(),
        instance_waits: Vec::new(),
    }
}

async fn run_placed(placement: Placement, mut slot: SlotPermit) -> InvokeRunResult {
    let Placement {
        provider,
        launches,
        host,
        task,
        mut spec,
        artifact,
        input,
        kind,
        timeout,
        gate,
        exit_reports,
        owner,
        trusted_pins,
        channel,
    } = placement;
    let started = Instant::now();
    let failed = |reason: String| InvokeRunResult {
        exit: InvokeExit::Trapped { reason },
        memory_peak_bytes: 0,
        duration: started.elapsed(),
        instance_waits: Vec::new(),
    };
    // Bound to the root lease the lifecycle bound; nothing else may name it.
    let Some(binding) = host.workflow_api_binding() else {
        return failed("the run has no root lease to bind its workflow API".into());
    };
    let info = RunInfo {
        instance_id: binding.instance_id.clone(),
        tenant_id: binding.tenant_id.clone(),
        workflow_id: None,
        version: None,
        attempt: None,
        launch_kind: match kind {
            LaunchKind::Start => "start",
            LaunchKind::Wake => "wake",
            LaunchKind::Resume => "resume",
        }
        .into(),
        debug: false,
        deadline_ms: None,
        api_version: runtara_workflow_api::api_version(),
        capability_id: None,
    };
    let shorten = {
        let task = task.clone();
        Arc::new(move |after: Duration| {
            let at = tokio::time::Instant::now() + after;
            let mut abort_at = task.abort_at.lock().unwrap_or_else(|e| e.into_inner());
            if abort_at.is_none_or(|current| at < current) {
                *abort_at = Some(at);
            }
        })
    };
    let runtime: Arc<dyn RuntimeHost> = host.clone();
    if let Err(error) = launches.begin(
        binding.clone(),
        info,
        input,
        Arc::downgrade(&runtime),
        WorkflowApiControls::remote(shorten, trusted_pins),
    ) {
        return failed(error);
    }
    let Some(channel) = channel else {
        let _ = launches.finish(&binding.launch_id);
        return failed("the runner has no run channel for the workflow API".into());
    };
    if let Err(error) = channel.attach(&mut spec, &launches, &binding) {
        let _ = launches.finish(&binding.launch_id);
        return failed(error);
    }
    // Placed while its durable start is confirmed: the run starts meanwhile,
    // and its first call waits at the run channel's start gate.
    let run = match provider.place(spec, &artifact, &binding).await {
        Ok(run) => run,
        Err(error) => {
            // Refused for now (no room): not run here, so recovered later.
            let _ = launches.finish(&binding.launch_id);
            exit_reports
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    owner,
                    RunExitReport::Lost {
                        reason: format!("{} refused the run: {error}", provider.name()),
                    },
                );
            // Nothing will confirm this start: its monitor reads the report
            // above now, instead of failing the run at the gate's deadline.
            if let Some(gate) = &gate {
                gate.decline();
            }
            return lost_result(started);
        }
    };
    *task.run.lock().unwrap_or_else(|e| e.into_inner()) = Some(run.clone());
    if let Some(gate) = gate {
        let outcome = gate.wait_and_confirm().await;
        if !matches!(outcome, StartGateOutcome::Opened) {
            launches.refuse_start(&binding.launch_id);
            provider.stop(&run).await;
            let _ = launches.finish(&binding.launch_id);
            return failed(format!(
                "start gate did not confirm before launch: {outcome:?}"
            ));
        }
    }
    launches.open_start(&binding.launch_id);
    // Placed and started: the slot bounds placements, not running runs.
    slot.give_back();
    eprintln!(
        "{} runner: launch {} of {} as run {run}",
        provider.name(),
        binding.launch_id,
        binding.instance_id
    );
    let deadline = tokio::time::Instant::now() + timeout;
    let mut stopping: Option<RemoteGuestExit> = None;
    let ended = loop {
        // Its outcome ends the run, whatever its provider reports later.
        if stopping.is_none() && launches.posted(&binding.launch_id) {
            break RemoteGuestExit::Returned;
        }
        let end = tokio::select! {
            end = provider.ended(&run, WAIT) => end,
            _ = launches.outcome_posted(&binding.launch_id), if stopping.is_none() => None,
        };
        match end {
            Some(ProviderEnd::Lost(why)) if stopping.is_none() => {
                let _ = launches.finish(&binding.launch_id);
                eprintln!(
                    "{} runner: launch {} of {} {why}",
                    provider.name(),
                    binding.launch_id,
                    binding.instance_id
                );
                exit_reports
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        owner,
                        RunExitReport::Lost {
                            reason: format!("{} run {run} {why}", provider.name()),
                        },
                    );
                return lost_result(started);
            }
            Some(end) => {
                break match (end, stopping.take()) {
                    (_, Some(reason)) => reason,
                    (ProviderEnd::Failed(reason), None) => RemoteGuestExit::Trapped(reason),
                    // What it posted decides; a non-zero code with nothing
                    // posted says it trapped.
                    (ProviderEnd::Exited(code), None) if code != 0 => {
                        RemoteGuestExit::ExitedWith(code as i32)
                    }
                    (ProviderEnd::Exited(_), None) => RemoteGuestExit::Returned,
                    (ProviderEnd::Lost(why), None) => RemoteGuestExit::Trapped(why),
                };
            }
            None => {}
        }
        if stopping.is_none() {
            let now = tokio::time::Instant::now();
            let aborting = task
                .abort_at
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|at| now >= at);
            let reason = if task.cancel.load(Ordering::SeqCst) {
                Some(RemoteGuestExit::Cancelled)
            } else if now >= deadline || aborting {
                Some(RemoteGuestExit::Timeout)
            } else {
                None
            };
            if let Some(reason) = reason {
                provider.stop(&run).await;
                stopping = Some(reason);
            }
        }
    };
    let result = remote_invoke_result(&launches, &binding.launch_id, ended, started.elapsed());
    publish_terminal(&host, &binding.instance_id, &result).await;
    result
}

#[async_trait]
impl Runner for ProviderRunner {
    fn runner_type(&self) -> &'static str {
        self.provider.name()
    }

    /// A provider's runner gives a run only WASI, so workflows compose the
    /// stdlib in.
    fn links_workflow_stdlib(&self) -> bool {
        false
    }

    async fn try_launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        let permit: OwnedSemaphorePermit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| RunnerError::CapacityUnavailable)?;
        let slot = SlotPermit {
            permit: Some(permit),
            released: self.released.clone(),
        };
        let Staged {
            artifact,
            component_bytes,
            trusted_pins,
            ..
        } = self.staged(options).await?;
        let input = self.input(options).await?;
        let handle_id = format!("{}_{}", self.provider.name(), uuid::Uuid::new_v4().simple());
        let cancel = Arc::new(AtomicBool::new(false));
        let task = Arc::new(ProviderTask {
            cancel: cancel.clone(),
            run: Mutex::default(),
            abort_at: Mutex::default(),
            finished: AtomicBool::new(false),
            done: Notify::new(),
        });
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(handle_id.clone(), task.clone());
        let host = Arc::new(
            PersistenceRuntimeHost::new(
                self.handler_state.clone(),
                options.instance_id.clone(),
                false,
            )
            .with_trusted_launch(match options.launch_kind {
                LaunchKind::Start => TrustedLaunch::Start,
                LaunchKind::Wake => TrustedLaunch::Wake,
                LaunchKind::Resume => TrustedLaunch::Resume,
            })
            .with_workflow_api(self.launches.clone())
            .with_cancel_token(cancel.clone()),
        );
        let lifecycle = LaunchLifecycle {
            persistence: self.persistence.clone(),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            launch_id: options.launch_id.clone(),
            invocation_owner: handle_id.clone(),
            start_gate: options.start_gate.clone(),
            cancel,
            // Its workflow API binding is the root lease: never run without one.
            requires_root_lease: true,
            exit_reports: self.exit_reports.clone(),
        };
        let metrics = Arc::new(tokio::sync::Mutex::new(ContainerMetrics::default()));
        let placement = Placement {
            provider: self.provider.clone(),
            launches: self.launches.clone(),
            host: host.clone(),
            task: task.clone(),
            spec: self.provider.spec(options, component_bytes),
            artifact,
            input,
            kind: options.launch_kind,
            timeout: options.timeout,
            gate: options.start_gate.clone(),
            exit_reports: self.exit_reports.clone(),
            owner: handle_id.clone(),
            trusted_pins,
            channel: self.run_channel.clone(),
        };
        let (tasks, metrics_for_task, id) =
            (self.tasks.clone(), metrics.clone(), handle_id.clone());
        tokio::spawn(async move {
            let ran =
                run_launch_lifecycle(lifecycle, host, move |_lease| run_placed(placement, slot))
                    .await;
            if let Some(run) = ran {
                metrics_for_task.lock().await.memory_peak_bytes = Some(run.memory_peak_bytes);
            }
            task.finished.store(true, Ordering::SeqCst);
            task.done.notify_waiters();
            tasks.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        });
        Ok(RunnerHandle {
            launch_id: options.launch_id.clone(),
            handle_id,
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: chrono::Utc::now(),
            metrics: Some(metrics),
        })
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        self.task(handle)
            .is_some_and(|task| !task.finished.load(Ordering::SeqCst))
    }

    async fn wait_for_exit(&self, handle: &RunnerHandle, poll_interval: Duration) {
        while let Some(task) = self.task(handle) {
            if task.finished.load(Ordering::SeqCst) {
                return;
            }
            tokio::select! {
                _ = task.done.notified() => {}
                _ = tokio::time::sleep(poll_interval.max(Duration::from_millis(200))) => {}
            }
        }
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        if let Some(task) = self.task(handle) {
            task.cancel.store(true, Ordering::SeqCst);
            let run = task.run.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(run) = run {
                self.provider.stop(&run).await;
            }
        }
        Ok(())
    }

    async fn schedule_abort(
        &self,
        handle: &RunnerHandle,
        deadline: tokio::time::Instant,
    ) -> Result<bool> {
        let Some(task) = self.task(handle) else {
            return Ok(false);
        };
        let mut abort_at = task.abort_at.lock().unwrap_or_else(|e| e.into_inner());
        if abort_at.is_none_or(|current| deadline < current) {
            *abort_at = Some(deadline);
        }
        Ok(true)
    }

    async fn take_exit_report(&self, handle: &RunnerHandle) -> Option<RunExitReport> {
        self.exit_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&handle.handle_id)
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics) {
        let metrics = match &handle.metrics {
            Some(metrics) => metrics.lock().await.clone(),
            None => ContainerMetrics::default(),
        };
        (None, None, metrics)
    }

    /// Runs being placed: what the dispatcher may start more of.
    fn occupancy(&self) -> Option<RunnerOccupancy> {
        Some(RunnerOccupancy {
            limit: self.limit as u64,
            held: (self.limit - self.slots.available_permits()) as u64,
        })
    }

    fn capacity_released(&self) -> Option<Arc<Notify>> {
        Some(self.released.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_channel_is_attached_with_a_fresh_credential_and_a_trace_id() {
        let launches = ApiLaunches::default();
        let binding = Binding {
            tenant_id: "t".into(),
            instance_id: "i".into(),
            launch_id: "fleet_launch.1".into(),
            root_lease_epoch: 4,
        };
        let channel = RunChannel {
            endpoint: "https://10.0.0.1:7444".into(),
            ca_pem: "ca".into(),
            pin: None,
            server_name: Some("control".into()),
        };
        let mut spec = json!({"mode":"command"});
        // Not serving: no credential.
        assert!(channel.attach(&mut spec, &launches, &binding).is_err());
        launches
            .begin_standalone(
                binding.clone(),
                runtara_component_host::api_launches::StandaloneTrust::Denied,
                runtara_component_host::api_launches::StandaloneControl::Denied,
            )
            .unwrap();
        channel.attach(&mut spec, &launches, &binding).unwrap();
        assert_eq!(spec["protocol"], 1);
        assert_eq!(spec["run"]["generation"], 4);
        assert_eq!(spec["run"]["trace_id"], "fleet-launch-1");
        assert!(
            spec["channel"]["credential"]
                .as_str()
                .is_some_and(|c| c.starts_with("rlc1_"))
        );
        assert_eq!(spec["channel"]["trust"]["server_name"], "control");
    }
}
