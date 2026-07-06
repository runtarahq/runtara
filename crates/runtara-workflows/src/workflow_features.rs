// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow feature analysis used by direct WebAssembly emitter migration.
//!
//! The direct emitter needs a deterministic description of each workflow before
//! it can decide whether to emit, reject, or route to the Rust compiler
//! fallback. This module intentionally performs no validation and no codegen;
//! it only summarizes already-parsed DSL graphs.

use std::collections::{BTreeMap, BTreeSet};

use runtara_dsl::{ChildVersion, ExecutionGraph, Step};

/// A normalized child workflow reference found in an `EmbedWorkflow` step.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildWorkflowReference {
    /// Step id of the `EmbedWorkflow` call site.
    pub step_id: String,
    /// Referenced child workflow id.
    pub workflow_id: String,
    /// Requested child workflow version as it appeared in the DSL.
    pub version: String,
}

/// Coarse workflow features that matter for direct-emitter support decisions.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowFeature {
    /// A workflow step invokes an agent capability.
    AgentCall,
    /// A workflow step runs an AI-agent loop.
    AiAgent,
    /// A workflow step invokes a child workflow.
    ChildWorkflow,
    /// A `Conditional` step chooses a true/false branch.
    ConditionalBranch,
    /// A `Switch` step performs multi-way routing.
    SwitchBranch,
    /// A `Split` step contains a nested per-item subgraph.
    SplitSubgraph,
    /// A `While` step contains a nested loop body subgraph.
    WhileLoop,
    /// A DSL condition expression is present.
    ConditionExpression,
    /// An execution-plan edge carries a condition.
    EdgeCondition,
    /// An execution-plan edge routes failures through `onError`.
    ErrorHandlerEdge,
    /// One step fans out through multiple unlabeled condition-less edges.
    ParallelEdges,
    /// A workflow step emits a log/debug event.
    LogEvent,
    /// A workflow step emits a terminal structured error.
    ExplicitError,
    /// A workflow step filters an array.
    Filter,
    /// A workflow step groups an array.
    GroupBy,
    /// A workflow step sleeps or delays.
    Delay,
    /// A workflow step waits for an external signal.
    WaitForSignal,
    /// A workflow step can suspend and resume later.
    SuspendResume,
    /// A workflow step references a connection id.
    Connection,
    /// A workflow step or graph needs checkpoint/durability semantics.
    Durability,
    /// A workflow step declares retry configuration.
    RetryPolicy,
    /// A workflow step declares timeout configuration.
    Timeout,
    /// A workflow step declares compensation behavior.
    Compensation,
    /// A workflow step has a debug breakpoint.
    Breakpoint,
    /// A graph declares workflow variables.
    Variables,
    /// A graph declares an input schema.
    InputSchema,
    /// A graph or wait step declares an output/response schema.
    OutputSchema,
}

/// Deterministic feature summary for an `ExecutionGraph`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFeatureSummary {
    /// Total number of steps including nested `Split`, `While`, and
    /// `WaitForSignal.onWait` subgraphs.
    pub total_steps: usize,
    /// Number of graphs visited including the root graph.
    pub graph_count: usize,
    /// Maximum nested graph depth. The root graph has depth `0`.
    pub max_graph_depth: usize,
    /// Root graph durability default. `None` in the DSL means durable.
    pub root_durable: bool,
    /// Step counts keyed by DSL step type name.
    pub step_type_counts: BTreeMap<String, usize>,
    /// Coarse features present anywhere in the graph.
    pub features: BTreeSet<WorkflowFeature>,
    /// Canonical agent ids used by `Agent` steps.
    pub agent_ids: Vec<String>,
    /// Connection ids referenced by `Agent` and `AiAgent` steps.
    pub connection_ids: Vec<String>,
    /// Child workflow call sites referenced by `EmbedWorkflow` steps.
    pub child_workflows: Vec<ChildWorkflowReference>,
}

impl WorkflowFeatureSummary {
    /// Return whether this summary contains `feature`.
    pub fn has(&self, feature: WorkflowFeature) -> bool {
        self.features.contains(&feature)
    }

