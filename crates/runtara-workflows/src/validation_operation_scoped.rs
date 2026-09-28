// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where operation-scoped steps (suspending or control Agent steps and
//! WaitForInstances steps) may appear.
//!
//! The verdicts come from `runtara_dsl::step_context_rules`; this module only
//! works out which contexts a step is in. [`validate_suspending_steps`] judges
//! every Agent step of one graph (loops, `onWait` and nested graphs included);
//! [`validate_operation_scoped_embed_sites`] judges each EmbedWorkflow call
//! site of a closure against what its embedded workflows call.
//!
//! Durability follows the direct manifest: a graph is durable unless it (or,
//! for a nested graph, its enclosing graph) says `durable: false`, a step
//! unless it or its graph does, and an embedded workflow by its own flag.

use std::collections::{BTreeSet, HashMap, HashSet};

use runtara_dsl::agent_meta::{AgentCatalog, CONTROL_AGENT_ID, canonical_agent_id};
use runtara_dsl::step_context_rules::{
    CONTROL_START_CAPABILITY_ID, CONTROL_START_RUN_LABEL_FIELD, CONTROL_START_WORKFLOW_ID_FIELD,
    ContextVerdict, OperationScopedKind, SUSPEND_DEADLINE_MARGIN_MS, StepContext,
    step_context_rule,
};
use runtara_dsl::{AgentStep, EmbedWorkflowStep, ExecutionGraph, MappingValue, Step};

use super::{ValidationError, ValidationResult, ValidationWarning, reachable_over_normal_flow};

/// What encloses a graph: inherited by every step in it.
#[derive(Debug, Clone, Default)]
struct EnclosingScope {
    /// Innermost enclosing Split or While.
    in_loop: Option<String>,
    /// Innermost parallel window: the rule and the Split or fan-out step.
    parallel: Option<(StepContext, String)>,
    /// Innermost enclosing Split that retries its whole body.
    enclosing_retry: Option<String>,
    on_error: bool,
    on_wait: bool,
}

/// Every context one step is in.
#[derive(Debug, Clone)]
struct SiteContext {
    /// Effective durability of the graph holding the step.
    graph_durable: bool,
    in_loop: Option<String>,
    parallel: Option<(StepContext, String)>,
    enclosing_retry: Option<String>,
    on_error: bool,
    on_wait: bool,
    /// Set when an AiAgent edge targets the step.
    ai_role: Option<StepContext>,
}

/// Visit every step of `graph` and its nested graphs with its contexts.
fn walk_sites(
    graph: &ExecutionGraph,
    inherited_durable: bool,
    scope: &EnclosingScope,
    visit: &mut dyn FnMut(&str, &Step, &SiteContext),
) {
    let graph_durable = graph.durable.unwrap_or(inherited_durable);
    let on_error_region = on_error_region(graph);
    let branch_groups = parallel_branch_groups(graph);
    let ai_roles = ai_agent_roles(graph);
    let mut step_ids: Vec<&String> = graph.steps.keys().collect();
    step_ids.sort();
    for step_id in step_ids {
        let step = &graph.steps[step_id];
        let context = SiteContext {
            graph_durable,
            in_loop: scope.in_loop.clone(),
            parallel: scope.parallel.clone().or_else(|| {
                branch_groups
                    .get(step_id.as_str())
                    .map(|owner| (StepContext::ParallelBranchGroup, owner.clone()))
            }),
            enclosing_retry: scope.enclosing_retry.clone(),
            on_error: scope.on_error || on_error_region.contains(step_id.as_str()),
            on_wait: scope.on_wait,
            ai_role: ai_roles.get(step_id.as_str()).copied(),
        };
        visit(step_id, step, &context);
        let nested = |parallel, enclosing_retry, on_wait| EnclosingScope {
            in_loop: Some(step_id.clone()),
            parallel,
            enclosing_retry,
            on_error: context.on_error,
            on_wait,
        };
        match step {
            Step::Split(split) => {
                let config = split.config.as_ref();
                let parallel = config
                    .and_then(|config| config.parallelism)
                    .filter(|parallelism| *parallelism != 1)
                    .map(|_| (StepContext::ParallelSplit, step_id.clone()))
                    .or_else(|| context.parallel.clone());
                let retry = config
                    .and_then(|config| config.max_retries)
                    .filter(|retries| *retries > 0)
                    .map(|_| step_id.clone())
                    .or_else(|| context.enclosing_retry.clone());
                let scope = nested(parallel, retry, context.on_wait);
                walk_sites(&split.subgraph, graph_durable, &scope, visit);
            }
            Step::While(while_step) => {
                let scope = nested(
                    context.parallel.clone(),
                    context.enclosing_retry.clone(),
                    context.on_wait,
                );
                walk_sites(&while_step.subgraph, graph_durable, &scope, visit);
            }
            Step::WaitForSignal(wait) => {
                if let Some(on_wait) = &wait.on_wait {
                    let scope = EnclosingScope {
                        in_loop: scope.in_loop.clone(),
                        parallel: context.parallel.clone(),
                        enclosing_retry: context.enclosing_retry.clone(),
                        on_error: context.on_error,
                        on_wait: true,
                    };
                    walk_sites(on_wait, graph_durable, &scope, visit);
                }
            }
            _ => {}
        }
    }
}

