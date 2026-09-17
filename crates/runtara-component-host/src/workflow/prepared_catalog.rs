//! Immutable child code retained with the verified root preparation token.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use runtara_workflow_wit::isolation_package::{Binding, InvocationManifest};
use wasmtime::{
    Engine,
    component::{Component, InstancePre, Linker, types::ComponentItem},
};

use super::WorkflowState;

#[path = "invocation_abi.rs"]
mod invocation_abi;

type ChildPre = Arc<InstancePre<WorkflowState>>;

/// Package-local bindings to linked child code. No guest instances, mutable
/// workflow state, credentials or filesystem paths are kept in this catalog.
/// Clones of the enclosing prepared token share these immutable definitions.
#[derive(Default)]
pub struct PreparedChildCatalog {
    artifacts: BTreeMap<String, ChildPre>,
    bindings: BTreeMap<String, Binding>,
    invocations: Option<InvocationManifest>,
}

impl PreparedChildCatalog {
    pub(super) fn set_invocations(
        &mut self,
        invocations: Option<InvocationManifest>,
    ) -> Result<()> {
        if let Some(invocations) = &invocations {
            invocations.validate(&self.bindings)?;
        }
        self.invocations = invocations;
        Ok(())
    }
    /// Compiler invocation authority from the same trusted package as this code.
    pub fn invocations(&self) -> Option<&InvocationManifest> {
        self.invocations.as_ref()
    }
    pub fn artifact_count(&self) -> usize {
        self.artifacts.len()
    }

    pub(super) fn validate_engine(&self, engine: &Engine) -> Result<()> {
        ensure!(
            self.artifacts
                .values()
                .all(|pre| Engine::same(pre.component().engine(), engine)),
            "isolated catalog belongs to a different execution engine"
        );
        Ok(())
    }

    pub fn binding_count(&self) -> usize {
        self.bindings.len()
    }

    /// Resolve only names authorized by this exact package. The returned
    /// interface comes from the verified catalog, not from invocation input.
    pub fn resolve(&self, id: &str) -> Option<(&Binding, &ChildPre)> {
        let binding = self.bindings.get(id)?;
        Some((binding, self.artifacts.get(&binding.artifact)?))
    }

    pub(super) fn prepare(
        linker: &Linker<WorkflowState>,
        artifacts: BTreeMap<String, Component>,
        bindings: Vec<Binding>,
    ) -> Result<Self> {
        let mut catalog = Self::default();
        // No instantiation or compilation: this only checks imports and retains
        // the worker's prepared definitions. Guest initializers cannot run here.
        for (digest, component) in artifacts {
            let pre = linker
                .instantiate_pre(&component)
                .map_err(|error| anyhow::anyhow!("link isolated child {digest}: {error:#}"))?;
            catalog.artifacts.insert(digest, Arc::new(pre));
        }
        let mut used = std::collections::BTreeSet::new();
        for binding in bindings {
            ensure!(!binding.id.is_empty(), "empty isolated binding id");
            let pre = catalog
                .artifacts
                .get(&binding.artifact)
                .ok_or_else(|| anyhow::anyhow!("isolated binding references missing child"))?;
            let (item, interface) = pre
                .component()
                .get_export(None, &binding.interface)
                .ok_or_else(|| {
                    anyhow::anyhow!("isolated child has no interface {}", binding.interface)
                })?;
            ensure!(
                matches!(item, ComponentItem::ComponentInstance(_)),
                "isolated binding export is not an interface"
            );
            let Some((ComponentItem::ComponentFunc(invoke), _)) =
                pre.component().get_export(Some(&interface), "invoke")
            else {
                anyhow::bail!("isolated binding interface has no invoke function");
            };
            invocation_abi::validate(&invoke, &binding.interface).with_context(|| {
                format!(
                    "invalid isolated binding `{}` ({})",
                    binding.id, binding.interface
                )
            })?;
            used.insert(binding.artifact.clone());
            ensure!(
                catalog
                    .bindings
                    .insert(binding.id.clone(), binding)
                    .is_none(),
                "duplicate isolated binding id"
            );
        }
        ensure!(
            used.len() == catalog.artifacts.len(),
            "unreferenced isolated child"
        );
        Ok(catalog)
    }
}