    /// Return whether direct output will need runtime, stdlib, or agent imports
    /// after static composition.
    pub fn requires_composed_imports(&self) -> bool {
        self.features.iter().any(|feature| {
            matches!(
                feature,
                WorkflowFeature::AgentCall
                    | WorkflowFeature::AiAgent
                    | WorkflowFeature::ChildWorkflow
                    | WorkflowFeature::LogEvent
                    | WorkflowFeature::ExplicitError
                    | WorkflowFeature::Filter
                    | WorkflowFeature::GroupBy
                    | WorkflowFeature::Delay
                    | WorkflowFeature::WaitForSignal
                    | WorkflowFeature::SuspendResume
                    | WorkflowFeature::Connection
                    | WorkflowFeature::Durability
            )
        })
    }
}

impl Default for WorkflowFeatureSummary {
    fn default() -> Self {
        Self {
            total_steps: 0,
            graph_count: 0,
            max_graph_depth: 0,
            root_durable: true,
            step_type_counts: BTreeMap::new(),
            features: BTreeSet::new(),
            agent_ids: Vec::new(),
            connection_ids: Vec::new(),
            child_workflows: Vec::new(),
        }
    }
}

/// Analyze a parsed execution graph and return a deterministic feature summary.
pub fn analyze_workflow_features(graph: &ExecutionGraph) -> WorkflowFeatureSummary {
    let mut analyzer = FeatureAnalyzer {
        summary: WorkflowFeatureSummary {
            root_durable: graph.durable.unwrap_or(true),
            ..WorkflowFeatureSummary::default()
        },
        agent_ids: BTreeSet::new(),
        connection_ids: BTreeSet::new(),
        child_workflows: BTreeSet::new(),
    };

    analyzer.visit_graph(graph, 0, analyzer.summary.root_durable);
    analyzer.finish()
}

struct FeatureAnalyzer {
    summary: WorkflowFeatureSummary,
    agent_ids: BTreeSet<String>,
    connection_ids: BTreeSet<String>,
    child_workflows: BTreeSet<ChildWorkflowReference>,
}

impl FeatureAnalyzer {
    fn finish(mut self) -> WorkflowFeatureSummary {
        self.summary.agent_ids = self.agent_ids.into_iter().collect();
        self.summary.connection_ids = self.connection_ids.into_iter().collect();
        self.summary.child_workflows = self.child_workflows.into_iter().collect();
        self.summary
    }

    fn visit_graph(&mut self, graph: &ExecutionGraph, depth: usize, inherited_durable: bool) {
        let graph_durable = graph.durable.unwrap_or(inherited_durable);

        self.summary.graph_count += 1;
        self.summary.max_graph_depth = self.summary.max_graph_depth.max(depth);

        if !graph.variables.is_empty() {
            self.summary.features.insert(WorkflowFeature::Variables);
        }
        if !graph.input_schema.is_empty() {
            self.summary.features.insert(WorkflowFeature::InputSchema);
        }
        if !graph.output_schema.is_empty() {
            self.summary.features.insert(WorkflowFeature::OutputSchema);
        }

        self.visit_edges(graph);

        for step in graph.steps.values() {
            self.visit_step(step, depth, graph_durable);
        }
    }

    fn visit_edges(&mut self, graph: &ExecutionGraph) {
        let mut unlabeled_default_edges_by_source: BTreeMap<&str, usize> = BTreeMap::new();

        for edge in &graph.execution_plan {
            if edge.condition.is_some() {
                self.summary.features.insert(WorkflowFeature::EdgeCondition);
                self.summary
                    .features
                    .insert(WorkflowFeature::ConditionExpression);
            }

            if edge.label.as_deref() == Some("onError") {
                self.summary
                    .features
                    .insert(WorkflowFeature::ErrorHandlerEdge);
            }

            let label = edge.label.as_deref().unwrap_or_default();
            if label.is_empty() && edge.condition.is_none() {
                *unlabeled_default_edges_by_source
                    .entry(edge.from_step.as_str())
                    .or_default() += 1;
            }
        }

        if unlabeled_default_edges_by_source
            .values()
            .any(|count| *count > 1)
        {
            self.summary.features.insert(WorkflowFeature::ParallelEdges);
        }
    }

