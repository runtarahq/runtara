// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct core run-plan model and construction used by the Wasm emitter.
//!
//! Turns the flat manifest into the `DirectRunPlan` — a structured, tree-shaped
//! execution model (one variant per lowerable concern) that captures what runs in
//! what order. A tree, not an arbitrary CFG, because core Wasm offers only
//! structured control flow (`block`/`loop`/`if`/`br`): every diamond must
//! re-converge at exactly one point. Branching steps (Conditional / routing Switch
//! / conditioned edges) lower to if/else carrying per-branch sub-plans plus a
//! single `merge_plan` — the shared continuation, emitted once rather than
//! duplicated into every branch (which would be exponential); each branch recurses
//! with the merge as its stop point and reaches it as a `Join` no-op.
//! Unconditional fan-out is topologically linearised (Kahn's algorithm) into one
//! sequential chain per *region* — the unconditional DAG rooted at the graph entry,
//! a branch target, a merge continuation, or an onError handler — so every step
//! runs exactly once, after all of its predecessors, even when branches cross-link
//! (a shape per-fan-out merge recursion cannot represent without duplicating or
//! dropping steps). `direct_find_merge_point` locates where exclusive branches
//! re-converge. `Split`/`While`/`WaitForSignal`/`EmbedWorkflow` get dedicated
//! lowerings, and `DirectFailureTarget`/`DirectHandledTarget` track how many
//! enclosing Wasm blocks a failure must `Br` out through. The traversal
//! deliberately mirrors the generated compiler so both accept the same graphs and
//! execute them identically (A/B parity).

use std::collections::HashMap;
use std::rc::Rc;

use super::error::DirectCompileError;
use super::manifest::{
    DirectAgentManifest, DirectChildWorkflowGraphManifest, DirectDelayManifest, DirectEdgeManifest,
    DirectGraphManifest, DirectSplitManifest, DirectStepManifest, DirectWorkflowManifest,
};