/// Steps reached only through an `onError` edge: reachable from an onError
/// target, but not over normal flow from the entry point. A step where the
/// handler rejoins the normal path is outside the region.
fn on_error_region(graph: &ExecutionGraph) -> HashSet<String> {
    let normal = if graph.steps.contains_key(&graph.entry_point) {
        reachable_over_normal_flow(graph, &graph.entry_point)
    } else {
        HashSet::new()
    };
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.execution_plan {
        adjacency
            .entry(edge.from_step.as_str())
            .or_default()
            .push(edge.to_step.as_str());
    }
    let mut region = HashSet::new();
    let mut stack: Vec<&str> = graph
        .execution_plan
        .iter()
        .filter(|edge| edge.label.as_deref() == Some("onError"))
        .map(|edge| edge.to_step.as_str())
        .collect();
    let mut seen = HashSet::new();
    while let Some(step) = stack.pop() {
        if !seen.insert(step) {
            continue;
        }
        if !normal.contains(step) {
            region.insert(step.to_string());
        }
        stack.extend(adjacency.get(step).into_iter().flatten().copied());
    }
    region
}

/// Steps on a branch of an unconditioned fan-out (2+ unlabeled, condition-less
/// successors), mapped to the fan-out step. The merge point and what follows
/// it are outside the group.
fn parallel_branch_groups(graph: &ExecutionGraph) -> HashMap<String, String> {
    let mut by_from: HashMap<&str, (Vec<&str>, bool)> = HashMap::new();
    for edge in &graph.execution_plan {
        if !matches!(edge.label.as_deref(), None | Some("next")) {
            continue;
        }
        let entry = by_from.entry(edge.from_step.as_str()).or_default();
        if edge.condition.is_some() {
            entry.1 = true;
        } else if !entry.0.contains(&edge.to_step.as_str()) {
            entry.0.push(edge.to_step.as_str());
        }
    }
    let mut from_steps: Vec<_> = by_from.into_iter().collect();
    from_steps.sort_by_key(|(from, _)| *from);
    let mut groups = HashMap::new();
    for (from, (targets, conditioned)) in from_steps {
        if conditioned || targets.len() < 2 {
            continue;
        }
        let reachable: Vec<HashSet<String>> = targets
            .iter()
            .map(|start| reachable_over_normal_flow(graph, start))
            .collect();
        let merge: HashSet<&String> = reachable[0]
            .iter()
            .filter(|step| reachable[1..].iter().all(|set| set.contains(*step)))
            .collect();
        for set in &reachable {
            for step in set.iter().filter(|step| !merge.contains(step)) {
                groups
                    .entry(step.clone())
                    .or_insert_with(|| from.to_string());
            }
        }
    }
    groups
}

