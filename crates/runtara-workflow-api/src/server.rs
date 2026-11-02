//! The host-side `/v1` router.
//!
//! [`router`] serves the workflow API for every runtime that can attach a
//! [`Binding`]: the embedded runner's `send_request` hook calls it in process,
//! and runtara-cloud's binding gateway mounts it behind node traffic. Neither
//! path lets the guest set the binding; it arrives as a request extension.
//!
//! What the router owns, for every endpoint: the binding check, version
//! negotiation, body caps, `Idempotency-Key` receipts, the `fenced` and
//! `finished` checks, and the error envelope. What it delegates: the effect
//! itself, through [`WorkflowApiBackend`]. Endpoint families land one at a
//! time; anything not served yet answers `unsupported`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use bytes::Bytes;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    ApiError, Binding, Caps, ErrorCode, ErrorInfo, OutcomeKind, RunInfo, Suspension, headers,
    parse_api_version, serves_version, validate_idempotency_key,
};

/// Where a launch stands, as far as the API is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchState {
    /// The binding's launch holds the run's current lease and has not posted
    /// its outcome.
    Active,
    /// The launch posted its outcome; only `GET /v1/run` is still served.
    Finished,
    /// The binding's lease epoch is older than the run's current lease.
    Fenced,
    /// The launch paused the run itself, acknowledging a pause at a
    /// checkpoint, and nothing has claimed the run since: only its outcome,
    /// the suspension the pause asked for, is still taken.
    Released,
}

pub use crate::PostedOutcome;

mod connections;
mod control;
mod runtime;
mod tasks;
pub use crate::{CustomSignalNotice, Event, LifecycleNotice};
pub use connections::OutboundReply;
pub use runtime::{CheckpointWrite, LifecycleSignal};