    fn visit_step(&mut self, step: &Step, depth: usize, graph_durable: bool) {
        self.summary.total_steps += 1;
        *self
            .summary
            .step_type_counts
            .entry(step_type_name(step).to_string())
            .or_default() += 1;

        if step_has_breakpoint(step) {
            self.summary.features.insert(WorkflowFeature::Breakpoint);
        }

        match step {
            Step::Finish(_) => {}
            Step::Agent(step) => {
                self.summary.features.insert(WorkflowFeature::AgentCall);
                self.agent_ids.insert(canonicalize_agent_id(&step.agent_id));
                if let Some(connection_id) = &step.connection_id {
                    self.connection_ids.insert(connection_id.clone());
                    self.summary.features.insert(WorkflowFeature::Connection);
                }
                if step.max_retries.is_some() || step.retry_delay.is_some() {
                    self.summary.features.insert(WorkflowFeature::RetryPolicy);
                }
                if step.timeout.is_some() {
                    self.summary.features.insert(WorkflowFeature::Timeout);
                }
                if step.compensation.is_some() {
                    self.summary.features.insert(WorkflowFeature::Compensation);
                }
                if graph_durable && step.durable.unwrap_or(true) {
                    self.summary.features.insert(WorkflowFeature::Durability);
                }
            }
            Step::Conditional(_) => {
                self.summary
                    .features
                    .insert(WorkflowFeature::ConditionalBranch);
                self.summary
                    .features
                    .insert(WorkflowFeature::ConditionExpression);
            }
            Step::Split(step) => {
                self.summary.features.insert(WorkflowFeature::SplitSubgraph);
                if graph_durable && step.durable.unwrap_or(true) {
                    self.summary.features.insert(WorkflowFeature::Durability);
                }
                self.visit_graph(&step.subgraph, depth + 1, graph_durable);
            }
            Step::Switch(_) => {
                self.summary.features.insert(WorkflowFeature::SwitchBranch);
            }
            Step::EmbedWorkflow(step) => {
                self.summary.features.insert(WorkflowFeature::ChildWorkflow);
                self.child_workflows.insert(ChildWorkflowReference {
                    step_id: step.id.clone(),
                    workflow_id: step.child_workflow_id.clone(),
                    version: child_version_to_string(&step.child_version),
                });
                if step.max_retries.is_some() || step.retry_delay.is_some() {
                    self.summary.features.insert(WorkflowFeature::RetryPolicy);
                }
                if step.timeout.is_some() {
                    self.summary.features.insert(WorkflowFeature::Timeout);
                }
                if graph_durable && step.durable.unwrap_or(true) {
                    self.summary.features.insert(WorkflowFeature::Durability);
                }
            }
            Step::While(step) => {
                self.summary.features.insert(WorkflowFeature::WhileLoop);
                self.summary
                    .features
                    .insert(WorkflowFeature::ConditionExpression);
                if step
                    .config
                    .as_ref()
                    .and_then(|config| config.timeout)
                    .is_some()
                {
                    self.summary.features.insert(WorkflowFeature::Timeout);
                }
                self.visit_graph(&step.subgraph, depth + 1, graph_durable);
            }
            Step::Log(_) => {
                self.summary.features.insert(WorkflowFeature::LogEvent);
            }
            Step::Error(_) => {
                self.summary.features.insert(WorkflowFeature::ExplicitError);
            }
            Step::Filter(_) => {
                self.summary.features.insert(WorkflowFeature::Filter);
                self.summary
                    .features
                    .insert(WorkflowFeature::ConditionExpression);
            }
            Step::GroupBy(_) => {
                self.summary.features.insert(WorkflowFeature::GroupBy);
            }
            Step::Delay(step) => {
                self.summary.features.insert(WorkflowFeature::Delay);
                if graph_durable && step.durable.unwrap_or(true) {
                    self.summary.features.insert(WorkflowFeature::Durability);
                    self.summary.features.insert(WorkflowFeature::SuspendResume);
                }
            }
            Step::WaitForSignal(step) => {
                self.summary.features.insert(WorkflowFeature::WaitForSignal);
                self.summary.features.insert(WorkflowFeature::SuspendResume);
                if graph_durable {
                    self.summary.features.insert(WorkflowFeature::Durability);
                }
                if step.timeout_ms.is_some() {
                    self.summary.features.insert(WorkflowFeature::Timeout);
                }
                if step.response_schema.is_some() {
                    self.summary.features.insert(WorkflowFeature::OutputSchema);
                }
                if let Some(on_wait) = &step.on_wait {
                    self.visit_graph(on_wait, depth + 1, graph_durable);
                }
            }
            Step::AiAgent(step) => {
                self.summary.features.insert(WorkflowFeature::AiAgent);
                if let Some(connection_id) = &step.connection_id {
                    self.connection_ids.insert(connection_id.clone());
                    self.summary.features.insert(WorkflowFeature::Connection);
                }
                if graph_durable && step.durable.unwrap_or(true) {
                    self.summary.features.insert(WorkflowFeature::Durability);
                }
            }
        }
    }
}

