//! Runtime grants from the verified compiler inventory, never from user data.
use super::*;
use runtara_component_host::PreparedChildCatalog;
use runtara_component_host::execution_host::{Entry, ExecutionError, StartRequest};
use runtara_workflow_wit::isolation_package::{
    CheckpointNamespace, InvocationSelector, NamespaceFrame,
};

/// Bound to the exact prepared catalog and inherited parent namespace. Package
/// reset eligibility and execution ownership remain the root runner's concern.
/// Older experimental inventories need their original explicit scope policy.
pub struct CompilerInvocationAuthority {
    catalog: Arc<PreparedChildCatalog>,
    inherited: Vec<NamespaceFrame>,
}
impl CompilerInvocationAuthority {
    /// Require a prepared inventory with explicit checkpoint contracts.
    pub fn new(
        catalog: Arc<PreparedChildCatalog>,
        inherited: Vec<NamespaceFrame>,
    ) -> Result<Self, ExecutionError> {
        if catalog.invocations().is_none_or(|inventory| {
            inventory.version != 4 || !inventory.checkpoint_conflicts().is_empty()
        }) {
            return Err(ExecutionError::InvalidContext);
        }
        Ok(Self { catalog, inherited })
    }
}

struct Checkpoints(Option<CheckpointNamespace>);
impl CheckpointAuthority for Checkpoints {
    fn authorize(&self, key: &str) -> Result<(), String> {
        self.0
            .as_ref()
            .ok_or_else(|| "capability has no checkpoint authority".to_string())?
            .authorize(key)
            .map_err(str::to_owned)
    }
}
impl InvocationAuthority for CompilerInvocationAuthority {
    fn authorize(&self, request: &StartRequest) -> Result<AuthorizedChild, ExecutionError> {
        let Entry::Capability(capability) = &request.entry else {
            return Err(ExecutionError::InvalidBinding);
        };
        let inventory = self
            .catalog
            .invocations()
            .ok_or(ExecutionError::InvalidContext)?;
        let invocation = inventory
            .resolve_scoped_agent_invocation(
                &request.binding,
                capability,
                &request.context.path,
                request.context.attempt,
                &self.inherited,
            )
            .map_err(|_| ExecutionError::InvalidContext)?;
        let InvocationSelector::CallSite(token) = invocation.selector else {
            return Err(ExecutionError::InvalidContext);
        };
        let contract = inventory
            .checkpoint_contracts
            .get(&token)
            .ok_or(ExecutionError::InvalidContext)?;
        let checkpoints = contract
            .grant(&invocation, &request.input)
            .map_err(|_| ExecutionError::InvalidContext)?;
        Ok(AuthorizedChild {
            checkpoints: Arc::new(Checkpoints(checkpoints)),
            execution: None,
        })
    }
}
