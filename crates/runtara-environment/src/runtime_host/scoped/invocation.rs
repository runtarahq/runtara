//! Bind a prepared child launch to root-owned persistence and host authority.
use super::*;
use runtara_component_host::execution_host::{ExecutionContext, ExecutionError, StartRequest};
use runtara_component_host::{
    ChildInvocationScope, ChildInvocationSpec, InvocationScopeFactory, InvokeExit, WorkflowLimits,
    WorkflowRunSpec,
};
use std::collections::HashMap;

/// Result of authorizing one child against its parent and verified package.
pub struct AuthorizedChild {
    /// Compiler-owned caller durability. Older inventories have no such fact.
    /// None must not be interpreted as authority to enable durable fencing.
    pub durable: Option<bool>,
    /// Authority over already-derived checkpoint addresses; never a second prefix.
    pub checkpoints: Arc<dyn CheckpointAuthority>,
    /// Fresh, unstarted ownership context for this child's descendants, if any.
    /// Never reuse a parent's or sibling's context. Its cleanup is transferred
    /// to the prepared launcher's task supervisor outside the cancellable run.
    pub execution: Option<Arc<ExecutionContext>>,
}

/// Host-supplied policy bound to immutable parent/package authority. It must
/// validate the binding, entry, logical path and attempt against that parent,
/// including the compiler's namespace contract. No permissive default exists.
/// This synchronous call must not perform IO or launch descendants. Input bytes
/// may not select a tenant, root, prepared catalog, environment or credentials.
pub trait InvocationAuthority: Send + Sync {
    /// Authorize one relative invocation and allocate only unstarted ownership.
    fn authorize(&self, request: &StartRequest) -> Result<AuthorizedChild, ExecutionError>;
}

/// Settings fixed by the root runner, not by child input or relative metadata.
pub struct ScopedRunSettings {
    /// Explicitly approved guest environment, including existing opaque IO context.
    pub env: HashMap<String, String>,
    /// Absolute active-run deadline shared by all children. Admission and
    /// earlier siblings consume this budget; a child never resets it.
    pub deadline: Instant,
    /// Independent root cancellation signal, in addition to each real task token.
    pub root_cancel: Option<Arc<AtomicBool>>,
    /// Per-memory and per-table limits; aggregate reservations remain separate.
    pub limits: WorkflowLimits,
}

/// Persistence-backed scope construction for `PreparedInvocationLauncher`.
/// The embedding still supplies validated package/parent authority and must
/// retain root ownership through cleanup, command finalization and durable
/// attempt fencing. Constructing this factory does not enable a runtime backend.
pub struct ScopedInvocationFactory {
    owner: Arc<ScopedRuntimeOwner>,
    authority: Arc<dyn InvocationAuthority>,
    settings: Arc<ScopedRunSettings>,
}

impl ScopedInvocationFactory {
    /// Bind the factory to immutable root persistence, policy and run settings.
    pub fn new(
        owner: Arc<ScopedRuntimeOwner>,
        authority: Arc<dyn InvocationAuthority>,
        settings: Arc<ScopedRunSettings>,
    ) -> Self {
        Self {
            owner,
            authority,
            settings,
        }
    }

    /// Explicit durable path for an already-admitted, host-selected attempt.
    /// Carries the same IO authority into both the runtime and task lifecycle;
    /// callers cannot accidentally attach fenced IO without supervised settlement.
    pub fn prepare_fenced_child(
        &self,
        request: &StartRequest,
        io: Arc<InvocationIo>,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        if io.fence().path != request.context.path
            || io.fence().lease.instance_id != self.owner.root.instance_id
            || !Arc::ptr_eq(&io.persistence, &self.owner.root.state.persistence)
        {
            return Err(ExecutionError::InvalidContext);
        }
        self.prepare(request, Some(io))
    }

    fn prepare(
        &self,
        request: &StartRequest,
        io: Option<Arc<InvocationIo>>,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        self.owner
            .ensure_open()
            .map_err(|_| ExecutionError::Closed)?;
        if request.context.path.is_empty() || request.context.attempt == 0 {
            return Err(ExecutionError::InvalidContext);
        }
        let authorized = self.authority.authorize(request)?;
        if io.is_some() && authorized.durable != Some(true) {
            return Err(ExecutionError::InvalidContext);
        }
        let owner = self.owner.clone();
        let settings = self.settings.clone();
        let input = request.input.clone();
        let path = request.context.path.clone();
        Ok(ChildInvocationScope {
            lifecycle: io.as_ref().map(|io| {
                io.clone() as Arc<dyn runtara_component_host::isolated_tasks::TaskLifecycle>
            }),
            make_spec: Box::new(move |cancel| {
                // Recheck the root admission fence inside the actual task,
                // including a close between authorization and task start.
                let runtime = match io {
                    Some(io) => owner.child_fenced(input, authorized.checkpoints, cancel, io)?,
                    None => owner.child(input, path, authorized.checkpoints, cancel)?,
                };
                Ok(ChildInvocationSpec {
                    deadline: Some(settings.deadline),
                    spec: WorkflowRunSpec {
                        env: settings.env.clone(),
                        stderr: None,
                        timeout: settings.deadline.saturating_duration_since(Instant::now()),
                        cancel: settings.root_cancel.clone(),
                        limits: settings.limits.clone(),
                        runtime: Some(runtime.clone()),
                    },
                    outcome_check: Some(Box::new(move |exit| check_terminal(&runtime, exit))),
                })
            }),
            execution: authorized.execution,
        })
    }
}

impl InvocationScopeFactory for ScopedInvocationFactory {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<ChildInvocationScope, ExecutionError> {
        self.prepare(request, None)
    }
}

fn check_terminal(runtime: &ScopedRuntimeHost, exit: &InvokeExit) -> Result<(), String> {
    // A trap, cancellation, timeout or suspension cannot publish a staged
    // completion. Preserve that typed control outcome instead of replacing it
    // with a callback mismatch. The callback bytes never reach root persistence.
    if !matches!(exit, InvokeExit::Completed(_) | InvokeExit::Failed(_)) {
        return Ok(());
    }
    match (runtime.terminal()?, exit) {
        (None, _) => Ok(()),
        (Some(ChildTerminal::Complete(expected)), InvokeExit::Completed(actual))
            if &expected == actual =>
        {
            Ok(())
        }
        // Runtime failure payloads can contain additional metadata beyond the
        // exported error-info record. Preserve the exported typed error, as the
        // root's deferred terminal wrapper does, without parsing/re-encoding it.
        (Some(ChildTerminal::Fail(_)), InvokeExit::Failed(_)) => Ok(()),
        _ => Err("child terminal callback disagrees with invocation outcome".into()),
    }
}