fn canonicalize_agent_id(agent_id: &str) -> String {
    agent_id.to_ascii_lowercase().replace('_', "-")
}

fn child_version_to_string(version: &ChildVersion) -> String {
    match version {
        ChildVersion::Latest(value) => value.clone(),
        ChildVersion::Specific(value) => value.to_string(),
    }
}

fn step_has_breakpoint(step: &Step) -> bool {
    match step {
        Step::Finish(step) => step.breakpoint.unwrap_or(false),
        Step::Agent(step) => step.breakpoint.unwrap_or(false),
        Step::Conditional(step) => step.breakpoint.unwrap_or(false),
        Step::Split(step) => step.breakpoint.unwrap_or(false),
        Step::Switch(step) => step.breakpoint.unwrap_or(false),
        Step::EmbedWorkflow(step) => step.breakpoint.unwrap_or(false),
        Step::While(step) => step.breakpoint.unwrap_or(false),
        Step::Log(step) => step.breakpoint.unwrap_or(false),
        Step::Error(step) => step.breakpoint.unwrap_or(false),
        Step::Filter(step) => step.breakpoint.unwrap_or(false),
        Step::GroupBy(step) => step.breakpoint.unwrap_or(false),
        Step::Delay(step) => step.breakpoint.unwrap_or(false),
        Step::WaitForSignal(step) => step.breakpoint.unwrap_or(false),
        Step::AiAgent(step) => step.breakpoint.unwrap_or(false),
    }
}

