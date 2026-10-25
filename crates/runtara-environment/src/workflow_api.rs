// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workflow API for runs of the embedded runner.
//!
//! A run on the workflow API ABI (`wasi:cli/run`) reaches its host over
//! `wasi:http`; the component host routes those requests to the `/v1` router
//! with the run's binding (see `runtara_component_host::workflow_api`). This
//! module is what the router serves them from: [`ApiLaunches`] holds every
//! launch currently serving the API in this process, registered by its
//! runtime host when the entry starts and removed when the host takes the
//! posted outcome back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use bytes::Bytes;
use runtara_component_host::connection_resolver_host::{
    ConnectionResolverHost, RunConnectionResolver, resolver_for_run,
};
use runtara_component_host::control_host::{self, ControlAuthority, ControlHost};
use runtara_component_host::database_host::{Operation, database_for_run};
use runtara_component_host::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitError, InstanceWaitPoll, InstanceWaitRequest,
    InstanceWaitResolution, wait_instances_id,
};
use runtara_component_host::operation_scope_host::{OperationIdentity, parse_agent_operation_key};
use runtara_component_host::outbound_http::{
    ConnectionDestination, Destination, RequestOptions, send_bounded,
};
use runtara_component_host::run_state_host::{
    RunStateAuthority, RunStateError, RunStateErrorCode, RunStateHost, check_state_read_key,
    state_operation_id,
};
use runtara_component_host::runtime_host::{RuntimeHost, WorkflowApiControls};
use runtara_component_host::trusted::TrustedExecutor;
use runtara_component_host::{DatabaseHost, InstanceWaitHost, OutboundHttpHost};
use runtara_component_host::{WorkflowApiFuture, WorkflowApiTransport};
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
    /// Instance waits this launch registered whose last read was pending.
    /// A suspension it posts wakes on them too, as the old ABI's host
    /// attached them to the run's result.
    pending_waits: Mutex<Vec<String>>,
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
    run_state: Option<Arc<dyn RunStateHost>>,
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
            run_state: None,
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
        Self {
            run_state: Some(run_state),
            ..Self::default()
        }
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
                pending_waits: Mutex::default(),
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

    /// How many launches currently serve the API.
    pub fn serving(&self) -> usize {
        self.slots
            .lock()
            .expect("workflow API launches poisoned")
            .len()
    }

    fn slot(&self, binding: &Binding) -> Option<Arc<LaunchSlot>> {
        self.slots
            .lock()
            .expect("workflow API launches poisoned")
            .get(&binding.launch_id)
            .filter(|slot| slot.binding == *binding)
            .cloned()
    }

    /// The `/v1` router over these launches, as a component-host transport.
    pub fn transport(self: &Arc<Self>) -> Arc<dyn WorkflowApiTransport> {
        let state = WorkflowApiState {
            backend: Arc::new(Backend(Arc::clone(self))),
            receipts: Arc::clone(&self.receipts),
            caps: Caps::default(),
        };
        Arc::new(RouterTransport(router(state)))
    }
}

struct Backend(Arc<ApiLaunches>);

impl Backend {
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

fn input_read(state: runtara_component_host::runtime_host::RuntimeInputState) -> InputRead {
    use runtara_component_host::runtime_host::RuntimeInputState;
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
        let host = self.0.run_state.clone().ok_or_else(|| {
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
        let posted = slot
            .outcome
            .lock()
            .expect("workflow API outcome poisoned")
            .is_some();
        Ok(if posted {
            LaunchState::Finished
        } else {
            LaunchState::Active
        })
    }

    async fn run_info(&self, binding: &Binding) -> Result<RunInfo, ApiError> {
        Ok(self.0.slot(binding).ok_or_else(not_serving)?.info.clone())
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
        self.runtime(binding)?
            .close_input(signal_id.to_string())
            .await
            .map(input_read)
            .map_err(input_error)
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
        let host = runtara_component_host::outbound_http::for_run(
            self.0.outbound_http.get(),
            Some(&binding.tenant_id),
            Some(&binding.instance_id),
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
        for event in events {
            let payload = serde_json::to_vec(&event.payload)
                .map_err(|e| ApiError::new(ErrorCode::Invalid, e.to_string()))?;
            runtime
                .custom_event(event.kind, payload)
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
        let host = self.0.control.get().cloned().ok_or_else(|| {
            ApiError::new(ErrorCode::Unavailable, "no control service is configured")
        })?;
        let authority = ControlAuthority {
            tenant: binding.tenant_id.clone(),
            caller: Some(binding.instance_id.clone()),
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
        let executor = self.0.trusted.get().cloned().ok_or_else(|| {
            ApiError::new(ErrorCode::Unavailable, "no trusted executor is configured")
        })?;
        let launch = self.runtime(binding)?.trusted_launch();
        executor
            .invoke_pinned(
                Some(&slot.controls.trusted_pins),
                launch,
                &binding.tenant_id,
                &call.agent_id,
                &call.capability_id,
                &call.connection_id,
                call.input,
                tokio::time::Instant::now() + runtara_component_host::trusted::TIME_LIMIT,
            )
            .await
            .map_err(|envelope| trusted_error(&envelope))
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

fn lifecycle_signal(
    signal: runtara_component_host::runtime_host::RuntimeSignalInfo,
) -> LifecycleSignal {
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
        Weak::<crate::runtime_host::PersistenceRuntimeHost>::new()
    }

    fn controls() -> WorkflowApiControls {
        WorkflowApiControls {
            shorten_deadline: Arc::new(|_| {}),
            trusted_pins: Arc::default(),
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
}
