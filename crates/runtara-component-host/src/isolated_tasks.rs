//! Owned execution tasks for isolated Stores. No workflow scheduling policy.
//!
//! The runner supplies a future owning the child Store and installs the supplied
//! cancellation token in its epoch callback. This module arbitrates live
//! cancellation/completion, destroys that future before publishing its result,
//! and bounds retained task/result state. Durable attempt fencing belongs to the
//! persistence adapter, not this process-local registry.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinHandle;
use wasmtime::Engine;

use crate::InvokeExit;

/// Owned descendant teardown. Construct before starting the parent task, and
/// only start descendants inside that task's execution future. This must finish
/// after all descendant execution resources have been destroyed; no guest result
/// is published when cleanup fails or panics.
pub type TaskCleanup = Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'static>>;

/// Optional host-only durable admission and outcome arbitration. The guest still
/// chooses graph successors and retries. Ordinary tasks install no lifecycle.
#[async_trait::async_trait]
pub trait TaskLifecycle: Send + Sync {
    /// Runs inside cancellable execution before its factory/Store starts. None
    /// admits execution; Some returns an existing control outcome without a Store.
    /// Cancellation can drop this future after an external transaction commits:
    /// implementations must retain an idempotent identity outside this future.
    async fn admit(&self, cancel: TaskCancellation) -> Result<Option<InvokeExit>, TaskError>;
    /// Runs exactly once under the supervisor after execution and descendant
    /// cleanup, including pre-start cancellation, admission failure and panic.
    /// It must resolve uncertain admission and durable races before returning.
    /// The result is authoritative: cancellation arriving during settlement is
    /// a request, not permission to overwrite an already committed outcome.
    /// An incoming host failure cannot be promoted to success. Errors/panics
    /// suppress publication and fail root shutdown; callers must fence the root.
    /// Bound storage/cleanup time here. No Store or graph work may be started.
    async fn settle(
        &self,
        outcome: Result<InvokeExit, TaskError>,
        cancel: TaskCancellation,
    ) -> Result<InvokeExit, TaskError>;
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId {
    owner: u64,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskError {
    Closed,
    AtCapacity,
    WrongOwner,
    UnknownTask,
    IdExhausted,
    WorkerLost,
    NoRuntime,
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "isolated task: {self:?}")
    }
}
impl std::error::Error for TaskError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelResult {
    Requested,
    AlreadyRequested,
    AlreadyTerminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Running,
    Stopping,
    Terminal,
}

struct Control {
    phase: Mutex<Phase>,
    requested: AtomicBool,
    wake: Notify,
    engine: Arc<Engine>,
}

impl Control {
    fn cancel(&self) -> CancelResult {
        let mut phase = self.phase.lock().unwrap();
        match *phase {
            Phase::Terminal => CancelResult::AlreadyTerminal,
            Phase::Stopping => CancelResult::AlreadyRequested,
            Phase::Running => {
                *phase = Phase::Stopping;
                self.requested.store(true, Ordering::Release);
                // One execution future waits on this notification. notify_one
                // retains the permit if cancellation arrives before its poll.
                self.wake.notify_one();
                self.engine.increment_epoch();
                CancelResult::Requested
            }
        }
    }
}

/// Cancellation is monotonic. CPU-bound guests must check this in their Store's
/// epoch callback; native blocking work cannot be interrupted by dropping a future.
#[derive(Clone)]
pub struct TaskCancellation(Arc<Control>);

impl TaskCancellation {
    pub fn is_requested(&self) -> bool {
        self.0.requested.load(Ordering::Acquire)
    }
}

struct ResultBudget {
    used: Mutex<usize>,
    limit: usize,
}

/// Result bytes stay charged until the last owner releases this object, even
/// after the registry handle is released. Caller-created copies are outside this
/// registry's accounting and must be charged at their own transport boundary.
pub struct TaskResult {
    outcome: InvokeExit,
    budget: Arc<ResultBudget>,
    bytes: usize,
}