pub(crate) fn step_type_name(step: &Step) -> &'static str {
    match step {
        Step::Finish(_) => "Finish",
        Step::Agent(_) => "Agent",
        Step::Conditional(_) => "Conditional",
        Step::Split(_) => "Split",
        Step::Switch(_) => "Switch",
        Step::EmbedWorkflow(_) => "EmbedWorkflow",
        Step::While(_) => "While",
        Step::Log(_) => "Log",
        Step::Error(_) => "Error",
        Step::Filter(_) => "Filter",
        Step::GroupBy(_) => "GroupBy",
        Step::Delay(_) => "Delay",
        Step::WaitForSignal(_) => "WaitForSignal",
        Step::AiAgent(_) => "AiAgent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../tests/fixtures/conditional_workflow.json"),
            "embed" => include_str!("../tests/fixtures/embed_workflow_workflow.json"),
            "wait" => include_str!("../tests/fixtures/wait_for_signal_with_callback.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
    }

    fn graph(json: &str) -> ExecutionGraph {
        serde_json::from_str(json).expect("graph should parse")
    }

    #[test]
    fn simple_passthrough_has_minimal_features() {
        let summary = analyze_workflow_features(&fixture("simple"));

        assert_eq!(summary.total_steps, 1);
        assert_eq!(summary.graph_count, 1);
        assert_eq!(summary.max_graph_depth, 0);
        assert_eq!(summary.step_type_counts.get("Finish"), Some(&1));
        assert!(summary.root_durable);
        assert!(summary.features.is_empty());
        assert!(!summary.requires_composed_imports());
    }

    #[test]
    fn conditional_branches_are_reported_as_emitter_control_flow() {
        let summary = analyze_workflow_features(&fixture("conditional"));

        assert!(summary.has(WorkflowFeature::ConditionalBranch));
        assert!(summary.has(WorkflowFeature::ConditionExpression));
        assert!(!summary.has(WorkflowFeature::AgentCall));
        assert!(!summary.requires_composed_imports());
    }

    #[test]
    fn nested_wait_callbacks_are_counted_recursively() {
        let summary = analyze_workflow_features(&fixture("wait"));

        assert_eq!(summary.graph_count, 2);
        assert_eq!(summary.max_graph_depth, 1);
        assert_eq!(summary.step_type_counts.get("WaitForSignal"), Some(&1));
        assert_eq!(summary.step_type_counts.get("Log"), Some(&1));
        assert_eq!(summary.step_type_counts.get("Finish"), Some(&2));
        assert!(summary.has(WorkflowFeature::WaitForSignal));
        assert!(summary.has(WorkflowFeature::SuspendResume));
        assert!(summary.has(WorkflowFeature::LogEvent));
        assert!(summary.has(WorkflowFeature::Durability));
        assert!(summary.requires_composed_imports());
    }

    #[test]
    fn agent_and_ai_metadata_are_sorted_and_canonicalized() {
        let summary = analyze_workflow_features(&graph(
            r#"{
              "name": "Agent metadata",
              "steps": {
                "call": {
                  "stepType": "Agent",
                  "id": "call",
                  "agentId": "HTTP_Service",
                  "capabilityId": "request",
                  "connectionId": "api",
                  "durable": false
                },
                "ai": {
                  "stepType": "AiAgent",
                  "id": "ai",
                  "connectionId": "llm"
                },
                "finish": { "stepType": "Finish", "id": "finish" }
              },
              "entryPoint": "call",
              "executionPlan": [
                { "fromStep": "call", "toStep": "ai" },
                { "fromStep": "ai", "toStep": "finish" }
              ]
            }"#,
        ));

        assert_eq!(summary.agent_ids, vec!["http-service"]);
        assert_eq!(summary.connection_ids, vec!["api", "llm"]);
        assert!(summary.has(WorkflowFeature::AgentCall));
        assert!(summary.has(WorkflowFeature::AiAgent));
        assert!(summary.has(WorkflowFeature::Connection));
        assert!(summary.has(WorkflowFeature::Durability));
    }

    #[test]
    fn child_workflow_references_are_stable() {
        let summary = analyze_workflow_features(&fixture("embed"));

        assert_eq!(
            summary.child_workflows,
            vec![ChildWorkflowReference {
                step_id: "call_child".to_string(),
                workflow_id: "child_workflow".to_string(),
                version: "latest".to_string(),
            }]
        );
        assert!(summary.has(WorkflowFeature::ChildWorkflow));
        assert!(summary.has(WorkflowFeature::Durability));
    }

    #[test]
    fn execution_plan_edges_surface_routing_features() {
        let summary = analyze_workflow_features(&graph(
            r#"{
              "steps": {
                "start": {
                  "stepType": "Log",
                  "id": "start",
                  "message": "starting"
                },
                "a": { "stepType": "Finish", "id": "a" },
                "b": { "stepType": "Finish", "id": "b" }
              },
              "entryPoint": "start",
              "executionPlan": [
                {
                  "fromStep": "start",
                  "toStep": "a",
                  "condition": {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                      { "valueType": "reference", "value": "data.route" },
                      { "valueType": "immediate", "value": "a" }
                    ]
                  }
                },
                { "fromStep": "start", "toStep": "b", "label": "onError" }
              ]
            }"#,
        ));

        assert!(summary.has(WorkflowFeature::EdgeCondition));
        assert!(summary.has(WorkflowFeature::ConditionExpression));
        assert!(summary.has(WorkflowFeature::ErrorHandlerEdge));
        assert!(summary.has(WorkflowFeature::LogEvent));
    }
}
