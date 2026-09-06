//! Versioned generic child-execution imports. The guest owns orchestration.
//!
//! The embedding provides a launcher bound to immutable package/root/tenant
//! context. It must validate guest-supplied relative invocation metadata. Its
//! factory prepares owned execution futures; it must not compile or start I/O
//! before the task registry's pre-start cancellation boundary.
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use wasmtime::{
    StoreContextMut,
    component::{Linker, Resource, ResourceTable, ResourceType},
};

use crate::lifecycle::{WorkflowErrorInfo, WorkflowWake};
use crate::{
    InvokeExit,
    isolated_tasks::{CancelResult, IsolatedTasks, TaskCancellation, TaskError, TaskId},
};

pub use runtara_workflow_wit::EXECUTION_INTERFACE_NAME;

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

/// Must bind and validate relative context against the parent's authority and
/// prepared catalog. No mutable guest Store may be captured by the factory.
pub trait InvocationLauncher: Send + Sync {
    fn prepare(&self, request: StartRequest) -> Result<InvocationFactory, ExecutionError>;
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

    pub async fn shutdown(&self) -> Result<(), ExecutionError> {
        self.handles.close();
        self.tasks.shutdown().await.map_err(Into::into)
    }
}

/// Each Store exposes its own resource table and parent-bound execution context.
/// A launcher is absent for legacy runs and cannot be installed by guest input.
pub trait ExecutionView: Send + 'static {
    fn execution_table(&mut self) -> &mut ResourceTable;
    fn execution_context(&self) -> Option<&Arc<ExecutionContext>>;
}

/// Canonical resource identity protects the host-local representation. The
/// registry additionally checks its owner/generation; raw guest integers are
/// never accepted as task identities by these imports.
pub struct TaskHandle {
    id: TaskId,
    owner: Arc<ExecutionContext>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        // Also runs if the parent traps without calling resource.drop.
        let _ = self.owner.tasks.cancel(self.id);
    }
}

fn task<T: ExecutionView>(
    state: &mut T,
    handle: &Resource<TaskHandle>,
) -> wasmtime::Result<(Arc<ExecutionContext>, TaskId)> {
    let resource = state.execution_table().get(handle)?;
    let owner = resource.owner.clone();
    let id = resource.id;
    // Defense in depth if a host accidentally moves a resource between tables.
    wasmtime::ensure!(
        state
            .execution_context()
            .is_some_and(|context| Arc::ptr_eq(context, &owner)),
        "execution task belongs to another parent"
    );
    Ok((owner, id))
}

pub fn add_execution_to_linker<T: ExecutionView>(linker: &mut Linker<T>) -> anyhow::Result<()> {
    let mut interface = linker.instance(EXECUTION_INTERFACE_NAME)?;
    interface.resource_async(
        "task",
        ResourceType::host::<TaskHandle>(),
        |mut store, rep| {
            Box::new(async move {
                let handle = Resource::<TaskHandle>::new_own(rep);
                let (owner, id) = task(store.data_mut(), &handle)?;
                let resource = store.data_mut().execution_table().delete(handle)?;
                // Cancel first, even if the destructor future is interrupted.
                drop(resource);
                owner.tasks.release(id).await?;
                Ok(())
            })
        },
    )?;
    interface.func_wrap(
        "start",
        |mut store: StoreContextMut<'_, T>,
         (binding, entry, input, context): (String, Entry, Vec<u8>, InvocationContext)| {
            let Some(owner) = store.data().execution_context().cloned() else {
                return Ok((Err(ExecutionError::Unavailable),));
            };
            let factory = match owner.launcher.prepare(StartRequest {
                binding,
                entry,
                input,
                context,
            }) {
                Ok(factory) => factory,
                Err(error) => return Ok((Err(error),)),
            };
            let permit = match owner.handles.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(TryAcquireError::Closed) => return Ok((Err(ExecutionError::Closed),)),
                Err(TryAcquireError::NoPermits) => return Ok((Err(ExecutionError::Capacity),)),
            };
            let id = match owner.tasks.spawn(factory) {
                Ok(id) => id,
                Err(error) => return Ok((Err(ExecutionError::from(error)),)),
            };
            // Table insertion owns a cancel-on-drop handle even on insertion error.
            let handle = store.data_mut().execution_table().push(TaskHandle {
                id,
                owner,
                _permit: permit,
            })?;
            Ok((Ok::<_, ExecutionError>(handle),))
        },
    )?;
    interface.func_wrap(
        "request-cancel",
        |mut store: StoreContextMut<'_, T>, (handle,): (Resource<TaskHandle>,)| {
            let (owner, id) = task(store.data_mut(), &handle)?;
            let result = owner
                .tasks
                .cancel(id)
                .map(|status| match status {
                    CancelResult::Requested => CancelStatus::Requested,
                    CancelResult::AlreadyRequested => CancelStatus::AlreadyRequested,
                    CancelResult::AlreadyTerminal => CancelStatus::AlreadyTerminal,
                })
                .map_err(ExecutionError::from);
            Ok((result,))
        },
    )?;
    interface.func_wrap_concurrent("join", |accessor, (handle,): (Resource<TaskHandle>,)| {
        let task = accessor.with(|mut access| task(access.get(), &handle));
        Box::pin(async move {
            let (owner, id) = task?;
            let result = owner
                .tasks
                .join(id)
                .await
                .map(|result| TaskOutcome::from(result.outcome()))
                .map_err(ExecutionError::from);
            Ok((result,))
        })
    })?;
    interface.func_wrap_concurrent("release", |accessor, (handle,): (Resource<TaskHandle>,)| {
        let task = accessor.with(|mut access| task(access.get(), &handle));
        Box::pin(async move {
            let (owner, id) = task?;
            Ok((owner.tasks.release(id).await.map_err(ExecutionError::from),))
        })
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