#[derive(Debug, Clone)]
pub(super) enum DirectRunPlan {
    Finish {
        step_id: String,
        mapping_id: u32,
        breakpoint: bool,
    },
    Filter {
        step_id: String,
        filter_id: u32,
        breakpoint: bool,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchValue {
        step_id: String,
        switch_id: u32,
        breakpoint: bool,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchRoute {
        step_id: String,
        switch_id: u32,
        breakpoint: bool,
        branches: Vec<DirectSwitchRoutePlan>,
        default_plan: Box<DirectRunPlan>,
        /// Shared continuation from the point where all routes (and default)
        /// re-converge, emitted once after the dispatch. `None` when terminal.
        merge_plan: Option<Box<DirectRunPlan>>,
    },
    EdgeRoute {
        branches: Vec<DirectEdgeConditionPlan>,
        default_plan: Box<DirectRunPlan>,
        /// Shared continuation when the conditioned normal-flow edges re-converge,
        /// emitted once after the dispatch. `None` when terminal.
        merge_plan: Option<Box<DirectRunPlan>>,
    },
    GroupBy {
        step_id: String,
        group_id: u32,
        breakpoint: bool,
        next_plan: Box<DirectRunPlan>,
    },
    Split {
        step_id: String,
        split_id: u32,
        durable: bool,
        breakpoint: bool,
        max_retries: u32,
        retry_delay_ms: u64,
        dont_stop_on_failed: bool,
        /// Requested concurrency window from the Split's `parallelism` config
        /// (None / Some(0|1) = sequential). Whether the window actually runs
        /// concurrently is decided at emission time by the eligibility rules
        /// in `split.rs` (docs/wasip3-parallelism.md Phase 3); ineligible
        /// bodies degrade to the sequential lowering.
        parallel_window: Option<u32>,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
        timeout_ms: Option<u64>,
    },
    While {
        step_id: String,
        while_id: u32,
        breakpoint: bool,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
        timeout_ms: Option<u64>,
    },
    EmbedWorkflow {
        step_id: String,
        input_mapping_id: u32,
        durable: bool,
        breakpoint: bool,
        max_retries: u32,
        retry_delay_ms: u64,
        child_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
    },
    Delay {
        step_id: String,
        delay_id: u32,
        durable: bool,
        breakpoint: bool,
        next_plan: Box<DirectRunPlan>,
    },
    WaitForSignal {
        step_id: String,
        breakpoint: bool,
        on_wait_plan: Option<Box<DirectRunPlan>>,
        next_plan: Box<DirectRunPlan>,
        /// Timeout-expiry routing (GAP-14): when the wait deadline passes, the
        /// WAIT_TIMEOUT error dispatches here instead of failing the workflow.
        error_plan: Option<DirectErrorRoutePlan>,
    },
    Log {
        step_id: String,
        log_id: u32,
        breakpoint: bool,
        next_plan: Box<DirectRunPlan>,
    },
    Agent {
        step_id: String,
        agent_id: u32,
        agent_component_id: String,
        input_mapping_id: u32,
        durable_checkpoint: bool,
        breakpoint: bool,
        max_retries: u32,
        retry_delay_ms: u64,
        rate_limit_budget_ms: u64,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
    },
    /// Single-shot AiAgent: lowered as an invoke of the `ai_tools`
    /// `chat-completion` capability, with the output transformed into the
    /// `{response, iterations, toolCalls}` envelope via `ai-agent-output`.
    AiAgent {
        step_id: String,
        agent_id: u32,
        agent_component_id: String,
        input_mapping_id: u32,
        durable_checkpoint: bool,
        breakpoint: bool,
        /// Opt-in LLM-call retries (DSL default 0 — retries re-bill the
        /// model call, unlike Agent steps' default of 3).
        max_retries: u32,
        retry_delay_ms: u64,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
    },
    /// AiAgent with a tool loop: drive the `ai_tools` `chat-turn` capability,
    /// dispatching the returned tool calls back through the tool agent until the
    /// turn reports `complete`.
    AiAgentLoop {
        step_id: String,
        agent_id: u32,
        agent_component_id: String,
        input_mapping_id: u32,
        /// Per-turn durable checkpoints: each completed turn (LLM response +
        /// dispatched tool results + tool-call counter) is snapshotted under
        /// `{step}.turn.{n}` so a crash never re-runs (and re-bills) finished
        /// turns. Honors AiAgentStep.durable / the workflow durable flag.
        durable_checkpoint: bool,
        breakpoint: bool,
        max_iterations: u32,
        /// Tools in the same order as the advertised `tools` (so the capability's
        /// resolved tool index selects the right entry). Dispatched by index.
        tools: Vec<DirectAiToolPlan>,
        /// Conversation memory: load history before the loop, save it after.
        memory: Option<DirectAiMemoryPlan>,
        next_plan: Box<DirectRunPlan>,
        /// Loop-level failure routing (GAP-05): chat-turn (provider) and
        /// memory load/save failures dispatch here. Individual TOOL failures
        /// never route — they feed back to the LLM as the tool result.
        error_plan: Option<DirectErrorRoutePlan>,
    },
    Error {
        step_id: String,
        error_id: u32,
        breakpoint: bool,
    },
    Conditional {
        step_id: String,
        condition_id: u32,
        breakpoint: bool,
        true_plan: Box<DirectRunPlan>,
        false_plan: Box<DirectRunPlan>,
        /// When the two branches re-converge (a diamond), the shared continuation
        /// from the merge point onward, emitted ONCE after the `if/else` so the
        /// merge is not duplicated in each branch (which would be exponential).
        /// `None` when the branches are terminal (no merge).
        merge_plan: Option<Box<DirectRunPlan>>,
    },
    /// An unconditional fan-out whose branches run CONCURRENTLY, then re-converge
    /// (docs/wasip3-parallel-branches-plan.md). Unlike the linearised default,
    /// every branch's agent invoke is launched into the same waitable-set and the
    /// window drains them together before assembling each branch's result into the
    /// `steps` context in order — sequential-identical by construction (assemble IS
    /// the per-branch lowering, with the invoke memoized), because independent DAG
    /// branches never reference one another. Phase 4a: each branch is a single
    /// Agent (`Agent { next_plan: Join, .. }`); the shared continuation runs once
    /// as `merge_plan`, mirroring `Conditional`/`SwitchRoute`.
    ParallelBranches {
        branches: Vec<DirectRunPlan>,
        merge_plan: Box<DirectRunPlan>,
    },
    /// A branch that has reached its enclosing branching step's merge point. The
    /// merge (and everything after it) is emitted once by the parent as the shared
    /// continuation, so this terminal emits nothing — control falls through to it.
    Join,
    /// A terminal step that has no successor and no explicit `Finish` (e.g. a
    /// single-Agent workflow with no Finish step). The generated compiler returns
    /// `Ok(Value::Null)` in this case, so the workflow output is `null`; this
    /// plan node sets the output to `null` before `runtime.complete` runs.
    ImplicitFinish,
}

/// A tool the AiAgent loop can dispatch, by the capability-resolved tool index
/// (the tool's position in this list). Either an Agent-capability invoke or a
/// composed child workflow run (EmbedWorkflow tool).
#[derive(Debug, Clone)]
pub(super) enum DirectAiToolPlan {
    /// Invoke the target Agent step's capability with the LLM-provided arguments.
    Agent {
        agent_id: u32,
        agent_component_id: String,
        /// The advertised tool name (the edge label; the synthetic
        /// `<toolset>_search`/`_invoke` name for MCP meta-tools). Names the
        /// per-CALL checkpoint scope `{ai_step}.tool.{label}.{call}` when the
        /// target is a workflow-agent.
        label: String,
        /// The tool Agent step's own `timeout` (ms), injected as `timeout_ms`
        /// into the LLM-provided arguments so the dispatched call is bounded
        /// independently of the AiAgent turnTimeout. `None` leaves the tool
        /// capability's own default in effect.
        timeout_ms: Option<u64>,
    },
    /// Run a composed child workflow with the LLM-provided arguments as its input
    /// data and feed the child's final output back as the tool result. Mirrors the
    /// generated `emit_embed_workflow_tool_arm` (child run → result, else error).
    Embed {
        /// The EmbedWorkflow step id that owns the preloaded child graph; used to
        /// build the child variables/scope and debug events.
        step_id: String,
        /// The composed child workflow run plan (built from the preloaded graph).
        child_plan: Box<DirectRunPlan>,
    },
    /// Suspend the loop on a durable human-in-the-loop signal: emit an
    /// external-input request, durably poll until the signal arrives, and feed the
    /// payload back as the tool result. Mirrors the generated
    /// `emit_wait_for_signal_tool_arm`.
    Wait {
        /// The WaitForSignal step id that owns the signal config (event payload,
        /// poll interval). Reached via the tool edge, not the normal flow.
        step_id: String,
        /// The advertised tool name (edge label), folded into the per-call signal
        /// id `…/{ai_step}.tool.{label}.{call}`.
        label: String,
    },
}

/// Conversation-memory provider for an AiAgent loop: the memory agent's
/// load/save manifest agent ids, its component id, and the conversation-id
/// mapping used to build the load/save inputs.
#[derive(Debug, Clone)]
pub(super) struct DirectAiMemoryPlan {
    pub(super) load_agent_id: u32,
    pub(super) save_agent_id: u32,
    pub(super) agent_component_id: String,
    pub(super) conversation_mapping_id: u32,
    /// Compaction threshold: at most `max_messages` messages are kept before the
    /// conversation is saved (generated default 50; runs whenever memory is
    /// configured).
    pub(super) max_messages: u32,
    /// Summarize-strategy compaction provider. `None` → sliding window (drop the
    /// oldest); `Some` → invoke the `ai-tools` `summarize-memory` capability,
    /// which LLM-summarizes the oldest messages into a single message.
    pub(super) summarize: Option<DirectAiSummarizePlan>,
}

/// Summarize-strategy compaction provider for an AiAgent loop: the `ai-tools`
/// `summarize-memory` capability's manifest agent id and component id.
#[derive(Debug, Clone)]
pub(super) struct DirectAiSummarizePlan {
    pub(super) agent_id: u32,
    pub(super) agent_component_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct DirectSwitchRoutePlan {
    pub(super) label: String,
    pub(super) plan: Box<DirectRunPlan>,
}

#[derive(Debug, Clone)]
pub(super) struct DirectEdgeConditionPlan {
    pub(super) condition_id: u32,
    pub(super) plan: Box<DirectRunPlan>,
}

#[derive(Debug, Clone)]
pub(super) struct DirectErrorRoutePlan {
    pub(super) branches: Vec<DirectEdgeConditionPlan>,
    pub(super) default_plan: Option<Box<DirectRunPlan>>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum DirectFailureTarget {
    Split {
        split_id: u32,
        branch_depth: u32,
    },
    SplitRetry {
        branch_depth: u32,
    },
    WaitOnWait {
        step_id_offset: i32,
        step_id_len: i32,
    },
    EmbedWorkflow {
        branch_depth: u32,
    },
    StepError {
        branch_depth: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DirectHandledTarget {
    pub(super) branch_depth: u32,
}

impl DirectHandledTarget {
    pub(super) fn nested(self, extra_depth: u32) -> Self {
        Self {
            branch_depth: self.branch_depth + extra_depth,
        }
    }
}

impl DirectFailureTarget {
    pub(super) fn nested(self, extra_depth: u32) -> Self {
        match self {
            Self::Split {
                split_id,
                branch_depth,
            } => Self::Split {
                split_id,
                branch_depth: branch_depth + extra_depth,
            },
            Self::SplitRetry { branch_depth } => Self::SplitRetry {
                branch_depth: branch_depth + extra_depth,
            },
            Self::WaitOnWait {
                step_id_offset,
                step_id_len,
            } => Self::WaitOnWait {
                step_id_offset,
                step_id_len,
            },
            Self::EmbedWorkflow { branch_depth } => Self::EmbedWorkflow {
                branch_depth: branch_depth + extra_depth,
            },
            Self::StepError { branch_depth } => Self::StepError {
                branch_depth: branch_depth + extra_depth,
            },
        }
    }
}

pub(super) fn direct_run_plan(
    manifest: &DirectWorkflowManifest,
) -> Result<DirectRunPlan, DirectCompileError> {
    let entry = manifest
        .graph
        .steps
        .iter()
        .find(|step| step.id == manifest.graph.entry_point)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing direct entry step '{}'",
                manifest.graph.entry_point
            ))
        })?;

    match entry.step_type.as_str() {
        "Finish" | "Filter" | "Switch" | "GroupBy" | "Split" | "While" | "Delay"
        | "EmbedWorkflow" | "WaitForSignal" | "Log" | "Agent" | "AiAgent" | "Error"
        | "Conditional" => step_run_plan(
            &manifest.graph,
            &manifest.child_workflows,
            &manifest.graph.entry_point,
            &mut Vec::new(),
        ),
        other => Err(DirectCompileError::Component(format!(
            "direct run plan does not support entry step type '{other}'"
        ))),
    }
}

fn step_run_plan(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    step_id: &str,
    stack: &mut Vec<String>,
) -> Result<DirectRunPlan, DirectCompileError> {
    step_run_plan_inner(
        graph,
        child_workflows,
        step_id,
        stack,
        true,
        None,
        step_id,
        &mut DirectRegionOrderCache::new(),
    )
}

fn step_run_plan_without_on_error(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    step_id: &str,
    stack: &mut Vec<String>,
    orders: &mut DirectRegionOrderCache,
) -> Result<DirectRunPlan, DirectCompileError> {
    step_run_plan_inner(
        graph,
        child_workflows,
        step_id,
        stack,
        false,
        None,
        step_id,
        orders,
    )
}

fn step_breakpoint_enabled(graph: &DirectGraphManifest, step: &DirectStepManifest) -> bool {
    graph.durable
        && step
            .body
            .get("breakpoint")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn step_run_plan_inner(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    step_id: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
    // When set, the merge point of an enclosing branching step: reaching it ends
    // this branch with a `Join` (the merge is emitted once by the parent as the
    // shared continuation), so the branch does not recurse through it.
    stop_at: Option<&str>,
    // The root of the unconditional region `step_id` belongs to (the graph entry,
    // a branch target, a merge continuation, or an onError handler). Steps follow
    // the region's topological order, which linearizes fan-out and joins.
    region_root: &str,
    // Memoized region orders for `graph`, shared across the whole plan build.
    orders: &mut DirectRegionOrderCache,
) -> Result<DirectRunPlan, DirectCompileError> {
    if stop_at == Some(step_id) {
        return Ok(DirectRunPlan::Join);
    }
    if stack.iter().any(|visited| visited == step_id) {
        return Err(DirectCompileError::Component(format!(
            "direct run plan contains a cycle at step '{step_id}'"
        )));
    }

    let step = graph
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .ok_or_else(|| DirectCompileError::Component(format!("missing direct step '{step_id}'")))?;

    match step.step_type.as_str() {
        "Finish" => Ok(DirectRunPlan::Finish {
            step_id: step_id.to_string(),
            mapping_id: finish_mapping_id(graph, step_id)?,
            breakpoint: step_breakpoint_enabled(graph, step),
        }),
        "Filter" => {
            let filter_id = filter_id(graph, step_id)?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;

            Ok(DirectRunPlan::Filter {
                step_id: step_id.to_string(),
                filter_id,
                breakpoint: step_breakpoint_enabled(graph, step),
                next_plan: Box::new(next_plan),
            })
        }
        "Switch" => {
            let switch_id = switch_id(graph, step_id)?;
            if switch_is_routing(graph, step_id)? {
                let route_labels = switch_route_labels(graph, step_id)?;
                let mut branches = Vec::new();

                stack.push(step_id.to_string());
                // Detect a diamond: where all routes (and default) re-converge,
                // so the merge runs once as a shared continuation rather than
                // duplicated in every branch.
                let mut branch_starts: Vec<Option<String>> = route_labels
                    .iter()
                    .map(|label| {
                        branch_target(graph, step_id, label)
                            .map(|t| Some(t.to_string()))
                            .unwrap_or(None)
                    })
                    .collect();
                branch_starts.push(
                    branch_target(graph, step_id, "default")
                        .map(|t| Some(t.to_string()))
                        .unwrap_or(None),
                );
                let merge = direct_find_merge_point(graph, &branch_starts)
                    .filter(|m| Some(m.as_str()) != stop_at);
                let branch_stop = merge.as_deref().or(stop_at);

                for label in route_labels {
                    let target = branch_target(graph, step_id, &label)?.to_string();
                    let plan = step_run_plan_inner(
                        graph,
                        child_workflows,
                        &target,
                        stack,
                        include_on_error,
                        branch_stop,
                        &target,
                        orders,
                    )?;
                    branches.push(DirectSwitchRoutePlan {
                        label,
                        plan: Box::new(plan),
                    });
                }
                let default_target = branch_target(graph, step_id, "default")?.to_string();
                let default_plan = step_run_plan_inner(
                    graph,
                    child_workflows,
                    &default_target,
                    stack,
                    include_on_error,
                    branch_stop,
                    &default_target,
                    orders,
                )?;
                let merge_plan = match &merge {
                    Some(merge_step) => Some(Box::new(step_run_plan_inner(
                        graph,
                        child_workflows,
                        merge_step,
                        stack,
                        include_on_error,
                        stop_at,
                        merge_step,
                        orders,
                    )?)),
                    None => None,
                };
                stack.pop();

                Ok(DirectRunPlan::SwitchRoute {
                    step_id: step_id.to_string(),
                    switch_id,
                    breakpoint: step_breakpoint_enabled(graph, step),
                    branches,
                    default_plan: Box::new(default_plan),
                    merge_plan,
                })
            } else {
                let next_plan = normal_flow_plan(
                    graph,
                    child_workflows,
                    step_id,
                    stack,
                    include_on_error,
                    stop_at,
                    region_root,
                    orders,
                )?;

                Ok(DirectRunPlan::SwitchValue {
                    step_id: step_id.to_string(),
                    switch_id,
                    breakpoint: step_breakpoint_enabled(graph, step),
                    next_plan: Box::new(next_plan),
                })
            }
        }
        "GroupBy" => {
            let group_id = group_by_id(graph, step_id)?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;

            Ok(DirectRunPlan::GroupBy {
                step_id: step_id.to_string(),
                group_id,
                breakpoint: step_breakpoint_enabled(graph, step),
                next_plan: Box::new(next_plan),
            })
        }
        "Split" => {
            let split = split_manifest(graph, step_id)?;
            let dont_stop_on_failed = split_dont_stop_on_failed(graph, step_id)?;
            let nested_graph = split_subgraph(graph, step_id)?;
            let nested_plan = step_run_plan(
                nested_graph,
                child_workflows,
                &nested_graph.entry_point,
                &mut Vec::new(),
            )?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;
            let error_plan = if include_on_error {
                on_error_plan(graph, child_workflows, step_id, stack, orders)?
            } else {
                None
            };

            Ok(DirectRunPlan::Split {
                step_id: step_id.to_string(),
                split_id: split.id,
                durable: split.durable,
                breakpoint: step_breakpoint_enabled(graph, step),
                max_retries: split_effective_max_retries(split),
                retry_delay_ms: split_effective_retry_delay_ms(split),
                dont_stop_on_failed,
                parallel_window: split_parallel_window(graph, step_id)?,
                nested_plan: Box::new(nested_plan),
                next_plan: Box::new(next_plan),
                error_plan,
                timeout_ms: split_timeout_ms(graph, step_id)?,
            })
        }
        "While" => {
            let while_id = while_id(graph, step_id)?;
            let nested_graph = while_subgraph(graph, step_id)?;
            let nested_plan = step_run_plan(
                nested_graph,
                child_workflows,
                &nested_graph.entry_point,
                &mut Vec::new(),
            )?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;
            let error_plan = if include_on_error {
                on_error_plan(graph, child_workflows, step_id, stack, orders)?
            } else {
                None
            };

            Ok(DirectRunPlan::While {
                step_id: step_id.to_string(),
                while_id,
                breakpoint: step_breakpoint_enabled(graph, step),
                nested_plan: Box::new(nested_plan),
                next_plan: Box::new(next_plan),
                error_plan,
                timeout_ms: while_timeout_ms(graph, step_id),
            })
        }
        "EmbedWorkflow" => {
            let child = child_workflow_graph(child_workflows, step_id)?;
            let child_plan = step_run_plan(
                &child.graph,
                child_workflows,
                &child.graph.entry_point,
                &mut Vec::new(),
            )?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;
            let error_plan = if include_on_error {
                on_error_plan(graph, child_workflows, step_id, stack, orders)?
            } else {
                None
            };

            Ok(DirectRunPlan::EmbedWorkflow {
                step_id: step_id.to_string(),
                input_mapping_id: embed_workflow_input_mapping_id(graph, step_id)?,
                durable: graph.durable
                    && step
                        .body
                        .get("durable")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true),
                breakpoint: step_breakpoint_enabled(graph, step),
                max_retries: embed_workflow_effective_max_retries(step),
                retry_delay_ms: embed_workflow_effective_retry_delay_ms(step),
                child_plan: Box::new(child_plan),
                next_plan: Box::new(next_plan),
                error_plan,
            })
        }
        "Delay" => {
            let delay = delay_config(graph, step_id)?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;

            Ok(DirectRunPlan::Delay {
                step_id: step_id.to_string(),
                delay_id: delay.id,
                durable: delay.durable,
                breakpoint: step_breakpoint_enabled(graph, step),
                next_plan: Box::new(next_plan),
            })
        }
        "WaitForSignal" => {
            let on_wait_plan = wait_on_wait_subgraph(graph, step_id)?
                .map(|nested_graph| {
                    step_run_plan(
                        nested_graph,
                        child_workflows,
                        &nested_graph.entry_point,
                        &mut Vec::new(),
                    )
                })
                .transpose()?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;

            Ok(DirectRunPlan::WaitForSignal {
                step_id: step_id.to_string(),
                breakpoint: step_breakpoint_enabled(graph, step),
                on_wait_plan: on_wait_plan.map(Box::new),
                next_plan: Box::new(next_plan),
                error_plan: if include_on_error {
                    on_error_plan(graph, child_workflows, step_id, stack, orders)?
                } else {
                    None
                },
            })
        }
        "Log" => {
            let log_id = log_id(graph, step_id)?;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;

            Ok(DirectRunPlan::Log {
                step_id: step_id.to_string(),
                log_id,
                breakpoint: step_breakpoint_enabled(graph, step),
                next_plan: Box::new(next_plan),
            })
        }
        "Agent" => {
            let agent = agent_config(graph, step_id)?;
            let durable_checkpoint = agent.durable;
            let max_retries = agent_effective_max_retries(agent);
            let retry_delay_ms = agent_effective_retry_delay_ms(agent);
            let rate_limit_budget_ms = graph.rate_limit_budget_ms;
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;
            let error_plan = if include_on_error {
                on_error_plan(graph, child_workflows, step_id, stack, orders)?
            } else {
                None
            };

            Ok(DirectRunPlan::Agent {
                step_id: step_id.to_string(),
                agent_id: agent.id,
                agent_component_id: canonicalize_direct_agent_id(&agent.agent_id),
                input_mapping_id: agent.input_mapping_id,
                durable_checkpoint,
                breakpoint: step_breakpoint_enabled(graph, step),
                max_retries,
                retry_delay_ms,
                rate_limit_budget_ms,
                next_plan: Box::new(next_plan),
                error_plan,
            })
        }
        "AiAgent" => {
            // The manifest stores the AiAgent step as an agent entry targeting
            // `ai_tools`/`chat-completion` (single shot, no memory) or
            // `chat-turn` (tool loop and/or memory). The capability id decides.
            let agent = agent_config(graph, step_id)?;
            let tool_edges = graph
                .edges
                .iter()
                .filter(|edge| edge.from_step == step_id)
                .filter(|edge| {
                    edge.label.as_deref().is_some_and(|label| {
                        label != "next"
                            && label != "onError"
                            && label != "memory"
                            && !label.starts_with("mcp.")
                    })
                })
                .collect::<Vec<_>>();

            if agent.capability_id == "chat-completion" {
                let next_plan = normal_flow_plan(
                    graph,
                    child_workflows,
                    step_id,
                    stack,
                    include_on_error,
                    stop_at,
                    region_root,
                    orders,
                )?;
                let error_plan = if include_on_error {
                    on_error_plan(graph, child_workflows, step_id, stack, orders)?
                } else {
                    None
                };
                return Ok(DirectRunPlan::AiAgent {
                    step_id: step_id.to_string(),
                    agent_id: agent.id,
                    agent_component_id: canonicalize_direct_agent_id(&agent.agent_id),
                    input_mapping_id: agent.input_mapping_id,
                    durable_checkpoint: agent.durable,
                    breakpoint: step_breakpoint_enabled(graph, step),
                    max_retries: agent.max_retries.unwrap_or(0),
                    retry_delay_ms: agent.retry_delay.unwrap_or(1_000),
                    next_plan: Box::new(next_plan),
                    error_plan,
                });
            }

            // Build the tool table in the same order as the advertised `tools`
            // in the input mapping, so the capability's resolved tool index
            // selects the matching entry.
            let tool_names: Vec<String> = graph
                .mappings
                .iter()
                .find(|mapping| {
                    mapping.step_id == step_id && mapping.purpose == "agent.inputMapping"
                })
                .and_then(|mapping| mapping.value.get("tools"))
                .and_then(|tools| tools.get("value"))
                .and_then(|value| value.as_array())
                .map(|defs| {
                    defs.iter()
                        .filter_map(|def| {
                            def.get("name").and_then(|n| n.as_str()).map(String::from)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut tools = Vec::with_capacity(tool_names.len());
            for name in &tool_names {
                // Advertised tool order is Agent/Embed tools then MCP meta-tools.
                // An Agent or EmbedWorkflow tool's name is its edge label; an MCP
                // tool's name is `<toolset>_search`/`_invoke`, resolved to its
                // `agent.tool.mcp` provider entry (named after the synthetic tool).
                let tool_edge = tool_edges
                    .iter()
                    .find(|edge| edge.label.as_deref() == Some(name.as_str()));
                if let Some(edge) = tool_edge {
                    // An EmbedWorkflow tool target has a preloaded child graph;
                    // run it as the tool, feeding its output back to the model.
                    if child_workflows
                        .iter()
                        .any(|child| child.step_id == edge.to_step)
                    {
                        let child = child_workflow_graph(child_workflows, &edge.to_step)?;
                        let child_plan = step_run_plan(
                            &child.graph,
                            child_workflows,
                            &child.graph.entry_point,
                            &mut Vec::new(),
                        )?;
                        tools.push(DirectAiToolPlan::Embed {
                            step_id: edge.to_step.clone(),
                            child_plan: Box::new(child_plan),
                        });
                        continue;
                    }
                    // A WaitForSignal tool target suspends the loop on a durable
                    // human-in-the-loop signal and feeds the payload back.
                    if graph
                        .steps
                        .iter()
                        .any(|s| s.id == edge.to_step && s.step_type == "WaitForSignal")
                    {
                        tools.push(DirectAiToolPlan::Wait {
                            step_id: edge.to_step.clone(),
                            label: name.clone(),
                        });
                        continue;
                    }
                    let tool_agent = graph
                        .agents
                        .iter()
                        .find(|candidate| {
                            candidate.step_id == edge.to_step && candidate.purpose == "agent.config"
                        })
                        .ok_or_else(|| {
                            DirectCompileError::Component(format!(
                                "AiAgent tool target '{}' has no agent config",
                                edge.to_step
                            ))
                        })?;
                    tools.push(DirectAiToolPlan::Agent {
                        agent_id: tool_agent.id,
                        agent_component_id: canonicalize_direct_agent_id(&tool_agent.agent_id),
                        label: name.clone(),
                        timeout_ms: tool_agent.timeout,
                    });
                } else {
                    let tool_agent = graph
                        .agents
                        .iter()
                        .find(|candidate| {
                            candidate.step_id == step_id
                                && candidate.purpose == "agent.tool.mcp"
                                && candidate.name.as_deref() == Some(name.as_str())
                        })
                        .ok_or_else(|| {
                            DirectCompileError::Component(format!(
                                "AiAgent tool '{name}' has no execution-plan edge or MCP provider"
                            ))
                        })?;
                    tools.push(DirectAiToolPlan::Agent {
                        agent_id: tool_agent.id,
                        agent_component_id: canonicalize_direct_agent_id(&tool_agent.agent_id),
                        label: name.clone(),
                        // MCP tool providers carry their own transport timeout;
                        // this is typically None (no per-call override).
                        timeout_ms: tool_agent.timeout,
                    });
                }
            }
            let max_iterations = graph
                .steps
                .iter()
                .find(|candidate| candidate.id == step_id)
                .and_then(|manifest_step| manifest_step.body.get("config"))
                .and_then(|config| config.get("maxIterations"))
                .and_then(|value| value.as_u64())
                .map(|value| value as u32)
                .filter(|value| *value > 0)
                .unwrap_or(10);
            // Sliding-window compaction threshold (generated default 50).
            let max_messages = graph
                .steps
                .iter()
                .find(|candidate| candidate.id == step_id)
                .and_then(|manifest_step| manifest_step.body.get("config"))
                .and_then(|config| config.get("memory"))
                .and_then(|memory| memory.get("compaction"))
                .and_then(|compaction| compaction.get("maxMessages"))
                .and_then(|value| value.as_u64())
                .map(|value| value as u32)
                .filter(|value| *value > 0)
                .unwrap_or(50);
            // Conversation memory, when present, is recorded as load/save agent
            // entries plus a conversation-id mapping.
            let memory = match (
                graph
                    .agents
                    .iter()
                    .find(|a| a.step_id == step_id && a.purpose == "memory.load"),
                graph
                    .agents
                    .iter()
                    .find(|a| a.step_id == step_id && a.purpose == "memory.save"),
                graph
                    .mappings
                    .iter()
                    .find(|m| m.step_id == step_id && m.purpose == "memory.conversation"),
            ) {
                (Some(load), Some(save), Some(conv)) => {
                    // Summarize strategy is recorded as a `memory.summarize`
                    // provider agent (the `ai-tools` summarize-memory capability).
                    let summarize = graph
                        .agents
                        .iter()
                        .find(|a| a.step_id == step_id && a.purpose == "memory.summarize")
                        .map(|agent| DirectAiSummarizePlan {
                            agent_id: agent.id,
                            agent_component_id: canonicalize_direct_agent_id(&agent.agent_id),
                        });
                    Some(DirectAiMemoryPlan {
                        load_agent_id: load.id,
                        save_agent_id: save.id,
                        agent_component_id: canonicalize_direct_agent_id(&load.agent_id),
                        conversation_mapping_id: conv.id,
                        max_messages,
                        summarize,
                    })
                }
                _ => None,
            };
            let next_plan = normal_flow_plan(
                graph,
                child_workflows,
                step_id,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;

            Ok(DirectRunPlan::AiAgentLoop {
                step_id: step_id.to_string(),
                agent_id: agent.id,
                agent_component_id: canonicalize_direct_agent_id(&agent.agent_id),
                input_mapping_id: agent.input_mapping_id,
                durable_checkpoint: agent.durable,
                breakpoint: step_breakpoint_enabled(graph, step),
                max_iterations,
                tools,
                memory,
                next_plan: Box::new(next_plan),
                error_plan: if include_on_error {
                    on_error_plan(graph, child_workflows, step_id, stack, orders)?
                } else {
                    None
                },
            })
        }
        "Error" => Ok(DirectRunPlan::Error {
            step_id: step_id.to_string(),
            error_id: error_id(graph, step_id)?,
            breakpoint: step_breakpoint_enabled(graph, step),
        }),
        "Conditional" => {
            let condition_id = graph
                .conditions
                .iter()
                .find(|condition| {
                    condition.owner_id == step_id && condition.purpose == "conditional.condition"
                })
                .map(|condition| condition.id)
                .ok_or_else(|| {
                    DirectCompileError::Component(format!(
                        "missing Conditional condition for step '{step_id}'"
                    ))
                })?;

            let true_step = branch_target(graph, step_id, "true")?.to_string();
            let false_step = branch_target(graph, step_id, "false")?.to_string();

            // Detect a diamond: where the true/false branches re-converge. The
            // merge (and everything after) is emitted once as a shared
            // continuation, so each branch stops at it. `None` when the branches
            // are terminal (no re-merge). Don't treat the enclosing stop_at as a
            // new local merge.
            let merge = direct_find_merge_point(
                graph,
                &[Some(true_step.clone()), Some(false_step.clone())],
            )
            .filter(|m| Some(m.as_str()) != stop_at);
            let branch_stop = merge.as_deref().or(stop_at);

            stack.push(step_id.to_string());
            let true_plan = step_run_plan_inner(
                graph,
                child_workflows,
                &true_step,
                stack,
                include_on_error,
                branch_stop,
                &true_step,
                orders,
            )?;
            let false_plan = step_run_plan_inner(
                graph,
                child_workflows,
                &false_step,
                stack,
                include_on_error,
                branch_stop,
                &false_step,
                orders,
            )?;
            let merge_plan = match &merge {
                Some(merge_step) => Some(Box::new(step_run_plan_inner(
                    graph,
                    child_workflows,
                    merge_step,
                    stack,
                    include_on_error,
                    stop_at,
                    merge_step,
                    orders,
                )?)),
                None => None,
            };
            stack.pop();

            Ok(DirectRunPlan::Conditional {
                step_id: step_id.to_string(),
                condition_id,
                breakpoint: step_breakpoint_enabled(graph, step),
                true_plan: Box::new(true_plan),
                false_plan: Box::new(false_plan),
                merge_plan,
            })
        }
        other => Err(DirectCompileError::Component(format!(
            "direct run plan does not support step '{step_id}' with type '{other}'"
        ))),
    }
}

/// Whether a step routes its normal-flow successors through generated condition
/// code (Conditional, routing Switch, or any conditioned normal-flow edge)
/// rather than continuing unconditionally. Mirrors the generated
/// `branching::is_branching_step` + `has_conditioned_normal_flow_edges` so the
/// direct topological order stops at the same steps.
fn direct_is_branching_step(graph: &DirectGraphManifest, step: &DirectStepManifest) -> bool {
    match step.step_type.as_str() {
        "Conditional" => true,
        "Switch" => switch_is_routing(graph, &step.id).unwrap_or(false),
        _ => false,
    }
}

fn direct_has_conditioned_normal_flow_edges(graph: &DirectGraphManifest, step_id: &str) -> bool {
    graph.edges.iter().any(|edge| {
        edge.from_step == step_id
            && is_normal_label(edge.label.as_deref())
            && edge.condition_id.is_some()
    })
}

/// Topological order of the unconditional normal-flow region rooted at `root`
/// (Kahn's algorithm, FIFO — identical to the generated `build_execution_order`).
/// The entry backbone is the region rooted at the entry point; branch targets,
/// merge continuations, and onError handlers root their own regions. Traversal
/// stops at branching steps, whose successors are emitted by the branch
/// sub-plans instead, and never enters `stop_at` — the enclosing merge point,
/// which the parent emits as the shared continuation. The order linearizes
/// fan-out (a step with multiple unconditional successors) and the joins it
/// creates so each step is emitted exactly once, in dependency order — the same
/// sequential execution the generated path produces. Each call is O(V*E); plan
/// construction goes through `DirectRegionOrderCache` so a region's order is
/// computed once per graph rather than once per planned step.
pub(super) fn direct_execution_order(
    graph: &DirectGraphManifest,
    root: &str,
    stop_at: Option<&str>,
) -> Vec<String> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut reachable = HashSet::new();
    let mut discovery_order = Vec::new();
    let mut discovery_queue = VecDeque::new();

    reachable.insert(root.to_string());
    discovery_order.push(root.to_string());
    discovery_queue.push_back(root.to_string());

    while let Some(step_id) = discovery_queue.pop_front() {
        let Some(step) = graph.steps.iter().find(|candidate| candidate.id == step_id) else {
            continue;
        };
        if direct_is_branching_step(graph, step)
            || direct_has_conditioned_normal_flow_edges(graph, &step_id)
        {
            continue;
        }
        for edge in &graph.edges {
            if edge.from_step == step_id
                && is_normal_label(edge.label.as_deref())
                && Some(edge.to_step.as_str()) != stop_at
                && reachable.insert(edge.to_step.clone())
            {
                discovery_order.push(edge.to_step.clone());
                discovery_queue.push_back(edge.to_step.clone());
            }
        }
    }

    let mut indegree: HashMap<String, usize> = discovery_order
        .iter()
        .map(|step_id| (step_id.clone(), 0))
        .collect();
    for edge in &graph.edges {
        if is_normal_label(edge.label.as_deref())
            && reachable.contains(&edge.from_step)
            && reachable.contains(&edge.to_step)
        {
            *indegree.entry(edge.to_step.clone()).or_insert(0) += 1;
        }
    }

    let mut order = Vec::new();
    let mut ready = VecDeque::new();
    let mut queued = HashSet::new();
    ready.push_back(root.to_string());
    queued.insert(root.to_string());
    while let Some(step_id) = ready.pop_front() {
        order.push(step_id.clone());
        let Some(step) = graph.steps.iter().find(|candidate| candidate.id == step_id) else {
            continue;
        };
        if direct_is_branching_step(graph, step)
            || direct_has_conditioned_normal_flow_edges(graph, &step_id)
        {
            continue;
        }
        for edge in &graph.edges {
            if edge.from_step != step_id
                || !is_normal_label(edge.label.as_deref())
                || !reachable.contains(&edge.to_step)
            {
                continue;
            }
            if let Some(count) = indegree.get_mut(&edge.to_step) {
                *count = count.saturating_sub(1);
                if *count == 0 && queued.insert(edge.to_step.clone()) {
                    ready.push_back(edge.to_step.clone());
                }
            }
        }
    }
    // Deterministic fallback for any steps left out (validation rejects normal
    // -flow cycles, but keep the order total).
    for step_id in discovery_order {
        if !queued.contains(&step_id) {
            order.push(step_id);
        }
    }
    order
}

/// Memoized `direct_execution_order` results, keyed by `(region_root, stop_at)`.
/// Plan construction asks for a region's order once per step it plans
/// (`topo_successor`); recomputing the O(V*E) discovery + Kahn pass each time
/// made plan construction O(n^3) in chain length. One cache lives for the
/// duration of a single `step_run_plan` call and never crosses graphs:
/// Split/While/EmbedWorkflow subgraphs are separate `DirectGraphManifest`
/// instances planned through their own `step_run_plan` (fresh cache) calls.
struct DirectRegionOrderCache {
    orders: HashMap<(String, Option<String>), Rc<Vec<String>>>,
}

impl DirectRegionOrderCache {
    fn new() -> Self {
        Self {
            orders: HashMap::new(),
        }
    }

    /// The topological order of the region rooted at `root` (stopping at
    /// `stop_at`), computed on first use. `graph` must be the graph this cache
    /// was created for.
    fn region_order(
        &mut self,
        graph: &DirectGraphManifest,
        root: &str,
        stop_at: Option<&str>,
    ) -> Rc<Vec<String>> {
        Rc::clone(
            self.orders
                .entry((root.to_string(), stop_at.map(str::to_string)))
                .or_insert_with(|| Rc::new(direct_execution_order(graph, root, stop_at))),
        )
    }
}

/// The next step to run after `from_step` within its region — the step
/// immediately after it in the region's topological order. `None` when
/// `from_step` ends the region (its remaining edges, if any, exit to `stop_at`).
fn topo_successor(
    graph: &DirectGraphManifest,
    region_root: &str,
    stop_at: Option<&str>,
    from_step: &str,
    orders: &mut DirectRegionOrderCache,
) -> Option<String> {
    let order = orders.region_order(graph, region_root, stop_at);
    let position = order.iter().position(|step| step == from_step)?;
    order.get(position + 1).cloned()
}

#[allow(clippy::too_many_arguments)]
fn normal_flow_plan(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    from_step: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
    stop_at: Option<&str>,
    region_root: &str,
    orders: &mut DirectRegionOrderCache,
) -> Result<DirectRunPlan, DirectCompileError> {
    let edges = normal_flow_edges(graph, from_step);

    let mut conditional_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_some())
        .copied()
        .collect::<Vec<_>>();
    let default_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_none())
        .copied()
        .collect::<Vec<_>>();

    if conditional_edges.is_empty() {
        // Steps continue to their topological successor within their region,
        // which linearizes fan-out (multiple unconditional successors) and the
        // joins it creates so each step is emitted exactly once, after all of
        // its predecessors — even when fan-out branches cross-link (a shape a
        // recursive per-fan-out merge search cannot represent without
        // duplicating the shared continuation into earlier branches and
        // running steps before their inputs exist).
        if default_edges.len() > 1 {
            // Concurrent fan-out (docs/wasip3-parallel-branches-plan.md): when the
            // branches form a clean single-Agent diamond, emit them as a
            // ParallelBranches window so the branch agents run concurrently.
            // Anything else falls through to the sequential topological
            // linearization below (widened in later phases).
            if let Some(plan) = try_parallel_branches(
                graph,
                child_workflows,
                from_step,
                &default_edges,
                stack,
                include_on_error,
                stop_at,
                orders,
            )? {
                return Ok(plan);
            }
            // E073 guard: an unconditional fan-out must re-converge inside its
            // region (or exit to the enclosing merge). A region with two
            // terminals is an ambiguous multi-exit graph; validation rejects
            // it, guard here so we never mis-emit one.
            let region = orders.region_order(graph, region_root, stop_at);
            let hard_terminals = region
                .iter()
                .filter(|step_id| normal_flow_edges(graph, step_id).is_empty())
                .count();
            if hard_terminals >= 2 || (stop_at.is_some() && hard_terminals >= 1) {
                return Err(DirectCompileError::Component(format!(
                    "direct step '{from_step}' fans out to parallel branches that never re-converge"
                )));
            }
        }
        if let Some(next) = topo_successor(graph, region_root, stop_at, from_step, orders) {
            stack.push(from_step.to_string());
            let next_plan = step_run_plan_inner(
                graph,
                child_workflows,
                &next,
                stack,
                include_on_error,
                stop_at,
                region_root,
                orders,
            )?;
            stack.pop();
            return Ok(next_plan);
        }

        if edges.is_empty() {
            // Terminal step with no successor and no explicit Finish (e.g. a
            // single-Agent workflow). Match the generated compiler, which
            // returns `Ok(Value::Null)`: complete the workflow with a null
            // output instead of failing to build the plan.
            return Ok(DirectRunPlan::ImplicitFinish);
        }

        // `from_step` ends its region's topological order, so its remaining
        // unconditional edges can only exit to the enclosing merge (`stop_at`),
        // which the parent emits — the branch ends with a `Join`.
        if stop_at.is_some()
            && default_edges
                .iter()
                .all(|edge| Some(edge.to_step.as_str()) == stop_at)
        {
            return Ok(DirectRunPlan::Join);
        }

        // A successor that is neither in the region order nor the enclosing
        // merge: a normal-flow cycle back into the region. Validation rejects
        // cycles; guard here so we never mis-emit one.
        return Err(DirectCompileError::Component(format!(
            "direct run plan contains a cycle at step '{from_step}'"
        )));
    }

    if edges.is_empty() {
        return Err(DirectCompileError::Component(format!(
            "missing normal branch for direct step '{from_step}'"
        )));
    }

    let [default_edge] = default_edges.as_slice() else {
        return Err(DirectCompileError::Component(format!(
            "direct step '{from_step}' conditional edge routing requires exactly one default branch"
        )));
    };

    conditional_edges.sort_by(|left, right| {
        (
            -i64::from(left.priority.unwrap_or(0)),
            left.ordinal,
            left.to_step.as_str(),
        )
            .cmp(&(
                -i64::from(right.priority.unwrap_or(0)),
                right.ordinal,
                right.to_step.as_str(),
            ))
    });

    // Detect a diamond: where the conditioned edges (and default) re-converge, so
    // the merge runs once as a shared continuation rather than duplicated.
    let branch_starts: Vec<Option<String>> = conditional_edges
        .iter()
        .map(|edge| Some(edge.to_step.clone()))
        .chain(std::iter::once(Some(default_edge.to_step.clone())))
        .collect();
    let merge =
        direct_find_merge_point(graph, &branch_starts).filter(|m| Some(m.as_str()) != stop_at);
    let branch_stop = merge.as_deref().or(stop_at);

    stack.push(from_step.to_string());
    let branches = conditional_edges
        .into_iter()
        .map(|edge| {
            let condition_id = edge.condition_id.ok_or_else(|| {
                DirectCompileError::Component(format!(
                    "missing edge condition id for direct step '{from_step}'"
                ))
            })?;
            let plan = step_run_plan_inner(
                graph,
                child_workflows,
                &edge.to_step,
                stack,
                include_on_error,
                branch_stop,
                &edge.to_step,
                orders,
            )?;
            Ok(DirectEdgeConditionPlan {
                condition_id,
                plan: Box::new(plan),
            })
        })
        .collect::<Result<Vec<_>, DirectCompileError>>()?;
    let default_plan = step_run_plan_inner(
        graph,
        child_workflows,
        &default_edge.to_step,
        stack,
        include_on_error,
        branch_stop,
        &default_edge.to_step,
        orders,
    )?;
    let merge_plan = match &merge {
        Some(merge_step) => Some(Box::new(step_run_plan_inner(
            graph,
            child_workflows,
            merge_step,
            stack,
            include_on_error,
            stop_at,
            merge_step,
            orders,
        )?)),
        None => None,
    };
    stack.pop();

    Ok(DirectRunPlan::EdgeRoute {
        branches,
        default_plan: Box::new(default_plan),
        merge_plan,
    })
}

/// Control-flow successors of a step that the merge analysis follows: the
/// branch-labeled edges of a branching step (Conditional / routing Switch), the
/// normal-flow edges otherwise; never `onError`. Mirrors the generated
/// `branching::collect_reachable_steps` successor rule.
fn direct_control_successors(graph: &DirectGraphManifest, step_id: &str) -> Vec<String> {
    let Some(step) = graph.steps.iter().find(|step| step.id == step_id) else {
        return Vec::new();
    };
    if matches!(step.step_type.as_str(), "Finish" | "Error") {
        return Vec::new();
    }
    if direct_is_branching_step(graph, step) {
        graph
            .edges
            .iter()
            .filter(|edge| edge.from_step == step_id && edge.label.as_deref() != Some("onError"))
            .map(|edge| edge.to_step.clone())
            .collect()
    } else {
        normal_flow_edges(graph, step_id)
            .into_iter()
            .map(|edge| edge.to_step.clone())
            .collect()
    }
}

/// Steps reachable from `start` via control-flow successors, in BFS order.
fn direct_collect_reachable(graph: &DirectGraphManifest, start: &str) -> Vec<String> {
    let mut reachable = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start.to_string());
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        reachable.push(current.clone());
        for next in direct_control_successors(graph, &current) {
            if !visited.contains(&next) {
                queue.push_back(next);
            }
        }
    }
    reachable
}

/// The first step reachable from ALL branch starts — the diamond merge point —
/// or `None` if fewer than two valid branches exist or they never re-converge.
/// Mirrors the generated `branching::find_merge_point_n` so the direct emitter
/// structures conditional/switch diamonds the same way (the merge is a shared
/// continuation emitted once, not duplicated in each branch).
fn direct_find_merge_point(
    graph: &DirectGraphManifest,
    branch_starts: &[Option<String>],
) -> Option<String> {
    let starts: Vec<&String> = branch_starts.iter().filter_map(|s| s.as_ref()).collect();
    if starts.len() < 2 {
        return None;
    }
    let reachable_sets: Vec<Vec<String>> = starts
        .iter()
        .map(|start| direct_collect_reachable(graph, start))
        .collect();
    reachable_sets[0]
        .iter()
        .find(|step_id| reachable_sets[1..].iter().all(|set| set.contains(step_id)))
        .cloned()
}

/// Try to lower an unconditional fan-out at `from_step` as concurrent
/// `ParallelBranches` (docs/wasip3-parallel-branches-plan.md). Phase 4a: only a
/// clean single-Agent diamond qualifies — every branch is exactly one Agent step
/// that re-converges at a shared merge, non-durable, no retries, no breakpoint.
/// Any other shape returns `None` so the caller linearizes it (transitional; the
/// eligible set widens in 4b/4c). The plan is structural — whether the branches
/// actually run concurrently (vs. a sequential fallback) is decided at emission
/// time by `static_data.parallel_enabled` and the workflow-agent exclusion.
#[allow(clippy::too_many_arguments)]
fn try_parallel_branches(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    from_step: &str,
    default_edges: &[&DirectEdgeManifest],
    stack: &mut Vec<String>,
    include_on_error: bool,
    stop_at: Option<&str>,
    orders: &mut DirectRegionOrderCache,
) -> Result<Option<DirectRunPlan>, DirectCompileError> {
    // Deterministic branch order (declaration order): (ordinal, to_step).
    let mut ordered: Vec<&DirectEdgeManifest> = default_edges.to_vec();
    ordered.sort_by(|left, right| {
        (left.ordinal, left.to_step.as_str()).cmp(&(right.ordinal, right.to_step.as_str()))
    });

    // Clean diamond: all branches re-converge at a single merge inside the region.
    let branch_starts: Vec<Option<String>> = ordered
        .iter()
        .map(|edge| Some(edge.to_step.clone()))
        .collect();
    let Some(merge) =
        direct_find_merge_point(graph, &branch_starts).filter(|m| Some(m.as_str()) != stop_at)
    else {
        return Ok(None);
    };

    // Try to plan the branches + merge as an isolated diamond. Any planning
    // FAILURE — a branch that itself fans out and cross-links (a shape the
    // per-branch recursion cannot represent), or a nested non-re-converging
    // region — means "not a clean diamond": restore the stack and decline, so the
    // caller's sequential linearization (which is authoritative and handles
    // cross-links) runs instead. A non-single-Agent branch declines the same way.
    let stack_depth = stack.len();
    stack.push(from_step.to_string());
    let built = plan_branch_diamond(
        graph,
        child_workflows,
        &ordered,
        &merge,
        stack,
        include_on_error,
        stop_at,
        orders,
    );
    stack.truncate(stack_depth);
    Ok(built.unwrap_or(None))
}

/// Plan each branch (single Agent → merge) plus the shared continuation, or
/// `Ok(None)` when a branch isn't a single Agent. Returns `Err` when a branch
/// cannot be planned at all (cross-linked / non-re-converging), which
/// `try_parallel_branches` maps to a decline.
#[allow(clippy::too_many_arguments)]
fn plan_branch_diamond(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    ordered: &[&DirectEdgeManifest],
    merge: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
    stop_at: Option<&str>,
    orders: &mut DirectRegionOrderCache,
) -> Result<Option<DirectRunPlan>, DirectCompileError> {
    let branch_stop = Some(merge);
    let mut branches = Vec::with_capacity(ordered.len());
    for edge in ordered {
        let plan = step_run_plan_inner(
            graph,
            child_workflows,
            &edge.to_step,
            stack,
            include_on_error,
            branch_stop,
            &edge.to_step,
            orders,
        )?;
        if !is_single_agent_branch(&plan) {
            return Ok(None);
        }
        branches.push(plan);
    }
    let merge_plan = step_run_plan_inner(
        graph,
        child_workflows,
        merge,
        stack,
        include_on_error,
        stop_at,
        merge,
        orders,
    )?;
    Ok(Some(DirectRunPlan::ParallelBranches {
        branches,
        merge_plan: Box::new(merge_plan),
    }))
}

/// Phase-4a.1 branch eligibility: exactly one Agent step that ends the branch
/// (`next_plan == Join`), non-durable and without a breakpoint. Retries ARE
/// allowed — the launch fires attempt 1 concurrently and assemble owns the
/// standard (sequential) retry loop via the memoized result, exactly like the
/// Split window's non-concurrent-backoff path. Durable branches are excluded here
/// (a later slice): the durable step key hashes the raw `source`, which assemble
/// rebuilds to accumulate earlier siblings, so a launch-time gate key would drift
/// from the assemble key and re-fire the agent on replay. A workflow-agent target
/// is excluded at emission time, not here.
fn is_single_agent_branch(plan: &DirectRunPlan) -> bool {
    matches!(
        plan,
        DirectRunPlan::Agent {
            durable_checkpoint: false,
            breakpoint: false,
            next_plan,
            ..
        } if matches!(**next_plan, DirectRunPlan::Join)
    )
}

fn on_error_plan(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    from_step: &str,
    stack: &mut Vec<String>,
    orders: &mut DirectRegionOrderCache,
) -> Result<Option<DirectErrorRoutePlan>, DirectCompileError> {
    let edges = on_error_edges(graph, from_step);
    if edges.is_empty() {
        return Ok(None);
    }

    let mut conditional_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_some())
        .copied()
        .collect::<Vec<_>>();
    let default_edges = edges
        .iter()
        .filter(|edge| edge.condition_id.is_none())
        .copied()
        .collect::<Vec<_>>();
    let default_edge = match default_edges.as_slice() {
        [] => None,
        [edge] => Some(*edge),
        _ => {
            return Err(DirectCompileError::Component(format!(
                "direct step '{from_step}' onError routing supports at most one default branch"
            )));
        }
    };

    conditional_edges.sort_by(|left, right| {
        (
            -i64::from(left.priority.unwrap_or(0)),
            left.ordinal,
            left.to_step.as_str(),
        )
            .cmp(&(
                -i64::from(right.priority.unwrap_or(0)),
                right.ordinal,
                right.to_step.as_str(),
            ))
    });

    stack.push(from_step.to_string());
    let branches = conditional_edges
        .into_iter()
        .map(|edge| {
            let condition_id = edge.condition_id.ok_or_else(|| {
                DirectCompileError::Component(format!(
                    "missing onError condition id for direct step '{from_step}'"
                ))
            })?;
            let plan = step_run_plan_without_on_error(
                graph,
                child_workflows,
                &edge.to_step,
                stack,
                orders,
            )?;
            Ok(DirectEdgeConditionPlan {
                condition_id,
                plan: Box::new(plan),
            })
        })
        .collect::<Result<Vec<_>, DirectCompileError>>()?;
    let default_plan = default_edge
        .map(|edge| {
            step_run_plan_without_on_error(graph, child_workflows, &edge.to_step, stack, orders)
        })
        .transpose()?
        .map(Box::new);
    stack.pop();

    Ok(Some(DirectErrorRoutePlan {
        branches,
        default_plan,
    }))
}

fn normal_flow_edges<'a>(
    graph: &'a DirectGraphManifest,
    from_step: &str,
) -> Vec<&'a DirectEdgeManifest> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from_step == from_step && is_normal_label(edge.label.as_deref()))
        .collect()
}