impl TaskResult {
    pub fn outcome(&self) -> &InvokeExit {
        &self.outcome
    }
}

impl Drop for TaskResult {
    fn drop(&mut self) {
        let mut used = self.budget.used.lock().unwrap();
        *used -= self.bytes;
    }
}

fn retained_bytes(outcome: &InvokeExit) -> usize {
    match outcome {
        InvokeExit::Completed(bytes) => bytes.capacity(),
        InvokeExit::Failed(error) => [
            error.code.capacity(),
            error.message.capacity(),
            error.category.capacity(),
            error.severity.capacity(),
            error.attributes.as_ref().map_or(0, String::capacity),
        ]
        .into_iter()
        .fold(0, usize::saturating_add),
        InvokeExit::Suspended(wakes) => wakes.iter().fold(
            wakes
                .capacity()
                .saturating_mul(std::mem::size_of::<crate::lifecycle::WorkflowWake>()),
            |size, wake| {
                let extra = match wake {
                    crate::lifecycle::WorkflowWake::OnSignal(signal) => {
                        signal.checkpoint_id.capacity()
                    }
                    _ => 0,
                };
                size.saturating_add(extra)
            },
        ),
        InvokeExit::Trapped { reason } => reason.capacity(),
        InvokeExit::Timeout | InvokeExit::Cancelled | InvokeExit::CleanupAborted => 0,
    }
}

struct Task {
    control: Arc<Control>,
    result: watch::Receiver<Option<Arc<TaskResult>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    _permit: OwnedSemaphorePermit,
}

impl Task {
    async fn join(&self) -> Result<Arc<TaskResult>, TaskError> {
        let mut result = self.result.clone();
        loop {
            if let Some(outcome) = result.borrow_and_update().clone() {
                return Ok(outcome);
            }
            result.changed().await.map_err(|_| TaskError::WorkerLost)?;
        }
    }

    async fn reap(&self) -> Result<(), TaskError> {
        let worker = self.worker.lock().unwrap().take();
        if let Some(worker) = worker {
            worker.await.map_err(|_| TaskError::WorkerLost)?;
        }
        self.join().await?;
        Ok(())
    }
}

struct RegistryState {
    closed: bool,
    shutdown_failed: bool,
    next_id: u64,
    tasks: BTreeMap<u64, Arc<Task>>,
}

/// One root execution owns one registry. Admission is fail-fast: a waiting
/// parent cannot consume a child permit and deadlock a shared admission queue.
/// Explicit shutdown cancels and reaps all children before the root is released.
pub struct IsolatedTasks {
    owner: u64,
    engine: Arc<Engine>,
    state: Mutex<RegistryState>,
    slots: Arc<Semaphore>,
    budget: Arc<ResultBudget>,
    shutdown_lock: tokio::sync::Mutex<()>,
}

