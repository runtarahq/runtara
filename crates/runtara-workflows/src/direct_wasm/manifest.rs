// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Versioned manifest emitted by the production direct WebAssembly compiler.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use runtara_dsl::{ExecutionGraph, ExecutionPlanEdge, Step};
use sha2::{Digest, Sha256};

use crate::compile::TEMPLATE_MAJOR_VERSION;
use crate::workflow_features::{WorkflowFeatureSummary, analyze_workflow_features};

/// Current direct workflow manifest schema version.
pub const DIRECT_WORKFLOW_MANIFEST_VERSION: u32 = 1;

/// Versioned, deterministic manifest for a workflow graph.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectWorkflowManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Generated workflow template major version used for cache invalidation.
    pub template_major_version: String,
    /// SHA-256 over the manifest with this field omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Root graph manifest.
    pub graph: DirectGraphManifest,
    /// Feature summary used by direct-emitter gating and cache metadata.
    pub feature_summary: WorkflowFeatureSummary,
}

impl DirectWorkflowManifest {
    /// Return the computed manifest checksum.
    pub fn checksum(&self) -> &str {
        self.checksum.as_deref().unwrap_or_default()
    }

    /// Serialize this manifest to stable JSON bytes.
    pub fn to_canonical_json(&self) -> Result<Vec<u8>, DirectManifestError> {
        serde_json::to_vec(self).map_err(DirectManifestError::Serialize)
    }
}

/// Deterministic manifest for one execution graph.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectGraphManifest {
    /// Human-readable graph name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Graph entry point step id.
    pub entry_point: String,
    /// Effective graph durability. `None` in the DSL inherits from the parent.
    pub durable: bool,
    /// Graph-level constant variables as canonical JSON.
    pub variables: serde_json::Value,
    /// Graph input schema as canonical JSON.
    pub input_schema: serde_json::Value,
    /// Graph output schema as canonical JSON.
    pub output_schema: serde_json::Value,
    /// Steps sorted by step id.
    pub steps: Vec<DirectStepManifest>,
    /// Mapping definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<DirectMappingManifest>,
    /// Condition definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<DirectConditionManifest>,
    /// Filter definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<DirectFilterManifest>,
    /// Switch definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<DirectSwitchManifest>,
    /// GroupBy definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_bys: Vec<DirectGroupByManifest>,
    /// Log definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<DirectLogManifest>,
    /// Error definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<DirectErrorManifest>,
    /// Agent definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<DirectAgentManifest>,
    /// Execution-plan edges in deterministic routing order.
    pub edges: Vec<DirectEdgeManifest>,
}

/// Deterministic manifest for one DSL step.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectStepManifest {
    /// DSL step id.
    pub id: String,
    /// DSL step type.
    pub step_type: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Canonical JSON serialization of the step.
    pub body: serde_json::Value,
    /// Nested graphs owned by this step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested_graphs: Vec<DirectNestedGraphManifest>,
}

/// Nested graph attached to a step.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectNestedGraphManifest {
    /// Role of the nested graph, for example `split.subgraph`.
    pub role: String,
    /// Nested graph manifest.
    pub graph: Box<DirectGraphManifest>,
}

/// Deterministic mapping definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectMappingManifest {
    /// Manifest-wide mapping identifier.
    pub id: u32,
    /// Step that owns this mapping.
    pub step_id: String,
    /// Step type that owns this mapping.
    pub step_type: String,
    /// Mapping role within the step, for example `finish.inputMapping`.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL mapping.
    pub value: serde_json::Value,
}

/// Deterministic condition definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectConditionManifest {
    /// Manifest-wide condition identifier.
    pub id: u32,
    /// Step or edge source that owns this condition.
    pub owner_id: String,
    /// Owner type, for example `Conditional` or `Edge`.
    pub owner_type: String,
    /// Condition role, for example `conditional.condition`.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL condition expression.
    pub value: serde_json::Value,
}

/// Deterministic Filter definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectFilterManifest {
    /// Manifest-wide Filter identifier.
    pub id: u32,
    /// Step that owns this Filter config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Filter config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL Filter config.
    pub value: serde_json::Value,
}