/// The effects behind the router. Implemented by the runtime that owns runs
/// (runtara-environment for the embedded runner). Every method receives the
/// request's binding and must scope everything it touches to it.
#[async_trait]
pub trait WorkflowApiBackend: Send + Sync + 'static {
    async fn launch_state(&self, binding: &Binding) -> Result<LaunchState, ApiError>;
    async fn run_info(&self, binding: &Binding) -> Result<RunInfo, ApiError>;
    async fn run_input(&self, binding: &Binding) -> Result<Vec<u8>, ApiError>;
    /// Accept the launch's single outcome. After this succeeds,
    /// [`Self::launch_state`] must report [`LaunchState::Finished`].
    async fn post_outcome(&self, binding: &Binding, outcome: PostedOutcome)
    -> Result<(), ApiError>;
    /// Shorten the launch deadline to `abort_after_ms` from now. Never extends.
    async fn set_deadline(&self, binding: &Binding, abort_after_ms: u64) -> Result<(), ApiError>;

    /// The bytes `offset`..`offset + length` (to its end, without a length)
    /// of the artifact whose SHA-256 is `sha256`, and its whole size, for a
    /// launch whose work names it: a compile's components (M7.15). Others are
    /// refused.
    async fn get_artifact(
        &self,
        binding: &Binding,
        sha256: &str,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(Vec<u8>, u64), ApiError> {
        let _ = (binding, sha256, offset, length);
        Err(unsupported_service("artifacts"))
    }

    /// Takes `bytes` as part `part` of `parts` of what the launch made (a
    /// compile's composed component, M7.15), whose SHA-256 is `sha256`:
    /// answers whether it is now whole (every part taken, and its SHA-256
    /// checked).
    async fn put_artifact(
        &self,
        binding: &Binding,
        sha256: &str,
        part: u32,
        parts: u32,
        bytes: Vec<u8>,
    ) -> Result<bool, ApiError> {
        let _ = (binding, sha256, part, parts, bytes);
        Err(unsupported_service("artifacts"))
    }

    // Run services. Each answers `unsupported` until a backend serves it.

    /// The checkpoint `id`, if one was committed.
    async fn get_checkpoint(
        &self,
        binding: &Binding,
        id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let _ = (binding, id);
        Err(unsupported_service("checkpoints"))
    }
    /// Commit `state` as checkpoint `id`; the first committed state wins.
    async fn put_checkpoint(
        &self,
        binding: &Binding,
        id: &str,
        state: Vec<u8>,
    ) -> Result<CheckpointWrite, ApiError> {
        let _ = (binding, id, state);
        Err(unsupported_service("checkpoints"))
    }
    async fn record_retry_attempt(
        &self,
        binding: &Binding,
        id: &str,
        attempt: u32,
        error: Option<String>,
    ) -> Result<(), ApiError> {
        let _ = (binding, id, attempt, error);
        Err(unsupported_service("retry attempts"))
    }
    /// The pending lifecycle command, without acknowledging it.
    async fn poll_lifecycle_signal(
        &self,
        binding: &Binding,
    ) -> Result<Option<LifecycleSignal>, ApiError> {
        let _ = binding;
        Err(unsupported_service("lifecycle signals"))
    }
    /// Acknowledge a lifecycle command the run observed.
    async fn ack_lifecycle_signal(
        &self,
        binding: &Binding,
        command_id: &str,
        signal_type: &str,
    ) -> Result<bool, ApiError> {
        let _ = (binding, command_id, signal_type);
        Err(unsupported_service("lifecycle signals"))
    }
    /// Poll the pending lifecycle command and handle it: true when a
    /// stop-like command was handled and the run should return.
    async fn check_lifecycle_signals(&self, binding: &Binding) -> Result<bool, ApiError> {
        let _ = binding;
        Err(unsupported_service("lifecycle signals"))
    }
    /// Whether a cancel is pending (acknowledging it) or was already handled.
    async fn lifecycle_cancelled(&self, binding: &Binding) -> Result<bool, ApiError> {
        let _ = binding;
        Err(unsupported_service("lifecycle signals"))
    }
    /// The custom signal payload waiting at `checkpoint_id`, if any.
    async fn poll_custom_signal(
        &self,
        binding: &Binding,
        checkpoint_id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let _ = (binding, checkpoint_id);
        Err(unsupported_service("custom signals"))
    }
    /// Persistence time, so deadlines agree with the host.
    async fn now_ms(&self, binding: &Binding) -> Result<u64, ApiError> {
        let _ = binding;
        Err(unsupported_service("time"))
    }
    /// Wait `ms`, the run waiting in process meanwhile. The request is held
    /// for the wait; dropping it ends the wait.
    async fn sleep(&self, binding: &Binding, ms: u64) -> Result<(), ApiError> {
        let _ = (binding, ms);
        Err(unsupported_service("sleeps"))
    }
    /// Arm the run's cleanup alarm: `ms` from now the host aborts the whole
    /// execution. The request is held while the alarm is armed and never
    /// succeeds; dropping it disarms the alarm.
    async fn alarm(&self, binding: &Binding, ms: u64) -> Result<(), ApiError> {
        let _ = (binding, ms);
        Err(unsupported_service("alarms"))
    }
    /// Start a child run of `start.binding` with `input`; a start with the
    /// context of a live task of this launch returns that task.
    async fn start_task(
        &self,
        binding: &Binding,
        start: crate::TaskStart,
        input: Vec<u8>,
    ) -> Result<crate::TaskStarted, ApiError> {
        let _ = (binding, start, input);
        Err(unsupported_service("tasks"))
    }
    /// The task's outcome once its execution was destroyed, waiting up to
    /// `wait_ms` for it; `None` while it runs.
    async fn task_outcome(
        &self,
        binding: &Binding,
        task_id: &str,
        wait_ms: u64,
    ) -> Result<Option<crate::TaskOutcome>, ApiError> {
        let _ = (binding, task_id, wait_ms);
        Err(unsupported_service("tasks"))
    }
    async fn cancel_task(
        &self,
        binding: &Binding,
        task_id: &str,
    ) -> Result<crate::CancelStatus, ApiError> {
        let _ = (binding, task_id);
        Err(unsupported_service("tasks"))
    }
    /// Cancel and reap the task. Idempotent.
    async fn release_task(&self, binding: &Binding, task_id: &str) -> Result<(), ApiError> {
        let _ = (binding, task_id);
        Err(unsupported_service("tasks"))
    }
    async fn heartbeat(&self, binding: &Binding) -> Result<(), ApiError> {
        let _ = binding;
        Err(unsupported_service("heartbeat"))
    }
    /// Pause the run at a breakpoint. A host that parks the paused run
    /// answers once the pause is recorded; `wait_ms` bounds a host that
    /// waits for the resume in process instead.
    async fn breakpoint(&self, binding: &Binding, wait_ms: u64) -> Result<(), ApiError> {
        let _ = (binding, wait_ms);
        Err(unsupported_service("breakpoints"))
    }
    /// Append events, in order.
    async fn append_events(&self, binding: &Binding, events: Vec<Event>) -> Result<(), ApiError> {
        let _ = (binding, events);
        Err(unsupported_service("events"))
    }

    /// Register the managed input `descriptor` (JSON with its `signal_id`)
    /// under the run, with its original deadline. A replay finds it again.
    async fn register_input(
        &self,
        binding: &Binding,
        descriptor: Vec<u8>,
        deadline_ms: Option<u64>,
    ) -> Result<(), ApiError> {
        let _ = (binding, descriptor, deadline_ms);
        Err(unsupported_service("managed inputs"))
    }
    /// Read the input, expiring it by persistence time.
    async fn poll_input(
        &self,
        binding: &Binding,
        signal_id: &str,
    ) -> Result<crate::InputRead, ApiError> {
        let _ = (binding, signal_id);
        Err(unsupported_service("managed inputs"))
    }
    /// Abandon the input without replacing a response that already won.
    async fn close_input(
        &self,
        binding: &Binding,
        signal_id: &str,
    ) -> Result<crate::InputRead, ApiError> {
        let _ = (binding, signal_id);
        Err(unsupported_service("managed inputs"))
    }

    /// Register the WaitForInstances step `key`'s wait on `request`
    /// (`{instanceIds, mode, deadlineMs}`), or find it on replay, and read it
    /// without blocking. The read carries `waitId`, which a suspension wakes on.
    async fn register_wait(
        &self,
        binding: &Binding,
        key: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        let _ = (binding, key, request);
        Err(unsupported_service("instance waits"))
    }
    /// Read the step `key`'s registered wait without blocking.
    async fn poll_wait(&self, binding: &Binding, key: &str) -> Result<serde_json::Value, ApiError> {
        let _ = (binding, key);
        Err(unsupported_service("instance waits"))
    }
    /// Drop the step `key`'s settled wait. Idempotent.
    async fn release_wait(&self, binding: &Binding, key: &str) -> Result<(), ApiError> {
        let _ = (binding, key);
        Err(unsupported_service("instance waits"))
    }

    /// Enter the operation of the call site `checkpoint_key` at `attempt`,
    /// returning its id and, with `load`, the continuation saved for it.
    async fn enter_operation(
        &self,
        binding: &Binding,
        checkpoint_key: &str,
        attempt: u32,
        load: bool,
    ) -> Result<(String, Option<Vec<u8>>), ApiError> {
        let _ = (binding, checkpoint_key, attempt, load);
        Err(unsupported_service("operations"))
    }
    /// Persist `state` as the entered operation's continuation and leave it
    /// suspended on `wakes`.
    async fn suspend_operation(
        &self,
        binding: &Binding,
        operation_id: &str,
        state: Vec<u8>,
        wakes: Vec<crate::Wake>,
    ) -> Result<(), ApiError> {
        let _ = (binding, operation_id, state, wakes);
        Err(unsupported_service("operations"))
    }
    /// Leave the entered operation; a failure discards what it kept.
    async fn exit_operation(
        &self,
        binding: &Binding,
        operation_id: &str,
        failed: bool,
    ) -> Result<(), ApiError> {
        let _ = (binding, operation_id, failed);
        Err(unsupported_service("operations"))
    }
    /// Drop everything kept for the operation. Idempotent.
    async fn release_operation(
        &self,
        binding: &Binding,
        operation_id: &str,
    ) -> Result<(), ApiError> {
        let _ = (binding, operation_id);
        Err(unsupported_service("operations"))
    }

    /// The safe descriptor of `connection_id` in the run's tenant (JSON).
    async fn describe_connection(
        &self,
        binding: &Binding,
        connection_id: &str,
    ) -> Result<Vec<u8>, ApiError> {
        let _ = (binding, connection_id);
        Err(unsupported_service("connections"))
    }
    /// Resolve a resource request (JSON) against `connection_id`.
    async fn resolve_resource(
        &self,
        binding: &Binding,
        connection_id: &str,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, ApiError> {
        let _ = (binding, connection_id, request);
        Err(unsupported_service("connections"))
    }
    /// Run a SQL `operation` (JSON request) on `connection_id`; the result is
    /// JSON.
    async fn sql(
        &self,
        binding: &Binding,
        connection_id: &str,
        operation: crate::SqlOperation,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, ApiError> {
        let _ = (binding, connection_id, operation, request);
        Err(unsupported_service("SQL"))
    }
    /// Send an outbound HTTP call. The outer error is the API's own; the inner
    /// one is the call's failure, with whatever upstream body was received.
    async fn http(
        &self,
        binding: &Binding,
        meta: crate::OutboundMeta,
        body: Option<Vec<u8>>,
    ) -> Result<Result<OutboundReply, (crate::OutboundFailure, Vec<u8>)>, ApiError> {
        let _ = (binding, meta, body);
        Err(unsupported_service("outbound HTTP"))
    }

    /// Merge `patch` into the run's published state, once per SetState `key`.
    async fn set_state(
        &self,
        binding: &Binding,
        key: &str,
        patch: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), ApiError> {
        let _ = (binding, key, patch);
        Err(unsupported_service("published state"))
    }
    /// The run's published state, read by the GetState step `key`.
    async fn get_state(
        &self,
        binding: &Binding,
        key: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
        let _ = (binding, key);
        Err(unsupported_service("published state"))
    }

    /// Run a control call for the run, inside the entered `operation` the
    /// request names, if any. The result is the call's JSON answer.
    async fn control(
        &self,
        binding: &Binding,
        operation: Option<&str>,
        call: crate::ControlCall,
    ) -> Result<serde_json::Value, ApiError> {
        let _ = (binding, operation, call);
        Err(unsupported_service("control calls"))
    }

    /// Run the approved built-in `capability_id` of `agent_id` with the
    /// tenant's `connection_id`; `input` and the answer are JSON bytes. The
    /// host checks the run's artifact pins before any credential lookup.
    async fn trusted(
        &self,
        binding: &Binding,
        operation: Option<&str>,
        call: TrustedCall,
    ) -> Result<Vec<u8>, ApiError> {
        let _ = (binding, operation, call);
        Err(unsupported_service("trusted calls"))
    }
}

/// One `POST /v1/trusted/{agentId}/{capabilityId}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedCall {
    pub agent_id: String,
    pub capability_id: String,
    pub connection_id: String,
    /// The capability input, as JSON bytes.
    pub input: Vec<u8>,
}

fn unsupported_service(service: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Unsupported,
        format!("{service} are not served by this host"),
    )
}

