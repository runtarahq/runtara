//! Resolve only prepared package bindings; orchestration stays in the guest.
use super::*;
use crate::execution_host::{
    Entry, ExecutionError, InvocationLauncher, PreparedInvocation, StartRequest,
};
use crate::isolated_tasks::{TaskCancellation, TaskLifecycle};
use runtara_workflow_wit::isolation_package::NamespaceFrame;

/// Runtime authority for one invocation. Constructing the spec happens inside
/// the owned task, after the registry's cancellation check and optional async
/// admission, so its
/// runtime adapter can use the actual task token. Do not copy an unrestricted
/// root RuntimeHost into this scope.
///
/// `execution`, when present, is a fresh context for this child's descendants.
/// The launcher transfers its cleanup to the supervisor, outside the execution
/// future. Scope construction must not start any descendant work.
pub struct ChildInvocationScope {
    pub make_spec:
        Box<dyn FnOnce(TaskCancellation) -> Result<ChildInvocationSpec, String> + Send + 'static>,
    pub execution: Option<Arc<ExecutionContext>>,
    /// Durable admission and settlement, kept outside cancellable Store execution.
    pub lifecycle: Option<Arc<dyn TaskLifecycle>>,
}

/// A successfully admitted child and optional local outcome validation. The
/// check runs after its Store is gone and may reject inconsistent captured
/// callbacks. It must not perform IO, publish root state or schedule graph work.
/// Descendant teardown still belongs to the task supervisor, even if this check
/// fails or panics. Cancellation of execution discards the check with the future.
pub struct ChildInvocationSpec {
    pub spec: WorkflowRunSpec,
    /// An inherited absolute deadline also covers Store setup. Unlike a root's
    /// active timeout, crossing a child start gate cannot reset this deadline.
    pub deadline: Option<Instant>,
    pub outcome_check: Option<ChildOutcomeCheck>,
}

pub type ChildOutcomeCheck = Box<dyn FnOnce(&InvokeExit) -> Result<(), String> + Send + 'static>;

impl From<WorkflowRunSpec> for ChildInvocationSpec {
    fn from(spec: WorkflowRunSpec) -> Self {
        Self {
            spec,
            deadline: None,
            outcome_check: None,
        }
    }
}

/// Bound to immutable parent/root/tenant authority by the embedding. Validate
/// relative invocation identity and authorize the binding/entry against that
/// scope. Input bytes and guest metadata must never select a different tenant,
/// root RuntimeHost, prepared catalog, or credential channel.
///
/// No I/O, compilation, guest execution or descendant launch is allowed here.
/// This interface supplies authority; it does not choose graph successors,
/// retries, recovery, or suspension policy.
pub trait InvocationScopeFactory: Send + Sync {
    fn prepare_child(&self, request: &StartRequest)
    -> Result<ChildInvocationScope, ExecutionError>;
}

/// A production launcher over the catalog retained by the verified preparation
/// token. Every Store is fresh; only immutable prepared code is shared. The
/// scope factory is required, so enabling task imports cannot accidentally
/// inherit the parent's unrestricted runtime authority.
pub struct PreparedInvocationLauncher {
    executor: Arc<WorkflowExecutor>,
    catalog: Arc<PreparedChildCatalog>,
    scopes: Arc<dyn InvocationScopeFactory>,
    inherited_namespace: Vec<NamespaceFrame>,
}

impl PreparedInvocationLauncher {
    pub fn new(
        executor: Arc<WorkflowExecutor>,
        catalog: Arc<PreparedChildCatalog>,
        scopes: Arc<dyn InvocationScopeFactory>,
    ) -> Result<Self> {
        catalog.validate_engine(executor.engine())?;
        Ok(Self {
            executor,
            catalog,
            scopes,
            inherited_namespace: Vec::new(),
        })
    }
    /// Bind this launcher to its host-authorized parent namespace. Root workflows
    /// use the empty default. Never derive this from an incoming StartRequest.
    pub fn with_inherited_namespace(mut self, namespace: Vec<NamespaceFrame>) -> Self {
        self.inherited_namespace = namespace;
        self
    }
}

impl InvocationLauncher for PreparedInvocationLauncher {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        let (binding, pre) = self
            .catalog
            .resolve(&request.binding)
            .ok_or(ExecutionError::InvalidBinding)?;
        let lifecycle = matches!(
            binding.interface.as_str(),
            runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME
                | runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME_V1
        );
        if lifecycle != matches!(request.entry, Entry::Workflow) {
            return Err(ExecutionError::InvalidBinding);
        }
        if let Some(invocations) = self.catalog.invocations() {
            let Entry::Capability(capability) = &request.entry else {
                return Err(ExecutionError::InvalidBinding);
            };
            // Compiler namespace membership is checked before scope allocation.
            // Checkpoint grants and durable attempt fencing remain mandatory.
            let resolved = if matches!(invocations.version, 3 | 4) {
                invocations.resolve_scoped_agent_invocation(
                    &request.binding,
                    capability,
                    &request.context.path,
                    request.context.attempt,
                    &self.inherited_namespace,
                )
            } else {
                invocations.resolve_agent_invocation(
                    &request.binding,
                    capability,
                    &request.context.path,
                    request.context.attempt,
                )
            };
            resolved.map_err(|_| ExecutionError::InvalidContext)?;
        }
        let scope = self.scopes.prepare_child(&request)?;
        let cleanup = scope
            .execution
            .as_ref()
            .map(|execution| execution.clone().into_cleanup());
        let pre = pre.clone();
        let interface = binding.interface.clone();
        let executor = self.executor.clone();
        Ok(PreparedInvocation {
            run: Box::new(move |token| {
                Box::pin(async move {
                    let child = match (scope.make_spec)(token.clone()) {
                        Ok(child) => child,
                        Err(reason) => {
                            return InvokeExit::Trapped {
                                reason: format!("child scope setup failed: {reason}"),
                            };
                        }
                    };
                    let entry = match &request.entry {
                        Entry::Capability(capability) => InvocationEntry::Capability {
                            interface: &interface,
                            capability,
                        },
                        Entry::Workflow => InvocationEntry::Lifecycle {
                            interface: Some(&interface),
                        },
                    };
                    let exit = executor
                        .execute_entry(
                            &pre,
                            child.spec,
                            request.input,
                            None,
                            entry,
                            InvocationControl {
                                task_cancel: Some(token),
                                execution: scope.execution,
                                deadline: child.deadline,
                                ..Default::default()
                            },
                        )
                        .await
                        .exit;
                    if let Some(check) = child.outcome_check
                        && let Err(reason) = check(&exit)
                    {
                        return InvokeExit::Trapped {
                            reason: format!("child outcome validation failed: {reason}"),
                        };
                    }
                    exit
                })
            }),
            cleanup,
            lifecycle: scope.lifecycle,
        })
    }
}

#[cfg(test)]
#[path = "prepared_launcher_tests.rs"]
mod tests;
