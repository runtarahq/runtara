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
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use bytes::Bytes;
use runtara_component_host::{WorkflowApiFuture, WorkflowApiTransport};
use runtara_workflow_api::server::{
    LaunchState, MemoryReceiptStore, WorkflowApiBackend, WorkflowApiState, router,
};
use runtara_workflow_api::{ApiError, Binding, Caps, ErrorCode, PostedOutcome, RunInfo};
use tower::ServiceExt;

/// One launch serving the workflow API.
pub struct LaunchSlot {
    binding: Binding,
    info: RunInfo,
    input: Vec<u8>,
    outcome: Mutex<Option<PostedOutcome>>,
}

/// The launches of this process that currently serve the workflow API, by
/// launch id, and their idempotency receipts.
#[derive(Default)]
pub struct ApiLaunches {
    slots: Mutex<HashMap<String, Arc<LaunchSlot>>>,
    receipts: Arc<MemoryReceiptStore>,
}

impl ApiLaunches {
    /// Start serving `binding`'s launch. A launch begins once.
    pub fn begin(&self, binding: Binding, info: RunInfo, input: Vec<u8>) -> Result<(), String> {
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
        self.receipts.forget_launch(launch_id);
        slot.outcome
            .lock()
            .expect("workflow API outcome poisoned")
            .take()
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
            receipts: self.receipts.clone(),
            caps: Caps::default(),
        };
        Arc::new(RouterTransport(router(state)))
    }
}

struct Backend(Arc<ApiLaunches>);

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
        launches.begin(binding(1), info(), b"in".to_vec()).unwrap();
        assert!(launches.begin(binding(1), info(), Vec::new()).is_err());

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
