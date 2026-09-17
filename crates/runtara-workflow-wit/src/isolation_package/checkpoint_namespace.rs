//! Owned structured checkpoint subtrees. Grants never prepend or rewrite keys.
use super::{AgentInvocationPath, LoopFrame, NamespaceFrame};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum CheckpointContract {
    None,
    Child,
    Tool {
        ai_step_id: String,
        labels: Vec<String>,
    },
}

/// One host-approved, non-root namespace. Descendants share this subtree, while
/// the parent, other iterations, tool activations and sibling calls are excluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointNamespace {
    frames: Vec<NamespaceFrame>,
}

fn frame_value(frame: &NamespaceFrame) -> Value {
    match frame {
        NamespaceFrame::Child {
            workflow_id,
            loops,
            step_id,
        } => json!(["child", workflow_id, loops, [step_id]]),
        NamespaceFrame::ToolChild {
            workflow_id,
            loops,
            ai_step_id,
            label,
            call_counter,
        } => json!([
            "tool-child",
            workflow_id,
            loops,
            [ai_step_id, label, call_counter]
        ]),
    }
}

impl CheckpointNamespace {
    /// The caller must first authorize the invocation and its compiler contract.
    pub fn child(invocation: &AgentInvocationPath) -> Self {
        let mut frames = invocation.namespace.clone();
        frames.push(NamespaceFrame::Child {
            workflow_id: invocation.workflow_id.clone(),
            loops: invocation.loops.clone(),
            step_id: invocation.step_id.clone(),
        });
        Self { frames }
    }
    /// `ai_step_id` and `label` must come from the verified compiler contract.
    pub fn tool(invocation: &AgentInvocationPath, ai_step_id: &str, label: &str) -> Self {
        let mut frames = invocation.namespace.clone();
        frames.push(NamespaceFrame::ToolChild {
            workflow_id: invocation.workflow_id.clone(),
            loops: invocation.loops.clone(),
            ai_step_id: ai_step_id.into(),
            label: label.into(),
            call_counter: invocation.activation,
        });
        Self { frames }
    }
    pub fn encoded_prefix(&self) -> String {
        format!(
            "runtara:scope:v2:{}",
            Value::Array(self.frames.iter().map(frame_value).collect())
        )
    }
    /// Validate canonical base keys and the existing retry/attempt suffixes.
    /// Operation-specific data stays opaque: the guest owns workflow semantics.
    pub fn authorize(&self, key: &str) -> Result<(), &'static str> {
        const INVALID: &str = "checkpoint outside authorized namespace";
        let encoded = key.strip_prefix("runtara:v2:").ok_or(INVALID)?;
        type Key = (String, String, Vec<Value>, Vec<LoopFrame>, Value);
        let mut decoder = serde_json::Deserializer::from_str(encoded).into_iter::<Key>();
        let parsed = decoder.next().ok_or(INVALID)?.map_err(|_| INVALID)?;
        let end = decoder.byte_offset();
        if serde_json::to_string(&parsed).map_err(|_| INVALID)? != encoded[..end] {
            return Err(INVALID);
        }
        let suffix = &encoded[end..];
        if !suffix.is_empty() {
            let rest = ["::attempt::", "::retry_sleep::", "::retry::"]
                .iter()
                .find_map(|prefix| suffix.strip_prefix(prefix))
                .ok_or(INVALID)?;
            let counter: u32 = rest.parse().map_err(|_| INVALID)?;
            if counter.to_string() != rest {
                return Err(INVALID);
            }
        }
        let frames = parsed
            .2
            .into_iter()
            .map(super::invocation_path::namespace)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| INVALID)?;
        if !frames.starts_with(&self.frames) {
            return Err(INVALID);
        }
        Ok(())
    }
}

