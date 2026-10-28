// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workflow API served in process, for runs of this host's executor.
//!
//! A run on the workflow API ABI (`wasi:cli/run`) reaches its host over
//! `wasi:http`; the component host routes those requests to the `/v1` router
//! with the run's binding (see `crate::workflow_api`). This
//! module is what the router serves them from: [`ApiLaunches`] holds every
//! launch currently serving the API in this process, registered by its
//! runtime host when the entry starts and removed when the host takes the
//! posted outcome back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use crate::connection_resolver_host::{
    ConnectionResolverHost, RunConnectionResolver, resolver_for_run,
};
use crate::control_host::{self, ControlAuthority, ControlHost};
use crate::database_host::{Operation, database_for_run};
use crate::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitError, InstanceWaitPoll, InstanceWaitRequest,
    InstanceWaitResolution, wait_instances_id,
};
use crate::operation_scope_host::{OperationIdentity, parse_agent_operation_key};
use crate::outbound_http::{ConnectionDestination, Destination, RequestOptions, send_bounded};
use crate::run_state_host::{
    RunStateAuthority, RunStateError, RunStateErrorCode, RunStateHost, check_state_read_key,
    state_operation_id,
};
use crate::runtime_host::{RuntimeHost, WorkflowApiControls};
use crate::trusted::TrustedExecutor;
use crate::{DatabaseHost, InstanceWaitHost, OutboundHttpHost};
use crate::{WorkflowApiFuture, WorkflowApiTransport};
use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use bytes::Bytes;
use runtara_workflow_api::server::OutboundReply;
use runtara_workflow_api::server::{
    CheckpointWrite, CustomSignalNotice, Event, LaunchState, LifecycleSignal, MemoryReceiptStore,
    ReceiptStore, TrustedCall, WorkflowApiBackend, WorkflowApiState, router,
};
use runtara_workflow_api::{
    ApiError, Binding, Caps, ControlCall, ErrorCode, InputRead, OutboundDestination,
    OutboundFailure, OutboundMeta, PostedOutcome, RunInfo, SqlOperation,
};
use tower::ServiceExt;

/// One launch serving the workflow API.
pub struct LaunchSlot {
    binding: Binding,
    info: RunInfo,
    input: Vec<u8>,
    outcome: Mutex<Option<PostedOutcome>>,
    /// The run's runtime host, which serves its run services with the lease
    /// fencing it already applies. Weak: the host's drop releases the slot.
    runtime: Weak<dyn RuntimeHost>,
    controls: WorkflowApiControls,
    /// Connection resolution of this launch, with its per-run cache.
    resolver: std::sync::OnceLock<Arc<RunConnectionResolver>>,
    /// Operations this launch entered and has not left, by operation id.
    operations: Mutex<HashMap<String, OperationIdentity>>,
    /// A failure the run may not recover from, which ends it as trapped
    /// whatever it does next.
    fault: Mutex<Option<String>>,
    /// A launch outside any run (an agent test call, an isolated child): it
    /// is served the connection, HTTP, SQL and trusted endpoints, and nothing
    /// that needs a run. `None` for a run's launch.
    standalone: Option<(StandaloneTrust, StandaloneControl)>,
    /// Instance waits this launch registered whose last read was pending.
    /// A suspension it posts wakes on them too, as the old ABI's host
    /// attached them to the run's result.
    pending_waits: Mutex<Vec<String>>,
    /// The child runs this launch started and has not released, by task id.
    /// Dropping one cancels its child, so a launch that ends reaps them.
    tasks: Mutex<LaunchTasks>,
    /// When this launch's lease was last read from the store.
    lease_checked: Mutex<Option<std::time::Instant>>,
    /// What this launch may still append through `POST /v1/events`.
    events: Mutex<crate::event_budget::EventBudget>,
}

/// How often a launch's lease is read again while it calls the workflow API.
const LEASE_RECHECK: std::time::Duration = std::time::Duration::from_secs(1);

/// A launch's child runs.
#[derive(Default)]
struct LaunchTasks {
    next: u64,
    live: HashMap<String, Arc<crate::execution_host::TaskHandle>>,
}

impl LaunchSlot {
    fn track_wait(&self, wait_id: &str, poll: &InstanceWaitPoll) {
        let mut pending = self.pending_waits.lock().expect("pending waits poisoned");
        pending.retain(|id| id != wait_id);
        if poll.resolution.is_none() {
            pending.push(wait_id.to_string());
        }
    }
}

/// The launches of this process that currently serve the workflow API, by
/// launch id, and their idempotency receipts.
pub struct ApiLaunches {
    slots: Mutex<HashMap<String, Arc<LaunchSlot>>>,
    /// In memory by default: an embedded launch lives in one process.
    receipts: Arc<dyn ReceiptStore>,
    /// The published-state service the runner gives the old ABI as well.
    run_state: std::sync::OnceLock<Arc<dyn RunStateHost>>,
    /// The instance-wait service, once the runner is given one.
    instance_waits: std::sync::OnceLock<Arc<dyn InstanceWaitHost>>,
    /// The connection services the runner gives the old ABI as well.
    pub(crate) outbound_http: std::sync::OnceLock<Arc<dyn OutboundHttpHost>>,
    pub(crate) database: std::sync::OnceLock<Arc<dyn DatabaseHost>>,
    pub(crate) connection_resolver: std::sync::OnceLock<Arc<dyn ConnectionResolverHost>>,
    /// The control service the runner gives the old ABI as well.
    pub(crate) control: std::sync::OnceLock<Arc<dyn ControlHost>>,
    /// The approved built-in trusted executor the old ABI uses as well.
    pub(crate) trusted: std::sync::OnceLock<Arc<TrustedExecutor>>,
}

impl Default for ApiLaunches {
    fn default() -> Self {
        Self {
            slots: Mutex::default(),
            receipts: Arc::new(MemoryReceiptStore::default()),
            run_state: std::sync::OnceLock::new(),
            instance_waits: std::sync::OnceLock::new(),
            outbound_http: std::sync::OnceLock::new(),
            database: std::sync::OnceLock::new(),
            connection_resolver: std::sync::OnceLock::new(),
            control: std::sync::OnceLock::new(),
            trusted: std::sync::OnceLock::new(),
        }
    }
}

impl ApiLaunches {
    /// Launches whose published state is served by `run_state`.
    pub fn with_run_state(run_state: Arc<dyn RunStateHost>) -> Self {
        let launches = Self::default();
        let _ = launches.run_state.set(run_state);
        launches
    }

    /// Serve published state with `run_state`, for launches made before the
    /// host had it (a runner built after the router was mounted).
    pub fn set_run_state(&self, run_state: Arc<dyn RunStateHost>) -> Result<(), String> {
        self.run_state
            .set(run_state)
            .map_err(|_| "run state already configured for the workflow API".to_string())
    }

    /// Keep idempotency receipts in `receipts` instead of in memory, for
    /// launches that outlive the serving process.
    pub fn with_receipts(mut self, receipts: Arc<dyn ReceiptStore>) -> Self {
        self.receipts = receipts;
        self
    }

    /// Start serving `binding`'s launch. A launch begins once.
    pub fn begin(
        &self,
        binding: Binding,
        info: RunInfo,
        input: Vec<u8>,
        runtime: Weak<dyn RuntimeHost>,
        controls: WorkflowApiControls,
    ) -> Result<(), String> {
        let mut slots = self.slots.lock().expect("workflow API launches poisoned");
        if slots.contains_key(&binding.launch_id) {
            return Err(format!(
                "launch {} already serves the workflow API",
                binding.launch_id
            ));
        }
        slots.insert(
            binding.launch_id.clone(),
            Arc::new(LaunchSlot {
                binding,
                info,
                input,
                outcome: Mutex::new(None),
                runtime,
                controls,
                resolver: std::sync::OnceLock::new(),
                operations: Mutex::default(),
                fault: Mutex::default(),
                standalone: None,
                pending_waits: Mutex::default(),
                tasks: Mutex::default(),
                lease_checked: Mutex::default(),
                events: Mutex::new(crate::event_budget::EventBudget::new(
                    std::time::Instant::now(),
                )),
            }),
        );
        Ok(())
    }

    /// Start serving `binding` as a standalone launch: a call outside any run,
    /// such as an agent's test invocation, which reaches only the endpoints
    /// that need no run. `trust` decides its trusted calls, as the caller's
    /// own store would have.
    pub fn begin_standalone(
        &self,
        binding: Binding,
        trust: StandaloneTrust,
        control: StandaloneControl,
    ) -> Result<(), String> {
        self.begin_standalone_with_alarm(binding, trust, control, None, None)
    }

    /// [`Self::begin_standalone`] for a workflow store without a run, which
    /// still owns its cleanup alarm (`POST /v1/run/alarm`) and, when it has
    /// one, the child-run context of its isolated agents (`/v1/tasks`).
    pub(crate) fn begin_standalone_with_alarm(
        &self,
        binding: Binding,
        trust: StandaloneTrust,
        control: StandaloneControl,
        cleanup_alarm: Option<crate::cleanup_alarm::CleanupAlarmState>,
        execution: Option<Arc<crate::execution_host::ExecutionContext>>,
    ) -> Result<(), String> {
        let mut slots = self.slots.lock().expect("workflow API launches poisoned");
        if slots.contains_key(&binding.launch_id) {
            return Err(format!(
                "launch {} already serves the workflow API",
                binding.launch_id
            ));
        }
        let info = RunInfo {
            instance_id: binding.instance_id.clone(),
            tenant_id: binding.tenant_id.clone(),
            workflow_id: None,
            version: None,
            attempt: None,
            launch_kind: "standalone".into(),
            debug: false,
            deadline_ms: None,
            api_version: runtara_workflow_api::api_version(),
            capability_id: None,
        };
        slots.insert(
            binding.launch_id.clone(),
            Arc::new(LaunchSlot {
                binding,
                info,
                input: Vec::new(),
                outcome: Mutex::new(None),
                runtime: Weak::<NoRun>::new(),
                controls: WorkflowApiControls {
                    shorten_deadline: Arc::new(|_| {}),
                    trusted_pins: Arc::default(),
                    cleanup_alarm,
                    execution,
                    alarm_shortens_deadline: false,
                },
                resolver: std::sync::OnceLock::new(),
                operations: Mutex::default(),
                fault: Mutex::default(),
                standalone: Some((trust, control)),
                pending_waits: Mutex::default(),
                tasks: Mutex::default(),
                lease_checked: Mutex::default(),
                events: Mutex::new(crate::event_budget::EventBudget::new(
                    std::time::Instant::now(),
                )),
            }),
        );
        Ok(())
    }