/// A stored response for one `(launch, Idempotency-Key)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub request_hash: [u8; 32],
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The answer's body was over [`Caps::receipt_body_bytes`] and was not
    /// kept; the call ran, but its answer cannot be replayed.
    pub body_omitted: bool,
}

/// Idempotency receipts of running launches (a store may keep only a
/// launch's newest; see [`MemoryReceiptStore`]).
#[async_trait]
pub trait ReceiptStore: Send + Sync + 'static {
    /// Holds `(launch_id, key)` until the returned hold drops: a request
    /// with the key waits for one already running with it, then reads its
    /// receipt, so overlapping requests with one key run once. A store that
    /// several processes share must hold across them.
    async fn hold(&self, launch_id: &str, key: &str) -> Result<KeyHold, ApiError>;
    async fn get(&self, launch_id: &str, key: &str) -> Result<Option<Receipt>, ApiError>;
    /// Store `receipt` unless one is already stored for the key; the first
    /// stays.
    async fn put(&self, launch_id: &str, key: &str, receipt: Receipt) -> Result<(), ApiError>;
    /// Drop every receipt of `launch_id` once the launch has ended.
    async fn forget_launch(&self, launch_id: &str) -> Result<(), ApiError> {
        let _ = launch_id;
        Ok(())
    }
}

