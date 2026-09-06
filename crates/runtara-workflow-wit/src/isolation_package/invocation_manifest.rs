//! Compiler-owned call-site inventory; contains no inputs, credentials or graph
//! scheduling rules. Package v2 and the native worker envelope bind it to code.
use super::{Binding, PackageError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One Agent identity emitted by a scoped logical Agent bridge. Namespace and
/// loop ancestry remain in the invocation path, separate from these static parts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCallSite {
    pub binding: String,
    pub agent_id: String,
    pub capability: String,
    pub step_id: String,
    /// Allowed bridge domains: step, memory-load, AI turn/tool, summarize/save.
    pub domains: Vec<u32>,
}

/// Immutable inventory produced from the same normalized compiler manifest as
/// the workflow code. It supplies static invocation authority, not execution
/// order, live attempt fencing or checkpoint namespace policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationManifest {
    pub version: u32,
    pub workflow_id: String,
    pub agent_calls: Vec<AgentCallSite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_sites: Vec<InvocationCallSite>,
}

/// A compiler definition and caller, independent of authored step IDs. `token`
/// addresses this row in a v3 invocation; `identity` addresses `agent_calls`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationCallSite {
    pub token: u32,
    pub identity: u32,
    pub agent_reference: u32,
    pub caller_reference: u32,
    pub domain: u32,
}

impl InvocationManifest {
    /// Validate references and deterministic, unambiguous encoding before
    /// admission. Byte/allocation bounds come from the enclosing package limit.
    pub fn validate(&self, bindings: &BTreeMap<String, Binding>) -> Result<(), PackageError> {
        if !matches!(self.version, 1 | 2) {
            return Err(PackageError::UnsupportedVersion);
        }
        let mut used = BTreeSet::new();
        let mut previous = None;
        for site in &self.agent_calls {
            let identity = (
                &site.binding,
                &site.agent_id,
                &site.capability,
                &site.step_id,
            );
            if !bindings.contains_key(&site.binding)
                || site.binding != format!("agent:{}", site.agent_id)
                || site.domains.is_empty()
                || site.domains.iter().any(|&domain| domain > 5)
                || site.domains.windows(2).any(|pair| pair[0] >= pair[1])
                || previous.is_some_and(|previous| previous >= identity)
            {
                return Err(PackageError::InvalidManifest);
            }
            previous = Some(identity);
            used.insert(&site.binding);
        }
        if used.len() != bindings.len() {
            return Err(PackageError::MissingArtifact);
        }
        if self.version == 1 {
            if !self.call_sites.is_empty() {
                return Err(PackageError::InvalidManifest);
            }
        } else {
            let mut tokens = None;
            let mut origins = BTreeSet::new();
            let mut covered = BTreeSet::new();
            let mut definitions = BTreeMap::new();
            for site in &self.call_sites {
                let identity = self
                    .agent_calls
                    .get(site.identity as usize)
                    .ok_or(PackageError::InvalidManifest)?;
                if tokens.is_some_and(|previous| previous >= site.token)
                    || !identity.domains.contains(&site.domain)
                    || !origins.insert((site.agent_reference, site.caller_reference, site.domain))
                    || (site.domain != 3 && site.agent_reference != site.caller_reference)
                    || definitions
                        .insert(site.agent_reference, site.identity)
                        .is_some_and(|identity| identity != site.identity)
                {
                    return Err(PackageError::InvalidManifest);
                }
                tokens = Some(site.token);
                covered.insert((site.identity as usize, site.domain));
            }
            for (index, identity) in self.agent_calls.iter().enumerate() {
                if identity
                    .domains
                    .iter()
                    .any(|domain| !covered.contains(&(index, *domain)))
                {
                    return Err(PackageError::MissingArtifact);
                }
            }
        }
        Ok(())
    }
}