    /// Stop serving `launch_id` and hand back the outcome it posted, if any.
    /// Later requests from that launch are `fenced`.
    pub fn finish(&self, launch_id: &str) -> Option<PostedOutcome> {
        let slot = self
            .slots
            .lock()
            .expect("workflow API launches poisoned")
            .remove(launch_id)?;
        // Receipts go asynchronously: this runs from a host's drop, too.
        let receipts = Arc::clone(&self.receipts);
        let forgotten = launch_id.to_string();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = receipts.forget_launch(&forgotten).await {
                    tracing::warn!(%error, launch_id = %forgotten, "forgetting workflow API receipts");
                }
            });
        }
        slot.outcome
            .lock()
            .expect("workflow API outcome poisoned")
            .take()
    }

    /// Serve outbound HTTP with `service`, the one the old ABI uses.
    pub fn set_outbound_http(&self, service: Arc<dyn OutboundHttpHost>) -> Result<(), String> {
        self.outbound_http
            .set(service)
            .map_err(|_| "outbound HTTP already configured for the workflow API".to_string())
    }

    /// Serve SQL with `database`, the one the old ABI uses.
    pub fn set_database(&self, database: Arc<dyn DatabaseHost>) -> Result<(), String> {
        self.database
            .set(database)
            .map_err(|_| "database already configured for the workflow API".to_string())
    }

    /// Serve connection descriptors and resources with `resolver`.
    pub fn set_connection_resolver(
        &self,
        resolver: Arc<dyn ConnectionResolverHost>,
    ) -> Result<(), String> {
        self.connection_resolver
            .set(resolver)
            .map_err(|_| "connection resolver already configured for the workflow API".to_string())
    }

    /// Serve `/v1/instances` with `host`, the control service of the old ABI.
    pub fn set_control_host(&self, host: Arc<dyn ControlHost>) -> Result<(), String> {
        self.control
            .set(host)
            .map_err(|_| "control host already configured for the workflow API".to_string())
    }

    /// Serve `/v1/trusted` with `executor`, the approved built-in executor.
    pub fn set_trusted_executor(&self, executor: Arc<TrustedExecutor>) -> Result<(), String> {
        self.trusted
            .set(executor)
            .map_err(|_| "trusted executor already configured for the workflow API".to_string())
    }

    /// Serve instance waits with `host`, the one the runner gives the old ABI.
    pub fn set_instance_wait_host(&self, host: Arc<dyn InstanceWaitHost>) -> Result<(), String> {
        self.instance_waits
            .set(host)
            .map_err(|_| "instance wait host already configured".to_string())
    }

    /// Stop serving `launch_id`, dropping any outcome it posted. Idempotent.
    pub fn release(&self, launch_id: &str) {
        let _ = self.finish(launch_id);
    }

    /// Whether `launch_id` posted its outcome, without finishing it: a runner
    /// whose guest exited non-zero can tell a failed workflow from a trap.
    pub fn posted(&self, launch_id: &str) -> bool {
        let slots = self.slots.lock().expect("workflow API launches poisoned");
        slots.get(launch_id).is_some_and(|slot| {
            slot.outcome
                .lock()
                .expect("workflow API outcome poisoned")
                .is_some()
        })
    }

    /// The fault that ends `launch_id` as trapped, if one was recorded.
    pub fn fault(&self, launch_id: &str) -> Option<String> {
        let slots = self.slots.lock().expect("workflow API launches poisoned");
        slots
            .get(launch_id)?
            .fault
            .lock()
            .expect("launch fault poisoned")
            .clone()
    }

    /// Whether `launch_id`'s cleanup alarm expired, aborting the run.
    pub fn cleanup_aborted(&self, launch_id: &str) -> bool {
        let slots = self.slots.lock().expect("workflow API launches poisoned");
        slots
            .get(launch_id)
            .and_then(|slot| slot.controls.cleanup_alarm.as_ref())
            .is_some_and(|alarm| alarm.expired())
    }

    /// How many launches currently serve the API.
    pub fn serving(&self) -> usize {
        self.slots
            .lock()
            .expect("workflow API launches poisoned")
            .len()
    }

    /// The launch `binding` names, while it serves the API. An expired
    /// cleanup alarm has selected the run's abort: from then on it is served
    /// nothing, so no new call (an outcome, a signal acknowledgement) slips
    /// in between the guest's epoch checks.
    fn slot(&self, binding: &Binding) -> Option<Arc<LaunchSlot>> {
        self.slots
            .lock()
            .expect("workflow API launches poisoned")
            .get(&binding.launch_id)
            .filter(|slot| slot.binding == *binding)
            .filter(|slot| {
                !slot
                    .controls
                    .cleanup_alarm
                    .as_ref()
                    .is_some_and(|alarm| alarm.expired())
            })
            .cloned()
    }

    /// The `/v1` router over these launches, as a component-host transport.
    pub fn transport(self: &Arc<Self>) -> Arc<dyn WorkflowApiTransport> {
        Arc::new(RouterTransport(self.router()))
    }

    /// The `/v1` router over these launches, for a host whose guests run
    /// elsewhere (a VM behind a gateway). The caller attaches each request's
    /// [`Binding`] as an extension, derived from where the request came from,
    /// never from the request itself.
    pub fn router(self: &Arc<Self>) -> axum::Router {
        router(WorkflowApiState {
            backend: Arc::new(Backend(Arc::clone(self))),
            receipts: Arc::clone(&self.receipts),
            caps: Caps::default(),
        })
    }
}

struct Backend(Arc<ApiLaunches>);

impl Backend {
    /// The live task `task_id` of `binding`'s launch.
    fn task(
        &self,
        binding: &Binding,
        task_id: &str,
    ) -> Result<Arc<crate::execution_host::TaskHandle>, ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let tasks = slot.tasks.lock().expect("launch tasks poisoned");
        tasks
            .live
            .get(task_id)
            .map(Arc::clone)
            .ok_or_else(|| task_error(crate::execution_host::ExecutionError::InvalidTask))
    }

    /// Whether `task_id` names a task this launch runs in process, live or
    /// released. Any other id is a child run of the launch's instance, read
    /// from persistence.
    fn is_live_task(&self, binding: &Binding, task_id: &str) -> Result<bool, ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let tasks = slot.tasks.lock().expect("launch tasks poisoned");
        Ok(tasks.live.contains_key(task_id) || task_id.starts_with(IN_PROCESS_TASK_PREFIX))
    }

    /// The control host and the authority a child run of `binding`'s run is
    /// started and cancelled under. `operation` names the call site.
    fn child_control(
        &self,
        binding: &Binding,
        operation: Option<String>,
    ) -> Result<(Arc<dyn ControlHost>, ControlAuthority), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        if slot.standalone.is_some() {
            return Err(ApiError::new(
                ErrorCode::Denied,
                "child runs are not available in this context",
            ));
        }
        let host = self.0.control.get().cloned().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Denied,
                "child runs are not available in this context",
            )
        })?;
        Ok((
            host,
            ControlAuthority {
                tenant: binding.tenant_id.clone(),
                caller: Some(binding.instance_id.clone()),
                operation,
            },
        ))
    }

    /// Start (or, on a replay of the same call, find again) a child run.
    async fn start_child_run(
        &self,
        binding: &Binding,
        child: ChildRunBinding,
        start: runtara_workflow_api::TaskStart,
        input: Vec<u8>,
    ) -> Result<runtara_workflow_api::TaskStarted, ApiError> {
        if start.entry != runtara_workflow_api::TaskEntry::Workflow {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                "a workflow child run takes the `workflow` entry",
            ));
        }
        let data: serde_json::Value = serde_json::from_slice(&input)
            .map_err(|error| ApiError::new(ErrorCode::Invalid, format!("child input: {error}")))?;
        // The child runs with the envelope every root run gets.
        let input = serde_json::to_vec(&serde_json::json!({"data": data, "variables": {}}))
            .expect("a JSON value serializes");
        let (host, authority) =
            self.child_control(binding, Some(child_operation(&start.context)))?;
        let started = host
            .start(
                &authority,
                control_host::StartRequest {
                    workflow_id: child.workflow_id,
                    version: child.version,
                    input,
                    run_label: None,
                    parent_close_policy: control_host::ParentClosePolicy::Cancel,
                },
            )
            .await
            .map_err(|error| {
                tracing::warn!(
                    instance_id = %binding.instance_id,
                    binding = %start.binding,
                    code = ?error.code,
                    message = %error.message,
                    "child run start refused"
                );
                control_host::json::api_error(&error)
            })?;
        Ok(runtara_workflow_api::TaskStarted {
            task_id: started.instance_id.clone(),
            instance_id: Some(started.instance_id),
        })
    }

    /// A child run's outcome once it has ended, waiting up to `wait_ms`.
    async fn child_run_outcome(
        &self,
        binding: &Binding,
        task_id: &str,
        wait_ms: u64,
    ) -> Result<Option<runtara_workflow_api::TaskOutcome>, ApiError> {
        use control_host::InstanceStatus;
        let (host, authority) = self.child_control(binding, None)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
        loop {
            let detail = host
                .get(&authority, task_id.to_owned())
                .await
                .map_err(|error| {
                    tracing::warn!(
                        instance_id = %binding.instance_id,
                        task_id,
                        code = ?error.code,
                        message = %error.message,
                        "child run read refused"
                    );
                    control_host::json::api_error(&error)
                })?;
            if detail.instance.parent_instance_id.as_deref() != Some(binding.instance_id.as_str()) {
                return Err(task_error(
                    crate::execution_host::ExecutionError::InvalidTask,
                ));
            }
            let ended = match detail.instance.status {
                InstanceStatus::Completed => Some(child_completed(detail.terminal)),
                InstanceStatus::Failed => Some(child_failed(
                    detail.instance.termination_reason.as_deref(),
                    detail.terminal,
                )),
                InstanceStatus::Cancelled | InstanceStatus::NotStarted => {
                    Some(runtara_workflow_api::TaskOutcome::Cancelled)
                }
                InstanceStatus::Queued
                | InstanceStatus::Pending
                | InstanceStatus::Running
                | InstanceStatus::Suspended => None,
            };
            let now = tokio::time::Instant::now();
            if ended.is_some() || now >= deadline {
                return Ok(ended);
            }
            tokio::time::sleep((deadline - now).min(CHILD_RUN_POLL)).await;
        }
    }

    /// Request that a child run stop.
    async fn cancel_child_run(
        &self,
        binding: &Binding,
        task_id: &str,
    ) -> Result<runtara_workflow_api::CancelStatus, ApiError> {
        let (host, authority) =
            self.child_control(binding, Some(format!("task-cancel:{task_id}")))?;
        let result = host
            .cancel(
                &authority,
                control_host::CancelRequest {
                    instance_id: task_id.to_owned(),
                    reason: Some("cancelled by its parent".into()),
                    grace_ms: None,
                },
            )
            .await
            .map_err(|error| control_host::json::api_error(&error))?;
        use control_host::CommandOutcome;
        Ok(match result.outcome {
            CommandOutcome::Requested | CommandOutcome::Applied => {
                runtara_workflow_api::CancelStatus::Requested
            }
            CommandOutcome::Unchanged => runtara_workflow_api::CancelStatus::AlreadyRequested,
            CommandOutcome::AlreadyTerminal => runtara_workflow_api::CancelStatus::AlreadyTerminal,
        })
    }

    /// The runtime host of `binding`'s launch, while the launch is served.
    fn runtime(&self, binding: &Binding) -> Result<Arc<dyn RuntimeHost>, ApiError> {
        self.0
            .slot(binding)
            .and_then(|slot| slot.runtime.upgrade())
            .ok_or_else(not_serving)
    }
}

/// A runtime host refusal. The host's messages carry no payloads.
fn host_error(error: String) -> ApiError {
    ApiError::new(ErrorCode::Unavailable, error)
}

/// Managed-input refusals are the guest's own mistakes more often than not
/// (an unknown input, a malformed descriptor), so they are `invalid`.
fn input_error(error: String) -> ApiError {
    ApiError::new(ErrorCode::Invalid, error)
}

fn input_read(state: crate::runtime_host::RuntimeInputState) -> InputRead {
    use crate::runtime_host::RuntimeInputState;
    match state {
        RuntimeInputState::Open => InputRead::Open,
        RuntimeInputState::Accepted(value) => InputRead::Accepted {
            value: serde_json::from_slice(&value).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&value).into_owned())
            }),
        },
        RuntimeInputState::Closed(reason) => InputRead::Closed { reason },
    }
}