/// A held `(launch, key)`, released when dropped.
pub struct KeyHold(#[allow(dead_code)] Box<dyn Send + Sync>);

impl KeyHold {
    /// A hold that `guard` keeps until it drops.
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self(Box::new(guard))
    }
}

/// Keys being run, each with the requests that hold or wait for it.
type HeldKeys = Arc<Mutex<HashMap<(String, String), Arc<tokio::sync::Mutex<()>>>>>;

/// One process's hold on a key. Dropping it lets the next waiter run, and
/// forgets the key when no one else holds or waits for it.
struct MemoryHold {
    keys: HeldKeys,
    key: (String, String),
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for MemoryHold {
    fn drop(&mut self) {
        self.guard.take();
        let mut keys = self.keys.lock().expect("held keys poisoned");
        if keys
            .get(&self.key)
            .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            keys.remove(&self.key);
        }
    }
}

/// Receipts a launch keeps in memory before its oldest go.
pub const RECEIPTS_PER_LAUNCH: usize = 4096;
/// Answer bytes (bodies and headers) a launch keeps in memory before its
/// oldest receipts go.
pub const RECEIPT_BYTES_PER_LAUNCH: usize = 16 * 1024 * 1024;

/// In-memory receipts for one process. The embedded runner forgets a launch's
/// receipts when the launch ends; a relaunch is a new launch with new keys.
///
/// A launch keeps its newest receipts, up to a count and a total size; past
/// either the oldest go. A receipt only matters to a retry of its own call,
/// and a guest has few calls in flight (the node serves a run 4 at a time,
/// and the SDK mints a new key for every call), so a call a guest could still
/// retry is never among those dropped: this bounds a launch's memory without
/// letting an honest retry run twice.
pub struct MemoryReceiptStore {
    launches: Mutex<HashMap<String, LaunchReceipts>>,
    /// A launch is served by one process, so holding a key in this process
    /// is enough.
    held: HeldKeys,
    max_receipts: usize,
    max_bytes: usize,
}

#[derive(Default)]
struct LaunchReceipts {
    /// Keys, oldest first.
    order: VecDeque<String>,
    by_key: HashMap<String, Receipt>,
    bytes: usize,
}

fn receipt_bytes(receipt: &Receipt) -> usize {
    receipt.body.len()
        + receipt
            .headers
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>()
}

impl Default for MemoryReceiptStore {
    fn default() -> Self {
        Self::with_limits(RECEIPTS_PER_LAUNCH, RECEIPT_BYTES_PER_LAUNCH)
    }
}

impl MemoryReceiptStore {
    /// A store whose launches each keep at most `max_receipts` receipts and
    /// `max_bytes` of answers (the newest receipt always stays).
    pub fn with_limits(max_receipts: usize, max_bytes: usize) -> Self {
        Self {
            launches: Mutex::default(),
            held: HeldKeys::default(),
            max_receipts: max_receipts.max(1),
            max_bytes,
        }
    }

    pub fn forget_launch(&self, launch_id: &str) {
        self.launches
            .lock()
            .expect("receipt store poisoned")
            .remove(launch_id);
    }
}

#[async_trait]
impl ReceiptStore for MemoryReceiptStore {
    async fn hold(&self, launch_id: &str, key: &str) -> Result<KeyHold, ApiError> {
        let key = (launch_id.to_string(), key.to_string());
        let lock = Arc::clone(
            self.held
                .lock()
                .expect("held keys poisoned")
                .entry(key.clone())
                .or_default(),
        );
        let guard = lock.lock_owned().await;
        Ok(KeyHold::new(MemoryHold {
            keys: Arc::clone(&self.held),
            key,
            guard: Some(guard),
        }))
    }

    async fn get(&self, launch_id: &str, key: &str) -> Result<Option<Receipt>, ApiError> {
        Ok(self
            .launches
            .lock()
            .expect("receipt store poisoned")
            .get(launch_id)
            .and_then(|launch| launch.by_key.get(key))
            .cloned())
    }

    async fn put(&self, launch_id: &str, key: &str, receipt: Receipt) -> Result<(), ApiError> {
        let mut launches = self.launches.lock().expect("receipt store poisoned");
        let launch = launches.entry(launch_id.to_string()).or_default();
        if launch.by_key.contains_key(key) {
            return Ok(());
        }
        launch.bytes += receipt_bytes(&receipt);
        launch.by_key.insert(key.to_string(), receipt);
        launch.order.push_back(key.to_string());
        while launch.order.len() > 1
            && (launch.order.len() > self.max_receipts || launch.bytes > self.max_bytes)
        {
            let oldest = launch.order.pop_front().expect("more than one receipt");
            if let Some(dropped) = launch.by_key.remove(&oldest) {
                launch.bytes -= receipt_bytes(&dropped);
            }
        }
        Ok(())
    }