fn on_error_edges<'a>(
    graph: &'a DirectGraphManifest,
    from_step: &str,
) -> Vec<&'a DirectEdgeManifest> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from_step == from_step && edge.label.as_deref() == Some("onError"))
        .collect()
}

fn is_normal_label(label: Option<&str>) -> bool {
    label.is_none_or(|label| label.is_empty() || label == "next")
}

fn branch_target<'a>(
    graph: &'a DirectGraphManifest,
    from_step: &str,
    label: &str,
) -> Result<&'a str, DirectCompileError> {
    graph
        .edges
        .iter()
        .find(|edge| edge.from_step == from_step && edge.label.as_deref() == Some(label))
        .map(|edge| edge.to_step.as_str())
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing '{label}' branch for Conditional step '{from_step}'"
            ))
        })
}

fn filter_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Filter")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Filter step"
        )));
    }

    graph
        .filters
        .iter()
        .find(|filter| filter.step_id == step_id && filter.purpose == "filter.config")
        .map(|filter| filter.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Filter config for step '{step_id}'"))
        })
}

fn switch_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Switch")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Switch step"
        )));
    }

    graph
        .switches
        .iter()
        .find(|switch| switch.step_id == step_id && switch.purpose == "switch.config")
        .map(|switch| switch.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Switch config for step '{step_id}'"))
        })
}

