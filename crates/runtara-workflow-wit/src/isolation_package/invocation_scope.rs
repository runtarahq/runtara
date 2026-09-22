//! Static namespace membership. Dynamic iteration values and inherited authority
//! come from an execution; this metadata never decides what runs next.
use super::{
    AgentInvocationPath, InvocationManifest, InvocationPathError, InvocationSelector, LoopFrame,
    LoopKind, NamespaceFrame,
};
use serde::{Deserialize, Serialize};

/// Authored loop identity; the execution supplies its unsigned iteration index.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LoopPattern(pub LoopKind, pub String);

/// An inline child boundary captures its parent's loops and resets local loops.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildScopePattern {
    pub step_id: String,
    pub loops: Vec<LoopPattern>,
}

/// One structurally permitted path relative to a host-approved inherited scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationScopePattern {
    pub namespace: Vec<ChildScopePattern>,
    pub loops: Vec<LoopPattern>,
}

fn loops_match(pattern: &[LoopPattern], actual: &[LoopFrame]) -> bool {
    pattern.len() == actual.len()
        && pattern.iter().zip(actual).all(
            |(LoopPattern(kind, id), LoopFrame(actual_kind, actual_id, _))| {
                kind == actual_kind && id == actual_id
            },
        )
}

impl InvocationScopePattern {
    fn matches(&self, path: &AgentInvocationPath, inherited: &[NamespaceFrame]) -> bool {
        let Some(relative) = path.namespace.strip_prefix(inherited) else {
            return false;
        };
        self.namespace.len() == relative.len()
            && self.namespace.iter().zip(relative).all(|(pattern, frame)| {
                matches!(frame, NamespaceFrame::Child { workflow_id, loops, step_id }
                    if workflow_id == &path.workflow_id && step_id == &pattern.step_id && loops_match(&pattern.loops, loops))
            })
            && loops_match(&self.loops, &path.loops)
    }
}

impl InvocationManifest {
    /// Validate compiler identity plus structural namespace membership before
    /// scope allocation. `inherited` is fixed by the host parent, never copied
    /// from request input. This does not prove a live iteration or grant IO.
    pub fn resolve_scoped_agent_invocation(
        &self,
        binding: &str,
        capability: &str,
        path: &str,
        attempt: u64,
        inherited: &[NamespaceFrame],
    ) -> Result<AgentInvocationPath, InvocationPathError> {
        if !matches!(self.version, 3..=super::INVOCATION_MANIFEST_VERSION) {
            return Err(InvocationPathError::UnknownCall);
        }
        let path = self.resolve_agent_invocation(binding, capability, path, attempt)?;
        let InvocationSelector::CallSite(token) = path.selector else {
            return Err(InvocationPathError::UnknownCall);
        };
        if !self.scope_paths.get(&token).is_some_and(|patterns| {
            patterns
                .iter()
                .any(|pattern| pattern.matches(&path, inherited))
        }) {
            return Err(InvocationPathError::UnknownCall);
        }
        Ok(path)
    }
}