/// Targets of AiAgent tool (and MCP) edges and of `memory` edges.
fn ai_agent_roles(graph: &ExecutionGraph) -> HashMap<&str, StepContext> {
    let mut roles = HashMap::new();
    for edge in &graph.execution_plan {
        if !matches!(graph.steps.get(&edge.from_step), Some(Step::AiAgent(_))) {
            continue;
        }
        let role = match edge.label.as_deref() {
            None | Some("next") | Some("onError") => continue,
            Some("memory") => StepContext::AiAgentMemory,
            Some(_) => StepContext::AiAgentTool,
        };
        roles.insert(edge.to_step.as_str(), role);
    }
    roles
}

/// The operation-scoped call a diagnostic is about.
struct Site<'a> {
    step_id: &'a str,
    child_workflow_id: Option<&'a str>,
    /// The Agent step's timeout, for W078.
    timeout_ms: Option<u64>,
}

/// Push the diagnostic `context`'s rule prescribes for each `(kind,
/// capability)` present. Errors are per kind; a warning is pushed once.
fn apply_rule(
    context: StepContext,
    kinds: &[(OperationScopedKind, String)],
    site: &Site<'_>,
    owner: Option<&str>,
    result: &mut ValidationResult,
) {
    let rule = step_context_rule(context);
    let mut warned = false;
    for (kind, capability) in kinds {
        let step_id = site.step_id.to_string();
        let child_workflow_id = site.child_workflow_id.map(str::to_string);
        let owner = owner.unwrap_or(site.step_id).to_string();
        match rule.verdict(*kind) {
            ContextVerdict::Rejected { code } => result.errors.push(match code {
                "E028" => ValidationError::SuspendingCapabilityNotDurable {
                    step_id,
                    capability: capability.clone(),
                    child_workflow_id,
                },
                "E029" => ValidationError::SuspendingCapabilityMissingTimeout {
                    step_id,
                    capability: capability.clone(),
                },
                "E131" => ValidationError::SuspendingCapabilityUnsupportedContext {
                    step_id,
                    capability: capability.clone(),
                    context: rule.key.to_string(),
                    child_workflow_id,
                },
                "E132" => ValidationError::ControlCapabilityUnsupportedContext {
                    step_id,
                    capability: capability.clone(),
                    context: rule.key.to_string(),
                    child_workflow_id,
                },
                other => unreachable!("rule {} rejects with unknown code {other}", rule.key),
            }),
            ContextVerdict::Serialized { .. } | ContextVerdict::Warned { .. } if warned => {}
            ContextVerdict::Serialized { code } | ContextVerdict::Warned { code } => {
                warned = true;
                result.warnings.push(match code {
                    "W075" => ValidationWarning::SerializedOperationScopedStep {
                        step_id,
                        context: rule.key.to_string(),
                        owner_step_id: owner,
                    },
                    "W076" => ValidationWarning::OperationScopedStepUnderEnclosingRetry {
                        step_id,
                        retry_step_id: owner,
                        child_workflow_id,
                    },
                    "W074" => ValidationWarning::ConstantRunLabelInLoop {
                        step_id,
                        loop_step_id: owner,
                    },
                    "W077" => ValidationWarning::DynamicControlStartTarget { step_id },
                    "W078" => ValidationWarning::WaitTimeoutBelowDeadlineMargin {
                        step_id,
                        timeout_ms: site.timeout_ms.unwrap_or_default(),
                        margin_ms: SUSPEND_DEADLINE_MARGIN_MS,
                    },
                    other => unreachable!("rule {} warns with unknown code {other}", rule.key),
                });
            }
            ContextVerdict::Allowed
            | ContextVerdict::PublishRefused { .. }
            | ContextVerdict::NotApplicable => {}
        }
    }
}

/// Contexts shared by an Agent site and an EmbedWorkflow call site.
fn apply_shared_contexts(
    kinds: &[(OperationScopedKind, String)],
    site: &Site<'_>,
    context: &SiteContext,
    site_durable: bool,
    enclosing_retry: Option<&str>,
    result: &mut ValidationResult,
) {
    if let Some(role) = context.ai_role {
        apply_rule(role, kinds, site, None, result);
    }
    if context.on_wait {
        apply_rule(StepContext::OnWait, kinds, site, None, result);
    }
    if context.on_error {
        apply_rule(StepContext::OnErrorRegion, kinds, site, None, result);
    }
    if !site_durable {
        apply_rule(StepContext::NonDurable, kinds, site, None, result);
    }
    if let Some((parallel, owner)) = &context.parallel {
        apply_rule(*parallel, kinds, site, Some(owner), result);
    }
    if let Some(owner) = enclosing_retry {
        apply_rule(
            StepContext::EnclosingRetry,
            kinds,
            site,
            Some(owner),
            result,
        );
    }
}

