// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keep the DSL's step map keys and the emitter's inner IDs identical.

use runtara_dsl::{ExecutionGraph, Step};

pub(crate) struct IdentityMismatch {
    pub graph_path: String,
    pub step_key: String,
    pub step_id: String,
}

pub(crate) fn identity_message(graph_path: &str, step_key: &str, step_id: &str) -> String {
    let graph = if graph_path.is_empty() {
        "<root>"
    } else {
        graph_path
    };
    format!(
        "Graph {graph:?}: step map key {step_key:?} differs from step.id {step_id:?}; set step.id to the map key"
    )
}

/// Paths are JSON pointers into the graph (or the caller's child input bundle).
/// Check declarations independently of reachability and entry-point validity.
pub(crate) fn identity_mismatches(
    graph: &ExecutionGraph,
    graph_path: &str,
) -> Vec<IdentityMismatch> {
    fn collect(graph: &ExecutionGraph, path: &str, errors: &mut Vec<IdentityMismatch>) {
        for (key, step) in &graph.steps {
            if key != step_id(step) {
                errors.push(IdentityMismatch {
                    graph_path: path.to_owned(),
                    step_key: key.clone(),
                    step_id: step_id(step).to_owned(),
                });
            }
            let nested = match step {
                Step::Split(step) => Some(("subgraph", &step.subgraph)),
                Step::While(step) => Some(("subgraph", &step.subgraph)),
                Step::WaitForSignal(step) => step.on_wait.as_ref().map(|graph| ("onWait", graph)),
                _ => None,
            };
            if let Some((field, nested)) = nested {
                let key = key.replace('~', "~0").replace('/', "~1");
                collect(nested, &format!("{path}/steps/{key}/{field}"), errors);
            }
        }
    }
    let mut errors = Vec::new();
    collect(graph, graph_path, &mut errors);
    errors.sort_by(|left, right| {
        (&left.graph_path, &left.step_key).cmp(&(&right.graph_path, &right.step_key))
    });
    errors
}

pub(crate) fn step_id(step: &Step) -> &str {
    match step {
        Step::Finish(step) => &step.id,
        Step::Agent(step) => &step.id,
        Step::Conditional(step) => &step.id,
        Step::Split(step) => &step.id,
        Step::Switch(step) => &step.id,
        Step::EmbedWorkflow(step) => &step.id,
        Step::While(step) => &step.id,
        Step::Log(step) => &step.id,
        Step::Error(step) => &step.id,
        Step::Filter(step) => &step.id,
        Step::GroupBy(step) => &step.id,
        Step::Delay(step) => &step.id,
        Step::WaitForSignal(step) => &step.id,
        Step::AiAgent(step) => &step.id,
    }
}