/// Deterministic Switch definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectSwitchManifest {
    /// Manifest-wide Switch identifier.
    pub id: u32,
    /// Step that owns this Switch config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Switch config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL Switch config.
    pub value: serde_json::Value,
}

/// Deterministic GroupBy definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectGroupByManifest {
    /// Manifest-wide GroupBy identifier.
    pub id: u32,
    /// Step that owns this GroupBy config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this GroupBy config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL GroupBy config.
    pub value: serde_json::Value,
}

/// Deterministic Log definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectLogManifest {
    /// Manifest-wide Log identifier.
    pub id: u32,
    /// Step that owns this Log config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Log config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL Log step.
    pub value: serde_json::Value,
}

/// Deterministic Error definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectErrorManifest {
    /// Manifest-wide Error identifier.
    pub id: u32,
    /// Step that owns this Error config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Error config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL Error step.
    pub value: serde_json::Value,
}

/// Deterministic Agent definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectAgentManifest {
    /// Manifest-wide Agent identifier.
    pub id: u32,
    /// Step that owns this Agent config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Agent config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Agent component id.
    pub agent_id: String,
    /// Capability id passed to the agent component.
    pub capability_id: String,
    /// Optional workflow connection id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    /// Manifest-wide mapping id for Agent inputs.
    pub input_mapping_id: u32,
    /// Maximum retry attempts configured on the Agent step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Base retry delay configured on the Agent step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,
    /// Step timeout configured on the Agent step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Deterministic manifest for one execution-plan edge.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectEdgeManifest {
    /// Original position in `ExecutionGraph.executionPlan`.
    pub ordinal: usize,
    /// Source step id.
    pub from_step: String,
    /// Target step id.
    pub to_step: String,
    /// Optional edge label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional edge condition as canonical JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<serde_json::Value>,
    /// Manifest-wide condition identifier for this edge condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition_id: Option<u32>,
    /// Optional edge priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
}

/// Errors returned while building or serializing a direct workflow manifest.
#[derive(Debug)]
pub enum DirectManifestError {
    /// A DSL value failed to serialize into the manifest.
    Serialize(serde_json::Error),
}

impl fmt::Display for DirectManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectManifestError::Serialize(err) => {
                write!(f, "failed to serialize direct workflow manifest: {err}")
            }
        }
    }
}

impl std::error::Error for DirectManifestError {}

/// Build a deterministic direct workflow manifest from a parsed DSL graph.
pub fn build_direct_workflow_manifest(
    graph: &ExecutionGraph,
) -> Result<DirectWorkflowManifest, DirectManifestError> {
    let feature_summary = analyze_workflow_features(graph);
    let root_durable = graph.durable.unwrap_or(true);
    let mut state = DirectManifestBuildState::default();
    let mut manifest = DirectWorkflowManifest {
        version: DIRECT_WORKFLOW_MANIFEST_VERSION,
        template_major_version: TEMPLATE_MAJOR_VERSION.to_string(),
        checksum: None,
        graph: graph_manifest(graph, root_durable, &mut state)?,
        feature_summary,
    };

    let canonical = serde_json::to_vec(&manifest).map_err(DirectManifestError::Serialize)?;
    manifest.checksum = Some(sha256_hex(&canonical));
    Ok(manifest)
}

#[derive(Debug, Default)]
struct DirectManifestBuildState {
    next_mapping_id: u32,
    next_condition_id: u32,
    next_filter_id: u32,
    next_switch_id: u32,
    next_group_by_id: u32,
    next_log_id: u32,
    next_error_id: u32,
    next_agent_id: u32,
}

impl DirectManifestBuildState {
    fn allocate_mapping_id(&mut self) -> u32 {
        let id = self.next_mapping_id;
        self.next_mapping_id += 1;
        id
    }

    fn allocate_condition_id(&mut self) -> u32 {
        let id = self.next_condition_id;
        self.next_condition_id += 1;
        id
    }

