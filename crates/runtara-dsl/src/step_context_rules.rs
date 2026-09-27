// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where operation-scoped Agent steps may appear: the v1 context matrix.
//!
//! An operation-scoped call site (see [`crate::agent_meta::is_operation_scoped`])
//! is either a *suspending* capability, which parks its workflow with a typed
//! suspension, or a capability of the built-in *control* agent, whose mutations
//! the host makes replay-safe under the call site's operation identity. Both
//! kinds need a stable operation identity; a suspending one additionally needs
//! durable replay and a step deadline.
//!
//! [`STEP_CONTEXT_RULES`] is the single source for these rules. The workflow
//! validator derives its codes from it (E028, E029, E131, E132, W074-W078),
//! and the authoring schema renders it through [`step_context_rules_json`].

use crate::agent_meta::AgentCatalog;

/// The two kinds of operation-scoped call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationScopedKind {
    /// A capability declaring `suspends` (including control's `wait`).
    Suspending,
    /// A non-suspending capability of the control agent.
    Control,
}

impl OperationScopedKind {
    /// Classify a call to `capability_id` of `agent_id`; `None` when the call
    /// is not operation-scoped or the capability is unknown. Suspension wins,
    /// so control's `wait` follows the stricter suspending rules.
    pub fn classify(catalog: &AgentCatalog, agent_id: &str, capability_id: &str) -> Option<Self> {
        if catalog.capability_suspends(agent_id, capability_id) {
            Some(Self::Suspending)
        } else if catalog.is_operation_scoped(agent_id, capability_id) {
            Some(Self::Control)
        } else {
            None
        }
    }

    /// Stable key used in diagnostics and the authoring schema.
    pub fn key(self) -> &'static str {
        match self {
            Self::Suspending => "suspending",
            Self::Control => "control",
        }
    }
}

/// A context an operation-scoped step can be found in, or a property of the
/// step that the rules judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepContext {
    /// The workflow's own top-level graph.
    TopLevel,
    /// One arm of a Conditional, a Switch, or conditioned executionPlan edges.
    BranchArm,
    /// The body of a While or of a Split that runs sequentially.
    SequentialLoop,
    /// Inside a workflow embedded by EmbedWorkflow.
    EmbeddedWorkflow,
    /// The step's own `maxRetries`.
    StepRetry,
    /// Inside a region an enclosing Split or EmbedWorkflow retries as a whole.
    EnclosingRetry,
    /// The body of a Split whose `parallelism` asks for concurrency.
    ParallelSplit,
    /// A branch of an unconditioned (parallel) executionPlan fan-out.
    ParallelBranchGroup,
    /// The step's effective durability is off.
    NonDurable,
    /// The step has no `timeout`, or `timeout` is 0.
    MissingTimeout,
    /// The step's `timeout` leaves no room to park before its deadline.
    TimeoutBelowDeadlineMargin,
    /// Reached only through an `onError` edge.
    OnErrorRegion,
    /// Inside a WaitForSignal `onWait` subgraph.
    OnWait,
    /// The target of an AiAgent tool edge (including an EmbedWorkflow tool).
    AiAgentTool,
    /// The target of an AiAgent `memory` edge.
    AiAgentMemory,
    /// A control `start` inside a loop with a constant `runLabel`.
    ConstantRunLabelInLoop,
    /// A control `start` whose `workflowId` is not a literal.
    DynamicStartTarget,
    /// A workflow published as a workflow-agent.
    PublishedWorkflowAgent,
}

/// What the rules decide for one kind in one context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextVerdict {
    /// Supported with no diagnostic.
    Allowed,
    /// Supported, but the site runs serialized; the validator warns.
    Serialized {
        /// Warning code.
        code: &'static str,
    },
    /// Supported, with an advisory warning.
    Warned {
        /// Warning code.
        code: &'static str,
    },
    /// Rejected by validation (save, compile and browser).
    Rejected {
        /// Error code.
        code: &'static str,
    },
    /// Refused when publishing the workflow as a workflow-agent.
    PublishRefused {
        /// Stable feature key of the publish refusal.
        feature: &'static str,
    },
    /// The rule does not concern this kind.
    NotApplicable,
}

impl ContextVerdict {
    /// The validator code this verdict emits, if any.
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Serialized { code } | Self::Warned { code } | Self::Rejected { code } => {
                Some(code)
            }
            Self::Allowed | Self::PublishRefused { .. } | Self::NotApplicable => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Serialized { .. } => "serialized",
            Self::Warned { .. } => "warning",
            Self::Rejected { .. } => "rejected",
            Self::PublishRefused { .. } => "publish-refused",
            Self::NotApplicable => "not-applicable",
        }
    }
}

/// One row of the v1 matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepContextRule {
    /// The context this row judges.
    pub context: StepContext,
    /// Stable key used in diagnostics and the authoring schema.
    pub key: &'static str,
    /// What the context is and why the verdict holds.
    pub description: &'static str,
    /// Verdict for a suspending capability.
    pub suspending: ContextVerdict,
    /// Verdict for a non-suspending control capability.
    pub control: ContextVerdict,
}