/// The diagnostic label of a WaitForInstances site, in the place of an
/// `agent:capability`.
const WAIT_FOR_INSTANCES_LABEL: &str = "WaitForInstances";

/// E028, E029, E131, E132 and W074-W078 for every Agent and WaitForInstances
/// step of `graph`.
pub(super) fn validate_suspending_steps(
    graph: &ExecutionGraph,
    catalog: &AgentCatalog,
    result: &mut ValidationResult,
) {
    walk_sites(
        graph,
        true,
        &EnclosingScope::default(),
        &mut |step_id, step, context| {
            if let Step::WaitForInstances(_) = step {
                let kinds = [(
                    OperationScopedKind::WaitForInstances,
                    WAIT_FOR_INSTANCES_LABEL.to_string(),
                )];
                let site = Site {
                    step_id,
                    child_workflow_id: None,
                    timeout_ms: None,
                };
                apply_shared_contexts(
                    &kinds,
                    &site,
                    context,
                    context.graph_durable,
                    context.enclosing_retry.as_deref(),
                    result,
                );
                return;
            }
            let Step::Agent(agent) = step else {
                return;
            };
            let Some(kind) =
                OperationScopedKind::classify(catalog, &agent.agent_id, &agent.capability_id)
            else {
                return;
            };
            let kinds = [(kind, format!("{}:{}", agent.agent_id, agent.capability_id))];
            let site = Site {
                step_id,
                child_workflow_id: None,
                timeout_ms: agent.timeout,
            };
            let site_durable = context.graph_durable && agent.durable.unwrap_or(true);
            apply_shared_contexts(
                &kinds,
                &site,
                context,
                site_durable,
                context.enclosing_retry.as_deref(),
                result,
            );
            match agent.timeout {
                None | Some(0) => {
                    apply_rule(StepContext::MissingTimeout, &kinds, &site, None, result)
                }
                Some(timeout) if timeout <= SUSPEND_DEADLINE_MARGIN_MS => apply_rule(
                    StepContext::TimeoutBelowDeadlineMargin,
                    &kinds,
                    &site,
                    None,
                    result,
                ),
                Some(_) => {}
            }
            if is_control_start(agent) {
                if let Some(loop_step) = &context.in_loop
                    && input(agent, CONTROL_START_RUN_LABEL_FIELD).is_some_and(is_constant)
                {
                    apply_rule(
                        StepContext::ConstantRunLabelInLoop,
                        &kinds,
                        &site,
                        Some(loop_step),
                        result,
                    );
                }
                if input(agent, CONTROL_START_WORKFLOW_ID_FIELD)
                    .is_some_and(|target| !matches!(target, MappingValue::Immediate(_)))
                {
                    apply_rule(StepContext::DynamicStartTarget, &kinds, &site, None, result);
                }
            }
        },
    );
}

fn is_control_start(agent: &AgentStep) -> bool {
    canonical_agent_id(&agent.agent_id) == CONTROL_AGENT_ID
        && agent.capability_id == CONTROL_START_CAPABILITY_ID
}

fn input<'a>(agent: &'a AgentStep, field: &str) -> Option<&'a MappingValue> {
    agent.input_mapping.as_ref()?.get(field)
}

/// A literal, or a template with nothing to interpolate.
fn is_constant(value: &MappingValue) -> bool {
    match value {
        MappingValue::Immediate(_) => true,
        MappingValue::Template(template) => {
            !template.value.contains("{{") && !template.value.contains("{%")
        }
        _ => false,
    }
}

/// The first suspending and the first control call anywhere in a workflow's
/// closure (nested graphs and deeper embeds included), as `agent:capability`,
/// and whether it holds a WaitForInstances step.
#[derive(Debug, Default)]
struct ClosureOperationSites {
    suspending: Option<String>,
    control: Option<String>,
    wait_for_instances: bool,
}