impl CheckpointContract {
    /// Return the grant only when the compiler-produced workflow envelope agrees
    /// with the derived namespace. Native payloads remain opaque and get no IO.
    pub fn grant(
        &self,
        invocation: &AgentInvocationPath,
        input: &[u8],
    ) -> Result<Option<CheckpointNamespace>, &'static str> {
        if matches!(self, Self::None) {
            return Ok(None);
        }
        #[derive(Deserialize)]
        struct Envelope {
            variables: Variables,
        }
        #[derive(Deserialize)]
        struct Variables {
            _cache_key_prefix: String,
        }
        let envelope: Envelope =
            serde_json::from_slice(input).map_err(|_| "invalid workflow-agent scope envelope")?;
        let matches = |grant: &CheckpointNamespace| {
            grant.encoded_prefix() == envelope.variables._cache_key_prefix
        };
        match self {
            Self::Child => {
                let grant = CheckpointNamespace::child(invocation);
                if matches(&grant) {
                    Ok(Some(grant))
                } else {
                    Err("workflow-agent scope envelope mismatch")
                }
            }
            Self::Tool { ai_step_id, labels } => labels
                .iter()
                .map(|label| CheckpointNamespace::tool(invocation, ai_step_id, label))
                .find(matches)
                .map(Some)
                .ok_or("workflow-agent tool scope envelope mismatch"),
            Self::None => unreachable!(),
        }
    }
}