impl StepContextRule {
    /// The verdict for `kind`.
    pub fn verdict(&self, kind: OperationScopedKind) -> ContextVerdict {
        match kind {
            OperationScopedKind::Suspending => self.suspending,
            OperationScopedKind::Control => self.control,
        }
    }
}

use ContextVerdict::{Allowed, NotApplicable, PublishRefused, Rejected, Serialized, Warned};

/// The v1 matrix: every context an operation-scoped step is judged in.
pub const STEP_CONTEXT_RULES: &[StepContextRule] = &[
    StepContextRule {
        context: StepContext::TopLevel,
        key: "top-level",
        description: "A step in the workflow's top-level graph.",
        suspending: Allowed,
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::BranchArm,
        key: "branch-arm",
        description: "One arm of a Conditional, a Switch or conditioned executionPlan edges; only one arm runs.",
        suspending: Allowed,
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::SequentialLoop,
        key: "sequential-loop",
        description: "The body of a While, or of a Split without a parallelism request; each iteration gets its own operation identity.",
        suspending: Allowed,
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::EmbeddedWorkflow,
        key: "embedded-workflow",
        description: "A step of a workflow embedded by EmbedWorkflow; the embed call site's context applies too.",
        suspending: Allowed,
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::StepRetry,
        key: "step-retry",
        description: "The step's own maxRetries; every attempt keeps the step's operation identity.",
        suspending: Allowed,
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::EnclosingRetry,
        key: "enclosing-retry",
        description: "A Split or EmbedWorkflow retrying the region around the step; a retry replays the operation's first outcome instead of repeating it.",
        suspending: Warned { code: "W076" },
        control: Warned { code: "W076" },
    },
    StepContextRule {
        context: StepContext::ParallelSplit,
        key: "parallel-split",
        description: "The body of a Split with parallelism other than 1; iterations holding the step run one at a time.",
        suspending: Serialized { code: "W075" },
        control: Serialized { code: "W075" },
    },
    StepContextRule {
        context: StepContext::ParallelBranchGroup,
        key: "parallel-branch-group",
        description: "A branch of an unconditioned executionPlan fan-out; the whole group runs sequentially.",
        suspending: Serialized { code: "W075" },
        control: Serialized { code: "W075" },
    },
    StepContextRule {
        context: StepContext::NonDurable,
        key: "non-durable",
        description: "The step (or the EmbedWorkflow call site around it) is not durable; a parked run would replay side effects.",
        suspending: Rejected { code: "E028" },
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::MissingTimeout,
        key: "missing-timeout",
        description: "The step has no timeout, or timeout 0; a suspending step needs a hard deadline.",
        suspending: Rejected { code: "E029" },
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::TimeoutBelowDeadlineMargin,
        key: "timeout-below-deadline-margin",
        description: "The step timeout is at most the deadline margin, so the step times out instead of parking.",
        suspending: Warned { code: "W078" },
        control: NotApplicable,
    },
    StepContextRule {
        context: StepContext::OnErrorRegion,
        key: "on-error-region",
        description: "A step reached only through an onError edge.",
        suspending: Rejected { code: "E131" },
        control: Allowed,
    },
    StepContextRule {
        context: StepContext::OnWait,
        key: "on-wait",
        description: "A step inside a WaitForSignal onWait subgraph.",
        suspending: Rejected { code: "E131" },
        control: Rejected { code: "E132" },
    },
    StepContextRule {
        context: StepContext::AiAgentTool,
        key: "ai-agent-tool",
        description: "The target of an AiAgent tool edge, or an EmbedWorkflow tool whose workflow holds such a step.",
        suspending: Rejected { code: "E131" },
        control: Rejected { code: "E132" },
    },
    StepContextRule {
        context: StepContext::AiAgentMemory,
        key: "ai-agent-memory",
        description: "The target of an AiAgent memory edge.",
        suspending: Rejected { code: "E131" },
        control: Rejected { code: "E132" },
    },
    StepContextRule {
        context: StepContext::ConstantRunLabelInLoop,
        key: "constant-run-label-in-loop",
        description: "A control start inside a Split or While with a literal runLabel; run labels are unique per parent, so the second iteration fails with label-conflict.",
        suspending: NotApplicable,
        control: Warned { code: "W074" },
    },
    StepContextRule {
        context: StepContext::DynamicStartTarget,
        key: "dynamic-start-target",
        description: "A control start whose workflowId is not a literal, so the target is only checked when the step runs.",
        suspending: NotApplicable,
        control: Warned { code: "W077" },
    },
    StepContextRule {
        context: StepContext::PublishedWorkflowAgent,
        key: "published-workflow-agent",
        description: "The workflow, or a workflow it embeds, is published as a workflow-agent.",
        suspending: PublishRefused {
            feature: "suspending-capability",
        },
        control: PublishRefused {
            feature: "control-agent",
        },
    },
];