/// A connection service refusal; its messages carry no secrets.
fn connection_error(error: String) -> ApiError {
    ApiError::domain("connection-error", ErrorCode::Invalid, error)
}

/// A database failure: the serialized `DatabaseError` goes in `details`.
fn database_error(error: String) -> ApiError {
    match serde_json::from_str::<serde_json::Value>(&error) {
        Ok(details) => {
            let message = details
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("database error")
                .to_string();
            ApiError::domain("database-error", ErrorCode::Invalid, message).with_details(details)
        }
        Err(_) => ApiError::domain("database-error", ErrorCode::Invalid, error),
    }
}

fn wait_error(error: InstanceWaitError) -> ApiError {
    let message = error.message.clone();
    let mut api = match error.code.as_str() {
        "invalid" => ApiError::new(ErrorCode::Invalid, message),
        "denied" => ApiError::new(ErrorCode::Denied, message),
        "not-found" => ApiError::new(ErrorCode::NotFound, message),
        "too-large" => ApiError::new(ErrorCode::TooLarge, message),
        "replay-conflict" => ApiError::new(ErrorCode::ReplayConflict, message),
        "unavailable" => ApiError::new(ErrorCode::Unavailable, message),
        // `not-child`, `closed`: the request was refused as given.
        other => ApiError::domain(other, ErrorCode::Invalid, message),
    };
    api.retry_after_ms = error.retry_after_ms;
    api
}

/// A wait read as the API returns it: the old ABI's read plus its `waitId`.
fn wait_read(wait_id: &str, poll: &InstanceWaitPoll) -> Result<serde_json::Value, ApiError> {
    let mut read = serde_json::to_value(poll).map_err(|_| {
        ApiError::new(
            ErrorCode::Unavailable,
            "a finished run's result could not be read",
        )
    })?;
    read["waitId"] = serde_json::Value::String(wait_id.to_string());
    Ok(read)
}

fn state_error(error: RunStateError) -> ApiError {
    match error.code {
        RunStateErrorCode::Invalid => ApiError::new(ErrorCode::Invalid, error.message),
        RunStateErrorCode::TooLarge => ApiError::new(ErrorCode::TooLarge, error.message),
        RunStateErrorCode::NotRunning => {
            ApiError::domain("not-running", ErrorCode::Finished, error.message)
        }
        RunStateErrorCode::Unavailable => ApiError::new(ErrorCode::Unavailable, error.message),
    }
}

impl Backend {
    /// The launch's connection resolver, created on first use.
    fn resolver(&self, binding: &Binding) -> Result<Arc<RunConnectionResolver>, ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        if let Some(resolver) = slot.resolver.get() {
            return Ok(Arc::clone(resolver));
        }
        let resolver = resolver_for_run(self.0.connection_resolver.get(), Some(&binding.tenant_id))
            .map_err(|e| ApiError::new(ErrorCode::Unavailable, e))?;
        Ok(Arc::clone(slot.resolver.get_or_init(|| resolver)))
    }

    /// The wait service and the binding's run as the waiter, while the launch
    /// is served.
    fn wait_service(
        &self,
        binding: &Binding,
    ) -> Result<(Arc<dyn InstanceWaitHost>, InstanceWaitAuthority), ApiError> {
        self.0.slot(binding).ok_or_else(not_serving)?;
        let host = self.0.instance_waits.get().cloned().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Unavailable,
                "no instance wait service is configured",
            )
        })?;
        Ok((
            host,
            InstanceWaitAuthority {
                tenant: binding.tenant_id.clone(),
                caller: binding.instance_id.clone(),
            },
        ))
    }

    /// The state service and the binding's run, while the launch is served.
    fn state_service(
        &self,
        binding: &Binding,
    ) -> Result<(Arc<dyn RunStateHost>, RunStateAuthority), ApiError> {
        self.0.slot(binding).ok_or_else(not_serving)?;
        let host = self.0.run_state.get().cloned().ok_or_else(|| {
            ApiError::new(ErrorCode::Unavailable, "no run state service is configured")
        })?;
        Ok((
            host,
            RunStateAuthority {
                tenant: binding.tenant_id.clone(),
                instance: binding.instance_id.clone(),
            },
        ))
    }
}

/// Every in-process task id starts with this; a child run's id is its
/// instance id, which never does.
const IN_PROCESS_TASK_PREFIX: &str = "task-";

/// How often a join re-reads a child run that has not ended.
const CHILD_RUN_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// A `/v1/tasks` binding naming a workflow to run as a child run of its own:
/// `workflow:<workflow id>` or `workflow:<workflow id>@<version>`.
struct ChildRunBinding {
    workflow_id: String,
    version: Option<u32>,
}

impl ChildRunBinding {
    fn parse(binding: &str) -> Result<Option<Self>, ApiError> {
        let Some(rest) = binding.strip_prefix("workflow:") else {
            return Ok(None);
        };
        let invalid = || ApiError::new(ErrorCode::Invalid, format!("child binding `{binding}`"));
        let (workflow_id, version) = match rest.split_once('@') {
            Some((id, version)) => (id, Some(version.parse().map_err(|_| invalid())?)),
            None => (rest, None),
        };
        if workflow_id.is_empty() {
            return Err(invalid());
        }
        Ok(Some(Self {
            workflow_id: workflow_id.to_owned(),
            version,
        }))
    }
}

/// The operation a child run is started under: one per call site and
/// attempt, so a replay of the parent reaching the same call finds the child
/// it started before, and a retry (a new attempt) starts a new one.
fn child_operation(context: &runtara_workflow_api::TaskContext) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"task\0");
    digest.update(context.path.as_bytes());
    digest.update(b"\0");
    digest.update(context.attempt.to_be_bytes());
    let digest = digest.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("task:{hex}")
}

/// A completed child run's output, as the composed call returned it.
fn child_completed(terminal: control_host::TerminalResult) -> runtara_workflow_api::TaskOutcome {
    match terminal.output {
        Some(output) => runtara_workflow_api::TaskOutcome::Completed(output),
        None if terminal.output_omitted => runtara_workflow_api::TaskOutcome::Failed(child_error(
            "CHILD_OUTPUT_TOO_LARGE",
            "the child run's output is over the cap",
        )),
        None => runtara_workflow_api::TaskOutcome::Completed(b"null".to_vec()),
    }
}

/// A failed child run's error, as its own `error-info` when it posted one.
fn child_failed(
    termination_reason: Option<&str>,
    terminal: control_host::TerminalResult,
) -> runtara_workflow_api::TaskOutcome {
    if termination_reason.is_some_and(|reason| reason.contains("timeout")) {
        return runtara_workflow_api::TaskOutcome::TimedOut;
    }
    let error = terminal.error.as_deref().map(|bytes| {
        serde_json::from_slice::<runtara_workflow_api::ErrorInfo>(bytes)
            .unwrap_or_else(|_| child_error("CHILD_FAILED", &String::from_utf8_lossy(bytes)))
    });
    runtara_workflow_api::TaskOutcome::Failed(
        error.unwrap_or_else(|| child_error("CHILD_FAILED", "the child run failed")),
    )
}

