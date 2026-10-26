//! Child runs of a workflow, served as the workflow API's `/v1/tasks`. The
//! guest owns orchestration.
//!
//! The embedding provides a launcher bound to immutable package/root/tenant
//! context. It must validate guest-supplied relative invocation metadata. Its
//! factory prepares owned execution futures; it must not compile or start I/O
//! before the task registry's pre-start cancellation boundary.
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

use crate::lifecycle::{WorkflowErrorInfo, WorkflowWake};
use crate::{
    InvokeExit,
    isolated_tasks::{
        CancelResult, IsolatedTasks, TaskCancellation, TaskCleanup, TaskError, TaskId,
        TaskLifecycle, TeardownDisposition,
    },
};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum Entry {
    #[component(name = "capability")]
    Capability(String),
    #[component(name = "workflow")]
    Workflow,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(record)]
pub struct InvocationContext {
    pub path: String,
    pub attempt: u64,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(enum)]
#[repr(u8)]
pub enum ExecutionError {
    #[component(name = "unavailable")]
    Unavailable,
    #[component(name = "closed")]
    Closed,
    #[component(name = "capacity")]
    Capacity,
    #[component(name = "invalid-binding")]
    InvalidBinding,
    #[component(name = "invalid-context")]
    InvalidContext,
    #[component(name = "invalid-task")]
    InvalidTask,
    #[component(name = "worker-lost")]
    WorkerLost,
}