/// The rule for `context`.
pub fn step_context_rule(context: StepContext) -> &'static StepContextRule {
    STEP_CONTEXT_RULES
        .iter()
        .find(|rule| rule.context == context)
        .expect("every StepContext has a rule")
}

/// A suspension that would park within this many milliseconds of its step
/// deadline fails with the step's timeout instead, so a step timeout at or
/// below it can never park (W078).
pub const SUSPEND_DEADLINE_MARGIN_MS: u64 = 1_000;

/// Capability id of control's `start`.
pub const CONTROL_START_CAPABILITY_ID: &str = "start";
/// Input field of `start` naming the workflow to start.
pub const CONTROL_START_WORKFLOW_ID_FIELD: &str = "workflowId";
/// Input field of `start` carrying the child's run label.
pub const CONTROL_START_RUN_LABEL_FIELD: &str = "runLabel";

/// The matrix as authoring-schema JSON: one entry per rule with the verdict
/// and code for each kind.
pub fn step_context_rules_json() -> serde_json::Value {
    let verdict = |verdict: ContextVerdict| {
        let mut value = serde_json::json!({ "verdict": verdict.key() });
        if let Some(code) = verdict.code() {
            value["code"] = code.into();
        }
        if let PublishRefused { feature } = verdict {
            value["feature"] = feature.into();
        }
        value
    };
    serde_json::json!({
        "appliesTo": "Agent steps whose capability declares suspends, and every capability of the control agent",
        "suspendingRequirements": format!(
            "A suspending step must be durable (E028) and set timeout > 0 (E029); a timeout of at most {SUSPEND_DEADLINE_MARGIN_MS} ms times out instead of parking (W078)."
        ),
        "rules": STEP_CONTEXT_RULES
            .iter()
            .map(|rule| serde_json::json!({
                "context": rule.key,
                "description": rule.description,
                "suspending": verdict(rule.suspending),
                "control": verdict(rule.control),
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_context_has_exactly_one_rule_with_a_unique_key() {
        let contexts: HashSet<_> = STEP_CONTEXT_RULES.iter().map(|rule| rule.context).collect();
        let keys: HashSet<_> = STEP_CONTEXT_RULES.iter().map(|rule| rule.key).collect();
        assert_eq!(contexts.len(), STEP_CONTEXT_RULES.len());
        assert_eq!(keys.len(), STEP_CONTEXT_RULES.len());
        for rule in STEP_CONTEXT_RULES {
            assert_eq!(step_context_rule(rule.context), rule);
        }
    }

    #[test]
    fn the_v1_matrix_is_frozen() {
        use OperationScopedKind::{Control, Suspending};
        let code = |context, kind| step_context_rule(context).verdict(kind).code();
        for context in [
            StepContext::TopLevel,
            StepContext::BranchArm,
            StepContext::SequentialLoop,
            StepContext::EmbeddedWorkflow,
            StepContext::StepRetry,
        ] {
            for kind in [Suspending, Control] {
                assert_eq!(step_context_rule(context).verdict(kind), Allowed);
            }
        }
        assert_eq!(code(StepContext::NonDurable, Suspending), Some("E028"));
        assert_eq!(code(StepContext::NonDurable, Control), None);
        assert_eq!(code(StepContext::MissingTimeout, Suspending), Some("E029"));
        assert_eq!(code(StepContext::OnErrorRegion, Suspending), Some("E131"));
        assert_eq!(code(StepContext::OnErrorRegion, Control), None);
        for context in [
            StepContext::OnWait,
            StepContext::AiAgentTool,
            StepContext::AiAgentMemory,
        ] {
            assert_eq!(code(context, Suspending), Some("E131"));
            assert_eq!(code(context, Control), Some("E132"));
        }
        for context in [StepContext::ParallelSplit, StepContext::ParallelBranchGroup] {
            assert!(matches!(
                step_context_rule(context).verdict(Suspending),
                Serialized { code: "W075" }
            ));
        }
        assert_eq!(code(StepContext::EnclosingRetry, Control), Some("W076"));
        assert_eq!(
            code(StepContext::ConstantRunLabelInLoop, Control),
            Some("W074")
        );
        assert_eq!(code(StepContext::DynamicStartTarget, Control), Some("W077"));
        assert_eq!(
            code(StepContext::TimeoutBelowDeadlineMargin, Suspending),
            Some("W078")
        );
    }

    #[test]
    fn the_authoring_json_renders_every_rule() {
        let json = step_context_rules_json();
        let rules = json["rules"].as_array().unwrap();
        assert_eq!(rules.len(), STEP_CONTEXT_RULES.len());
        for (rendered, rule) in rules.iter().zip(STEP_CONTEXT_RULES) {
            assert_eq!(rendered["context"], rule.key);
            assert_eq!(
                rendered["suspending"]["code"].as_str(),
                rule.suspending.code()
            );
            assert_eq!(rendered["control"]["code"].as_str(), rule.control.code());
        }
    }
}