fn child_error(code: &str, message: &str) -> runtara_workflow_api::ErrorInfo {
    runtara_workflow_api::ErrorInfo {
        code: code.to_owned(),
        message: message.to_owned(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
        details: None,
    }
}

/// A child-run failure as the API's error envelope.
fn task_error(error: crate::execution_host::ExecutionError) -> ApiError {
    use crate::execution_host::ExecutionError;
    let (code, like) = match error {
        ExecutionError::Unavailable => ("unavailable", ErrorCode::Unavailable),
        ExecutionError::Closed => ("closed", ErrorCode::Unavailable),
        ExecutionError::Capacity => ("capacity", ErrorCode::Capacity),
        ExecutionError::InvalidBinding => ("invalid-binding", ErrorCode::Invalid),
        ExecutionError::InvalidContext => ("invalid-context", ErrorCode::Invalid),
        ExecutionError::InvalidTask => ("invalid-task", ErrorCode::NotFound),
        ExecutionError::WorkerLost => ("worker-lost", ErrorCode::Unavailable),
    };
    ApiError::domain(code, like, format!("child run: {code}"))
}

/// A terminal child run in its wire form.
fn wire_outcome(outcome: crate::execution_host::TaskOutcome) -> runtara_workflow_api::TaskOutcome {
    use crate::execution_host::TaskOutcome;
    use crate::lifecycle::WorkflowWake;
    use runtara_workflow_api::{SignalWait, Wake};
    match outcome {
        TaskOutcome::Completed(bytes) => runtara_workflow_api::TaskOutcome::Completed(bytes),
        TaskOutcome::Failed(error) => {
            runtara_workflow_api::TaskOutcome::Failed(runtara_workflow_api::ErrorInfo {
                code: error.code,
                message: error.message,
                category: error.category,
                severity: error.severity,
                retryable: error.retryable,
                retry_after_ms: error.retry_after_ms,
                attributes: error.attributes.map(serde_json::Value::String),
                details: error.details.map(serde_json::Value::String),
            })
        }
        TaskOutcome::Suspended(wakes) => runtara_workflow_api::TaskOutcome::Suspended(
            wakes
                .into_iter()
                .map(|wake| match wake {
                    WorkflowWake::At(at) => Wake::At(at),
                    WorkflowWake::OnSignal(wait) => Wake::OnSignal(SignalWait {
                        checkpoint_id: wait.checkpoint_id,
                        deadline_ms: wait.deadline_ms,
                    }),
                    WorkflowWake::OnResume => Wake::OnResume,
                    WorkflowWake::Instances(id) => Wake::Instances(id),
                })
                .collect(),
        ),
        TaskOutcome::Cancelled => runtara_workflow_api::TaskOutcome::Cancelled,
        TaskOutcome::TimedOut => runtara_workflow_api::TaskOutcome::TimedOut,
        TaskOutcome::Trapped(message) => runtara_workflow_api::TaskOutcome::Trapped(message),
    }
}

fn not_serving() -> ApiError {
    ApiError::new(
        ErrorCode::Fenced,
        "this launch no longer serves the workflow API",
    )
}

#[async_trait]
impl WorkflowApiBackend for Backend {
    async fn launch_state(&self, binding: &Binding) -> Result<LaunchState, ApiError> {
        // A launch that is not registered with exactly this binding (ended,
        // replaced, or another epoch) no longer holds the run.
        let Some(slot) = self.0.slot(binding) else {
            return Ok(LaunchState::Fenced);
        };
        let runtime = slot.runtime.upgrade();
        // Nor does one whose writes the store refused for another owner.
        if runtime.as_ref().is_some_and(|runtime| runtime.superseded()) {
            return Ok(LaunchState::Fenced);
        }
        let posted = slot
            .outcome
            .lock()
            .expect("workflow API outcome poisoned")
            .is_some();
        if posted {
            return Ok(LaunchState::Finished);
        }
        // Nor one whose lease was revoked (recovered elsewhere, stopped)
        // while it made no write the store could refuse: an outbound call,
        // SQL or a child run is fenced too. The lease is read again at most
        // every LEASE_RECHECK.
        if let Some(runtime) = runtime {
            let due = {
                let mut checked = slot
                    .lease_checked
                    .lock()
                    .expect("workflow API lease check poisoned");
                let due = checked.is_none_or(|at| at.elapsed() >= LEASE_RECHECK);
                if due {
                    *checked = Some(std::time::Instant::now());
                }
                due
            };
            if due && !runtime.holds_run().await {
                return Ok(LaunchState::Fenced);
            }
        }
        Ok(LaunchState::Active)
    }

    async fn run_info(&self, binding: &Binding) -> Result<RunInfo, ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let mut info = slot.info.clone();
        // Debug mode, and the instance of a run its host did not bind, are the
        // run host's to say, asked when the guest asks.
        if let Some(runtime) = slot.runtime.upgrade() {
            info.debug = runtime.debug_mode_enabled().map_err(host_error)?;
            if info.instance_id.is_empty() {
                info.instance_id = runtime.instance_id().map_err(host_error)?;
            }
        }
        Ok(info)
    }

    async fn run_input(&self, binding: &Binding) -> Result<Vec<u8>, ApiError> {
        Ok(self.0.slot(binding).ok_or_else(not_serving)?.input.clone())
    }

    async fn post_outcome(
        &self,
        binding: &Binding,
        outcome: PostedOutcome,
    ) -> Result<(), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        // A launch that lost the run must not be told its outcome was taken.
        if let Some(runtime) = slot.runtime.upgrade()
            && !runtime.holds_run().await
        {
            return Err(ApiError::new(
                ErrorCode::Fenced,
                "this launch no longer holds the run's lease",
            ));
        }
        let mut outcome = outcome;
        if let PostedOutcome::Suspended(suspension) = &mut outcome {
            for wait_id in slot
                .pending_waits
                .lock()
                .expect("pending waits poisoned")
                .iter()
            {
                let wake = runtara_workflow_api::Wake::Instances(wait_id.clone());
                if !suspension.wakes.contains(&wake) {
                    suspension.wakes.push(wake);
                }
            }
        }
        let mut posted = slot.outcome.lock().expect("workflow API outcome poisoned");
        if posted.is_some() {
            return Err(ApiError::new(
                ErrorCode::Finished,
                "the run already posted its outcome",
            ));
        }
        *posted = Some(outcome);
        Ok(())
    }

    async fn sleep(&self, binding: &Binding, ms: u64) -> Result<(), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        // The run waits in process meanwhile; a standalone launch has none.
        let _waiting = slot
            .runtime
            .upgrade()
            .and_then(|runtime| runtime.in_process_wait());
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(())
    }

    async fn alarm(&self, binding: &Binding, ms: u64) -> Result<(), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let alarm = slot.controls.cleanup_alarm.clone().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Denied,
                "cleanup alarms are not available for this execution",
            )
        })?;
        // The alarm never resolves on its own except by expiring, which
        // latched the abort; dropping this call disarms it.
        let _ = alarm.arm(ms).await;
        if slot.controls.alarm_shortens_deadline {
            (slot.controls.shorten_deadline)(std::time::Duration::ZERO);
        }
        Ok(())
    }

    async fn set_deadline(&self, binding: &Binding, abort_after_ms: u64) -> Result<(), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        (slot.controls.shorten_deadline)(std::time::Duration::from_millis(abort_after_ms));
        Ok(())
    }

    async fn get_checkpoint(
        &self,
        binding: &Binding,
        id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        self.runtime(binding)?
            .get_checkpoint(id.to_string())
            .await
            .map_err(host_error)
    }

    async fn put_checkpoint(
        &self,
        binding: &Binding,
        id: &str,
        state: Vec<u8>,
    ) -> Result<CheckpointWrite, ApiError> {
        let result = self
            .runtime(binding)?
            .checkpoint(id.to_string(), state.clone())
            .await
            .map_err(host_error)?;
        Ok(CheckpointWrite {
            stored: !result.found,
            // The host returns the stored state only on a hit.
            state: if result.found { result.state } else { state },
            pending_signal: result.pending_signal.map(lifecycle_signal),
            custom_signal: result.custom_signal.map(|signal| CustomSignalNotice {
                signal_id: signal.signal_id,
                checkpoint_id: signal.checkpoint_id,
            }),
        })
    }

    async fn record_retry_attempt(
        &self,
        binding: &Binding,
        id: &str,
        attempt: u32,
        error: Option<String>,
    ) -> Result<(), ApiError> {
        self.runtime(binding)?
            .record_retry_attempt(id.to_string(), attempt, error)
            .await
            .map_err(host_error)
    }

    async fn poll_lifecycle_signal(
        &self,
        binding: &Binding,
    ) -> Result<Option<LifecycleSignal>, ApiError> {
        Ok(self
            .runtime(binding)?
            .poll_signal()
            .await
            .map_err(host_error)?
            .map(lifecycle_signal))
    }

    async fn ack_lifecycle_signal(
        &self,
        binding: &Binding,
        command_id: &str,
        signal_type: &str,
    ) -> Result<bool, ApiError> {
        self.runtime(binding)?
            .handle_checkpoint_signal(signal_type.to_string(), command_id.to_string())
            .await
            .map_err(host_error)
    }

    async fn check_lifecycle_signals(&self, binding: &Binding) -> Result<bool, ApiError> {
        self.runtime(binding)?
            .check_signals()
            .await
            .map_err(host_error)
    }

    async fn lifecycle_cancelled(&self, binding: &Binding) -> Result<bool, ApiError> {
        self.runtime(binding)?
            .is_cancelled()
            .await
            .map_err(host_error)
    }

    async fn poll_custom_signal(
        &self,
        binding: &Binding,
        checkpoint_id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        self.runtime(binding)?
            .poll_custom_signal(checkpoint_id.to_string())
            .await
            .map_err(host_error)
    }

    async fn now_ms(&self, binding: &Binding) -> Result<u64, ApiError> {
        self.runtime(binding)?.now_ms().map_err(host_error)
    }

    async fn heartbeat(&self, binding: &Binding) -> Result<(), ApiError> {
        self.runtime(binding)?.heartbeat().await.map_err(host_error)
    }

    async fn start_task(
        &self,
        binding: &Binding,
        start: runtara_workflow_api::TaskStart,
        input: Vec<u8>,
    ) -> Result<runtara_workflow_api::TaskStarted, ApiError> {
        if let Some(child) = ChildRunBinding::parse(&start.binding)? {
            return self.start_child_run(binding, child, start, input).await;
        }
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let execution = slot
            .controls
            .execution
            .clone()
            .ok_or_else(|| task_error(crate::execution_host::ExecutionError::Unavailable))?;
        // Every start is a task of its own: concurrent calls of one call
        // site (a parallel Split's iterations) share their context.
        let mut tasks = slot.tasks.lock().expect("launch tasks poisoned");
        let handle = execution
            .start_task(crate::execution_host::StartRequest {
                binding: start.binding.clone(),
                entry: match &start.entry {
                    runtara_workflow_api::TaskEntry::Capability(id) => {
                        crate::execution_host::Entry::Capability(id.clone())
                    }
                    runtara_workflow_api::TaskEntry::Workflow => {
                        crate::execution_host::Entry::Workflow
                    }
                },
                input,
                context: crate::execution_host::InvocationContext {
                    path: start.context.path.clone(),
                    attempt: start.context.attempt,
                },
            })
            .map_err(task_error)?;
        tasks.next += 1;
        let id = format!("{IN_PROCESS_TASK_PREFIX}{}", tasks.next);
        tasks.live.insert(id.clone(), Arc::new(handle));
        Ok(runtara_workflow_api::TaskStarted {
            task_id: id,
            instance_id: None,
        })
    }

    async fn task_outcome(
        &self,
        binding: &Binding,
        task_id: &str,
        wait_ms: u64,
    ) -> Result<Option<runtara_workflow_api::TaskOutcome>, ApiError> {
        if !self.is_live_task(binding, task_id)? {
            return self.child_run_outcome(binding, task_id, wait_ms).await;
        }
        let task = self.task(binding, task_id)?;
        match tokio::time::timeout(std::time::Duration::from_millis(wait_ms), task.join()).await {
            Ok(outcome) => outcome
                .map(|outcome| Some(wire_outcome(outcome)))
                .map_err(task_error),
            Err(_) => Ok(None),
        }
    }

    async fn cancel_task(
        &self,
        binding: &Binding,
        task_id: &str,
    ) -> Result<runtara_workflow_api::CancelStatus, ApiError> {
        use crate::execution_host::CancelStatus;
        if !self.is_live_task(binding, task_id)? {
            return self.cancel_child_run(binding, task_id).await;
        }
        Ok(
            match self
                .task(binding, task_id)?
                .request_cancel()
                .map_err(task_error)?
            {
                CancelStatus::Requested => runtara_workflow_api::CancelStatus::Requested,
                CancelStatus::AlreadyRequested => {
                    runtara_workflow_api::CancelStatus::AlreadyRequested
                }
                CancelStatus::AlreadyTerminal => {
                    runtara_workflow_api::CancelStatus::AlreadyTerminal
                }
            },
        )
    }

    async fn release_task(&self, binding: &Binding, task_id: &str) -> Result<(), ApiError> {
        if !self.is_live_task(binding, task_id)? {
            // A child run that is still going is cancelled; a finished one
            // needs nothing.
            if self.child_run_outcome(binding, task_id, 0).await?.is_none() {
                self.cancel_child_run(binding, task_id).await?;
            }
            return Ok(());
        }
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let task = slot
            .tasks
            .lock()
            .expect("launch tasks poisoned")
            .live
            .get(task_id)
            .map(Arc::clone);
        // Releasing a released task is a no-op.
        if let Some(task) = task {
            task.release().await.map_err(task_error)?;
            slot.tasks
                .lock()
                .expect("launch tasks poisoned")
                .live
                .remove(task_id);
        }
        Ok(())
    }

    /// The run parks at the breakpoint, so the pause answers at once.
    async fn breakpoint(&self, binding: &Binding, _wait_ms: u64) -> Result<(), ApiError> {
        self.runtime(binding)?
            .breakpoint_pause()
            .await
            .map_err(host_error)
    }

    async fn register_input(
        &self,
        binding: &Binding,
        descriptor: Vec<u8>,
        deadline_ms: Option<u64>,
    ) -> Result<(), ApiError> {
        self.runtime(binding)?
            .register_input(descriptor, deadline_ms)
            .await
            .map_err(input_error)
    }

    async fn poll_input(&self, binding: &Binding, signal_id: &str) -> Result<InputRead, ApiError> {
        self.runtime(binding)?
            .poll_input(signal_id.to_string())
            .await
            .map(input_read)
            .map_err(input_error)
    }

    async fn close_input(&self, binding: &Binding, signal_id: &str) -> Result<InputRead, ApiError> {
        let closed = self
            .runtime(binding)?
            .close_input(signal_id.to_string())
            .await;
        // An abandonment the host cannot confirm ends the run, so guest error
        // handling never continues with an orphaned request (the old ABI's
        // import trapped).
        if closed.is_err()
            && let Some(slot) = self.0.slot(binding)
        {
            *slot.fault.lock().expect("launch fault poisoned") =
                Some("managed input abandonment could not be confirmed".into());
        }
        closed.map(input_read).map_err(input_error)
    }

    async fn register_wait(
        &self,
        binding: &Binding,
        key: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        let wait_id = wait_instances_id(key).map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        let request: InstanceWaitRequest = serde_json::from_value(request)
            .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("wait request: {e}")))?;
        // No targets settle at once, without a registration.
        if request.instance_ids.is_empty() {
            let poll = InstanceWaitPoll {
                mode: request.mode,
                resolution: Some(InstanceWaitResolution::Empty),
                finished: Vec::new(),
                remaining: Vec::new(),
                deadline_ms: None,
            };
            return wait_read(&wait_id, &poll);
        }
        let (host, authority) = self.wait_service(binding)?;
        match host.register(&authority, &wait_id, request).await {
            Ok(poll) => {
                if let Some(slot) = self.0.slot(binding) {
                    slot.track_wait(&wait_id, &poll);
                }
                wait_read(&wait_id, &poll)
            }
            Err(error) => {
                // A failed registration is not kept: a retry registers afresh.
                self.runtime(binding)?
                    .operation_wait_close(wait_id)
                    .await
                    .map_err(host_error)?;
                Err(wait_error(error))
            }
        }
    }

    async fn poll_wait(&self, binding: &Binding, key: &str) -> Result<serde_json::Value, ApiError> {
        let wait_id = wait_instances_id(key).map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        let (host, authority) = self.wait_service(binding)?;
        let poll = host.poll(&authority, &wait_id).await.map_err(wait_error)?;
        if let Some(slot) = self.0.slot(binding) {
            slot.track_wait(&wait_id, &poll);
        }
        wait_read(&wait_id, &poll)
    }

    async fn release_wait(&self, binding: &Binding, key: &str) -> Result<(), ApiError> {
        let wait_id = wait_instances_id(key).map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        if let Some(slot) = self.0.slot(binding) {
            slot.pending_waits
                .lock()
                .expect("pending waits poisoned")
                .retain(|id| id != &wait_id);
        }
        self.runtime(binding)?
            .operation_release(wait_id)
            .await
            .map_err(host_error)
    }

    async fn enter_operation(
        &self,
        binding: &Binding,
        checkpoint_key: &str,
        attempt: u32,
        load: bool,
    ) -> Result<(String, Option<Vec<u8>>), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let identity = parse_agent_operation_key(checkpoint_key, attempt)
            .map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        let operation_id = identity.op_hash.clone();
        let continuation = if load {
            self.runtime(binding)?
                .operation_continuation_load(operation_id.clone(), attempt)
                .await
                .map_err(host_error)?
        } else {
            None
        };
        let mut entered = slot.operations.lock().expect("operations poisoned");
        if entered.contains_key(&operation_id) {
            return Err(ApiError::domain(
                "already-entered",
                ErrorCode::ReplayConflict,
                "this call site's operation is already entered",
            ));
        }
        entered.insert(operation_id.clone(), identity);
        Ok((operation_id, continuation))
    }

    async fn suspend_operation(
        &self,
        binding: &Binding,
        operation_id: &str,
        state: Vec<u8>,
        wakes: Vec<runtara_workflow_api::Wake>,
    ) -> Result<(), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let identity = slot
            .operations
            .lock()
            .expect("operations poisoned")
            .get(operation_id)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such entered operation"))?;
        // An agent may wake only `at`; it registers no instance wait of its own.
        let invalid = |message: String| {
            ApiError::domain(
                "agent-invalid-suspension",
                ErrorCode::Invalid,
                format!(
                    "{}: {message}",
                    runtara_agent_suspension::AGENT_INVALID_SUSPENSION
                ),
            )
        };
        let contract = wakes
            .iter()
            .map(|wake| match wake {
                runtara_workflow_api::Wake::At(at) => Some(runtara_agent_suspension::Wake::At(*at)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid("an agent may wake only `at`".into()))?;
        runtara_agent_suspension::validate_suspension(&contract, &state).map_err(invalid)?;
        self.runtime(binding)?
            .operation_continuation_store(identity.op_hash, identity.attempt, state)
            .await
            .map_err(host_error)?;
        slot.operations
            .lock()
            .expect("operations poisoned")
            .remove(operation_id);
        Ok(())
    }

    async fn exit_operation(
        &self,
        binding: &Binding,
        operation_id: &str,
        failed: bool,
    ) -> Result<(), ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let left = slot
            .operations
            .lock()
            .expect("operations poisoned")
            .remove(operation_id);
        // A failure is not checkpointed, so a replay or retry starts the
        // operation afresh: its wait and its continuation go.
        if failed && left.is_some() {
            let runtime = self.runtime(binding)?;
            runtime
                .operation_wait_close(operation_id.to_string())
                .await
                .map_err(host_error)?;
            runtime
                .operation_release(operation_id.to_string())
                .await
                .map_err(host_error)?;
        }
        Ok(())
    }

    async fn release_operation(
        &self,
        binding: &Binding,
        operation_id: &str,
    ) -> Result<(), ApiError> {
        if operation_id.len() != 64 || !operation_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                "an operation id is a sha256 hex digest",
            ));
        }
        self.runtime(binding)?
            .operation_release(operation_id.to_string())
            .await
            .map_err(host_error)
    }

    async fn describe_connection(
        &self,
        binding: &Binding,
        connection_id: &str,
    ) -> Result<Vec<u8>, ApiError> {
        self.resolver(binding)?
            .describe(connection_id.to_string())
            .await
            .map_err(connection_error)
    }

    async fn resolve_resource(
        &self,
        binding: &Binding,
        connection_id: &str,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, ApiError> {
        self.resolver(binding)?
            .resolve_resource(connection_id.to_string(), request)
            .await
            .map_err(connection_error)
    }

    async fn sql(
        &self,
        binding: &Binding,
        connection_id: &str,
        operation: SqlOperation,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, ApiError> {
        self.0.slot(binding).ok_or_else(not_serving)?;
        let database = database_for_run(self.0.database.get(), Some(&binding.tenant_id))
            .map_err(|e| ApiError::new(ErrorCode::Unavailable, e))?;
        let operation = match operation {
            SqlOperation::Query => Operation::Query,
            SqlOperation::Execute => Operation::Execute,
            SqlOperation::Batch => Operation::Batch,
        };
        database
            .call(operation, connection_id.to_string(), request)
            .await
            .map_err(database_error)
    }

    async fn http(
        &self,
        binding: &Binding,
        meta: OutboundMeta,
        body: Option<Vec<u8>>,
    ) -> Result<Result<OutboundReply, (OutboundFailure, Vec<u8>)>, ApiError> {
        self.0.slot(binding).ok_or_else(not_serving)?;
        // A launch outside any run has no instance.
        let host = crate::outbound_http::for_run(
            self.0.outbound_http.get(),
            Some(&binding.tenant_id),
            Some(binding.instance_id.as_str()).filter(|instance| !instance.is_empty()),
        )
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e))?;
        let destination = match meta.destination {
            OutboundDestination::Connection(target) => {
                Destination::Connection(ConnectionDestination {
                    connection_id: target.connection_id,
                    url: target.url,
                    endpoint: target.endpoint,
                    endpoint_ref: target.endpoint_ref,
                    ai_provider: target.ai_provider,
                    aws_service: target.aws_service,
                })
            }
            OutboundDestination::Public(url) => Destination::Public(url),
        };
        let request = RequestOptions {
            destination,
            method: meta.method,
            headers: meta.headers,
            body,
            timeout_ms: meta.timeout_ms,
            max_response_bytes: meta.max_response_bytes,
        };
        Ok(match send_bounded(&host, request, None).await {
            Ok(response) => Ok(OutboundReply {
                status: response.status,
                headers: response.headers,
                body: response.body,
            }),
            Err(error) => Err((
                OutboundFailure {
                    code: error.code,
                    message: error.message,
                    status: error.status,
                    retry_after_ms: error.retry_after_ms,
                },
                error.body,
            )),
        })
    }

    async fn set_state(
        &self,
        binding: &Binding,
        key: &str,
        patch: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), ApiError> {
        let operation_id =
            state_operation_id(key).map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        let (host, authority) = self.state_service(binding)?;
        host.set(&authority, &operation_id, patch)
            .await
            .map_err(state_error)
    }

    async fn get_state(
        &self,
        binding: &Binding,
        key: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
        check_state_read_key(key).map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        let (host, authority) = self.state_service(binding)?;
        host.get(&authority).await.map_err(state_error)
    }

    async fn append_events(&self, binding: &Binding, events: Vec<Event>) -> Result<(), ApiError> {
        let runtime = self.runtime(binding)?;
        let mut batch = Vec::with_capacity(events.len());
        for event in events {
            let payload = serde_json::to_vec(&event.payload)
                .map_err(|e| ApiError::new(ErrorCode::Invalid, e.to_string()))?;
            batch.push((event.kind, payload));
        }
        // A launch over its event budget is held until the budget refills,
        // and refused only when that would take too long.
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let bytes = batch
            .iter()
            .map(|(kind, payload)| kind.len() + payload.len())
            .sum();
        let taken = slot.events.lock().expect("event budget poisoned").take(
            batch.len(),
            bytes,
            tokio::time::Instant::now().into_std(),
        );
        match taken {
            Ok(wait) if !wait.is_zero() => tokio::time::sleep(wait).await,
            Ok(_) => {}
            Err(wait) => {
                return Err(ApiError::new(
                    ErrorCode::Capacity,
                    "this launch is over its event budget",
                )
                .with_retry_after(wait.as_millis() as u64));
            }
        }
        for (kind, payload) in batch {
            runtime
                .custom_event(kind, payload)
                .await
                .map_err(host_error)?;
        }
        Ok(())
    }

    /// The run acts as itself: the binding's tenant and instance, and an
    /// operation only when this launch entered it and has not left it.
    async fn control(
        &self,
        binding: &Binding,
        operation: Option<&str>,
        call: ControlCall,
    ) -> Result<serde_json::Value, ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        let operation = match operation {
            Some(id) => Some(
                slot.operations
                    .lock()
                    .expect("operations poisoned")
                    .get(id)
                    .map(|identity| identity.op_hash.clone())
                    .ok_or_else(|| {
                        ApiError::new(ErrorCode::NotFound, "no such entered operation")
                    })?,
            ),
            None => None,
        };
        // Control acts as the binding's run; a standalone launch has none,
        // and only an agent test call acts for its tenant without one.
        let caller = match slot.standalone.as_ref().map(|(_, control)| control) {
            None => Some(binding.instance_id.clone()),
            Some(StandaloneControl::Tenant) => None,
            Some(StandaloneControl::Denied) => {
                return Err(ApiError::new(
                    ErrorCode::Denied,
                    "control is not available in this context",
                ));
            }
        };
        // A host without control refuses, as its host-linked calls did.
        let host = self.0.control.get().cloned().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Denied,
                "control is not available in this context",
            )
        })?;
        let authority = ControlAuthority {
            tenant: binding.tenant_id.clone(),
            caller,
            operation,
        };
        control_host::json::dispatch(host.as_ref(), &authority, call)
            .await
            .map_err(|error| control_host::json::api_error(&error))
    }

    /// The run's artifact pins and launch kind decide the call before any
    /// credential lookup, as on the old ABI; the tenant is the binding's.
    async fn trusted(
        &self,
        binding: &Binding,
        operation: Option<&str>,
        call: TrustedCall,
    ) -> Result<Vec<u8>, ApiError> {
        let slot = self.0.slot(binding).ok_or_else(not_serving)?;
        if let Some(id) = operation
            && !slot
                .operations
                .lock()
                .expect("operations poisoned")
                .contains_key(id)
        {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                "no such entered operation",
            ));
        }
        // A host without trusted execution refuses, as its host-linked
        // calls did; retrying cannot help.
        let executor = self.0.trusted.get().cloned().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Denied,
                "trusted execution is not available in this context",
            )
        })?;
        // A run's calls are bound by its artifact's pins; a standalone
        // launch's by the trust its store had.
        let (pins, launch) = match slot.standalone.as_ref().map(|(trust, _)| trust) {
            None => (
                Some(slot.controls.trusted_pins.as_ref()),
                self.runtime(binding)?.trusted_launch(),
            ),
            Some(StandaloneTrust::Denied) => {
                return Err(ApiError::new(
                    ErrorCode::Denied,
                    "trusted execution is not available in this context",
                ));
            }
            Some(StandaloneTrust::Pinned(pins)) => {
                (Some(pins.as_ref()), crate::trusted::TrustedLaunch::Start)
            }
            Some(StandaloneTrust::Unpinned) => (None, crate::trusted::TrustedLaunch::Start),
        };
        executor
            .invoke_pinned(
                pins,
                launch,
                &binding.tenant_id,
                &call.agent_id,
                &call.capability_id,
                &call.connection_id,
                call.input,
                tokio::time::Instant::now() + crate::trusted::TIME_LIMIT,
            )
            .await
            .map_err(|envelope| trusted_error(&envelope))
    }
}