impl From<TaskError> for ExecutionError {
    fn from(error: TaskError) -> Self {
        match error {
            TaskError::Closed => Self::Closed,
            TaskError::AtCapacity | TaskError::IdExhausted => Self::Capacity,
            TaskError::WrongOwner | TaskError::UnknownTask => Self::InvalidTask,
            TaskError::WorkerLost => Self::WorkerLost,
            TaskError::NoRuntime => Self::Unavailable,
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(enum)]
#[repr(u8)]
pub enum CancelStatus {
    #[component(name = "requested")]
    Requested,
    #[component(name = "already-requested")]
    AlreadyRequested,
    #[component(name = "already-terminal")]
    AlreadyTerminal,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum TaskOutcome {
    #[component(name = "completed")]
    Completed(Vec<u8>),
    #[component(name = "failed")]
    Failed(WorkflowErrorInfo),
    #[component(name = "suspended")]
    Suspended(Vec<WorkflowWake>),
    #[component(name = "cancelled")]
    Cancelled,
    #[component(name = "timed-out")]
    TimedOut,
    #[component(name = "trapped")]
    Trapped(String),
}

impl From<&InvokeExit> for TaskOutcome {
    fn from(exit: &InvokeExit) -> Self {
        match exit {
            InvokeExit::Completed(bytes) => Self::Completed(bytes.clone()),
            InvokeExit::Failed(error) => Self::Failed(error.clone()),
            InvokeExit::Suspended(wakes) => Self::Suspended(wakes.clone()),
            InvokeExit::Cancelled => Self::Cancelled,
            InvokeExit::CleanupAborted => {
                Self::Trapped(crate::cleanup_alarm::CleanupGraceExpired.to_string())
            }
            InvokeExit::Timeout => Self::TimedOut,
            InvokeExit::Trapped { reason } => Self::Trapped(reason.clone()),
        }
    }
}

pub struct StartRequest {
    pub binding: String,
    pub entry: Entry,
    pub input: Vec<u8>,
    pub context: InvocationContext,
}

pub type ExecutionFuture = Pin<Box<dyn Future<Output = InvokeExit> + Send + 'static>>;
pub type InvocationFactory = Box<dyn FnOnce(TaskCancellation) -> ExecutionFuture + Send + 'static>;

/// An execution future and optional separately owned descendant teardown.
/// Cleanup and durable settlement must not live inside `run`: cancellation
/// would skip them. Launch this through `spawn_managed` to retain both.
pub struct PreparedInvocation {
    pub run: InvocationFactory,
    pub cleanup: Option<TaskCleanup>,
    /// Optional asynchronous admission and durable settlement owned by the supervisor.
    pub lifecycle: Option<Arc<dyn TaskLifecycle>>,
}

impl PreparedInvocation {
    pub fn leaf(run: InvocationFactory) -> Self {
        Self {
            run,
            cleanup: None,
            lifecycle: None,
        }
    }
}

/// Must bind and validate relative context against the parent's authority and
/// prepared catalog. No mutable guest Store may be captured by the factory.
pub trait InvocationLauncher: Send + Sync {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError>;
}

/// Owned outside the parent Store. The root runner must await `shutdown` after
/// dropping its guest execution future, including trap and cancellation paths.
/// Store destruction requests cancellation; it cannot itself await teardown.
pub struct ExecutionContext {
    tasks: Arc<IsolatedTasks>,
    launcher: Arc<dyn InvocationLauncher>,
    handles: Arc<Semaphore>,
}

impl ExecutionContext {
    /// `max_handles` bounds canonical resource metadata independently of live
    /// tasks. A released task's guest handle remains charged until resource-drop.
    pub fn new(
        tasks: Arc<IsolatedTasks>,
        launcher: Arc<dyn InvocationLauncher>,
        max_handles: usize,
    ) -> Result<Arc<Self>, ExecutionError> {
        if max_handles > Semaphore::MAX_PERMITS {
            return Err(ExecutionError::Capacity);
        }
        Ok(Arc::new(Self {
            tasks,
            launcher,
            handles: Arc::new(Semaphore::new(max_handles)),
        }))
    }

    /// Move an owned child scope into its parent's task supervisor. This future
    /// must be kept outside the guest execution future so cancellation runs it.
    pub fn into_cleanup(self: Arc<Self>) -> TaskCleanup {
        TaskCleanup::with_disposition(move |disposition| async move {
            self.shutdown_with(disposition)
                .await
                .map_err(|_| TaskError::WorkerLost)
        })
    }

    /// Start a child run, owned by the returned handle: dropping it cancels
    /// the child, as destroying its owner does.
    pub(crate) fn start_task(
        self: &Arc<Self>,
        request: StartRequest,
    ) -> Result<TaskHandle, ExecutionError> {
        let factory = self.launcher.prepare(request)?;
        let permit = match self.handles.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::Closed) => return Err(ExecutionError::Closed),
            Err(TryAcquireError::NoPermits) => return Err(ExecutionError::Capacity),
        };
        let id = self
            .tasks
            .spawn_managed(factory.run, factory.cleanup, factory.lifecycle)?;
        Ok(TaskHandle {
            id,
            owner: Arc::clone(self),
            _permit: permit,
        })
    }

    pub async fn shutdown(&self) -> Result<(), ExecutionError> {
        self.shutdown_with(TeardownDisposition::Terminal).await
    }

    pub async fn shutdown_with(
        &self,
        disposition: TeardownDisposition,
    ) -> Result<(), ExecutionError> {
        self.handles.close();
        self.tasks
            .shutdown_with(disposition)
            .await
            .map_err(Into::into)
    }
}

/// One child run, owned by its launch: dropping it cancels the child, as
/// destroying the owner does. The registry checks its owner and generation;
/// a guest names a task only by the id its launch handed out.
pub struct TaskHandle {
    id: TaskId,
    owner: Arc<ExecutionContext>,
    _permit: OwnedSemaphorePermit,
}

impl TaskHandle {
    pub(crate) fn request_cancel(&self) -> Result<CancelStatus, ExecutionError> {
        self.owner
            .tasks
            .cancel(self.id)
            .map(|status| match status {
                CancelResult::Requested => CancelStatus::Requested,
                CancelResult::AlreadyRequested => CancelStatus::AlreadyRequested,
                CancelResult::AlreadyTerminal => CancelStatus::AlreadyTerminal,
            })
            .map_err(ExecutionError::from)
    }

    /// The task's outcome, once its execution was destroyed.
    pub(crate) async fn join(&self) -> Result<TaskOutcome, ExecutionError> {
        self.owner
            .tasks
            .join(self.id)
            .await
            .map(|result| TaskOutcome::from(result.outcome()))
            .map_err(ExecutionError::from)
    }

    /// Cancel and reap the task.
    pub(crate) async fn release(&self) -> Result<(), ExecutionError> {
        self.owner
            .tasks
            .release(self.id)
            .await
            .map_err(ExecutionError::from)
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        // Automatic Store destruction has no lifecycle outcome yet. Explicit
        // guest drop/release records local cancellation before reaching here.
        let _ = self.owner.tasks.owner_dropped(self.id);
    }
}

#[cfg(test)]
mod tests;
