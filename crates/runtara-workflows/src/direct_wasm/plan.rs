// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct core run-plan model and construction used by the Wasm emitter.

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
    },
    Filter {
        step_id: String,
        filter_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchValue {
        step_id: String,
        switch_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    SwitchRoute {
        step_id: String,
        switch_id: u32,
        branches: Vec<DirectSwitchRoutePlan>,
        default_plan: Box<DirectRunPlan>,
    },
    EdgeRoute {
        branches: Vec<DirectEdgeConditionPlan>,
        default_plan: Box<DirectRunPlan>,
    },
    GroupBy {
        step_id: String,
        group_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    Split {
        step_id: String,
        split_id: u32,
        durable: bool,
        dont_stop_on_failed: bool,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
    },
    While {
        step_id: String,
        while_id: u32,
        nested_plan: Box<DirectRunPlan>,
        next_plan: Box<DirectRunPlan>,
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
        next_plan: Box<DirectRunPlan>,
    },
    WaitForSignal {
        step_id: String,
        breakpoint: bool,
        on_wait_plan: Option<Box<DirectRunPlan>>,
        next_plan: Box<DirectRunPlan>,
    },
    Log {
        log_id: u32,
        next_plan: Box<DirectRunPlan>,
    },
    Agent {
        step_id: String,
        agent_id: u32,
        agent_component_id: String,
        input_mapping_id: u32,
        durable_checkpoint: bool,
        max_retries: u32,
        retry_delay_ms: u64,
        rate_limit_budget_ms: u64,
        next_plan: Box<DirectRunPlan>,
        error_plan: Option<DirectErrorRoutePlan>,
    },
    Error {
        step_id: String,
        error_id: u32,
    },
    Conditional {
        step_id: String,
        condition_id: u32,
        true_plan: Box<DirectRunPlan>,
        false_plan: Box<DirectRunPlan>,
    },
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
    WaitOnWait {
        step_id_offset: i32,
        step_id_len: i32,
    },
    EmbedWorkflow {
        branch_depth: u32,
    },
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
        | "EmbedWorkflow" | "WaitForSignal" | "Log" | "Agent" | "Error" | "Conditional" => {
            step_run_plan(
                &manifest.graph,
                &manifest.child_workflows,
                &manifest.graph.entry_point,
                &mut Vec::new(),
            )
        }
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
    step_run_plan_inner(graph, child_workflows, step_id, stack, true)
}

fn step_run_plan_without_on_error(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    step_id: &str,
    stack: &mut Vec<String>,
) -> Result<DirectRunPlan, DirectCompileError> {
    step_run_plan_inner(graph, child_workflows, step_id, stack, false)
}

fn step_run_plan_inner(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    step_id: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
) -> Result<DirectRunPlan, DirectCompileError> {
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
        }),
        "Filter" => {
            let filter_id = filter_id(graph, step_id)?;
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Filter {
                step_id: step_id.to_string(),
                filter_id,
                next_plan: Box::new(next_plan),
            })
        }
        "Switch" => {
            let switch_id = switch_id(graph, step_id)?;
            if switch_is_routing(graph, step_id)? {
                let route_labels = switch_route_labels(graph, step_id)?;
                let mut branches = Vec::new();

                stack.push(step_id.to_string());
                for label in route_labels {
                    let target = branch_target(graph, step_id, &label)?.to_string();
                    let plan = step_run_plan_inner(
                        graph,
                        child_workflows,
                        &target,
                        stack,
                        include_on_error,
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
                )?;
                stack.pop();

                Ok(DirectRunPlan::SwitchRoute {
                    step_id: step_id.to_string(),
                    switch_id,
                    branches,
                    default_plan: Box::new(default_plan),
                })
            } else {
                let next_plan =
                    normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

                Ok(DirectRunPlan::SwitchValue {
                    step_id: step_id.to_string(),
                    switch_id,
                    next_plan: Box::new(next_plan),
                })
            }
        }
        "GroupBy" => {
            let group_id = group_by_id(graph, step_id)?;
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::GroupBy {
                step_id: step_id.to_string(),
                group_id,
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
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Split {
                step_id: step_id.to_string(),
                split_id: split.id,
                durable: split.durable,
                dont_stop_on_failed,
                nested_plan: Box::new(nested_plan),
                next_plan: Box::new(next_plan),
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
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::While {
                step_id: step_id.to_string(),
                while_id,
                nested_plan: Box::new(nested_plan),
                next_plan: Box::new(next_plan),
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
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;
            let error_plan = if include_on_error {
                on_error_plan(graph, child_workflows, step_id, stack)?
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
                breakpoint: graph.durable
                    && step
                        .body
                        .get("breakpoint")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                max_retries: embed_workflow_effective_max_retries(step),
                retry_delay_ms: embed_workflow_effective_retry_delay_ms(step),
                child_plan: Box::new(child_plan),
                next_plan: Box::new(next_plan),
                error_plan,
            })
        }
        "Delay" => {
            let delay = delay_config(graph, step_id)?;
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Delay {
                step_id: step_id.to_string(),
                delay_id: delay.id,
                durable: delay.durable,
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
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::WaitForSignal {
                step_id: step_id.to_string(),
                breakpoint: graph.durable
                    && step
                        .body
                        .get("breakpoint")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                on_wait_plan: on_wait_plan.map(Box::new),
                next_plan: Box::new(next_plan),
            })
        }
        "Log" => {
            let log_id = log_id(graph, step_id)?;
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;

            Ok(DirectRunPlan::Log {
                log_id,
                next_plan: Box::new(next_plan),
            })
        }
        "Agent" => {
            let agent = agent_config(graph, step_id)?;
            let durable_checkpoint = agent.durable;
            let max_retries = agent_effective_max_retries(agent);
            let retry_delay_ms = agent_effective_retry_delay_ms(agent);
            let rate_limit_budget_ms = graph.rate_limit_budget_ms;
            let next_plan =
                normal_flow_plan(graph, child_workflows, step_id, stack, include_on_error)?;
            let error_plan = if include_on_error {
                on_error_plan(graph, child_workflows, step_id, stack)?
            } else {
                None
            };

            Ok(DirectRunPlan::Agent {
                step_id: step_id.to_string(),
                agent_id: agent.id,
                agent_component_id: canonicalize_direct_agent_id(&agent.agent_id),
                input_mapping_id: agent.input_mapping_id,
                durable_checkpoint,
                max_retries,
                retry_delay_ms,
                rate_limit_budget_ms,
                next_plan: Box::new(next_plan),
                error_plan,
            })
        }
        "Error" => Ok(DirectRunPlan::Error {
            step_id: step_id.to_string(),
            error_id: error_id(graph, step_id)?,
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

            stack.push(step_id.to_string());
            let true_plan =
                step_run_plan_inner(graph, child_workflows, &true_step, stack, include_on_error)?;
            let false_plan =
                step_run_plan_inner(graph, child_workflows, &false_step, stack, include_on_error)?;
            stack.pop();

            Ok(DirectRunPlan::Conditional {
                step_id: step_id.to_string(),
                condition_id,
                true_plan: Box::new(true_plan),
                false_plan: Box::new(false_plan),
            })
        }
        other => Err(DirectCompileError::Component(format!(
            "direct run plan does not support step '{step_id}' with type '{other}'"
        ))),
    }
}

fn normal_flow_plan(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    from_step: &str,
    stack: &mut Vec<String>,
    include_on_error: bool,
) -> Result<DirectRunPlan, DirectCompileError> {
    let edges = normal_flow_edges(graph, from_step);
    if edges.is_empty() {
        return Err(DirectCompileError::Component(format!(
            "missing normal branch for direct step '{from_step}'"
        )));
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

    if conditional_edges.is_empty() {
        let [edge] = default_edges.as_slice() else {
            return Err(DirectCompileError::Component(format!(
                "direct step '{from_step}' has unsupported parallel normal branches"
            )));
        };
        stack.push(from_step.to_string());
        let next_plan = step_run_plan_inner(
            graph,
            child_workflows,
            &edge.to_step,
            stack,
            include_on_error,
        )?;
        stack.pop();
        return Ok(next_plan);
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
    )?;
    stack.pop();

    Ok(DirectRunPlan::EdgeRoute {
        branches,
        default_plan: Box::new(default_plan),
    })
}

fn on_error_plan(
    graph: &DirectGraphManifest,
    child_workflows: &[DirectChildWorkflowGraphManifest],
    from_step: &str,
    stack: &mut Vec<String>,
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
            let plan =
                step_run_plan_without_on_error(graph, child_workflows, &edge.to_step, stack)?;
            Ok(DirectEdgeConditionPlan {
                condition_id,
                plan: Box::new(plan),
            })
        })
        .collect::<Result<Vec<_>, DirectCompileError>>()?;
    let default_plan = default_edge
        .map(|edge| step_run_plan_without_on_error(graph, child_workflows, &edge.to_step, stack))
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

fn split_dont_stop_on_failed(
    graph: &DirectGraphManifest,
    step_id: &str,
) -> Result<bool, DirectCompileError> {
    Ok(split_config(graph, step_id)?
        .get("dontStopOnFailed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
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
    if !graph
        .steps
        .iter()
        .any(|step| step.id == step_id && step.step_type == "Agent")
    {
        return Err(DirectCompileError::Component(format!(
            "direct step '{step_id}' is not an Agent step"
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
            durable: true,
            rate_limited,
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