impl super::InvocationManifest {
    /// Tokens whose existing durable subtrees can overlap. The rollout selector
    /// must keep affected packages on the legacy path; runtime admission also
    /// rejects them. Requires a validated inventory. Dynamic indices are
    /// conservative wildcards at this stage.
    pub fn checkpoint_conflicts(&self) -> std::collections::BTreeSet<u32> {
        use super::LoopPattern;
        use std::collections::{BTreeMap, BTreeSet};
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
        enum Frame {
            Child(String, Vec<LoopPattern>),
            Tool(String, String, Vec<LoopPattern>),
        }
        let mut roots = BTreeMap::<Vec<Frame>, u32>::new();
        let mut conflicts = BTreeSet::new();
        for site in &self.call_sites {
            let Some(contract) = self.checkpoint_contracts.get(&site.token) else {
                continue;
            };
            let Some(patterns) = self.scope_paths.get(&site.token) else {
                continue;
            };
            for pattern in patterns {
                let prefix: Vec<_> = pattern
                    .namespace
                    .iter()
                    .map(|frame| Frame::Child(frame.step_id.clone(), frame.loops.clone()))
                    .collect();
                let terminals = match contract {
                    CheckpointContract::None => vec![],
                    CheckpointContract::Child => vec![Frame::Child(
                        self.agent_calls[site.identity as usize].step_id.clone(),
                        pattern.loops.clone(),
                    )],
                    CheckpointContract::Tool { ai_step_id, labels } => labels
                        .iter()
                        .map(|label| {
                            Frame::Tool(ai_step_id.clone(), label.clone(), pattern.loops.clone())
                        })
                        .collect(),
                };
                for terminal in terminals {
                    let mut path = prefix.clone();
                    path.push(terminal);
                    if let Some(other) = roots.insert(path, site.token)
                        && other != site.token
                    {
                        conflicts.extend([other, site.token]);
                    }
                }
            }
        }
        for (path, token) in &roots {
            for end in 1..path.len() {
                if let Some(other) = roots.get(&path[..end]) {
                    conflicts.extend([*other, *token]);
                }
            }
        }
        conflicts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isolation_package::{InvocationSelector, LoopKind};
    fn invocation() -> AgentInvocationPath {
        AgentInvocationPath {
            workflow_id: "parent::雪".into(),
            namespace: vec![],
            loops: vec![LoopFrame(LoopKind::Split, "items".into(), 3)],
            agent_id: "child".into(),
            capability: "run".into(),
            step_id: "call".into(),
            selector: InvocationSelector::CallSite(7),
            activation: 9,
        }
    }
    fn key(grant: &CheckpointNamespace) -> String {
        format!(
            "runtara:v2:{}",
            json!([
                "wait",
                "child-workflow",
                grant.frames.iter().map(frame_value).collect::<Vec<_>>(),
                [["While", "loop", 1]],
                ["instance", "signal"]
            ])
        )
    }
    #[test]
    fn owned_namespace_covers_child_and_descendants_but_not_parent_siblings_or_other_iterations() {
        let invocation = invocation();
        let grant = CheckpointNamespace::child(&invocation);
        let good = key(&grant);
        for suffix in [
            "",
            "::retry::0",
            "::attempt::4294967295",
            "::retry_sleep::2",
        ] {
            assert!(grant.authorize(&format!("{good}{suffix}")).is_ok());
        }
        let mut descendant = grant.clone();
        descendant.frames.push(NamespaceFrame::Child {
            workflow_id: "child-workflow".into(),
            loops: vec![],
            step_id: "nested".into(),
        });
        assert!(grant.authorize(&key(&descendant)).is_ok());
        for mode in ["parent", "sibling", "iteration", "workflow", "tool"] {
            let mut other = invocation.clone();
            match mode {
                "sibling" => other.step_id = "other".into(),
                "iteration" => other.loops[0].2 += 1,
                "workflow" => other.workflow_id = "foreign".into(),
                _ => {}
            }
            let mut scope = CheckpointNamespace::child(&other);
            if mode == "parent" {
                scope.frames.clear();
            }
            if mode == "tool" {
                scope = CheckpointNamespace::tool(&other, "ai", "tool");
            }
            assert!(grant.authorize(&key(&scope)).is_err(), "accepted {mode}");
        }
        for suffix in [
            " ",
            "::attempt::-1",
            "::attempt::01",
            "::attempt::4294967296",
            "::retry_sleep::+1",
            "::other::1",
            "::attempt::1::attempt::2",
        ] {
            assert!(grant.authorize(&format!("{good}{suffix}")).is_err());
        }
        assert!(
            grant
                .authorize(&good.replace("[\"wait\",", "[\"wait\", "))
                .is_err()
        );
        assert!(
            grant
                .authorize(&good.replace("runtara:v2:", "runtara:v1:"))
                .is_err()
        );
        for end in 0..good.len() {
            if good.is_char_boundary(end) {
                assert!(grant.authorize(&good[..end]).is_err());
            }
        }
    }
    #[test]
    fn workflow_envelope_must_match_compiler_child_or_tool_contract_and_activation() {
        let invocation = invocation();
        assert!(
            CheckpointContract::None
                .grant(&invocation, b"opaque native input")
                .unwrap()
                .is_none()
        );
        let envelope = |grant: &CheckpointNamespace| {
            serde_json::to_vec(&json!({"data":{"irrelevant":"payload"},"variables":{"_cache_key_prefix":grant.encoded_prefix()}})).unwrap()
        };
        let child = CheckpointNamespace::child(&invocation);
        assert_eq!(
            CheckpointContract::Child
                .grant(&invocation, &envelope(&child))
                .unwrap(),
            Some(child.clone())
        );
        let tool = CheckpointContract::Tool {
            ai_step_id: "brain".into(),
            labels: vec!["approved".into(), "雪".into()],
        };
        for label in ["approved", "雪"] {
            let namespace = CheckpointNamespace::tool(&invocation, "brain", label);
            assert_eq!(
                tool.grant(&invocation, &envelope(&namespace)).unwrap(),
                Some(namespace)
            );
        }
        for bad in [
            envelope(&child),
            envelope(&CheckpointNamespace::tool(
                &invocation,
                "foreign",
                "approved",
            )),
            envelope(&CheckpointNamespace::tool(&invocation, "brain", "unlisted")),
            b"{}".to_vec(),
        ] {
            assert!(tool.grant(&invocation, &bad).is_err());
        }
        let mut later = invocation.clone();
        later.activation += 1;
        assert!(
            tool.grant(
                &invocation,
                &envelope(&CheckpointNamespace::tool(&later, "brain", "approved"))
            )
            .is_err()
        );
        assert!(CheckpointContract::Child.grant(&invocation, b"{}").is_err());
    }
}