impl IsolatedTasks {
    pub fn new(
        engine: Arc<Engine>,
        max_tasks: usize,
        max_result_bytes: usize,
    ) -> Result<Self, TaskError> {
        let owner = NEXT_OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| TaskError::IdExhausted)?;
        // Avoid Semaphore's panic for an invalid operator-provided bound.
        if max_tasks > Semaphore::MAX_PERMITS {
            return Err(TaskError::AtCapacity);
        }
        Ok(Self {
            owner,
            engine,
            state: Mutex::new(RegistryState {
                closed: false,
                shutdown_failed: false,
                next_id: 1,
                tasks: BTreeMap::new(),
            }),
            slots: Arc::new(Semaphore::new(max_tasks)),
            budget: Arc::new(ResultBudget {
                used: Mutex::new(0),
                limit: max_result_bytes,
            }),
            shutdown_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// The factory must only construct a future. Instantiation, I/O and all guest
    /// work happen inside that future, after the pre-start cancellation check.
    pub fn spawn<F, Fut>(&self, run: F) -> Result<TaskId, TaskError>
    where
        F: FnOnce(TaskCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = InvokeExit> + Send + 'static,
    {
        self.spawn_inner(run, None, None)
    }

    /// Run descendant teardown after dropping execution, even on pre-start
    /// cancellation or panic. The supervisor owns cleanup outside the child
    /// future, so cancellation cannot skip it by dropping that future.
    pub fn spawn_scoped<F, Fut>(&self, run: F, cleanup: TaskCleanup) -> Result<TaskId, TaskError>
    where
        F: FnOnce(TaskCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = InvokeExit> + Send + 'static,
    {
        self.spawn_inner(run, Some(cleanup), None)
    }

    /// Attach host-only async admission/settlement while keeping teardown outside
    /// cancellable execution. No lifecycle preserves ordinary task semantics.
    pub fn spawn_managed<F, Fut>(
        &self,
        run: F,
        cleanup: Option<TaskCleanup>,
        lifecycle: Option<Arc<dyn TaskLifecycle>>,
    ) -> Result<TaskId, TaskError>
    where
        F: FnOnce(TaskCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = InvokeExit> + Send + 'static,
    {
        self.spawn_inner(run, cleanup, lifecycle)
    }

    fn spawn_inner<F, Fut>(
        &self,
        run: F,
        cleanup: Option<TaskCleanup>,
        lifecycle: Option<Arc<dyn TaskLifecycle>>,
    ) -> Result<TaskId, TaskError>
    where
        F: FnOnce(TaskCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = InvokeExit> + Send + 'static,
    {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(TaskError::Closed);
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| TaskError::NoRuntime)?;
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| TaskError::AtCapacity)?;
        let sequence = state.next_id;
        state.next_id = sequence.checked_add(1).ok_or(TaskError::IdExhausted)?;
        let control = Arc::new(Control {
            phase: Mutex::new(Phase::Running),
            requested: AtomicBool::new(false),
            wake: Notify::new(),
            engine: self.engine.clone(),
        });
        let (tx, rx) = watch::channel(None);
        let runner_control = control.clone();
        let admission = lifecycle.clone();
        let admitting = lifecycle.as_ref().map(|_| Arc::new(AtomicBool::new(false)));
        let worker_admitting = admitting.clone();
        let worker = runtime.spawn(async move {
            let cancel = TaskCancellation(runner_control.clone());
            if cancel.is_requested() {
                return Ok(InvokeExit::Cancelled);
            }
            let mut run = Box::pin(async move {
                if let Some(admission) = admission {
                    let admitting = worker_admitting.as_ref().expect("managed admission state");
                    admitting.store(true, Ordering::Release);
                    let result = admission.admit(cancel.clone()).await;
                    admitting.store(false, Ordering::Release);
                    if let Some(outcome) = result? {
                        return Ok(outcome);
                    }
                    if cancel.is_requested() {
                        return Ok(InvokeExit::Cancelled);
                    }
                }
                Ok(run(cancel).await)
            });
            let outcome = tokio::select! {
                biased;
                _=runner_control.wake.notified()=>Ok(InvokeExit::Cancelled),
                outcome=&mut run=>outcome,
            };
            // Includes admission, the child Store and its pending host futures.
            drop(run);
            outcome
        });
        let finish_control = control.clone();
        let budget = self.budget.clone();
        let cleanup_runtime = runtime.clone();
        let supervisor = runtime.spawn(async move {
            // Join also observes native panics and waits for task-owned values
            // to be dropped; an executor panic cannot strand join indefinitely.
            let mut candidate = worker.await.unwrap_or_else(|_| {
                if admitting
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::Acquire))
                {
                    Err(TaskError::WorkerLost)
                } else {
                    Ok(InvokeExit::Trapped {
                        reason: "isolated execution worker failed".into(),
                    })
                }
            });
            if let Some(cleanup) = cleanup
                && !matches!(cleanup_runtime.spawn(cleanup).await, Ok(Ok(())))
            {
                candidate = Err(TaskError::WorkerLost);
            }
            if *finish_control.phase.lock().unwrap() == Phase::Stopping && candidate.is_ok() {
                candidate = Ok(InvokeExit::Cancelled);
            }
            let managed = lifecycle.is_some();
            if let Some(lifecycle) = lifecycle {
                let failed = candidate.is_err();
                let token = TaskCancellation(finish_control.clone());
                candidate = cleanup_runtime
                    .spawn(async move { lifecycle.settle(candidate, token).await })
                    .await
                    .unwrap_or(Err(TaskError::WorkerLost));
                if failed {
                    return;
                } // a finalizer cannot erase unconfirmed cleanup/admission
            }
            let Ok(mut outcome) = candidate else {
                return;
            };
            let mut phase = finish_control.phase.lock().unwrap();
            if !managed && *phase == Phase::Stopping {
                outcome = InvokeExit::Cancelled;
            }
            let mut bytes = retained_bytes(&outcome);
            let mut used = budget.used.lock().unwrap();
            if bytes > budget.limit.saturating_sub(*used) {
                if managed {
                    return;
                } // never replace a durable winner with a recoverable guest trap
                // Fixed-size fallback is charged as task metadata, not payload.
                outcome = InvokeExit::Trapped {
                    reason: "isolated result budget exceeded".into(),
                };
                bytes = 0;
            }
            *used += bytes;
            drop(used);
            *phase = Phase::Terminal;
            tx.send_replace(Some(Arc::new(TaskResult {
                outcome,
                budget,
                bytes,
            })));
        });
        state.tasks.insert(
            sequence,
            Arc::new(Task {
                control,
                result: rx,
                worker: Mutex::new(Some(supervisor)),
                _permit: permit,
            }),
        );
        Ok(TaskId {
            owner: self.owner,
            sequence,
        })
    }

    fn task(&self, id: TaskId) -> Result<Arc<Task>, TaskError> {
        if id.owner != self.owner {
            return Err(TaskError::WrongOwner);
        }
        self.state
            .lock()
            .unwrap()
            .tasks
            .get(&id.sequence)
            .cloned()
            .ok_or(TaskError::UnknownTask)
    }

    pub fn cancel(&self, id: TaskId) -> Result<CancelResult, TaskError> {
        Ok(self.task(id)?.control.cancel())
    }

    pub async fn join(&self, id: TaskId) -> Result<Arc<TaskResult>, TaskError> {
        self.task(id)?.join().await
    }

    /// Releasing a live task means cancel-and-reap. Repeated release is safe;
    /// future joins on the released handle fail rather than alias a new task.
    pub async fn release(&self, id: TaskId) -> Result<(), TaskError> {
        if id.owner != self.owner {
            return Err(TaskError::WrongOwner);
        }
        let task = self.state.lock().unwrap().tasks.get(&id.sequence).cloned();
        if let Some(task) = task {
            task.control.cancel();
            task.reap().await?;
            self.state.lock().unwrap().tasks.remove(&id.sequence);
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), TaskError> {
        let _shutdown = self.shutdown_lock.lock().await;
        let tasks = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            state.tasks.values().cloned().collect::<Vec<_>>()
        };
        for task in &tasks {
            task.control.cancel();
        }
        let mut failed = false;
        for task in &tasks {
            failed |= task.reap().await.is_err();
        }
        let mut state = self.state.lock().unwrap();
        state.tasks.clear();
        // Clearing retained tasks must not clear evidence that teardown was
        // unconfirmed. Every later/concurrent owner must observe the same host
        // failure rather than publish a successful root result after a retry.
        state.shutdown_failed |= failed;
        if state.shutdown_failed {
            Err(TaskError::WorkerLost)
        } else {
            Ok(())
        }
    }

    pub fn retained_result_bytes(&self) -> usize {
        *self.budget.used.lock().unwrap()
    }
}

impl Drop for IsolatedTasks {
    fn drop(&mut self) {
        // Last-resort cancellation. Root runners must await shutdown for a
        // teardown guarantee; Rust Drop cannot await asynchronous Store cleanup.
        for task in self.state.get_mut().unwrap().tasks.values() {
            task.control.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