fn switch_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a serde_json::Value, DirectCompileError> {
    graph
        .switches
        .iter()
        .find(|switch| switch.step_id == step_id && switch.purpose == "switch.config")
        .map(|switch| &switch.value)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Switch config for step '{step_id}'"))
        })
}

fn switch_is_routing(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<bool, DirectCompileError> {
    Ok(switch_config(graph, step_id)?
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|cases| cases.iter().any(|case| case.get("route").is_some())))
}

fn switch_route_labels(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<Vec<String>, DirectCompileError> {
    let mut labels = switch_config(graph, step_id)?
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|case| case.get("route").and_then(serde_json::Value::as_str))
        .filter(|label| *label != "default")
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    Ok(labels)
}

fn group_by_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "GroupBy")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a GroupBy step"
        )));
    }

    graph
        .group_bys
        .iter()
        .find(|group_by| group_by.step_id == step_id && group_by.purpose == "groupBy.config")
        .map(|group_by| group_by.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing GroupBy config for step '{step_id}'"))
        })
}

fn split_manifest<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectSplitManifest, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Split")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Split step"
        )));
    }

    graph
        .splits
        .iter()
        .find(|split| split.step_id == step_id && split.purpose == "split.config")
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Split config for step '{step_id}'"))
        })
}