    fn allocate_filter_id(&mut self) -> u32 {
        let id = self.next_filter_id;
        self.next_filter_id += 1;
        id
    }

    fn allocate_switch_id(&mut self) -> u32 {
        let id = self.next_switch_id;
        self.next_switch_id += 1;
        id
    }

    fn allocate_group_by_id(&mut self) -> u32 {
        let id = self.next_group_by_id;
        self.next_group_by_id += 1;
        id
    }

    fn allocate_log_id(&mut self) -> u32 {
        let id = self.next_log_id;
        self.next_log_id += 1;
        id
    }

    fn allocate_error_id(&mut self) -> u32 {
        let id = self.next_error_id;
        self.next_error_id += 1;
        id
    }

    fn allocate_agent_id(&mut self) -> u32 {
        let id = self.next_agent_id;
        self.next_agent_id += 1;
        id
    }
}

fn graph_manifest(
    graph: &ExecutionGraph,
    inherited_durable: bool,
    state: &mut DirectManifestBuildState,
) -> Result<DirectGraphManifest, DirectManifestError> {
    let durable = graph.durable.unwrap_or(inherited_durable);
    let mut step_values = graph.steps.values().collect::<Vec<_>>();
    step_values.sort_by(|left, right| step_id(left).cmp(step_id(right)));

    let mut collections = DirectGraphManifestCollections::default();
    let steps = step_values
        .into_iter()
        .map(|step| step_manifest(step, durable, state, &mut collections))
        .collect::<Result<Vec<_>, _>>()?;

    collections
        .mappings
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .conditions
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .filters
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .switches
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .group_bys
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .logs
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .errors
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .agents
        .sort_by(|left, right| left.id.cmp(&right.id));

    let mut edges = graph
        .execution_plan
        .iter()
        .enumerate()
        .map(|(ordinal, edge)| edge_manifest(ordinal, edge, state, &mut collections))
        .collect::<Result<Vec<_>, _>>()?;
    edges.sort_by(compare_edges);

    Ok(DirectGraphManifest {
        name: graph.name.clone(),
        entry_point: graph.entry_point.clone(),
        durable,
        variables: canonical_json(&graph.variables)?,
        input_schema: canonical_json(&graph.input_schema)?,
        output_schema: canonical_json(&graph.output_schema)?,
        steps,
        mappings: collections.mappings,
        conditions: collections.conditions,
        filters: collections.filters,
        switches: collections.switches,
        group_bys: collections.group_bys,
        logs: collections.logs,
        errors: collections.errors,
        agents: collections.agents,
        edges,
    })
}

#[derive(Default)]
struct DirectGraphManifestCollections {
    mappings: Vec<DirectMappingManifest>,
    conditions: Vec<DirectConditionManifest>,
    filters: Vec<DirectFilterManifest>,
    switches: Vec<DirectSwitchManifest>,
    group_bys: Vec<DirectGroupByManifest>,
    logs: Vec<DirectLogManifest>,
    errors: Vec<DirectErrorManifest>,
    agents: Vec<DirectAgentManifest>,
}

