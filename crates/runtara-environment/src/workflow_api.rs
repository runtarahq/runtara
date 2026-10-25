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
use runtara_component_host::InstanceWaitHost;
use runtara_component_host::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitError, InstanceWaitPoll, InstanceWaitRequest,
    InstanceWaitResolution, wait_instances_id,
};
use runtara_component_host::run_state_host::{
    RunStateAuthority, RunStateError, RunStateErrorCode, RunStateHost, check_state_read_key,
    state_operation_id,
};
use runtara_component_host::runtime_host::RuntimeHost;
use runtara_component_host::{WorkflowApiFuture, WorkflowApiTransport};
use runtara_workflow_api::server::{
    CheckpointWrite, CustomSignalNotice, Event, LaunchState, LifecycleSignal, MemoryReceiptStore,
    ReceiptStore, WorkflowApiBackend, WorkflowApiState, router,
};
use runtara_workflow_api::{ApiError, Binding, Caps, ErrorCode, InputRead, PostedOutcome, RunInfo};
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
}

impl Default for ApiLaunches {
    fn default() -> Self {
        Self {
            slots: Mutex::default(),
            receipts: Arc::new(MemoryReceiptStore::default()),
            run_state: None,
            instance_waits: std::sync::OnceLock::new(),
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

    async fn set_deadline(&self, _binding: &Binding, _abort_after_ms: u64) -> Result<(), ApiError> {
        Err(ApiError::new(
            ErrorCode::Unsupported,
            "the embedded runner does not shorten launch deadlines yet",
        ))
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
            Ok(poll) => wait_read(&wait_id, &poll),
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
        wait_read(&wait_id, &poll)
    }

    async fn release_wait(&self, binding: &Binding, key: &str) -> Result<(), ApiError> {
        let wait_id = wait_instances_id(key).map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
        self.runtime(binding)?
            .operation_release(wait_id)
            .await
            .map_err(host_error)
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
            .begin(binding(1), info(), b"in".to_vec(), no_runtime())
            .unwrap();
        assert!(
            launches
                .begin(binding(1), info(), Vec::new(), no_runtime())
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