fn split_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a serde_json::Value, DirectCompileError> {
    graph
        .splits
        .iter()
        .find(|split| split.step_id == step_id && split.purpose == "split.config")
        .map(|split| &split.value)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Split config for step '{step_id}'"))
        })
}

/// The Split's requested `parallelism` window. 0 means "unlimited" per the
/// DSL contract — normalized here to u32::MAX and clamped at emission time to
/// the item count; absent/1 = sequential.
fn split_parallel_window(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<Option<u32>, DirectCompileError> {
    Ok(split_config(graph, step_id)?
        .get("parallelism")
        .and_then(serde_json::Value::as_u64)
        .map(|window| {
            if window == 0 {
                u32::MAX
            } else {
                u32::try_from(window).unwrap_or(u32::MAX)
            }
        })
        .filter(|window| *window > 1))
}

fn split_dont_stop_on_failed(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<bool, DirectCompileError> {
    Ok(split_config(graph, step_id)?
        .get("dontStopOnFailed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

/// Resolve a `Split` step's configured timeout in milliseconds, if any. A zero or
/// absent timeout is treated as "no timeout", matching the degenerate config and
/// the generated Rust path that does not enforce the field.
fn split_timeout_ms(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<Option<u64>, DirectCompileError> {
    Ok(split_config(graph, step_id)?
        .get("timeout")
        .and_then(serde_json::Value::as_u64)
        .filter(|ms| *ms > 0))
}

fn split_subgraph<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectGraphManifest, DirectCompileError> {
    graph
        .steps
        .iter()
        .find(|step| step.id == step_id && step.step_type == "Split")
        .and_then(|step| {
            step.nested_graphs
                .iter()
                .find(|nested| nested.role == "split.subgraph")
        })
        .map(|nested| nested.graph.as_ref())
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Split subgraph for step '{step_id}'"))
        })
}

fn while_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "While")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a While step"
        )));
    }

    graph
        .whiles
        .iter()
        .find(|while_step| while_step.step_id == step_id && while_step.purpose == "while.config")
        .map(|while_step| while_step.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing While config for step '{step_id}'"))
        })
}

