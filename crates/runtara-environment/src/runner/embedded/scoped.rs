//! Opt-in runner admission and ownership for compiler-scoped Agent packages.
use super::*;
use crate::runtime_host::{
    PersistenceRuntimeHost,
    scoped::{
        CompilerInvocationAuthority, ScopedInvocationFactory, ScopedRunSettings, ScopedRuntimeOwner,
    },
};
use runtara_component_host::execution_host::ExecutionContext;
use runtara_component_host::isolated_tasks::IsolatedTasks;
use runtara_component_host::{InvokeExit, InvokeRunResult, PreparedInvocationLauncher};
use std::collections::{BTreeMap, BTreeSet};

/// Operator-reviewed packages and per-root bounds for the opt-in scoped runner.
/// Approval covers fresh-store semantics and the compiler checkpoint contract.
/// This is not workflow input; unknown bytes cannot opt themselves into isolation.
#[derive(Clone, Debug)]
pub struct ScopedAgentRunnerConfig {
    /// Canonical Agent IDs mapped to approved lowercase SHA-256 component digests.
    pub reviewed_agents: BTreeMap<String, String>,
    /// Previously approved bytes retained for pinned artifacts and rollback.
    pub retained_agents: BTreeMap<String, BTreeSet<String>>,
    /// Maximum simultaneously retained child tasks per root.
    pub max_child_tasks: usize,
    /// Maximum completed child-result bytes retained per root.
    pub max_result_bytes: usize,
    /// Maximum canonical child handles per root, including released task handles.
    pub max_handles: usize,
}

impl ScopedAgentRunnerConfig {
    pub(super) fn validate(&self) -> Result<()> {
        if self.max_child_tasks == 0
            || self.max_child_tasks > tokio::sync::Semaphore::MAX_PERMITS
            || self.max_handles == 0
            || self.max_handles > tokio::sync::Semaphore::MAX_PERMITS
            || self.max_result_bytes == 0
        {
            return Err(RunnerError::Other(
                "invalid scoped Agent resource bounds".into(),
            ));
        }
        if self
            .reviewed_agents
            .iter()
            .chain(
                self.retained_agents
                    .iter()
                    .flat_map(|(id, digests)| digests.iter().map(move |digest| (id, digest))),
            )
            .any(|(id, digest)| {
                id.is_empty()
                    || digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(RunnerError::Other(
                "invalid scoped Agent review identity".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn admit(
    workflow: &PreparedWorkflow,
    executor: &Arc<WorkflowExecutor>,
    config: Option<&ScopedAgentRunnerConfig>,
) -> Result<Option<Arc<CompilerInvocationAuthority>>> {
    let Some(catalog) = workflow.child_catalog() else {
        return Ok(None);
    };
    let config = config.ok_or_else(|| {
        RunnerError::StartFailed(
            "scoped Agent runtime is disabled for this packaged artifact".into(),
        )
    })?;
    if !workflow.supports_scoped_runtime(executor.engine()) {
        return Err(RunnerError::StartFailed("scoped Agent roots require native lifecycle persistence without an internally composed HTTP runtime".into()));
    }
    let authority = CompilerInvocationAuthority::new(catalog.clone(), vec![]).map_err(|error| {
        RunnerError::StartFailed(format!("unsupported scoped Agent inventory: {error:?}"))
    })?;
    // Inventory validation has already proven exact binding coverage.
    for call in &catalog
        .invocations()
        .expect("authority requires inventory")
        .agent_calls
    {
        let (binding, _) = catalog
            .resolve(&call.binding)
            .expect("validated inventory binding");
        if config.reviewed_agents.get(&call.agent_id) != Some(&binding.artifact)
            && !config
                .retained_agents
                .get(&call.agent_id)
                .is_some_and(|digests| digests.contains(&binding.artifact))
        {
            return Err(RunnerError::StartFailed(format!(
                "scoped Agent `{}` has no matching runtime review",
                call.agent_id
            )));
        }
    }
    Ok(Some(Arc::new(authority)))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    executor: &Arc<WorkflowExecutor>,
    workflow: &PreparedWorkflow,
    mut spec: WorkflowRunSpec,
    root: Arc<PersistenceRuntimeHost>,
    input: Vec<u8>,
    confirmation: Option<Arc<dyn WorkflowStartConfirmation>>,
    authority: Arc<CompilerInvocationAuthority>,
    config: &ScopedAgentRunnerConfig,
) -> InvokeRunResult {
    let started = Instant::now();
    let owner = Arc::new(ScopedRuntimeOwner::new(root));
    let runtime = owner.root_runtime();
    let setup = || -> anyhow::Result<_> {
        let deadline = started
            .checked_add(spec.timeout)
            .ok_or_else(|| anyhow::anyhow!("scoped root deadline overflow"))?;
        let factory = Arc::new(ScopedInvocationFactory::new(
            owner,
            authority,
            Arc::new(ScopedRunSettings {
                env: spec.env.clone(),
                deadline,
                root_cancel: spec.cancel.clone(),
                limits: spec.limits.clone(),
            }),
        ));
        let tasks = Arc::new(
            IsolatedTasks::new(
                executor.engine().clone(),
                config.max_child_tasks,
                config.max_result_bytes,
            )
            .map_err(|error| anyhow::anyhow!("scoped task registry: {error:?}"))?,
        );
        let launcher = Arc::new(PreparedInvocationLauncher::new(
            executor.clone(),
            workflow.child_catalog().expect("admitted catalog").clone(),
            factory,
        )?);
        ExecutionContext::new(tasks, launcher, config.max_handles)
            .map_err(|error| anyhow::anyhow!("scoped execution context: {error:?}"))
    };
    let execution = match setup() {
        Ok(execution) => execution,
        Err(error) => {
            return InvokeRunResult {
                exit: InvokeExit::Trapped {
                    reason: format!("scoped root setup: {error:#}"),
                },
                memory_peak_bytes: 0,
                duration: started.elapsed(),
            };
        }
    };
    spec.runtime = Some(runtime.clone());
    spec.timeout = spec.timeout.saturating_sub(started.elapsed());
    executor
        .execute_invoke_with_coordinator(
            workflow.instance_pre(),
            spec,
            input,
            confirmation,
            execution,
            Some(runtime),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_runner_configuration_requires_finite_nonzero_capacity_and_exact_digests() {
        let base = ScopedAgentRunnerConfig {
            reviewed_agents: [("utils".into(), "a".repeat(64))].into(),
            retained_agents: BTreeMap::new(),
            max_child_tasks: 4,
            max_result_bytes: 1024,
            max_handles: 8,
        };
        base.validate().unwrap();
        for field in 0..5 {
            let mut config = base.clone();
            match field {
                0 => config.max_child_tasks = 0,
                1 => config.max_child_tasks = usize::MAX,
                2 => config.max_handles = 0,
                3 => config.max_handles = usize::MAX,
                _ => config.max_result_bytes = 0,
            }
            assert!(config.validate().is_err());
        }
        for digest in ["".into(), "g".repeat(64), "A".repeat(64), "a".repeat(63)] {
            let mut config = base.clone();
            config.reviewed_agents.insert("utils".into(), digest);
            assert!(config.validate().is_err());
        }
    }
}
