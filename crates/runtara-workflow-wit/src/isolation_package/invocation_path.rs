//! Decode logical-agent-call:2 identities before granting runtime authority.
use super::InvocationManifest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopKind {
    Split,
    While,
}

/// Exact compiler loop tuple: kind, authored step id, unsigned iteration index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopFrame(pub LoopKind, pub String, pub u32);

/// Structured child ancestry. This is decoded address data, not a permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceFrame {
    Child {
        workflow_id: String,
        loops: Vec<LoopFrame>,
        step_id: String,
    },
    ToolChild {
        workflow_id: String,
        loops: Vec<LoopFrame>,
        ai_step_id: String,
        label: String,
        call_counter: u32,
    },
}

/// A decoded static call identity. The host must additionally authorize its
/// namespace/loop ancestry and live attempt before granting child runtime IO.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInvocationPath {
    pub workflow_id: String,
    pub namespace: Vec<NamespaceFrame>,
    pub loops: Vec<LoopFrame>,
    pub agent_id: String,
    pub capability: String,
    pub step_id: String,
    pub domain: u32,
    pub activation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationPathError {
    Malformed,
    InvalidAttempt,
    UnknownCall,
}
impl std::fmt::Display for InvocationPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed logical Agent invocation path",
            Self::InvalidAttempt => "invalid logical Agent invocation attempt",
            Self::UnknownCall => "Agent invocation is absent from compiler inventory",
        })
    }
}
impl std::error::Error for InvocationPathError {}

type Key = (String, String, Vec<Value>, Vec<LoopFrame>, [String; 3]);

impl AgentInvocationPath {
    /// The compiler emits one canonical JSON tuple followed by two fixed-width
    /// a–p encoded u32 fields. Reject alternative encodings so equivalent JSON
    /// cannot create distinct durable cancellation addresses. No new arbitrary
    /// byte/nesting cap is imposed here; enclosing admission owns byte budgets.
    pub fn decode(path: &str) -> Result<Self, InvocationPathError> {
        let invalid = InvocationPathError::Malformed;
        let (path, activation) = path.rsplit_once(':').ok_or(invalid)?;
        let (path, domain) = path.rsplit_once(':').ok_or(invalid)?;
        let activation = counter(activation)?;
        let domain = counter(domain)?;
        if domain > 5 || (!matches!(domain, 2 | 3) && activation != 0) {
            return Err(invalid);
        }
        let json = path.strip_prefix("runtara:v2:").ok_or(invalid)?;
        let key: Key = serde_json::from_str(json).map_err(|_| invalid)?;
        if key.0 != "agent" || serde_json::to_string(&key).map_err(|_| invalid)? != json {
            return Err(invalid);
        }
        let namespace = key.2.into_iter().map(namespace).collect::<Result<_, _>>()?;
        let [agent_id, capability, step_id] = key.4;
        Ok(Self {
            workflow_id: key.1,
            namespace,
            loops: key.3,
            agent_id,
            capability,
            step_id,
            domain,
            activation,
        })
    }
}

fn counter(value: &str) -> Result<u32, InvocationPathError> {
    if value.len() != 8 {
        return Err(InvocationPathError::Malformed);
    }
    value.bytes().try_fold(0, |acc, byte| {
        if (b'a'..=b'p').contains(&byte) {
            Ok((acc << 4) | u32::from(byte - b'a'))
        } else {
            Err(InvocationPathError::Malformed)
        }
    })
}

fn namespace(value: Value) -> Result<NamespaceFrame, InvocationPathError> {
    let invalid = InvocationPathError::Malformed;
    let (kind, workflow_id, loops, parts): (String, String, Vec<LoopFrame>, Value) =
        serde_json::from_value(value).map_err(|_| invalid)?;
    match kind.as_str() {
        "child" => {
            let [step_id]: [String; 1] = serde_json::from_value(parts).map_err(|_| invalid)?;
            Ok(NamespaceFrame::Child {
                workflow_id,
                loops,
                step_id,
            })
        }
        "tool-child" => {
            let (ai_step_id, label, call_counter): (String, String, u32) =
                serde_json::from_value(parts).map_err(|_| invalid)?;
            Ok(NamespaceFrame::ToolChild {
                workflow_id,
                loops,
                ai_step_id,
                label,
                call_counter,
            })
        }
        _ => Err(invalid),
    }
}

impl InvocationManifest {
    /// Resolve only compiler-emitted binding/entry/workflow/step/domain tuples.
    /// Requires a previously validated package inventory. Successful resolution
    /// is not namespace permission or durable attempt fencing: the scope factory
    /// must check the returned ancestry before authorizing checkpoint access.
    pub fn resolve_agent_invocation(
        &self,
        binding: &str,
        capability: &str,
        path: &str,
        attempt: u64,
    ) -> Result<AgentInvocationPath, InvocationPathError> {
        let path = AgentInvocationPath::decode(path)?;
        if attempt == 0 || (path.domain != 0 && attempt != 1) {
            return Err(InvocationPathError::InvalidAttempt);
        }
        if self.version != 1
            || self.workflow_id != path.workflow_id
            || capability != path.capability
        {
            return Err(InvocationPathError::UnknownCall);
        }
        let index = self
            .agent_calls
            .binary_search_by(|site| {
                (
                    site.binding.as_str(),
                    site.agent_id.as_str(),
                    site.capability.as_str(),
                    site.step_id.as_str(),
                )
                    .cmp(&(
                        binding,
                        path.agent_id.as_str(),
                        path.capability.as_str(),
                        path.step_id.as_str(),
                    ))
            })
            .map_err(|_| InvocationPathError::UnknownCall)?;
        if !self.agent_calls[index].domains.contains(&path.domain) {
            return Err(InvocationPathError::UnknownCall);
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests;