/// Resolve a `While` step's configured timeout in milliseconds, if any.
///
/// `WhileConfig.timeout` is a static config value. A zero (or absent) timeout is
/// treated as "no timeout", matching the degenerate config and the generated
/// Rust path that does not enforce the field at all.
fn while_timeout_ms(graph: &DirectGraphManifest, step_id: &str) -> Option<u64> {
    graph
        .whiles
        .iter()
        .find(|while_step| while_step.step_id == step_id && while_step.purpose == "while.config")
        .and_then(|while_step| while_step.value.get("timeout"))
        .and_then(serde_json::Value::as_u64)
        .filter(|ms| *ms > 0)
}

fn while_subgraph<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectGraphManifest, DirectCompileError> {
    graph
        .steps
        .iter()
        .find(|step| step.id == step_id && step.step_type == "While")
        .and_then(|step| {
            step.nested_graphs
                .iter()
                .find(|nested| nested.role == "while.subgraph")
        })
        .map(|nested| nested.graph.as_ref())
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing While subgraph for step '{step_id}'"))
        })
}

fn wait_on_wait_subgraph<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<Option<&'a DirectGraphManifest>, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "WaitForSignal")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a WaitForSignal step"
        )));
    }

    Ok(graph
        .steps
        .iter()
        .find(|step| step.id == step_id && step.step_type == "WaitForSignal")
        .and_then(|step| {
            step.nested_graphs
                .iter()
                .find(|nested| nested.role == "waitForSignal.onWait")
        })
        .map(|nested| nested.graph.as_ref()))
}