fn step_manifest(
    step: &Step,
    inherited_durable: bool,
    state: &mut DirectManifestBuildState,
    collections: &mut DirectGraphManifestCollections,
) -> Result<DirectStepManifest, DirectManifestError> {
    let mut nested_graphs = Vec::new();
    match step {
        Step::Finish(step) => {
            let value = step
                .input_mapping
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            collections.mappings.push(DirectMappingManifest {
                id: state.allocate_mapping_id(),
                step_id: step.id.clone(),
                step_type: "Finish".to_string(),
                purpose: "finish.inputMapping".to_string(),
                value,
            });
        }
        Step::Conditional(step) => {
            collections.conditions.push(DirectConditionManifest {
                id: state.allocate_condition_id(),
                owner_id: step.id.clone(),
                owner_type: "Conditional".to_string(),
                purpose: "conditional.condition".to_string(),
                value: canonical_json(&step.condition)?,
            });
        }
        Step::Split(step) => {
            nested_graphs.push(DirectNestedGraphManifest {
                role: "split.subgraph".to_string(),
                graph: Box::new(graph_manifest(&step.subgraph, inherited_durable, state)?),
            });
        }
        Step::Switch(step) => {
            let value = step
                .config
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            collections.switches.push(DirectSwitchManifest {
                id: state.allocate_switch_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Switch".to_string(),
                purpose: "switch.config".to_string(),
                value,
            });
        }
        Step::While(step) => {
            nested_graphs.push(DirectNestedGraphManifest {
                role: "while.subgraph".to_string(),
                graph: Box::new(graph_manifest(&step.subgraph, inherited_durable, state)?),
            });
        }
        Step::Filter(step) => {
            collections.filters.push(DirectFilterManifest {
                id: state.allocate_filter_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Filter".to_string(),
                purpose: "filter.config".to_string(),
                value: canonical_json(&step.config)?,
            });
        }
        Step::GroupBy(step) => {
            collections.group_bys.push(DirectGroupByManifest {
                id: state.allocate_group_by_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "GroupBy".to_string(),
                purpose: "groupBy.config".to_string(),
                value: canonical_json(&step.config)?,
            });
        }
        Step::Log(step) => {
            collections.logs.push(DirectLogManifest {
                id: state.allocate_log_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Log".to_string(),
                purpose: "log.config".to_string(),
                value: canonical_json(step)?,
            });
        }
        Step::Error(step) => {
            collections.errors.push(DirectErrorManifest {
                id: state.allocate_error_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Error".to_string(),
                purpose: "error.config".to_string(),
                value: canonical_json(step)?,
            });
        }
        Step::Agent(step) => {
            let input_mapping = step
                .input_mapping
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            let input_mapping_id = state.allocate_mapping_id();
            collections.mappings.push(DirectMappingManifest {
                id: input_mapping_id,
                step_id: step.id.clone(),
                step_type: "Agent".to_string(),
                purpose: "agent.inputMapping".to_string(),
                value: input_mapping,
            });
            collections.agents.push(DirectAgentManifest {
                id: state.allocate_agent_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Agent".to_string(),
                purpose: "agent.config".to_string(),
                agent_id: step.agent_id.clone(),
                capability_id: step.capability_id.clone(),
                connection_id: step.connection_id.clone(),
                input_mapping_id,
                max_retries: step.max_retries,
                retry_delay: step.retry_delay,
                timeout: step.timeout,
            });
        }
        Step::WaitForSignal(step) => {
            if let Some(on_wait) = &step.on_wait {
                nested_graphs.push(DirectNestedGraphManifest {
                    role: "waitForSignal.onWait".to_string(),
                    graph: Box::new(graph_manifest(on_wait, inherited_durable, state)?),
                });
            }
        }
        _ => {}
    }

    Ok(DirectStepManifest {
        id: step_id(step).to_string(),
        step_type: step_type_name(step).to_string(),
        name: step_name(step).map(ToOwned::to_owned),
        body: canonical_json(step)?,
        nested_graphs,
    })
}

fn edge_manifest(
    ordinal: usize,
    edge: &ExecutionPlanEdge,
    state: &mut DirectManifestBuildState,
    collections: &mut DirectGraphManifestCollections,
) -> Result<DirectEdgeManifest, DirectManifestError> {
    let condition_id = if let Some(condition) = edge.condition.as_ref() {
        let id = state.allocate_condition_id();
        collections.conditions.push(DirectConditionManifest {
            id,
            owner_id: edge.from_step.clone(),
            owner_type: "Edge".to_string(),
            purpose: "edge.condition".to_string(),
            value: canonical_json(condition)?,
        });
        Some(id)
    } else {
        None
    };

    Ok(DirectEdgeManifest {
        ordinal,
        from_step: edge.from_step.clone(),
        to_step: edge.to_step.clone(),
        label: edge.label.clone(),
        condition: edge.condition.as_ref().map(canonical_json).transpose()?,
        condition_id,
        priority: edge.priority,
    })
}