/// What a standalone launch may do through `/v1/trusted`.
#[derive(Clone, Debug)]
pub enum StandaloneTrust {
    /// A direct agent call, not version-bound by any artifact (agent test
    /// calls, as their host-linked path was).
    Unpinned,
    /// Bound by the calling artifact's trusted pins (an isolated child).
    Pinned(Arc<std::collections::HashSet<String>>),
    /// No trusted execution, as for a store given no trusted executor.
    Denied,
}

/// What a standalone launch may do through `/v1/instances`.
#[derive(Clone, Copy, Debug)]
pub enum StandaloneControl {
    /// Act for the tenant with no calling run (agent test calls, as their
    /// host-linked path did): reads, and mutations the service refuses.
    Tenant,
    /// No control, as for a run that was not granted it.
    Denied,
}

/// The runtime of a standalone launch, which has none: only ever behind a
/// dangling `Weak`, never called.
struct NoRun;

#[async_trait]
impl RuntimeHost for NoRun {
    fn instance_id(&self) -> Result<String, String> {
        unreachable!()
    }
    async fn custom_event(&self, _: String, _: Vec<u8>) -> Result<(), String> {
        unreachable!()
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        unreachable!()
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        unreachable!()
    }
    async fn heartbeat(&self) -> Result<(), String> {
        unreachable!()
    }
    async fn poll_signal(&self) -> Result<Option<crate::runtime_host::RuntimeSignalInfo>, String> {
        unreachable!()
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        unreachable!()
    }
    async fn check_signals(&self) -> Result<bool, String> {
        unreachable!()
    }
    async fn poll_custom_signal(&self, _: String) -> Result<Option<Vec<u8>>, String> {
        unreachable!()
    }
    async fn get_checkpoint(&self, _: String) -> Result<Option<Vec<u8>>, String> {
        unreachable!()
    }
    async fn checkpoint(
        &self,
        _: String,
        _: Vec<u8>,
    ) -> Result<crate::runtime_host::RuntimeCheckpointResult, String> {
        unreachable!()
    }
    async fn handle_checkpoint_signal(&self, _: String, _: String) -> Result<bool, String> {
        unreachable!()
    }
    async fn record_retry_attempt(
        &self,
        _: String,
        _: u32,
        _: Option<String>,
    ) -> Result<(), String> {
        unreachable!()
    }
    async fn durable_sleep_checkpoint(&self, _: String, _: Vec<u8>, _: u64) -> Result<(), String> {
        unreachable!()
    }
}