fn delay_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectDelayManifest, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Delay")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Delay step"
        )));
    }

    graph
        .delays
        .iter()
        .find(|delay| delay.step_id == step_id && delay.purpose == "delay.config")
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Delay config for step '{step_id}'"))
        })
}

fn child_workflow_graph<'a>(
    child_workflows: &'a [DirectChildWorkflowGraphManifest],
    step_id: &str,
) -> Result<&'a DirectChildWorkflowGraphManifest, DirectCompileError> {
    child_workflows
        .iter()
        .find(|child| child.step_id == step_id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing direct child workflow graph for EmbedWorkflow step '{step_id}'"
            ))
        })
}

fn log_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Log")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not a Log step"
        )));
    }

    graph
        .logs
        .iter()
        .find(|log| log.step_id == step_id && log.purpose == "log.config")
        .map(|log| log.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Log config for step '{step_id}'"))
        })
}

fn error_id(graph: &DirectGraphManifest, step_id: &str) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Error")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not an Error step"
        )));
    }

    graph
        .errors
        .iter()
        .find(|error| error.step_id == step_id && error.purpose == "error.config")
        .map(|error| error.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Error config for step '{step_id}'"))
        })
}

fn agent_config<'a>(
    graph: &'a DirectGraphManifest,
    step_id: &str,
) -> Result<&'a DirectAgentManifest, DirectCompileError> {
    // Both Agent and AiAgent steps are recorded as agent entries (an AiAgent
    // lowers to an `ai-tools`/`chat-completion` invoke), so accept either.
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && matches!(step.step_type.as_str(), "Agent" | "AiAgent"))
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not an Agent or AiAgent step"
        )));
    }

    graph
        .agents
        .iter()
        .find(|agent| agent.step_id == step_id && agent.purpose == "agent.config")
        .ok_or_else(|| {
            DirectCompileError::Component(format!("missing Agent config for step '{step_id}'"))
        })
}

fn agent_effective_max_retries(agent: &DirectAgentManifest) -> u32 {
    agent
        .max_retries
        .unwrap_or(if agent.rate_limited { 5 } else { 3 })
}

fn agent_effective_retry_delay_ms(agent: &DirectAgentManifest) -> u64 {
    agent
        .retry_delay
        .unwrap_or(if agent.rate_limited { 2_000 } else { 1_000 })
}

fn embed_workflow_effective_max_retries(step: &DirectStepManifest) -> u32 {
    step.body
        .get("maxRetries")
        .and_then(serde_json::Value::as_u64)
        .and_then(|max_retries| u32::try_from(max_retries).ok())
        .unwrap_or(3)
}

fn embed_workflow_effective_retry_delay_ms(step: &DirectStepManifest) -> u64 {
    step.body
        .get("retryDelay")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1_000)
}

fn split_effective_max_retries(split: &DirectSplitManifest) -> u32 {
    split
        .value
        .get("maxRetries")
        .and_then(serde_json::Value::as_u64)
        .and_then(|max_retries| u32::try_from(max_retries).ok())
        .unwrap_or(0)
}

fn split_effective_retry_delay_ms(split: &DirectSplitManifest) -> u64 {
    split
        .value
        .get("retryDelay")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1_000)
}

fn finish_mapping_id(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Finish")
    {
        return Err(DirectCompileError::Component(format!(
            "direct branch target '{step_id}' is not a Finish step"
        )));
    }

    graph
        .mappings
        .iter()
        .find(|mapping| mapping.step_id == step_id && mapping.purpose == "finish.inputMapping")
        .map(|mapping| mapping.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing Finish input mapping for step '{step_id}'"
            ))
        })
}

fn embed_workflow_input_mapping_id(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<u32, DirectCompileError> {
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "EmbedWorkflow")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not an EmbedWorkflow step"
        )));
    }

    graph
        .mappings
        .iter()
        .find(|mapping| {
            mapping.step_id == step_id && mapping.purpose == "embedWorkflow.inputMapping"
        })
        .map(|mapping| mapping.id)
        .ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing EmbedWorkflow input mapping for step '{step_id}'"
            ))
        })
}