impl ClosureOperationSites {
    fn kinds(&self) -> Vec<(OperationScopedKind, String)> {
        let mut kinds = Vec::new();
        if let Some(capability) = &self.suspending {
            kinds.push((OperationScopedKind::Suspending, capability.clone()));
        }
        if let Some(capability) = &self.control {
            kinds.push((OperationScopedKind::Control, capability.clone()));
        }
        if self.wait_for_instances {
            kinds.push((
                OperationScopedKind::WaitForInstances,
                WAIT_FOR_INSTANCES_LABEL.to_string(),
            ));
        }
        kinds
    }
}

fn collect_closure_sites(
    workflow_id: &str,
    children: &HashMap<String, ExecutionGraph>,
    catalog: &AgentCatalog,
    visited: &mut BTreeSet<String>,
    sites: &mut ClosureOperationSites,
) {
    if !visited.insert(workflow_id.to_string()) {
        return;
    }
    let Some(graph) = children.get(workflow_id) else {
        return;
    };
    let mut embedded = Vec::new();
    walk_sites(
        graph,
        true,
        &EnclosingScope::default(),
        &mut |_, step, _| match step {
            Step::Agent(agent) => {
                let capability = || format!("{}:{}", agent.agent_id, agent.capability_id);
                match OperationScopedKind::classify(catalog, &agent.agent_id, &agent.capability_id)
                {
                    Some(OperationScopedKind::Suspending) if sites.suspending.is_none() => {
                        sites.suspending = Some(capability());
                    }
                    Some(OperationScopedKind::Control) if sites.control.is_none() => {
                        sites.control = Some(capability());
                    }
                    _ => {}
                }
            }
            Step::WaitForInstances(_) => sites.wait_for_instances = true,
            Step::EmbedWorkflow(embed) => embedded.push(embed.child_workflow_id.clone()),
            _ => {}
        },
    );
    for child in embedded {
        collect_closure_sites(&child, children, catalog, visited, sites);
    }
}

/// Judge every EmbedWorkflow call site of `graph` against the operation-scoped
/// calls in the workflow it embeds: child durability (E028), an embed used as
/// an AiAgent tool or in `onWait` (E131/E132), in an onError region (E131),
/// serialized windows (W075) and retries around it (W076). `children` holds
/// every workflow of the closure by id.
pub(super) fn validate_operation_scoped_embed_sites(
    graph: &ExecutionGraph,
    children: &HashMap<String, ExecutionGraph>,
    catalog: &AgentCatalog,
    result: &mut ValidationResult,
) {
    walk_sites(
        graph,
        true,
        &EnclosingScope::default(),
        &mut |step_id, step, context| {
            let Step::EmbedWorkflow(embed) = step else {
                return;
            };
            let mut sites = ClosureOperationSites::default();
            collect_closure_sites(
                &embed.child_workflow_id,
                children,
                catalog,
                &mut BTreeSet::new(),
                &mut sites,
            );
            let kinds = sites.kinds();
            if kinds.is_empty() {
                return;
            }
            let site = Site {
                step_id,
                child_workflow_id: Some(&embed.child_workflow_id),
                timeout_ms: None,
            };
            apply_shared_contexts(
                &kinds,
                &site,
                context,
                embed_site_durable(embed, context),
                embed_retry(embed, step_id, context),
                result,
            );
        },
    );
}

fn embed_site_durable(embed: &EmbedWorkflowStep, context: &SiteContext) -> bool {
    context.graph_durable && embed.durable.unwrap_or(true)
}

/// The embed's own retries (default 3) re-run the whole child, else an
/// enclosing retrying Split.
fn embed_retry<'a>(
    embed: &EmbedWorkflowStep,
    step_id: &'a str,
    context: &'a SiteContext,
) -> Option<&'a str> {
    if embed.max_retries.unwrap_or(3) > 0 {
        Some(step_id)
    } else {
        context.enclosing_retry.as_deref()
    }
}

#[cfg(test)]
#[path = "validation_operation_scoped_tests.rs"]
mod tests;