/// A trusted call's agent error envelope as the API's `trusted-error`, with
/// the envelope in `details` and the closest generic status.
fn trusted_error(envelope: &str) -> ApiError {
    let Ok(details) = serde_json::from_str::<serde_json::Value>(envelope) else {
        return ApiError::new(ErrorCode::Unavailable, "the trusted call failed");
    };
    let code = details
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let like = if details
        .get("retryable")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        ErrorCode::Unavailable
    } else if code.ends_with("_DENIED") || code == "TRUSTED_VERSION_REQUIRED" {
        ErrorCode::Denied
    } else if code == "TRUSTED_TIMEOUT" {
        ErrorCode::Timeout
    } else if code == "TRUSTED_UNAVAILABLE" {
        ErrorCode::Unavailable
    } else if code.ends_with("_LIMIT") {
        ErrorCode::TooLarge
    } else {
        ErrorCode::Invalid
    };
    let message = details
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("the trusted call failed")
        .to_string();
    ApiError::domain("trusted-error", like, message).with_details(details)
}

fn lifecycle_signal(signal: crate::runtime_host::RuntimeSignalInfo) -> LifecycleSignal {
    LifecycleSignal {
        signal_type: signal.signal_type,
        command_id: signal.command_id,
        payload: signal.payload,
        checkpoint_id: signal.checkpoint_id,
    }
}

/// Serves a request through the router in process.
struct RouterTransport(axum::Router);