fn canonicalize_direct_agent_id(agent_id: &str) -> String {
    agent_id.to_lowercase().replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Walks a plan and records every emitted step id (and structural markers)
    // in emission order, so tests can assert linearization properties.
    fn collect_plan_steps(plan: &DirectRunPlan, out: &mut Vec<String>) {
        match plan {
            DirectRunPlan::Finish { step_id, .. } => out.push(format!("Finish:{step_id}")),
            DirectRunPlan::Filter {
                step_id, next_plan, ..
            }
            | DirectRunPlan::SwitchValue {
                step_id, next_plan, ..
            }
            | DirectRunPlan::GroupBy {
                step_id, next_plan, ..
            }
            | DirectRunPlan::Delay {
                step_id, next_plan, ..
            }
            | DirectRunPlan::Log {
                step_id, next_plan, ..
            } => {
                out.push(step_id.clone());
                collect_plan_steps(next_plan, out);
            }
            DirectRunPlan::SwitchRoute {
                step_id,
                branches,
                default_plan,
                merge_plan,
                ..
            } => {
                out.push(format!("SwitchRoute:{step_id}"));
                for branch in branches {
                    out.push(format!("  route:{}", branch.label));
                    collect_plan_steps(&branch.plan, out);
                }
                out.push("  route:default".to_string());
                collect_plan_steps(default_plan, out);
                if let Some(merge) = merge_plan {
                    out.push(format!("  merge-of:{step_id}"));
                    collect_plan_steps(merge, out);
                }
            }
            DirectRunPlan::EdgeRoute {
                branches,
                default_plan,
                merge_plan,
            } => {
                out.push("EdgeRoute".to_string());
                for branch in branches {
                    collect_plan_steps(&branch.plan, out);
                }
                collect_plan_steps(default_plan, out);
                if let Some(merge) = merge_plan {
                    out.push("  merge-of:EdgeRoute".to_string());
                    collect_plan_steps(merge, out);
                }
            }
            DirectRunPlan::Split {
                step_id,
                nested_plan,
                next_plan,
                error_plan,
                ..
            } => {
                out.push(format!("Split:{step_id}"));
                out.push("  split-nested".to_string());
                collect_plan_steps(nested_plan, out);
                out.push("  split-next".to_string());
                collect_plan_steps(next_plan, out);
                if let Some(error_plan) = error_plan {
                    collect_error_plan(step_id, error_plan, out);
                }
            }
            DirectRunPlan::While {
                step_id,
                nested_plan,
                next_plan,
                error_plan,
                ..
            } => {
                out.push(format!("While:{step_id}"));
                collect_plan_steps(nested_plan, out);
                collect_plan_steps(next_plan, out);
                if let Some(error_plan) = error_plan {
                    collect_error_plan(step_id, error_plan, out);
                }
            }
            DirectRunPlan::EmbedWorkflow {
                step_id,
                child_plan,
                next_plan,
                error_plan,
                ..
            } => {
                out.push(format!("Embed:{step_id}"));
                collect_plan_steps(child_plan, out);
                collect_plan_steps(next_plan, out);
                if let Some(error_plan) = error_plan {
                    collect_error_plan(step_id, error_plan, out);
                }
            }
            DirectRunPlan::WaitForSignal {
                step_id,
                on_wait_plan,
                next_plan,
                error_plan,
                ..
            } => {
                out.push(format!("Wait:{step_id}"));
                if let Some(on_wait) = on_wait_plan {
                    collect_plan_steps(on_wait, out);
                }
                collect_plan_steps(next_plan, out);
                if let Some(error_plan) = error_plan {
                    collect_error_plan(step_id, error_plan, out);
                }
            }
            DirectRunPlan::Agent {
                step_id,
                next_plan,
                error_plan,
                ..
            }
            | DirectRunPlan::AiAgent {
                step_id,
                next_plan,
                error_plan,
                ..
            } => {
                out.push(step_id.clone());
                if let Some(error_plan) = error_plan {
                    collect_error_plan(step_id, error_plan, out);
                }
                collect_plan_steps(next_plan, out);
            }
            DirectRunPlan::AiAgentLoop {
                step_id,
                next_plan,
                error_plan,
                ..
            } => {
                out.push(format!("AiLoop:{step_id}"));
                if let Some(error_plan) = error_plan {
                    collect_error_plan(step_id, error_plan, out);
                }
                collect_plan_steps(next_plan, out);
            }
            DirectRunPlan::Error { step_id, .. } => out.push(format!("Error:{step_id}")),
            DirectRunPlan::Conditional {
                step_id,
                true_plan,
                false_plan,
                merge_plan,
                ..
            } => {
                out.push(format!("Conditional:{step_id}"));
                out.push("  true:".to_string());
                collect_plan_steps(true_plan, out);
                out.push("  false:".to_string());
                collect_plan_steps(false_plan, out);
                if let Some(merge) = merge_plan {
                    out.push(format!("  merge-of:{step_id}"));
                    collect_plan_steps(merge, out);
                }
            }
            DirectRunPlan::ParallelBranches {
                branches,
                merge_plan,
            } => {
                out.push("ParallelBranches".to_string());
                for branch in branches {
                    out.push("  branch:".to_string());
                    collect_plan_steps(branch, out);
                }
                out.push("  merge-of:ParallelBranches".to_string());
                collect_plan_steps(merge_plan, out);
            }
            DirectRunPlan::Join => out.push("Join".to_string()),
            DirectRunPlan::ImplicitFinish => out.push("ImplicitFinish".to_string()),
        }
    }

    fn collect_error_plan(step_id: &str, error_plan: &DirectErrorRoutePlan, out: &mut Vec<String>) {
        out.push(format!("  onError-of:{step_id}"));
        for branch in &error_plan.branches {
            collect_plan_steps(&branch.plan, out);
        }
        if let Some(default_plan) = &error_plan.default_plan {
            collect_plan_steps(default_plan, out);
        }
        out.push(format!("  onError-of:{step_id}:end"));
    }

    fn agent_step_json(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "stepType": "Agent",
            "agentId": "utils",
            "capabilityId": "get-current-iso-datetime",
            "inputMapping": {}
        })
    }

    /// Regression for the reported fan-out drop: inside a Conditional branch,
    /// `gate` fans out to two chains that cross-link several times before
    /// re-converging (the distilled CategorizeViaUnspsc miss-path: `fts` and
    /// `trgm` each need both chains, `bprompt` needs four predecessors). The
    /// old per-fan-out merge recursion inlined the shared merge (`fts`) into
    /// the first branch, so `pvs` ran before `pfe` produced its input and the
    /// second branch (`embq`/`pfe`) was never scheduled once the iteration
    /// failed. The region's topological order must emit every step exactly
    /// once, each after all of its predecessors.
    #[test]
    fn cross_linked_fanout_linearizes_each_step_once() {
        let mut steps = serde_json::Map::new();
        for id in [
            "hit", "gate", "bcp", "embq", "dprof", "pfe", "fts", "rfts", "trgm", "bvs", "pvs",
            "rvec", "ematch", "bprompt", "pmatch", "judge", "fpick", "persist", "tail",
        ] {
            steps.insert(id.to_string(), agent_step_json(id));
        }
        steps.insert(
            "cond".to_string(),
            serde_json::json!({
                "id": "cond",
                "stepType": "Conditional",
                "condition": {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                        {"value": "x", "valueType": "immediate"},
                        {"value": "y", "valueType": "immediate"}
                    ]
                }
            }),
        );
        steps.insert(
            "finish".to_string(),
            serde_json::json!({
                "id": "finish",
                "stepType": "Finish",
                "inputMapping": {"out": {"value": "ok", "valueType": "immediate"}}
            }),
        );
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_value(serde_json::json!({
            "entryPoint": "cond",
            "steps": steps,
            "executionPlan": [
                {"fromStep": "cond", "label": "true", "toStep": "hit"},
                {"fromStep": "cond", "label": "false", "toStep": "gate"},
                {"fromStep": "gate", "toStep": "bcp"},
                {"fromStep": "gate", "toStep": "embq"},
                {"fromStep": "bcp", "toStep": "dprof"},
                {"fromStep": "embq", "toStep": "pfe"},
                {"fromStep": "dprof", "toStep": "fts"},
                {"fromStep": "dprof", "toStep": "trgm"},
                {"fromStep": "dprof", "toStep": "bprompt"},
                {"fromStep": "pfe", "toStep": "fts"},
                {"fromStep": "pfe", "toStep": "rfts"},
                {"fromStep": "pfe", "toStep": "trgm"},
                {"fromStep": "fts", "toStep": "bvs"},
                {"fromStep": "bvs", "toStep": "pvs"},
                {"fromStep": "pvs", "toStep": "rvec"},
                {"fromStep": "rvec", "toStep": "ematch"},
                {"fromStep": "rvec", "toStep": "bprompt"},
                {"fromStep": "rfts", "toStep": "ematch"},
                {"fromStep": "rfts", "toStep": "bprompt"},
                {"fromStep": "trgm", "toStep": "ematch"},
                {"fromStep": "trgm", "toStep": "bprompt"},
                {"fromStep": "ematch", "toStep": "pmatch"},
                {"fromStep": "bprompt", "toStep": "judge"},
                {"fromStep": "pmatch", "toStep": "fpick"},
                {"fromStep": "judge", "toStep": "fpick"},
                {"fromStep": "fpick", "toStep": "persist"},
                {"fromStep": "persist", "toStep": "tail"},
                {"fromStep": "hit", "toStep": "tail"},
                {"fromStep": "tail", "toStep": "finish"}
            ]
        }))
        .expect("graph parses");

        let manifest =
            super::super::manifest::build_direct_workflow_manifest(&graph).expect("build manifest");
        let plan = direct_run_plan(&manifest).expect("build plan");
        let mut emitted = Vec::new();
        collect_plan_steps(&plan, &mut emitted);

        // The false branch is one linear chain in dependency order: both
        // fan-out targets are scheduled, and every consumer runs after all of
        // its producers (`pvs` after `pfe`, `bprompt` after `rvec`, ...).
        let false_chain: Vec<&str> = emitted
            .iter()
            .skip_while(|entry| *entry != "  false:")
            .skip(1)
            .take_while(|entry| *entry != "  merge-of:cond")
            .map(String::as_str)
            .collect();
        assert_eq!(
            false_chain,
            vec![
                "gate", "bcp", "embq", "dprof", "pfe", "fts", "rfts", "trgm", "bvs", "pvs", "rvec",
                "ematch", "bprompt", "pmatch", "judge", "fpick", "persist", "Join",
            ],
            "false branch should linearize the cross-linked fan-out in dependency order: {emitted:?}"
        );

        // The merge (`tail`) is emitted exactly once, by the Conditional.
        let step_ids: Vec<&str> = emitted
            .iter()
            .map(String::as_str)
            .filter(|entry| !entry.starts_with("  ") && !entry.starts_with("Conditional:"))
            .map(|entry| entry.strip_prefix("Finish:").unwrap_or(entry))
            .filter(|entry| *entry != "Join")
            .collect();
        let mut deduped = step_ids.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            step_ids.len(),
            "every step should be emitted exactly once: {emitted:?}"
        );
    }

    /// A clean single-Agent diamond `a → {b, c} → m → finish` lowers to
    /// concurrent `ParallelBranches` (docs/wasip3-parallel-branches-plan.md 4a):
    /// the two branch agents form the window and the merge `m` runs once after.
    #[test]
    fn single_agent_diamond_lowers_to_parallel_branches() {
        let mut steps = serde_json::Map::new();
        for id in ["a", "b", "c", "m"] {
            steps.insert(id.to_string(), agent_step_json(id));
        }
        steps.insert(
            "finish".to_string(),
            serde_json::json!({
                "id": "finish",
                "stepType": "Finish",
                "inputMapping": {"out": {"value": "ok", "valueType": "immediate"}}
            }),
        );
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_value(serde_json::json!({
            "entryPoint": "a",
            // 4a.1 parallelizes non-durable branches; durable is 4a.2.
            "durable": false,
            "steps": steps,
            "executionPlan": [
                {"fromStep": "a", "toStep": "b"},
                {"fromStep": "a", "toStep": "c"},
                {"fromStep": "b", "toStep": "m"},
                {"fromStep": "c", "toStep": "m"},
                {"fromStep": "m", "toStep": "finish"}
            ]
        }))
        .expect("graph parses");

        let manifest =
            super::super::manifest::build_direct_workflow_manifest(&graph).expect("build manifest");
        let plan = direct_run_plan(&manifest).expect("build plan");
        let mut emitted = Vec::new();
        collect_plan_steps(&plan, &mut emitted);

        assert_eq!(
            emitted,
            vec![
                "a",
                "ParallelBranches",
                "  branch:",
                "b",
                "Join",
                "  branch:",
                "c",
                "Join",
                "  merge-of:ParallelBranches",
                "m",
                "Finish:finish",
            ],
            "clean single-agent diamond should lower to ParallelBranches: {emitted:?}"
        );
    }

    fn direct_agent_manifest_with_retry_defaults(
        rate_limited: bool,
        max_retries: Option<u32>,
        retry_delay: Option<u64>,
    ) -> DirectAgentManifest {
        DirectAgentManifest {
            id: 0,
            step_id: "agent".to_string(),
            name: None,
            step_type: "Agent".to_string(),
            purpose: "agent.config".to_string(),
            agent_id: "utils".to_string(),
            capability_id: "normalize".to_string(),
            connection_id: None,
            connection_ref: None,
            durable: true,
            rate_limited,
            is_workflow_agent: false,
            input_mapping_id: 0,
            required_inputs: vec![],
            max_retries,
            retry_delay,
            timeout: None,
        }
    }

    fn direct_embed_step_manifest(
        max_retries: Option<u32>,
        retry_delay: Option<u64>,
    ) -> DirectStepManifest {
        let mut body = serde_json::json!({
            "stepType": "EmbedWorkflow",
            "id": "call_child",
            "childWorkflowId": "child_workflow",
            "childVersion": "latest"
        });
        if let Some(max_retries) = max_retries {
            body["maxRetries"] = serde_json::json!(max_retries);
        }
        if let Some(retry_delay) = retry_delay {
            body["retryDelay"] = serde_json::json!(retry_delay);
        }

        DirectStepManifest {
            id: "call_child".to_string(),
            step_type: "EmbedWorkflow".to_string(),
            name: None,
            body,
            nested_graphs: vec![],
        }
    }

    #[test]
    fn direct_agent_effective_retry_policy_matches_generated_defaults() {
        assert_eq!(
            agent_effective_max_retries(&direct_agent_manifest_with_retry_defaults(
                false, None, None,
            )),
            3
        );
        assert_eq!(
            agent_effective_retry_delay_ms(&direct_agent_manifest_with_retry_defaults(
                false, None, None,
            )),
            1_000
        );
        assert_eq!(
            agent_effective_max_retries(&direct_agent_manifest_with_retry_defaults(
                true, None, None,
            )),
            5
        );
        assert_eq!(
            agent_effective_retry_delay_ms(&direct_agent_manifest_with_retry_defaults(
                true, None, None,
            )),
            2_000
        );
        assert_eq!(
            agent_effective_max_retries(&direct_agent_manifest_with_retry_defaults(
                true,
                Some(2),
                Some(750),
            )),
            2
        );
        assert_eq!(
            agent_effective_retry_delay_ms(&direct_agent_manifest_with_retry_defaults(
                true,
                Some(2),
                Some(750),
            )),
            750
        );
    }

    #[test]
    fn direct_embed_workflow_effective_retry_policy_matches_generated_defaults() {
        let defaults = direct_embed_step_manifest(None, None);
        assert_eq!(embed_workflow_effective_max_retries(&defaults), 3);
        assert_eq!(embed_workflow_effective_retry_delay_ms(&defaults), 1_000);

        let no_retry = direct_embed_step_manifest(Some(0), Some(0));
        assert_eq!(embed_workflow_effective_max_retries(&no_retry), 0);
        assert_eq!(embed_workflow_effective_retry_delay_ms(&no_retry), 0);

        let custom = direct_embed_step_manifest(Some(2), Some(250));
        assert_eq!(embed_workflow_effective_max_retries(&custom), 2);
        assert_eq!(embed_workflow_effective_retry_delay_ms(&custom), 250);
    }
}