    async fn forget_launch(&self, launch_id: &str) -> Result<(), ApiError> {
        MemoryReceiptStore::forget_launch(self, launch_id);
        Ok(())
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    fn receipt(body: usize) -> Receipt {
        Receipt {
            request_hash: [0; 32],
            status: 200,
            headers: Vec::new(),
            body: vec![b'x'; body],
            body_omitted: false,
        }
    }

    async fn keys(store: &MemoryReceiptStore, launch: &str, keys: &[&str]) -> Vec<bool> {
        let mut kept = Vec::new();
        for key in keys {
            kept.push(store.get(launch, key).await.unwrap().is_some());
        }
        kept
    }

    #[tokio::test]
    async fn a_launch_keeps_its_newest_receipts_up_to_a_count() {
        let store = MemoryReceiptStore::with_limits(3, usize::MAX);
        for key in ["k0", "k1", "k2", "k3"] {
            store.put("a", key, receipt(1)).await.unwrap();
        }
        store.put("b", "k0", receipt(1)).await.unwrap();
        assert_eq!(
            keys(&store, "a", &["k0", "k1", "k2", "k3"]).await,
            [false, true, true, true]
        );
        assert_eq!(keys(&store, "b", &["k0"]).await, [true], "per launch");
        // A second put of a kept key leaves the first receipt, and the order.
        store.put("a", "k1", receipt(5)).await.unwrap();
        assert_eq!(store.get("a", "k1").await.unwrap().unwrap().body.len(), 1);
        store.put("a", "k4", receipt(1)).await.unwrap();
        assert_eq!(
            keys(&store, "a", &["k1", "k2", "k3", "k4"]).await,
            [false, true, true, true]
        );
    }

    #[tokio::test]
    async fn a_held_key_makes_the_next_holder_wait_and_is_then_forgotten() {
        let store = Arc::new(MemoryReceiptStore::default());
        let first = store.hold("a", "k").await.unwrap();
        let other = store.hold("a", "other").await.unwrap();
        let waiter = tokio::spawn({
            let store = Arc::clone(&store);
            async move { store.hold("a", "k").await.unwrap() }
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "the key is held");
        drop(first);
        let second = waiter.await.unwrap();
        drop((second, other));
        assert!(store.held.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_launch_keeps_its_newest_receipts_up_to_a_size() {
        let store = MemoryReceiptStore::with_limits(usize::MAX, 10);
        store.put("a", "k0", receipt(4)).await.unwrap();
        store.put("a", "k1", receipt(4)).await.unwrap();
        store.put("a", "k2", receipt(4)).await.unwrap();
        assert_eq!(
            keys(&store, "a", &["k0", "k1", "k2"]).await,
            [false, true, true]
        );
        // One answer over the whole budget is still kept: it is the newest.
        store.put("a", "big", receipt(64)).await.unwrap();
        assert_eq!(
            keys(&store, "a", &["k1", "k2", "big"]).await,
            [false, false, true]
        );
        store.forget_launch("a");
        assert_eq!(keys(&store, "a", &["big"]).await, [false]);
    }
}

/// Everything the router needs.
#[derive(Clone)]
pub struct WorkflowApiState {
    pub backend: Arc<dyn WorkflowApiBackend>,
    pub receipts: Arc<dyn ReceiptStore>,
    /// Holds the calls that start external work while the run's tenant is
    /// over its limit for them; `None` holds nothing.
    pub limiter: Option<Arc<dyn CallLimiter>>,
    pub caps: Caps,
}

/// Paces a tenant's calls that start external work: outbound HTTP, SQL,
/// connection resources, child tasks, started instances and trusted
/// capabilities. Calls that keep a run durable (checkpoints, signals,
/// heartbeats, outcomes) are never paced.
#[async_trait]
pub trait CallLimiter: Send + Sync + 'static {
    /// Returns once `binding`'s tenant may make one more such call. The call
    /// waits meanwhile, so a run over its tenant's limit slows down rather
    /// than fails; a guest that gives up drops the wait.
    async fn admit(&self, binding: &Binding);
}

/// Holds an external-work call until the run's tenant may make it.
async fn pace_external(
    State(state): State<WorkflowApiState>,
    request: Request,
    next: Next,
) -> Response {
    if let (Some(limiter), Ok(binding)) = (&state.limiter, binding_of(&request)) {
        limiter.admit(&binding).await;
    }
    next.run(request).await
}

/// The `/v1` router. The caller attaches the [`Binding`] as a request
/// extension before handing it over.
pub fn router(state: WorkflowApiState) -> Router {
    let paced = || middleware::from_fn_with_state(state.clone(), pace_external);
    let api = Router::new()
        .route("/v1/run", get(get_run))
        .route("/v1/run/input", get(get_run_input))
        .route("/v1/run/outcome", post(post_run_outcome))
        .route("/v1/run/deadline", put(put_run_deadline))
        .route("/v1/run/alarm", post(post_run_alarm))
        .route(
            "/v1/artifacts/{sha256}",
            get(get_artifact).put(put_artifact),
        )
        .route(
            "/v1/checkpoints/{id}",
            get(runtime::get_checkpoint).put(runtime::put_checkpoint),
        )
        .route(
            "/v1/checkpoints/{id}/retry-attempts",
            post(runtime::post_retry_attempt),
        )
        .route("/v1/signals/lifecycle", get(runtime::get_lifecycle_signal))
        .route(
            "/v1/signals/lifecycle/{command_id}/ack",
            post(runtime::ack_lifecycle_signal),
        )
        .route(
            "/v1/signals/lifecycle/check",
            post(runtime::check_lifecycle_signals),
        )
        .route(
            "/v1/signals/lifecycle/cancelled",
            post(runtime::lifecycle_cancelled),
        )
        .route(
            "/v1/signals/custom/{checkpoint_id}",
            get(runtime::get_custom_signal),
        )
        .route("/v1/time", get(runtime::get_time))
        .route("/v1/time/sleep", get(runtime::get_sleep))
        .route("/v1/heartbeat", post(runtime::post_heartbeat))
        .route("/v1/debug/breakpoint", post(runtime::post_breakpoint))
        .route("/v1/tasks", post(tasks::post_task).route_layer(paced()))
        .route("/v1/tasks/{task_id}", delete(tasks::delete_task))
        .route("/v1/tasks/{task_id}/outcome", get(tasks::get_task_outcome))
        .route("/v1/tasks/{task_id}/cancel", post(tasks::post_task_cancel))
        .route("/v1/events", post(runtime::post_events))
        .route(
            "/v1/inputs/{signal_id}",
            put(runtime::put_input)
                .get(runtime::get_input)
                .delete(runtime::delete_input),
        )
        .route(
            "/v1/waits/{key}",
            put(runtime::put_wait)
                .get(runtime::get_wait)
                .delete(runtime::delete_wait),
        )
        .route("/v1/operations", post(runtime::post_operation))
        .route(
            "/v1/operations/{operation_id}",
            axum::routing::delete(runtime::delete_operation),
        )
        .route(
            "/v1/operations/{operation_id}/suspend",
            post(runtime::post_operation_suspend),
        )
        .route(
            "/v1/operations/{operation_id}/exit",
            post(runtime::post_operation_exit),
        )
        .route(
            "/v1/connections/{connection_id}",
            get(connections::get_connection),
        )
        .route(
            "/v1/connections/{connection_id}/resources",
            post(connections::post_resources).route_layer(paced()),
        )
        .route(
            "/v1/connections/{connection_id}/sql/{operation}",
            post(connections::post_sql).route_layer(paced()),
        )
        .route(
            "/v1/http",
            post(connections::post_http).route_layer(paced()),
        )
        .route(
            "/v1/instances",
            post(control::post_start).route_layer(paced()),
        )
        .route("/v1/instances/query", post(control::post_query))
        .route(
            "/v1/instances/signals/pending",
            post(control::post_pending_signals),
        )
        .route("/v1/instances/{id}", get(control::get_instance))
        .route("/v1/instances/{id}/state", get(control::get_instance_state))
        .route("/v1/instances/{id}/signals", post(control::post_signal))
        .route("/v1/instances/{id}/cancel", post(control::post_cancel))
        .route("/v1/instances/{id}/pause", post(control::post_pause))
        .route("/v1/instances/{id}/resume", post(control::post_resume))
        .route(
            "/v1/trusted/{agent_id}/{capability_id}",
            post(control::post_trusted).route_layer(paced()),
        )
        .route(
            "/v1/state/{key}",
            get(runtime::get_state).patch(runtime::patch_state),
        )
        .fallback(unsupported)
        .layer(middleware::from_fn_with_state(state.clone(), guard_launch))
        .layer(middleware::from_fn_with_state(state.clone(), idempotency))
        .layer(middleware::from_fn(check_version))
        .layer(middleware::from_fn(require_binding));
    api.with_state(state)
}

/// Render an [`ApiError`] as the uniform envelope.
pub fn error_response(error: &ApiError) -> Response {
    let status = StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
    let body = serde_json::to_vec(error).unwrap_or_default();
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

struct Failure(ApiError);

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Self(error)
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        error_response(&self.0)
    }
}

fn binding_of(request: &Request) -> Result<Binding, ApiError> {
    request
        .extensions()
        .get::<Binding>()
        .cloned()
        .ok_or_else(|| ApiError::new(ErrorCode::Denied, "request carries no run binding"))
}

async fn require_binding(request: Request, next: Next) -> Response {
    match binding_of(&request) {
        Ok(_) => next.run(request).await,
        Err(error) => error_response(&error),
    }
}

async fn check_version(request: Request, next: Next) -> Response {
    if let Some(value) = request.headers().get(headers::API_VERSION) {
        let requested = value.to_str().ok().and_then(parse_api_version);
        match requested {
            Some(version) if serves_version(version) => {}
            _ => {
                return error_response(&ApiError::new(
                    ErrorCode::Unsupported,
                    format!("this host serves workflow API {}", crate::api_version()),
                ));
            }
        }
    }
    next.run(request).await
}

fn request_hash(method: &Method, uri: &str, headers: &HeaderMap, body: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(method.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(uri.as_bytes());
    hasher.update([0]);
    // Metadata headers change what a request means, so they are part of it.
    for name in [
        headers::OUTCOME,
        headers::OPERATION,
        headers::WAKES,
        headers::OUTBOUND,
        headers::TASK,
    ] {
        if let Some(value) = headers.get(name) {
            hasher.update(name.as_bytes());
            hasher.update(b":");
            hasher.update(value.as_bytes());
        }
        hasher.update([0]);
    }
    hasher.update(body);
    hasher.finalize().into()
}

fn receipt_response(receipt: &Receipt) -> Response {
    let status = StatusCode::from_u16(receipt.status).unwrap_or(StatusCode::OK);
    let mut response = (status, receipt.body.clone()).into_response();
    for (name, value) in &receipt.headers {
        if let (Ok(name), Ok(value)) = (
            header::HeaderName::try_from(name.as_str()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

/// Retryable failures are not remembered, so a retry runs the request again.
fn keep_receipt(status: StatusCode) -> bool {
    !matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

async fn idempotency(
    State(state): State<WorkflowApiState>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::GET {
        return next.run(request).await;
    }
    let binding = match binding_of(&request) {
        Ok(binding) => binding,
        Err(error) => return error_response(&error),
    };
    let key = match request.headers().get(headers::IDEMPOTENCY_KEY) {
        Some(value) => match value.to_str() {
            Ok(key) if validate_idempotency_key(key).is_ok() => key.to_string(),
            _ => {
                return error_response(&ApiError::new(
                    ErrorCode::Invalid,
                    "Idempotency-Key must be 1-128 printable ASCII bytes",
                ));
            }
        },
        None => {
            return error_response(&ApiError::new(
                ErrorCode::Invalid,
                "every non-GET request needs an Idempotency-Key",
            ));
        }
    };

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, state.caps.request_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return error_response(&ApiError::new(
                ErrorCode::TooLarge,
                format!(
                    "request body is over {} bytes",
                    state.caps.request_body_bytes
                ),
            ));
        }
    };
    let uri = parts.uri.to_string();
    let hash = request_hash(&parts.method, &uri, &parts.headers, &body);

    // Held until the receipt is stored: an overlapping request with the key
    // waits, then replays this one's answer or conflicts with it.
    let _hold = match state.receipts.hold(&binding.launch_id, &key).await {
        Ok(hold) => hold,
        Err(error) => return error_response(&error),
    };
    match state.receipts.get(&binding.launch_id, &key).await {
        Ok(Some(receipt)) if receipt.request_hash == hash && receipt.body_omitted => {
            return error_response(&ApiError::domain(
                "not-replayable",
                ErrorCode::ReplayConflict,
                "this request already ran; its answer was too large to keep for a replay",
            ));
        }
        Ok(Some(receipt)) if receipt.request_hash == hash => return receipt_response(&receipt),
        Ok(Some(_)) => {
            return error_response(&ApiError::new(
                ErrorCode::ReplayConflict,
                "Idempotency-Key was already used for a different request",
            ));
        }
        Ok(None) => {}
        Err(error) => return error_response(&error),
    }

    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    let (parts, body) = response.into_parts();
    let body = match to_bytes(body, state.caps.response_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return error_response(&ApiError::new(
                ErrorCode::TooLarge,
                "response body is over the cap",
            ));
        }
    };
    if keep_receipt(parts.status) {
        let body_omitted = body.len() > state.caps.receipt_body_bytes;
        let receipt = Receipt {
            request_hash: hash,
            status: parts.status.as_u16(),
            headers: parts
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
                })
                .collect(),
            body: if body_omitted {
                Vec::new()
            } else {
                body.to_vec()
            },
            body_omitted,
        };
        if let Err(error) = state.receipts.put(&binding.launch_id, &key, receipt).await {
            return error_response(&error);
        }
    }
    Response::from_parts(parts, Body::from(body))
}

async fn guard_launch(
    State(state): State<WorkflowApiState>,
    request: Request,
    next: Next,
) -> Response {
    let binding = match binding_of(&request) {
        Ok(binding) => binding,
        Err(error) => return error_response(&error),
    };
    let is_run_read = request.method() == Method::GET && request.uri().path() == "/v1/run";
    let is_outcome_post =
        request.method() == Method::POST && request.uri().path() == "/v1/run/outcome";
    let fenced = || {
        error_response(&ApiError::new(
            ErrorCode::Fenced,
            "this launch no longer holds the run's lease",
        ))
    };
    match state.backend.launch_state(&binding).await {
        Ok(LaunchState::Active) => {
            let response = next.run(request).await;
            // A write the store refused because another launch now holds the
            // run fences this one: the guest must stop, not retry.
            if response.status() == axum::http::StatusCode::SERVICE_UNAVAILABLE
                && matches!(
                    state.backend.launch_state(&binding).await,
                    Ok(LaunchState::Fenced)
                )
            {
                return fenced();
            }
            response
        }
        Ok(LaunchState::Finished) if is_run_read => next.run(request).await,
        Ok(LaunchState::Finished) => error_response(&ApiError::new(
            ErrorCode::Finished,
            "the run already posted its outcome",
        )),
        Ok(LaunchState::Released) if is_outcome_post => next.run(request).await,
        Ok(LaunchState::Fenced | LaunchState::Released) => fenced(),
        Err(error) => error_response(&error),
    }
}

async fn unsupported() -> Response {
    error_response(&ApiError::new(
        ErrorCode::Unsupported,
        "endpoint not served by this host",
    ))
}

async fn get_run(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let info = state.backend.run_info(&binding).await?;
    let mut response = serde_json::to_vec(&info)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

async fn get_run_input(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let input = state.backend.run_input(&binding).await?;
    let mut response = Bytes::from(input).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Ok(response)
}

fn parse_outcome(
    caps: &Caps,
    kind: Option<&HeaderValue>,
    body: Bytes,
) -> Result<PostedOutcome, ApiError> {
    let kind = kind
        .and_then(|value| value.to_str().ok())
        .and_then(OutcomeKind::parse)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Invalid,
                "Runtara-Outcome must be completed, failed or suspended",
            )
        })?;
    match kind {
        OutcomeKind::Completed => Ok(PostedOutcome::Completed(body.to_vec())),
        OutcomeKind::Failed => serde_json::from_slice::<ErrorInfo>(&body)
            .map(PostedOutcome::Failed)
            .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("failed outcome: {e}"))),
        OutcomeKind::Suspended => {
            let suspension: Suspension = serde_json::from_slice(&body).map_err(|e| {
                ApiError::new(ErrorCode::Invalid, format!("suspended outcome: {e}"))
            })?;
            if suspension.wakes.is_empty() {
                return Err(ApiError::new(
                    ErrorCode::Invalid,
                    "a suspension needs at least one wake",
                ));
            }
            if suspension.wakes.len() > caps.wakes_per_suspension {
                return Err(ApiError::new(
                    ErrorCode::TooLarge,
                    format!(
                        "a suspension carries at most {} wakes",
                        caps.wakes_per_suspension
                    ),
                ));
            }
            for wake in &suspension.wakes {
                check_wake(caps, wake)?;
            }
            Ok(PostedOutcome::Suspended(suspension))
        }
    }
}