fn compare_edges(left: &DirectEdgeManifest, right: &DirectEdgeManifest) -> Ordering {
    (
        &left.from_step,
        left.label.as_deref().unwrap_or_default(),
        edge_route_rank(left),
        left.ordinal,
        &left.to_step,
    )
        .cmp(&(
            &right.from_step,
            right.label.as_deref().unwrap_or_default(),
            edge_route_rank(right),
            right.ordinal,
            &right.to_step,
        ))
}

fn edge_route_rank(edge: &DirectEdgeManifest) -> i64 {
    if edge.condition.is_some() {
        -(i64::from(edge.priority.unwrap_or(0)))
    } else {
        i64::MAX
    }
}

fn canonical_json<T: serde::Serialize>(
    value: &T,
) -> Result<serde_json::Value, DirectManifestError> {
    let value = serde_json::to_value(value).map_err(DirectManifestError::Serialize)?;
    Ok(sort_json(value))
}

fn sort_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(sort_json).collect())
        }
        serde_json::Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, sort_json(value)))
                .collect::<BTreeMap<_, _>>();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        other => other,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn step_id(step: &Step) -> &str {
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

fn step_name(step: &Step) -> Option<&str> {
    match step {
        Step::Finish(step) => step.name.as_deref(),
        Step::Agent(step) => step.name.as_deref(),
        Step::Conditional(step) => step.name.as_deref(),
        Step::Split(step) => step.name.as_deref(),
        Step::Switch(step) => step.name.as_deref(),
        Step::EmbedWorkflow(step) => step.name.as_deref(),
        Step::While(step) => step.name.as_deref(),
        Step::Log(step) => step.name.as_deref(),
        Step::Error(step) => step.name.as_deref(),
        Step::Filter(step) => step.name.as_deref(),
        Step::GroupBy(step) => step.name.as_deref(),
        Step::Delay(step) => step.name.as_deref(),
        Step::WaitForSignal(step) => step.name.as_deref(),
        Step::AiAgent(step) => step.name.as_deref(),
    }
}

fn step_type_name(step: &Step) -> &'static str {
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
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> ExecutionGraph {
        let json = match name {
            "simple" => include_str!("../../tests/fixtures/simple_passthrough.json"),
            "conditional" => include_str!("../../tests/fixtures/conditional_workflow.json"),
            "filter" => include_str!("../../tests/fixtures/filter_simple.json"),
            "switch_value" => include_str!("../../tests/fixtures/switch_value_simple.json"),
            "group_by" => include_str!("../../tests/fixtures/group_by_simple.json"),
            "log" => include_str!("../../tests/fixtures/log_no_context.json"),
            "error" => include_str!("../../tests/fixtures/error_direct_simple.json"),
            "edge_condition" => include_str!("../../tests/fixtures/edge_condition_priority.json"),
            "transform" => include_str!("../../tests/fixtures/transform_workflow.json"),
            "wait" => include_str!("../../tests/fixtures/wait_for_signal_with_callback.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(json).expect("fixture should parse")
    }

    #[test]
    fn manifest_checksum_is_deterministic() {
        let graph = fixture("simple");

        let first = build_direct_workflow_manifest(&graph).expect("manifest");
        let second = build_direct_workflow_manifest(&graph).expect("manifest");

        assert_eq!(first.checksum(), second.checksum());
        assert_eq!(
            first.to_canonical_json().expect("json"),
            second.to_canonical_json().expect("json")
        );
        assert_eq!(first.version, DIRECT_WORKFLOW_MANIFEST_VERSION);
        assert_eq!(first.graph.entry_point, "finish");
    }

    #[test]
    fn manifest_assigns_finish_mapping_id() {
        let manifest = build_direct_workflow_manifest(&fixture("simple")).expect("manifest");

        assert_eq!(manifest.graph.mappings.len(), 1);
        let mapping = &manifest.graph.mappings[0];
        assert_eq!(mapping.id, 0);
        assert_eq!(mapping.step_id, "finish");
        assert_eq!(mapping.step_type, "Finish");
        assert_eq!(mapping.purpose, "finish.inputMapping");
        assert_eq!(mapping.value["result"]["valueType"], "reference");
        assert_eq!(mapping.value["result"]["value"], "data.input");
    }

    #[test]
    fn manifest_assigns_conditional_condition_id() {
        let manifest = build_direct_workflow_manifest(&fixture("conditional")).expect("manifest");

        assert_eq!(manifest.graph.conditions.len(), 1);
        let condition = &manifest.graph.conditions[0];
        assert_eq!(condition.id, 0);
        assert_eq!(condition.owner_id, "check");
        assert_eq!(condition.owner_type, "Conditional");
        assert_eq!(condition.purpose, "conditional.condition");
        assert_eq!(condition.value["type"], "operation");
        assert_eq!(condition.value["op"], "EQ");
    }

    #[test]
    fn manifest_assigns_filter_id() {
        let manifest = build_direct_workflow_manifest(&fixture("filter")).expect("manifest");

        assert_eq!(manifest.graph.filters.len(), 1);
        let filter = &manifest.graph.filters[0];
        assert_eq!(filter.id, 0);
        assert_eq!(filter.step_id, "filter");
        assert_eq!(filter.name.as_deref(), Some("Filter Active Items"));
        assert_eq!(filter.step_type, "Filter");
        assert_eq!(filter.purpose, "filter.config");
        assert_eq!(filter.value["condition"]["op"], "EQ");
        assert_eq!(filter.value["value"]["valueType"], "reference");
        assert_eq!(filter.value["value"]["value"], "data.items");
    }

    #[test]
    fn manifest_assigns_switch_id() {
        let manifest = build_direct_workflow_manifest(&fixture("switch_value")).expect("manifest");

        assert_eq!(manifest.graph.switches.len(), 1);
        let switch = &manifest.graph.switches[0];
        assert_eq!(switch.id, 0);
        assert_eq!(switch.step_id, "switch");
        assert_eq!(switch.name.as_deref(), Some("Classify Status"));
        assert_eq!(switch.step_type, "Switch");
        assert_eq!(switch.purpose, "switch.config");
        assert_eq!(switch.value["value"]["valueType"], "reference");
        assert_eq!(switch.value["value"]["value"], "data.status");
        assert_eq!(switch.value["cases"][0]["matchType"], "EQ");
    }

    #[test]
    fn manifest_assigns_group_by_id() {
        let manifest = build_direct_workflow_manifest(&fixture("group_by")).expect("manifest");

        assert_eq!(manifest.graph.group_bys.len(), 1);
        let group_by = &manifest.graph.group_bys[0];
        assert_eq!(group_by.id, 0);
        assert_eq!(group_by.step_id, "group");
        assert_eq!(group_by.name.as_deref(), Some("Group by Status"));
        assert_eq!(group_by.step_type, "GroupBy");
        assert_eq!(group_by.purpose, "groupBy.config");
        assert_eq!(group_by.value["key"], "status");
        assert_eq!(group_by.value["value"]["valueType"], "reference");
        assert_eq!(group_by.value["value"]["value"], "data.items");
    }

    #[test]
    fn manifest_assigns_log_ids() {
        let manifest = build_direct_workflow_manifest(&fixture("log")).expect("manifest");

        assert_eq!(manifest.graph.logs.len(), 2);
        let log = &manifest.graph.logs[0];
        assert_eq!(log.id, 0);
        assert_eq!(log.step_id, "log_default_level");
        assert_eq!(log.step_type, "Log");
        assert_eq!(log.purpose, "log.config");
        assert_eq!(log.value["level"], "info");
        assert_eq!(log.value["message"], "Log with default level (info)");
    }

    #[test]
    fn manifest_assigns_error_ids() {
        let manifest = build_direct_workflow_manifest(&fixture("error")).expect("manifest");

        assert_eq!(manifest.graph.errors.len(), 1);
        let error = &manifest.graph.errors[0];
        assert_eq!(error.id, 0);
        assert_eq!(error.step_id, "fail");
        assert_eq!(error.step_type, "Error");
        assert_eq!(error.purpose, "error.config");
        assert_eq!(error.value["category"], "permanent");
        assert_eq!(error.value["code"], "DIRECT_FAILURE");
        assert_eq!(error.value["severity"], "critical");
    }

    #[test]
    fn manifest_assigns_agent_ids_and_input_mapping() {
        let manifest = build_direct_workflow_manifest(&fixture("transform")).expect("manifest");

        assert_eq!(manifest.graph.agents.len(), 1);
        let agent = &manifest.graph.agents[0];
        assert_eq!(agent.id, 0);
        assert_eq!(agent.step_id, "transform");
        assert_eq!(agent.step_type, "Agent");
        assert_eq!(agent.purpose, "agent.config");
        assert_eq!(agent.agent_id, "transform");
        assert_eq!(agent.capability_id, "map-fields");
        assert_eq!(agent.connection_id, None);
        assert_eq!(agent.input_mapping_id, 1);

        assert_eq!(manifest.graph.mappings.len(), 2);
        let mapping = manifest
            .graph
            .mappings
            .iter()
            .find(|mapping| mapping.id == agent.input_mapping_id)
            .expect("agent input mapping");
        assert_eq!(mapping.step_id, "transform");
        assert_eq!(mapping.step_type, "Agent");
        assert_eq!(mapping.purpose, "agent.inputMapping");
        assert_eq!(mapping.value["source"]["valueType"], "reference");
        assert_eq!(mapping.value["source"]["value"], "data");
        assert_eq!(
            mapping.value["mapping"]["value"]["output_field"],
            "$.input_field"
        );
    }

    #[test]
    fn manifest_assigns_edge_condition_ids() {
        let manifest =
            build_direct_workflow_manifest(&fixture("edge_condition")).expect("manifest");

        assert_eq!(manifest.graph.conditions.len(), 2);
        assert_eq!(
            manifest
                .graph
                .conditions
                .iter()
                .map(|condition| condition.owner_type.as_str())
                .collect::<Vec<_>>(),
            vec!["Edge", "Edge"]
        );
        assert_eq!(
            manifest
                .graph
                .conditions
                .iter()
                .map(|condition| condition.purpose.as_str())
                .collect::<Vec<_>>(),
            vec!["edge.condition", "edge.condition"]
        );
        let conditioned_edges = manifest
            .graph
            .edges
            .iter()
            .filter(|edge| edge.condition_id.is_some())
            .collect::<Vec<_>>();
        assert_eq!(conditioned_edges.len(), 2);
        assert_eq!(conditioned_edges[0].condition_id, Some(1));
        assert_eq!(conditioned_edges[0].priority, Some(10));
        assert_eq!(conditioned_edges[1].condition_id, Some(0));
        assert_eq!(conditioned_edges[1].priority, Some(5));
    }

    #[test]
    fn manifest_captures_nested_wait_graph() {
        let manifest = build_direct_workflow_manifest(&fixture("wait")).expect("manifest");

        let wait = manifest
            .graph
            .steps
            .iter()
            .find(|step| step.id == "wait")
            .expect("wait step");
        assert_eq!(wait.nested_graphs.len(), 1);
        assert_eq!(wait.nested_graphs[0].role, "waitForSignal.onWait");
        assert_eq!(wait.nested_graphs[0].graph.entry_point, "log");
        assert!(
            wait.nested_graphs[0]
                .graph
                .steps
                .iter()
                .any(|step| step.step_type == "Log")
        );
    }

    #[test]
    fn manifest_builds_deterministically_for_parseable_fixtures() {
        let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut parseable = 0usize;

        for entry in fs::read_dir(fixture_dir).expect("fixture dir") {
            let path = entry.expect("fixture entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let json = fs::read_to_string(&path).expect("fixture file");
            let Ok(graph) = serde_json::from_str::<ExecutionGraph>(&json) else {
                continue;
            };
            parseable += 1;

            let first = build_direct_workflow_manifest(&graph).expect("manifest");
            let second = build_direct_workflow_manifest(&graph).expect("manifest");
            assert_eq!(first.checksum(), second.checksum(), "{path:?}");
        }

        assert!(
            parseable >= 40,
            "expected broad fixture coverage, got {parseable}"
        );
    }
}