impl WorkflowApiTransport for RouterTransport {
    fn call(&self, request: http::Request<Bytes>) -> WorkflowApiFuture {
        let router = self.0.clone();
        Box::pin(async move {
            let response = match router.oneshot(request.map(Body::from)).await {
                Ok(response) => response,
                Err(never) => match never {},
            };
            let (parts, body) = response.into_parts();
            // The router caps bodies itself; this bound only guards the copy.
            let body = to_bytes(body, Caps::default().response_body_bytes)
                .await
                .unwrap_or_default();
            http::Response::from_parts(parts, body)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(epoch: u64) -> Binding {
        Binding {
            tenant_id: "tenant-a".into(),
            instance_id: "instance-1".into(),
            launch_id: "launch-1".into(),
            root_lease_epoch: epoch,
        }
    }

    /// A launch whose host is already gone: its run services are fenced.
    fn no_runtime() -> Weak<dyn RuntimeHost> {
        Weak::<NoRun>::new()
    }

    fn controls() -> WorkflowApiControls {
        WorkflowApiControls {
            shorten_deadline: Arc::new(|_| {}),
            trusted_pins: Arc::default(),
            cleanup_alarm: None,
            execution: None,
            alarm_shortens_deadline: false,
        }
    }

    fn info() -> RunInfo {
        RunInfo {
            instance_id: "instance-1".into(),
            tenant_id: "tenant-a".into(),
            workflow_id: None,
            version: None,
            attempt: None,
            launch_kind: "start".into(),
            debug: false,
            deadline_ms: None,
            api_version: runtara_workflow_api::api_version(),
            capability_id: None,
        }
    }

    #[tokio::test]
    async fn a_remote_launch_is_served_through_the_public_router() {
        let launches = Arc::new(ApiLaunches::default());
        let shortened = Arc::new(Mutex::new(None));
        let deadline = {
            let shortened = Arc::clone(&shortened);
            WorkflowApiControls::remote(
                Arc::new(move |after| *shortened.lock().unwrap() = Some(after)),
                Arc::default(),
            )
        };
        launches
            .begin(
                binding(1),
                info(),
                b"{\"data\":1}".to_vec(),
                no_runtime(),
                deadline,
            )
            .unwrap();
        let app = launches.router();
        let call = |method: &str, path: &str, body: &'static [u8]| {
            let mut request = axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("idempotency-key", format!("{method}-{path}"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            request.extensions_mut().insert(binding(1));
            app.clone().oneshot(request)
        };
        let input = call("GET", "/v1/run/input", b"").await.unwrap();
        assert_eq!(input.status(), 200);
        assert_eq!(
            to_bytes(input.into_body(), 1 << 20).await.unwrap(),
            &b"{\"data\":1}"[..]
        );
        let deadline = call("PUT", "/v1/run/deadline", b"{\"abortAfterMs\":250}")
            .await
            .unwrap();
        assert!(deadline.status().is_success(), "{}", deadline.status());
        assert_eq!(
            *shortened.lock().unwrap(),
            Some(std::time::Duration::from_millis(250))
        );
        // No in-process child runs: a remote launch's task calls are refused.
        let task = call("POST", "/v1/tasks", b"{}").await.unwrap();
        assert!(task.status().is_client_error(), "{}", task.status());
        assert_eq!(launches.finish("launch-1"), None);
    }

    #[tokio::test]
    async fn a_remote_run_ends_as_a_local_one_would() {
        use crate::{InvokeExit, RemoteGuestExit, remote_invoke_result};
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let begin = |n: u64| {
            let mut binding = binding(1);
            binding.launch_id = format!("launch-{n}");
            launches
                .begin(
                    binding.clone(),
                    info(),
                    Vec::new(),
                    no_runtime(),
                    controls(),
                )
                .unwrap();
            binding
        };
        let ended = |n: u64, how: RemoteGuestExit| {
            remote_invoke_result(
                &launches,
                &format!("launch-{n}"),
                how,
                std::time::Duration::ZERO,
            )
            .exit
        };
        let completed = begin(1);
        backend
            .post_outcome(&completed, PostedOutcome::Completed(b"out".to_vec()))
            .await
            .unwrap();
        assert!(
            matches!(ended(1, RemoteGuestExit::Returned), InvokeExit::Completed(out) if out == b"out")
        );
        // A return without an outcome, a trap and a timeout end as locally.
        begin(2);
        assert!(matches!(
            ended(2, RemoteGuestExit::Returned),
            InvokeExit::Trapped { .. }
        ));
        let trapped = begin(3);
        backend
            .post_outcome(&trapped, PostedOutcome::Completed(Vec::new()))
            .await
            .unwrap();
        assert!(matches!(
            ended(3, RemoteGuestExit::Trapped("vm lost".into())),
            InvokeExit::Trapped { reason } if reason == "vm lost"
        ));
        begin(4);
        assert!(matches!(
            ended(4, RemoteGuestExit::Timeout),
            InvokeExit::Timeout
        ));
        // A non-zero exit: what was posted decides; with nothing, the code
        // names the trap.
        let failed = begin(5);
        backend
            .post_outcome(&failed, PostedOutcome::Completed(b"done".to_vec()))
            .await
            .unwrap();
        assert!(launches.posted("launch-5"));
        assert!(
            matches!(ended(5, RemoteGuestExit::ExitedWith(1)), InvokeExit::Completed(out) if out == b"done")
        );
        assert!(!launches.posted("launch-5"), "finished");
        begin(6);
        assert!(!launches.posted("launch-6"));
        assert!(matches!(
            ended(6, RemoteGuestExit::ExitedWith(1)),
            InvokeExit::Trapped { reason } if reason.contains("exited with code 1")
        ));
        // Each one stopped being served.
        assert_eq!(
            backend.launch_state(&completed).await.unwrap(),
            LaunchState::Fenced
        );
    }

    #[tokio::test]
    async fn a_launch_serves_until_it_finishes() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        launches
            .begin(binding(1), info(), b"in".to_vec(), no_runtime(), controls())
            .unwrap();
        assert!(
            launches
                .begin(binding(1), info(), Vec::new(), no_runtime(), controls())
                .is_err()
        );

        assert_eq!(
            backend.launch_state(&binding(1)).await.unwrap(),
            LaunchState::Active
        );
        assert_eq!(backend.run_input(&binding(1)).await.unwrap(), b"in");
        // Another epoch of the same launch id is not this launch.
        assert_eq!(
            backend.launch_state(&binding(2)).await.unwrap(),
            LaunchState::Fenced
        );

        backend
            .post_outcome(&binding(1), PostedOutcome::Completed(b"out".to_vec()))
            .await
            .unwrap();
        assert_eq!(
            backend.launch_state(&binding(1)).await.unwrap(),
            LaunchState::Finished
        );
        assert!(
            backend
                .post_outcome(&binding(1), PostedOutcome::Completed(Vec::new()))
                .await
                .is_err()
        );

        assert_eq!(
            launches.finish("launch-1"),
            Some(PostedOutcome::Completed(b"out".to_vec()))
        );
        assert_eq!(
            backend.launch_state(&binding(1)).await.unwrap(),
            LaunchState::Fenced
        );
        assert_eq!(launches.finish("launch-1"), None);
    }

    /// A runtime whose run's lease is held or lost (as recovery leaves one),
    /// counting how often it is read and the events appended.
    struct Lease {
        holds: std::sync::atomic::AtomicBool,
        reads: std::sync::atomic::AtomicUsize,
        events: std::sync::atomic::AtomicUsize,
    }

    impl Lease {
        fn new(holds: bool) -> Self {
            Self {
                holds: holds.into(),
                reads: 0.into(),
                events: 0.into(),
            }
        }
    }

    #[async_trait]
    impl RuntimeHost for Lease {
        async fn holds_run(&self) -> bool {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.holds.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn instance_id(&self) -> Result<String, String> {
            unreachable!()
        }
        async fn custom_event(&self, _: String, _: Vec<u8>) -> Result<(), String> {
            self.events
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn debug_mode_enabled(&self) -> Result<bool, String> {
            unreachable!()
        }
        async fn breakpoint_pause(&self) -> Result<(), String> {
            unreachable!()
        }
        async fn heartbeat(&self) -> Result<(), String> {
            unreachable!()
        }
        async fn poll_signal(
            &self,
        ) -> Result<Option<crate::runtime_host::RuntimeSignalInfo>, String> {
            unreachable!()
        }
        async fn is_cancelled(&self) -> Result<bool, String> {
            unreachable!()
        }
        async fn check_signals(&self) -> Result<bool, String> {
            unreachable!()
        }
        async fn poll_custom_signal(&self, _: String) -> Result<Option<Vec<u8>>, String> {
            unreachable!()
        }
        async fn get_checkpoint(&self, _: String) -> Result<Option<Vec<u8>>, String> {
            unreachable!()
        }
        async fn checkpoint(
            &self,
            _: String,
            _: Vec<u8>,
        ) -> Result<crate::runtime_host::RuntimeCheckpointResult, String> {
            unreachable!()
        }
        async fn handle_checkpoint_signal(&self, _: String, _: String) -> Result<bool, String> {
            unreachable!()
        }
        async fn record_retry_attempt(
            &self,
            _: String,
            _: u32,
            _: Option<String>,
        ) -> Result<(), String> {
            unreachable!()
        }
        async fn durable_sleep_checkpoint(
            &self,
            _: String,
            _: Vec<u8>,
            _: u64,
        ) -> Result<(), String> {
            unreachable!()
        }
    }

    /// A launch past its event budget is held while the budget refills, and
    /// refused once it would wait too long; what it was refused is not
    /// appended.
    #[tokio::test(start_paused = true)]
    async fn a_launch_flooding_events_is_held_then_refused() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let lease = Arc::new(Lease::new(true));
        let runtime: Arc<dyn RuntimeHost> = lease.clone();
        launches
            .begin(
                binding(1),
                info(),
                Vec::new(),
                Arc::downgrade(&runtime),
                controls(),
            )
            .unwrap();
        let batch = |n: usize| {
            (0..n)
                .map(|_| Event {
                    kind: "flood".into(),
                    payload: serde_json::json!({}),
                })
                .collect::<Vec<_>>()
        };
        let appended = || lease.events.load(std::sync::atomic::Ordering::SeqCst);
        let start = tokio::time::Instant::now();
        // The burst passes at once.
        for _ in 0..10 {
            backend
                .append_events(&binding(1), batch(1000))
                .await
                .unwrap();
        }
        assert_eq!(appended(), 10_000);
        assert!(start.elapsed() < std::time::Duration::from_millis(1));
        // The next batch waits its second.
        backend
            .append_events(&binding(1), batch(1000))
            .await
            .unwrap();
        assert_eq!(appended(), 11_000);
        assert!(start.elapsed() >= std::time::Duration::from_secs(1));
        // One that would wait over the hold is refused, and not appended.
        let error = backend
            .append_events(&binding(1), batch(11_000))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Capacity, "{error:?}");
        assert!(error.retry_after_ms.is_some_and(|ms| ms > 10_000));
        assert_eq!(appended(), 11_000);
        assert_eq!(launches.finish("launch-1"), None);
    }

    /// A launch that lost its run is told its outcome was fenced, and the
    /// outcome is not taken for the run.
    #[tokio::test]
    async fn an_outcome_from_a_launch_that_lost_its_run_is_fenced() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let runtime: Arc<dyn RuntimeHost> = Arc::new(Lease::new(false));
        launches
            .begin(
                binding(1),
                info(),
                Vec::new(),
                Arc::downgrade(&runtime),
                controls(),
            )
            .unwrap();
        let error = backend
            .post_outcome(&binding(1), PostedOutcome::Completed(b"out".to_vec()))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Fenced, "{error:?}");
        assert_eq!(launches.finish("launch-1"), None);
    }

    /// A launch whose lease was revoked is fenced on its next call, whatever
    /// the call: it need not have made a write the store refused. The lease
    /// is read at most once a second.
    #[tokio::test]
    async fn a_launch_is_fenced_once_its_lease_is_revoked() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let lease = Arc::new(Lease::new(true));
        let runtime: Arc<dyn RuntimeHost> = lease.clone();
        launches
            .begin(
                binding(1),
                info(),
                Vec::new(),
                Arc::downgrade(&runtime),
                controls(),
            )
            .unwrap();
        for _ in 0..5 {
            assert_eq!(
                backend.launch_state(&binding(1)).await.unwrap(),
                LaunchState::Active
            );
        }
        assert_eq!(lease.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        lease
            .holds
            .store(false, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(LEASE_RECHECK).await;
        assert_eq!(
            backend.launch_state(&binding(1)).await.unwrap(),
            LaunchState::Fenced
        );
    }

    /// A standalone launch's trusted calls follow the trust its store had:
    /// an isolated child stays bound by its artifact's pins, a store with no
    /// trusted executor is refused, and only an agent test call is unpinned.
    #[tokio::test]
    async fn a_standalone_launch_keeps_its_stores_trusted_pins() {
        let engine = crate::engine::build_engine(&crate::engine::EngineConfig::default()).unwrap();
        let launches = Arc::new(ApiLaunches::default());
        launches
            .set_trusted_executor(Arc::new(TrustedExecutor::new(engine)))
            .unwrap();
        let backend = Backend(launches.clone());
        let call = || runtara_workflow_api::server::TrustedCall {
            agent_id: "crypto".into(),
            capability_id: "hash".into(),
            connection_id: "connection".into(),
            input: b"{}".to_vec(),
        };
        let standalone = |id: &str, trust| {
            let binding = Binding {
                launch_id: id.into(),
                ..binding(0)
            };
            launches
                .begin_standalone(binding.clone(), trust, StandaloneControl::Denied)
                .unwrap();
            binding
        };
        let pinned = standalone("pinned", StandaloneTrust::Pinned(Arc::default()));
        let error = backend.trusted(&pinned, None, call()).await.unwrap_err();
        assert!(
            matches!(&error.code, ErrorCode::Domain { like, .. } if **like == ErrorCode::Denied),
            "{error:?}"
        );
        assert!(
            error.message.contains("approved trusted dependency"),
            "{error:?}"
        );

        let denied = standalone("denied", StandaloneTrust::Denied);
        let error = backend.trusted(&denied, None, call()).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Denied);
        assert!(error.message.contains("not available"), "{error:?}");

        // Unpinned passes the version check and reaches the executor.
        let unpinned = standalone("unpinned", StandaloneTrust::Unpinned);
        let error = backend.trusted(&unpinned, None, call()).await.unwrap_err();
        assert!(
            !error.message.contains("approved trusted dependency"),
            "{error:?}"
        );
    }

    /// The cleanup alarm is held by its call: dropping the call before it
    /// expires disarms it, and letting it run latches the run's abort, after
    /// which the launch serves nothing.
    #[tokio::test]
    async fn a_dropped_alarm_call_disarms_and_an_expired_one_aborts() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let alarm = crate::cleanup_alarm::CleanupAlarmState::default();
        let mut armed = controls();
        armed.cleanup_alarm = Some(alarm.clone());
        launches
            .begin(binding(1), info(), Vec::new(), no_runtime(), armed)
            .unwrap();

        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            backend.alarm(&binding(1), 200),
        )
        .await;
        assert!(dropped.is_err(), "the alarm call is held until it expires");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!alarm.expired(), "dropping the call disarmed the alarm");

        // A sleep is held for its duration.
        let started = std::time::Instant::now();
        backend.sleep(&binding(1), 30).await.unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));

        backend.alarm(&binding(1), 20).await.unwrap();
        assert!(alarm.expired(), "an alarm left armed aborts the run");
        assert!(matches!(
            backend.launch_state(&binding(1)).await.unwrap(),
            LaunchState::Fenced
        ));
        assert!(backend.run_info(&binding(1)).await.is_err());
        assert!(backend.heartbeat(&binding(1)).await.is_err());
    }

    /// A remote launch arms its cleanup alarm too (a deadline scope's hard
    /// stop). Expired, it ends the run through `shorten_deadline`, which the
    /// runner acts on, and the run ends as cleanup-aborted.
    #[tokio::test]
    async fn a_remote_launchs_expired_alarm_ends_the_run() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let shortened = Arc::new(Mutex::new(None));
        let controls = {
            let shortened = Arc::clone(&shortened);
            WorkflowApiControls::remote(
                Arc::new(move |after| *shortened.lock().unwrap() = Some(after)),
                Arc::default(),
            )
        };
        launches
            .begin(binding(1), info(), Vec::new(), no_runtime(), controls)
            .unwrap();
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            backend.alarm(&binding(1), 200),
        )
        .await;
        assert!(dropped.is_err(), "armed: held until it expires");
        assert_eq!(
            *shortened.lock().unwrap(),
            None,
            "a disarmed alarm ends nothing"
        );
        assert!(!launches.cleanup_aborted("launch-1"));

        backend.alarm(&binding(1), 20).await.unwrap();
        assert_eq!(
            *shortened.lock().unwrap(),
            Some(std::time::Duration::ZERO),
            "expired: the runner is told to end the run now"
        );
        let result = crate::workflow::remote_invoke_result(
            &launches,
            "launch-1",
            crate::workflow::RemoteGuestExit::Timeout,
            std::time::Duration::from_millis(30),
        );
        assert!(
            matches!(result.exit, crate::InvokeExit::CleanupAborted),
            "{:?}",
            result.exit
        );
    }

    /// Child runs: the outcome is held until the child ends, release is idempotent, and a launch that
    /// ends reaps the tasks it did not release.
    #[tokio::test]
    async fn tasks_start_join_release_and_end_with_their_launch() {
        use crate::execution_host::{
            ExecutionContext, ExecutionError, InvocationLauncher, PreparedInvocation, StartRequest,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Dropped(Arc<AtomicUsize>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        // Each child says it started, waits for `go`, then completes with
        // its input.
        struct Gated {
            go: Arc<tokio::sync::Notify>,
            started: Arc<tokio::sync::Notify>,
            dropped: Arc<AtomicUsize>,
        }
        impl InvocationLauncher for Gated {
            fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
                let (go, started, dropped) =
                    (self.go.clone(), self.started.clone(), self.dropped.clone());
                Ok(PreparedInvocation::leaf(Box::new(move |_| {
                    Box::pin(async move {
                        let _guard = Dropped(dropped);
                        started.notify_one();
                        go.notified().await;
                        crate::InvokeExit::Completed(request.input)
                    })
                })))
            }
        }
        let engine = crate::build_engine(&Default::default()).unwrap();
        let go = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicUsize::new(0));
        let context = ExecutionContext::new(
            Arc::new(crate::isolated_tasks::IsolatedTasks::new(engine, 4, 1024 * 1024).unwrap()),
            Arc::new(Gated {
                go: go.clone(),
                started: started.clone(),
                dropped: dropped.clone(),
            }),
            8,
        )
        .unwrap();
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let mut with_tasks = controls();
        with_tasks.execution = Some(context.clone());
        launches
            .begin(binding(1), info(), Vec::new(), no_runtime(), with_tasks)
            .unwrap();
        let start = |path: &str| runtara_workflow_api::TaskStart {
            binding: "child".into(),
            entry: runtara_workflow_api::TaskEntry::Workflow,
            context: runtara_workflow_api::TaskContext {
                path: path.into(),
                attempt: 1,
            },
        };

        let id = backend
            .start_task(&binding(1), start("a"), b"in".to_vec())
            .await
            .unwrap()
            .task_id;
        started.notified().await;
        assert_eq!(
            backend.task_outcome(&binding(1), &id, 0).await.unwrap(),
            None
        );
        go.notify_one();
        assert_eq!(
            backend.task_outcome(&binding(1), &id, 5_000).await.unwrap(),
            Some(runtara_workflow_api::TaskOutcome::Completed(b"in".to_vec()))
        );
        assert_eq!(
            backend.cancel_task(&binding(1), &id).await.unwrap(),
            runtara_workflow_api::CancelStatus::AlreadyTerminal
        );
        backend.release_task(&binding(1), &id).await.unwrap();
        backend.release_task(&binding(1), &id).await.unwrap();
        let error = backend.task_outcome(&binding(1), &id, 0).await.unwrap_err();
        assert_eq!(error.code.as_str(), "invalid-task");

        // A task the launch never released is reaped when the launch ends.
        let live = backend
            .start_task(&binding(1), start("b"), Vec::new())
            .await
            .unwrap()
            .task_id;
        assert_ne!(live, id);
        started.notified().await;
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        launches.release(&binding(1).launch_id);
        context.shutdown().await.unwrap();
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            2,
            "the live child was reaped"
        );
    }

    /// Outside a run there is no alarm to arm.
    #[tokio::test]
    async fn a_standalone_launch_cannot_arm_an_alarm() {
        let launches = Arc::new(ApiLaunches::default());
        let backend = Backend(launches.clone());
        let binding = binding(0);
        launches
            .begin_standalone(
                binding.clone(),
                StandaloneTrust::Denied,
                StandaloneControl::Denied,
            )
            .unwrap();
        let error = backend.alarm(&binding, 10).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Denied);
    }

    /// A child's parent, status, termination reason and result.
    type ChildRun = (
        String,
        control_host::InstanceStatus,
        Option<String>,
        control_host::TerminalResult,
    );

    /// A control host whose children live in memory, started once per
    /// operation the way the server's durable admission does.
    #[derive(Default)]
    struct Children {
        by_operation: Mutex<HashMap<String, String>>,
        runs: Mutex<HashMap<String, ChildRun>>,
        starts: Mutex<Vec<(ControlAuthority, control_host::StartRequest)>>,
        cancels: Mutex<Vec<String>>,
    }

    impl Children {
        fn end(
            &self,
            id: &str,
            status: control_host::InstanceStatus,
            reason: Option<&str>,
            output: Option<&[u8]>,
            error: Option<&[u8]>,
        ) {
            let mut runs = self.runs.lock().unwrap();
            let run = runs.get_mut(id).unwrap();
            run.1 = status;
            run.2 = reason.map(str::to_owned);
            run.3.output = output.map(<[u8]>::to_vec);
            run.3.error = error.map(<[u8]>::to_vec);
        }
    }

    #[async_trait::async_trait]
    impl ControlHost for Children {
        async fn start(
            &self,
            authority: &ControlAuthority,
            request: control_host::StartRequest,
        ) -> Result<control_host::StartResult, control_host::ControlError> {
            self.starts
                .lock()
                .unwrap()
                .push((authority.clone(), request.clone()));
            let operation = authority.operation.clone().unwrap();
            let mut by_operation = self.by_operation.lock().unwrap();
            let replayed = by_operation.contains_key(&operation);
            let next = by_operation.len() + 1;
            let id = by_operation
                .entry(operation)
                .or_insert_with(|| format!("child-{next}"))
                .clone();
            self.runs
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_insert_with(|| {
                    (
                        authority.caller.clone().unwrap(),
                        control_host::InstanceStatus::Running,
                        None,
                        control_host::TerminalResult {
                            output: None,
                            output_bytes: None,
                            output_omitted: false,
                            error: None,
                            error_omitted: false,
                        },
                    )
                });
            Ok(control_host::StartResult {
                instance_id: id,
                workflow_id: request.workflow_id,
                version: request.version.unwrap_or(1),
                run_label: None,
                replayed,
            })
        }

        async fn get(
            &self,
            _: &ControlAuthority,
            instance_id: String,
        ) -> Result<control_host::InstanceDetail, control_host::ControlError> {
            let runs = self.runs.lock().unwrap();
            let (parent, status, reason, terminal) = runs.get(&instance_id).cloned().unwrap();
            Ok(control_host::InstanceDetail {
                instance: control_host::InstanceSummary {
                    instance_id,
                    workflow_id: "wf-child".into(),
                    version: Some(3),
                    run_label: None,
                    parent_instance_id: Some(parent),
                    status,
                    suspension_reason: None,
                    termination_reason: reason,
                    created_at_ms: 0,
                    started_at_ms: None,
                    finished_at_ms: None,
                },
                terminal,
            })
        }

        async fn cancel(
            &self,
            _: &ControlAuthority,
            request: control_host::CancelRequest,
        ) -> Result<control_host::CommandResult, control_host::ControlError> {
            self.cancels
                .lock()
                .unwrap()
                .push(request.instance_id.clone());
            Ok(control_host::CommandResult {
                instance_id: request.instance_id,
                outcome: control_host::CommandOutcome::Requested,
                replayed: false,
            })
        }
    }

    #[tokio::test]
    async fn a_workflow_child_is_a_run_of_its_own_found_again_on_replay() {
        use control_host::InstanceStatus;
        use runtara_workflow_api::{TaskContext, TaskEntry, TaskOutcome, TaskStart};
        let launches = Arc::new(ApiLaunches::default());
        let children = Arc::new(Children::default());
        launches.set_control_host(children.clone()).unwrap();
        let backend = Backend(launches.clone());
        launches
            .begin(binding(1), info(), Vec::new(), no_runtime(), controls())
            .unwrap();
        let start = |path: &str, attempt: u64| TaskStart {
            binding: "workflow:wf-child@3".into(),
            entry: TaskEntry::Workflow,
            context: TaskContext {
                path: path.into(),
                attempt,
            },
        };

        let first = backend
            .start_task(&binding(1), start("step-a", 1), br#"{"x":1}"#.to_vec())
            .await
            .unwrap();
        assert_eq!(first.instance_id.as_deref(), Some(first.task_id.as_str()));
        {
            let starts = children.starts.lock().unwrap();
            let (authority, request) = &starts[0];
            assert_eq!(authority.caller.as_deref(), Some("instance-1"));
            assert_eq!(request.workflow_id, "wf-child");
            assert_eq!(request.version, Some(3));
            assert_eq!(
                request.parent_close_policy,
                control_host::ParentClosePolicy::Cancel
            );
            let envelope: serde_json::Value = serde_json::from_slice(&request.input).unwrap();
            assert_eq!(
                envelope,
                serde_json::json!({"data": {"x": 1}, "variables": {}})
            );
        }
        // A replay of the parent reaching the same call finds the same child;
        // another call site or a retry starts a new one.
        let again = backend
            .start_task(&binding(1), start("step-a", 1), br#"{"x":1}"#.to_vec())
            .await
            .unwrap();
        assert_eq!(again.task_id, first.task_id);
        let sibling = backend
            .start_task(&binding(1), start("step-b", 1), b"{}".to_vec())
            .await
            .unwrap();
        let retry = backend
            .start_task(&binding(1), start("step-a", 2), b"{}".to_vec())
            .await
            .unwrap();
        assert_ne!(sibling.task_id, first.task_id);
        assert_ne!(retry.task_id, first.task_id);

        // A running child has no outcome yet; an ended one answers with its
        // own output, error or end.
        assert_eq!(
            backend
                .task_outcome(&binding(1), &first.task_id, 0)
                .await
                .unwrap(),
            None
        );
        children.end(
            &first.task_id,
            InstanceStatus::Completed,
            Some("completed"),
            Some(br#"{"y":2}"#),
            None,
        );
        assert_eq!(
            backend
                .task_outcome(&binding(1), &first.task_id, 1_000)
                .await
                .unwrap(),
            Some(TaskOutcome::Completed(br#"{"y":2}"#.to_vec()))
        );
        let failure = br#"{"code":"BOOM","message":"no","category":"permanent","severity":"error","retryable":false}"#;
        children.end(
            &sibling.task_id,
            InstanceStatus::Failed,
            Some("application_error"),
            None,
            Some(failure),
        );
        match backend
            .task_outcome(&binding(1), &sibling.task_id, 0)
            .await
            .unwrap()
        {
            Some(TaskOutcome::Failed(error)) => assert_eq!(error.code, "BOOM"),
            other => panic!("{other:?}"),
        }
        children.end(
            &retry.task_id,
            InstanceStatus::Failed,
            Some("execution_timeout"),
            None,
            None,
        );
        assert_eq!(
            backend
                .task_outcome(&binding(1), &retry.task_id, 0)
                .await
                .unwrap(),
            Some(TaskOutcome::TimedOut)
        );

        // Releasing a finished child does nothing; a live one is cancelled.
        backend
            .release_task(&binding(1), &first.task_id)
            .await
            .unwrap();
        assert!(children.cancels.lock().unwrap().is_empty());
        let live = backend
            .start_task(&binding(1), start("step-c", 1), b"{}".to_vec())
            .await
            .unwrap();
        backend
            .release_task(&binding(1), &live.task_id)
            .await
            .unwrap();
        assert_eq!(
            *children.cancels.lock().unwrap(),
            vec![live.task_id.clone()]
        );

        // Another run's child is not this run's task.
        children
            .runs
            .lock()
            .unwrap()
            .get_mut(&live.task_id)
            .unwrap()
            .0 = "instance-2".into();
        let error = backend
            .task_outcome(&binding(1), &live.task_id, 0)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), "invalid-task");

        // A workflow child takes the `workflow` entry.
        let mut wrong = start("step-d", 1);
        wrong.entry = TaskEntry::Capability("run".into());
        let error = backend
            .start_task(&binding(1), wrong, b"{}".to_vec())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Invalid);
    }
}