/// Ids a wake names must be usable keys: instance-wait ids are bounded by the
/// wait-id cap, signal checkpoint ids must at least be present.
fn check_wake(caps: &Caps, wake: &crate::Wake) -> Result<(), ApiError> {
    match wake {
        crate::Wake::Instances(id) if id.is_empty() => Err(ApiError::new(
            ErrorCode::Invalid,
            "an instances wake needs a wait id",
        )),
        crate::Wake::Instances(id) if id.len() > caps.wait_id_bytes => Err(ApiError::new(
            ErrorCode::TooLarge,
            format!("a wait id is at most {} bytes", caps.wait_id_bytes),
        )),
        crate::Wake::OnSignal(wait) if wait.checkpoint_id.is_empty() => Err(ApiError::new(
            ErrorCode::Invalid,
            "an onSignal wake needs a checkpoint id",
        )),
        _ => Ok(()),
    }
}

async fn post_run_outcome(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "outcome body is over the cap"))?;
    let outcome = parse_outcome(&state.caps, parts.headers.get(headers::OUTCOME), body)?;
    state.backend.post_outcome(&binding, outcome).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The header that names an artifact's whole size, on each slice of it.
pub const ARTIFACT_SIZE: &str = "runtara-artifact-size";

fn checked_sha256(sha256: &str) -> Result<(), ApiError> {
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::new(ErrorCode::Invalid, "not a SHA-256"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Slice {
    offset: Option<u64>,
    length: Option<u64>,
}

/// An artifact, or the slice `offset`..`offset + length` of it: a guest's
/// host may bound what one response carries (M7.15). Its whole size is in
/// [`ARTIFACT_SIZE`].
async fn get_artifact(
    State(state): State<WorkflowApiState>,
    axum::extract::Path(sha256): axum::extract::Path<String>,
    axum::extract::Query(slice): axum::extract::Query<Slice>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    checked_sha256(&sha256)?;
    let (bytes, size) = state
        .backend
        .get_artifact(&binding, &sha256, slice.offset.unwrap_or(0), slice.length)
        .await?;
    let mut response = Bytes::from(bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(ARTIFACT_SIZE, HeaderValue::from(size));
    Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Part {
    part: u32,
    parts: u32,
}

/// Part `part` of `parts` of what the launch made, whose SHA-256 the path
/// names: a guest's host may bound what one request carries (M7.15).
async fn put_artifact(
    State(state): State<WorkflowApiState>,
    axum::extract::Path(sha256): axum::extract::Path<String>,
    axum::extract::Query(part): axum::extract::Query<Part>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    checked_sha256(&sha256)?;
    if part.parts == 0 || part.part >= part.parts {
        return Err(ApiError::new(ErrorCode::Invalid, "no such part").into());
    }
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "artifact part is over the cap"))?;
    let whole = state
        .backend
        .put_artifact(&binding, &sha256, part.part, part.parts, body.to_vec())
        .await?;
    Ok(axum::Json(serde_json::json!({ "sha256": sha256, "whole": whole })).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeadlineRequest {
    abort_after_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlarmRequest {
    after_ms: u64,
}

/// Held while the alarm is armed; it only ever answers with an error.
async fn post_run_alarm(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "alarm body is over the cap"))?;
    let alarm: AlarmRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("alarm: {e}")))?;
    state.backend.alarm(&binding, alarm.after_ms).await?;
    Err(ApiError::new(ErrorCode::Finished, "the alarm expired; the run is aborted").into())
}

async fn put_run_deadline(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "deadline body is over the cap"))?;
    let deadline: DeadlineRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("deadline: {e}")))?;
    state
        .backend
        .set_deadline(&binding, deadline.abort_after_ms)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests;
