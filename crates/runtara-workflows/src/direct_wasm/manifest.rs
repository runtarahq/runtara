// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Versioned manifest emitted by the production direct WebAssembly compiler.
//!
//! Flattens a parsed DSL `ExecutionGraph` (plus any preloaded child graphs) into
//! the `DirectWorkflowManifest` — the normalized IR the rest of the emitter reads
//! instead of touching raw DSL. Steps are walked in id order and their config
//! pulled into per-kind tables, each assigned a stable manifest-wide integer id,
//! because raw Wasm can only address data by small numeric ids and segment
//! offsets (not by walking a tree). Everything is canonicalized (key-sorted JSON,
//! sorted edges) and SHA-256 checksummed with the checksum field omitted, so
//! identical graphs produce byte-identical artifacts — deterministic,
//! content-addressable caching. It also performs direct-path-only desugaring with
//! no DSL equivalent: lowering an `AiAgent` step into `ai-tools` capability calls
//! (request mapping, tool defs from labelled edges, memory providers) and merging
//! child-graph agent ids up into the parent so composition imports them.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use runtara_dsl::agent_meta::AgentCatalog;
use runtara_dsl::{ExecutionGraph, ExecutionPlanEdge, MappingValue, Step};
use sha2::{Digest, Sha256};

use crate::compile::TEMPLATE_MAJOR_VERSION;
use crate::workflow_features::{
    WorkflowFeature, WorkflowFeatureSummary, analyze_workflow_features,
};

/// Current direct workflow manifest schema version.
pub const DIRECT_WORKFLOW_MANIFEST_VERSION: u32 = 2;

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
    /// Statically preloaded child workflow graphs available for inline
    /// `EmbedWorkflow` lowering. All graphs share the same manifest-wide id
    /// allocator as the root graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_workflows: Vec<DirectChildWorkflowGraphManifest>,
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
    /// Maximum cumulative rate-limit retry wait budget for this graph.
    pub rate_limit_budget_ms: u64,
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
    /// Split definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splits: Vec<DirectSplitManifest>,
    /// While definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub whiles: Vec<DirectWhileManifest>,
    /// Filter definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<DirectFilterManifest>,
    /// Switch definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<DirectSwitchManifest>,
    /// GroupBy definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_bys: Vec<DirectGroupByManifest>,
    /// Delay definitions addressable by generated direct Wasm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delays: Vec<DirectDelayManifest>,
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

/// Input used to include one statically preloaded child workflow in a direct
/// manifest.
#[derive(Debug, Clone, Copy)]
pub struct DirectManifestChildWorkflowInput<'a> {
    /// `EmbedWorkflow` call-site step id that references this child.
    pub step_id: &'a str,
    /// Referenced child workflow id.
    pub workflow_id: &'a str,
    /// Version requested by the parent DSL, such as `latest` or `2`.
    pub version_requested: &'a str,
    /// Version resolved by the caller before direct compilation.
    pub version_resolved: i32,
    /// Child workflow graph.
    pub execution_graph: &'a ExecutionGraph,
}

/// Statically preloaded child workflow graph serialized into the direct
/// manifest.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectChildWorkflowGraphManifest {
    /// `EmbedWorkflow` call-site step id that references this child.
    pub step_id: String,
    /// Referenced child workflow id.
    pub workflow_id: String,
    /// Version requested by the parent DSL, such as `latest` or `2`.
    pub version_requested: String,
    /// Version resolved by the caller before direct compilation.
    pub version_resolved: i32,
    /// Child graph manifest.
    pub graph: DirectGraphManifest,
    /// Feature summary for this child graph.
    pub feature_summary: WorkflowFeatureSummary,
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

/// Deterministic Split definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectSplitManifest {
    /// Manifest-wide Split identifier.
    pub id: u32,
    /// Step that owns this Split config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Split config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Effective Split durability after graph-level inheritance is applied.
    pub durable: bool,
    /// Canonical JSON serialization of the DSL Split config.
    pub value: serde_json::Value,
    /// Canonical JSON serialization of the per-iteration input schema.
    pub input_schema: serde_json::Value,
    /// Canonical JSON serialization of the per-iteration output schema.
    pub output_schema: serde_json::Value,
}

/// Deterministic While definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectWhileManifest {
    /// Manifest-wide While identifier.
    pub id: u32,
    /// Step that owns this While config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this While config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Canonical JSON serialization of the DSL While config.
    pub value: serde_json::Value,
    /// Canonical JSON serialization of the While condition expression.
    pub condition: serde_json::Value,
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

/// Deterministic Delay definition referenced by direct-emitted Wasm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectDelayManifest {
    /// Manifest-wide Delay identifier.
    pub id: u32,
    /// Step that owns this Delay config.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Step type that owns this Delay config.
    pub step_type: String,
    /// Config role within the step.
    pub purpose: String,
    /// Effective Delay durability after graph-level inheritance is applied.
    pub durable: bool,
    /// Canonical JSON serialization of `DelayStep.durationMs`.
    pub duration_ms: serde_json::Value,
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
    /// Optional workflow connection id (same-tenant literal binding).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    /// Optional resolvable connection binding (a `MappingValue`), evaluated
    /// against the execution source at runtime by the stdlib
    /// `resolve-connection-id`; wins over `connection_id` when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<serde_json::Value>,
    /// Effective Agent durability after graph-level inheritance is applied.
    pub durable: bool,
    /// Whether the referenced capability is marked rate-limited in the Agent catalog.
    pub rate_limited: bool,
    /// Manifest-wide mapping id for Agent inputs.
    pub input_mapping_id: u32,
    /// Required capability inputs validated after runtime references resolve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_inputs: Vec<DirectAgentRequiredInputManifest>,
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

/// Required Agent capability input metadata used by direct runtime validation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectAgentRequiredInputManifest {
    /// Field name.
    pub name: String,
    /// Field type for diagnostics.
    pub field_type: String,
    /// Optional field description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    build_direct_workflow_manifest_with_agent_catalog(graph, None)
}

/// Build a deterministic direct workflow manifest using an optional Agent catalog.
pub fn build_direct_workflow_manifest_with_agent_catalog(
    graph: &ExecutionGraph,
    agent_catalog: Option<&AgentCatalog>,
) -> Result<DirectWorkflowManifest, DirectManifestError> {
    build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(graph, &[], agent_catalog)
}

/// Build a deterministic direct workflow manifest with statically preloaded
/// child workflow graphs and an optional Agent catalog.
pub fn build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
    graph: &ExecutionGraph,
    child_workflows: &[DirectManifestChildWorkflowInput<'_>],
    agent_catalog: Option<&AgentCatalog>,
) -> Result<DirectWorkflowManifest, DirectManifestError> {
    let mut feature_summary = analyze_workflow_features(graph);
    // An AiAgent step lowers as an invoke of the `ai-tools` `chat-completion`
    // capability, so the workflow must import the ai-tools agent component even
    // though the AiAgent step carries no agent_id of its own. This is
    // direct-emitter-specific (the generated path links provider logic inline),
    // so it is added here rather than in the shared feature analyzer.
    if feature_summary.features.contains(&WorkflowFeature::AiAgent)
        && !feature_summary.agent_ids.iter().any(|id| id == "ai-tools")
    {
        feature_summary.agent_ids.push("ai-tools".to_string());
        feature_summary.agent_ids.sort();
    }
    let root_durable = graph.durable.unwrap_or(true);
    let mut state = DirectManifestBuildState::default();
    let root_graph = graph_manifest(graph, root_durable, &mut state, agent_catalog)?;
    let mut child_workflows = child_workflows.to_vec();
    child_workflows.sort_by(|left, right| {
        (
            left.step_id,
            left.workflow_id,
            left.version_resolved,
            left.version_requested,
        )
            .cmp(&(
                right.step_id,
                right.workflow_id,
                right.version_resolved,
                right.version_requested,
            ))
    });
    let child_workflows = child_workflows
        .into_iter()
        .map(|child| {
            let child_durable = child.execution_graph.durable.unwrap_or(true);
            let graph = graph_manifest(
                child.execution_graph,
                child_durable,
                &mut state,
                agent_catalog,
            )?;
            Ok(DirectChildWorkflowGraphManifest {
                step_id: child.step_id.to_string(),
                workflow_id: child.workflow_id.to_string(),
                version_requested: child.version_requested.to_string(),
                version_resolved: child.version_resolved,
                graph,
                feature_summary: analyze_workflow_features(child.execution_graph),
            })
        })
        .collect::<Result<Vec<_>, DirectManifestError>>()?;

    // Each EmbedWorkflow child runs inline in the composed parent, so the parent
    // must import every agent component its children reference (and ai-tools if a
    // child uses an AiAgent step). Without this merge a child Agent/AiAgent step
    // would have no matching import and fail composition — the reason embed
    // children were previously restricted to trivial control-flow-only graphs.
    for child in &child_workflows {
        for agent_id in &child.feature_summary.agent_ids {
            if !feature_summary.agent_ids.contains(agent_id) {
                feature_summary.agent_ids.push(agent_id.clone());
            }
        }
        if child
            .feature_summary
            .features
            .contains(&WorkflowFeature::AiAgent)
            && !feature_summary.agent_ids.iter().any(|id| id == "ai-tools")
        {
            feature_summary.agent_ids.push("ai-tools".to_string());
        }
    }
    feature_summary.agent_ids.sort();
    feature_summary.agent_ids.dedup();

    let mut manifest = DirectWorkflowManifest {
        version: DIRECT_WORKFLOW_MANIFEST_VERSION,
        template_major_version: TEMPLATE_MAJOR_VERSION.to_string(),
        checksum: None,
        graph: root_graph,
        child_workflows,
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
    next_split_id: u32,
    next_while_id: u32,
    next_filter_id: u32,
    next_switch_id: u32,
    next_group_by_id: u32,
    next_delay_id: u32,
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

    fn allocate_split_id(&mut self) -> u32 {
        let id = self.next_split_id;
        self.next_split_id += 1;
        id
    }

    fn allocate_while_id(&mut self) -> u32 {
        let id = self.next_while_id;
        self.next_while_id += 1;
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

    fn allocate_delay_id(&mut self) -> u32 {
        let id = self.next_delay_id;
        self.next_delay_id += 1;
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
    agent_catalog: Option<&AgentCatalog>,
) -> Result<DirectGraphManifest, DirectManifestError> {
    let durable = graph.durable.unwrap_or(inherited_durable);
    let mut step_values = graph.steps.values().collect::<Vec<_>>();
    step_values.sort_by(|left, right| step_id(left).cmp(step_id(right)));

    let mut collections = DirectGraphManifestCollections::default();
    let steps = step_values
        .into_iter()
        .map(|step| step_manifest(step, graph, durable, state, &mut collections, agent_catalog))
        .collect::<Result<Vec<_>, _>>()?;

    collections
        .mappings
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .conditions
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .splits
        .sort_by(|left, right| left.id.cmp(&right.id));
    collections
        .whiles
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
        .delays
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
        rate_limit_budget_ms: graph.rate_limit_budget_ms,
        variables: canonical_json(&graph.variables)?,
        input_schema: canonical_json(&graph.input_schema)?,
        output_schema: canonical_json(&graph.output_schema)?,
        steps,
        mappings: collections.mappings,
        conditions: collections.conditions,
        splits: collections.splits,
        whiles: collections.whiles,
        filters: collections.filters,
        switches: collections.switches,
        group_bys: collections.group_bys,
        delays: collections.delays,
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
    splits: Vec<DirectSplitManifest>,
    whiles: Vec<DirectWhileManifest>,
    filters: Vec<DirectFilterManifest>,
    switches: Vec<DirectSwitchManifest>,
    group_bys: Vec<DirectGroupByManifest>,
    delays: Vec<DirectDelayManifest>,
    logs: Vec<DirectLogManifest>,
    errors: Vec<DirectErrorManifest>,
    agents: Vec<DirectAgentManifest>,
}

fn step_manifest(
    step: &Step,
    graph: &ExecutionGraph,
    inherited_durable: bool,
    state: &mut DirectManifestBuildState,
    collections: &mut DirectGraphManifestCollections,
    agent_catalog: Option<&AgentCatalog>,
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
            let value = step
                .config
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            collections.splits.push(DirectSplitManifest {
                id: state.allocate_split_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Split".to_string(),
                purpose: "split.config".to_string(),
                durable: inherited_durable && step.durable.unwrap_or(true),
                value,
                input_schema: canonical_json(&step.input_schema)?,
                output_schema: canonical_json(&step.output_schema)?,
            });
            nested_graphs.push(DirectNestedGraphManifest {
                role: "split.subgraph".to_string(),
                graph: Box::new(graph_manifest(
                    &step.subgraph,
                    inherited_durable,
                    state,
                    agent_catalog,
                )?),
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
        Step::EmbedWorkflow(step) => {
            let value = step
                .input_mapping
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            collections.mappings.push(DirectMappingManifest {
                id: state.allocate_mapping_id(),
                step_id: step.id.clone(),
                step_type: "EmbedWorkflow".to_string(),
                purpose: "embedWorkflow.inputMapping".to_string(),
                value,
            });
        }
        Step::While(step) => {
            let value = step
                .config
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            collections.whiles.push(DirectWhileManifest {
                id: state.allocate_while_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "While".to_string(),
                purpose: "while.config".to_string(),
                value,
                condition: canonical_json(&step.condition)?,
            });
            nested_graphs.push(DirectNestedGraphManifest {
                role: "while.subgraph".to_string(),
                graph: Box::new(graph_manifest(
                    &step.subgraph,
                    inherited_durable,
                    state,
                    agent_catalog,
                )?),
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
        Step::Delay(step) => {
            collections.delays.push(DirectDelayManifest {
                id: state.allocate_delay_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "Delay".to_string(),
                purpose: "delay.config".to_string(),
                durable: inherited_durable && step.durable.unwrap_or(true),
                duration_ms: canonical_json(&step.duration_ms)?,
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
            let agent_id = canonicalize_direct_agent_id(&step.agent_id);
            let mut input_mapping = step
                .input_mapping
                .as_ref()
                .map(canonical_json)
                .transpose()?
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            // Make AgentStep.timeout real: inject it as `timeout_ms` into the
            // capability input so it reaches the outbound-HTTP layer (the proxy
            // honors the serialized timeout_ms). Only when explicitly set —
            // an explicit step timeout overrides any author-mapped timeout_ms;
            // when unset, the capability's own default applies. Harmless for
            // capabilities that don't read timeout_ms (inputs ignore unknown
            // fields), and enforced by those that do (e.g. http, ai-tools).
            if let Some(timeout) = step.timeout
                && let serde_json::Value::Object(map) = &mut input_mapping
            {
                map.insert(
                    "timeout_ms".to_string(),
                    serde_json::json!({ "valueType": "immediate", "value": timeout }),
                );
            }
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
                agent_id: agent_id.clone(),
                capability_id: step.capability_id.clone(),
                connection_id: step.connection_id.clone(),
                connection_ref: connection_ref_json(step.connection_ref.as_ref())?,
                durable: inherited_durable && step.durable.unwrap_or(true),
                rate_limited: agent_capability_rate_limited(
                    agent_catalog,
                    &agent_id,
                    &step.capability_id,
                ),
                input_mapping_id,
                required_inputs: required_agent_inputs(
                    agent_catalog,
                    &agent_id,
                    &step.capability_id,
                ),
                max_retries: step.max_retries,
                retry_delay: step.retry_delay,
                timeout: step.timeout,
            });
        }
        Step::AiAgent(step) => {
            // Single-shot AiAgent: lower as an invoke of the `ai-tools`
            // `chat-completion` capability with a synthesized input mapping that
            // builds the completion request from the AiAgent config.
            // Keys are snake_case to match the `chat-completion` capability
            // input fields (ChatCompletionInput deserializes snake_case).
            let mut mapping = serde_json::Map::new();
            if let Some(config) = step.config.as_ref() {
                mapping.insert(
                    "system_prompt".to_string(),
                    canonical_json(&config.system_prompt)?,
                );
                mapping.insert(
                    "user_prompt".to_string(),
                    canonical_json(&config.user_prompt)?,
                );
                mapping.insert(
                    "provider".to_string(),
                    serde_json::json!({
                        "valueType": "immediate",
                        "value": config.provider.as_str(),
                    }),
                );
                if let Some(output_schema) = &config.output_schema {
                    // Convert the DSL flat-map schema to JSON Schema, matching the
                    // generated loop, so the provider's structured-output params
                    // line up.
                    let json_schema =
                        runtara_dsl::schema_convert::dsl_schema_to_json_schema(output_schema);
                    mapping.insert(
                        "output_schema".to_string(),
                        serde_json::json!({ "valueType": "immediate", "value": json_schema }),
                    );
                }
                if let Some(model) = &config.model {
                    mapping.insert(
                        "model".to_string(),
                        serde_json::json!({ "valueType": "immediate", "value": model }),
                    );
                }
                if let Some(temperature) = config.temperature {
                    mapping.insert(
                        "temperature".to_string(),
                        serde_json::json!({ "valueType": "immediate", "value": temperature }),
                    );
                }
                if let Some(max_tokens) = config.max_tokens {
                    mapping.insert(
                        "max_tokens".to_string(),
                        serde_json::json!({ "valueType": "immediate", "value": max_tokens }),
                    );
                }
                // Per-attempt brain-turn timeout. Injected only when configured;
                // when absent, the `ai-tools` chat capability defaults it to
                // DEFAULT_STEP_TIMEOUT_MS (so the LLM call is never bounded by
                // the proxy's 30s no-timeout floor). The `timeout_ms` key feeds
                // both `chat-completion` (single shot) and `chat-turn` (loop).
                if let Some(turn_timeout) = config.turn_timeout {
                    mapping.insert(
                        "timeout_ms".to_string(),
                        serde_json::json!({ "valueType": "immediate", "value": turn_timeout }),
                    );
                }
            }
            // Tool edges (labelled, excluding next/onError/memory/mcp.*) turn this
            // into a tool-loop AiAgent driven by the `chat-turn` capability.
            // Conversation memory also forces the loop path (it manages chat
            // history). Otherwise it is a single-shot `chat-completion`.
            let has_memory = step
                .config
                .as_ref()
                .and_then(|config| config.memory.as_ref())
                .is_some();
            let tool_edges = ai_agent_tool_edges(graph, &step.id);
            let mcp_edges = ai_agent_mcp_edges(graph, &step.id);
            let has_mcp = !mcp_edges.is_empty();
            let capability_id = if tool_edges.is_empty() && !has_memory && !has_mcp {
                "chat-completion"
            } else {
                let mut tool_defs = tool_edges
                    .iter()
                    .map(|(label, target)| {
                        serde_json::json!({
                            "name": label,
                            "description": graph
                                .steps
                                .get(target)
                                .and_then(step_name)
                                .unwrap_or(label.as_str()),
                            "parameters": {
                                "type": "object",
                                "properties": {},
                                "additionalProperties": true
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                // MCP toolsets each advertise two synthetic meta-tools, appended
                // after the Agent tools (the LLM's tool_index resolves by this
                // order, which the run plan's tool list mirrors exactly).
                for (toolset, _target, _conn, _conn_ref) in &mcp_edges {
                    tool_defs.extend(ai_agent_mcp_tool_defs(toolset));
                }
                mapping.insert(
                    "tools".to_string(),
                    serde_json::json!({ "valueType": "immediate", "value": tool_defs }),
                );
                if has_mcp {
                    let toolsets = mcp_edges
                        .iter()
                        .map(|(toolset, _, _, _)| toolset.clone())
                        .collect::<Vec<_>>();
                    mapping.insert(
                        "system_prompt_suffix".to_string(),
                        serde_json::json!({
                            "valueType": "immediate",
                            "value": ai_agent_mcp_prompt_addition(&toolsets),
                        }),
                    );
                }
                "chat-turn"
            };
            let input_mapping_id = state.allocate_mapping_id();
            collections.mappings.push(DirectMappingManifest {
                id: input_mapping_id,
                step_id: step.id.clone(),
                step_type: "AiAgent".to_string(),
                purpose: "agent.inputMapping".to_string(),
                value: serde_json::Value::Object(mapping),
            });
            collections.agents.push(DirectAgentManifest {
                id: state.allocate_agent_id(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                step_type: "AiAgent".to_string(),
                purpose: "agent.config".to_string(),
                agent_id: "ai-tools".to_string(),
                capability_id: capability_id.to_string(),
                connection_id: step.connection_id.clone(),
                connection_ref: connection_ref_json(step.connection_ref.as_ref())?,
                durable: inherited_durable && step.durable.unwrap_or(true),
                rate_limited: agent_capability_rate_limited(
                    agent_catalog,
                    "ai-tools",
                    capability_id,
                ),
                input_mapping_id,
                required_inputs: required_agent_inputs(agent_catalog, "ai-tools", capability_id),
                // Retries are opt-in for AiAgent (default 0 — LLM calls
                // re-bill); the plan applies them on the single-shot path.
                max_retries: step.config.as_ref().and_then(|config| config.max_retries),
                retry_delay: step.config.as_ref().and_then(|config| config.retry_delay),
                timeout: None,
            });
            // Conversation memory: record the provider agent's load-memory and
            // save-memory entries plus a conversation-id mapping. The loop loads
            // history before the turns and saves the final history after.
            if let (true, Some((mem_agent, mem_conn, mem_conn_ref))) =
                (has_memory, ai_agent_memory_provider(graph, &step.id))
            {
                let memory = step.config.as_ref().and_then(|c| c.memory.as_ref());
                let mut conversation = serde_json::Map::new();
                if let Some(memory) = memory {
                    conversation.insert(
                        "conversation_id".to_string(),
                        canonical_json(&memory.conversation_id)?,
                    );
                }
                let conversation_mapping_id = state.allocate_mapping_id();
                collections.mappings.push(DirectMappingManifest {
                    id: conversation_mapping_id,
                    step_id: step.id.clone(),
                    step_type: "AiAgent".to_string(),
                    purpose: "memory.conversation".to_string(),
                    value: serde_json::Value::Object(conversation),
                });
                let mem_agent = canonicalize_direct_agent_id(&mem_agent);
                for (purpose, capability) in [
                    ("memory.load", "load-memory"),
                    ("memory.save", "save-memory"),
                ] {
                    collections.agents.push(DirectAgentManifest {
                        id: state.allocate_agent_id(),
                        step_id: step.id.clone(),
                        name: None,
                        step_type: "AiAgent".to_string(),
                        purpose: purpose.to_string(),
                        agent_id: mem_agent.clone(),
                        capability_id: capability.to_string(),
                        connection_id: mem_conn.clone(),
                        connection_ref: connection_ref_json(mem_conn_ref.as_ref())?,
                        durable: inherited_durable && step.durable.unwrap_or(true),
                        rate_limited: false,
                        input_mapping_id: conversation_mapping_id,
                        required_inputs: Vec::new(),
                        max_retries: None,
                        retry_delay: None,
                        timeout: None,
                    });
                }
                // Summarize-strategy compaction runs the `ai-tools`
                // summarize-memory capability before the save (the LLM
                // summarizes the oldest messages). The default sliding window
                // needs no provider. The summarize LLM call reuses the AiAgent's
                // own connection (provider/model are passed in the input).
                let use_summarize = memory
                    .and_then(|memory| memory.compaction.as_ref())
                    .and_then(|compaction| compaction.strategy.as_ref())
                    .is_some_and(|strategy| {
                        matches!(strategy, runtara_dsl::CompactionStrategy::Summarize)
                    });
                if use_summarize {
                    // Summarize reuses the AiAgent's own provider connection —
                    // literal or resolvable ref — resolved uniformly at the
                    // invoke boundary, so it shares the conversation mapping.
                    collections.agents.push(DirectAgentManifest {
                        id: state.allocate_agent_id(),
                        step_id: step.id.clone(),
                        name: None,
                        step_type: "AiAgent".to_string(),
                        purpose: "memory.summarize".to_string(),
                        agent_id: "ai-tools".to_string(),
                        capability_id: "summarize-memory".to_string(),
                        connection_id: step.connection_id.clone(),
                        connection_ref: connection_ref_json(step.connection_ref.as_ref())?,
                        durable: inherited_durable && step.durable.unwrap_or(true),
                        rate_limited: agent_capability_rate_limited(
                            agent_catalog,
                            "ai-tools",
                            "summarize-memory",
                        ),
                        input_mapping_id: conversation_mapping_id,
                        required_inputs: Vec::new(),
                        max_retries: None,
                        retry_delay: None,
                        timeout: None,
                    });
                }
            }
            // MCP toolsets: each `mcp.<toolset>` edge contributes two tool
            // provider entries (the `mcp` agent's mcp-tool-search /
            // mcp-tool-invoke capabilities), named after the synthetic tools so
            // the run plan can resolve each advertised tool to its provider.
            // Order matches `ai_agent_mcp_tool_defs`: search then invoke.
            for (toolset, _target, connection_id, connection_ref) in &mcp_edges {
                for (role, capability) in
                    [("search", "mcp-tool-search"), ("invoke", "mcp-tool-invoke")]
                {
                    collections.agents.push(DirectAgentManifest {
                        id: state.allocate_agent_id(),
                        step_id: step.id.clone(),
                        name: Some(format!("{toolset}_{role}")),
                        step_type: "AiAgent".to_string(),
                        purpose: "agent.tool.mcp".to_string(),
                        agent_id: "mcp".to_string(),
                        capability_id: capability.to_string(),
                        connection_id: connection_id.clone(),
                        connection_ref: connection_ref_json(connection_ref.as_ref())?,
                        durable: inherited_durable && step.durable.unwrap_or(true),
                        rate_limited: agent_capability_rate_limited(
                            agent_catalog,
                            "mcp",
                            capability,
                        ),
                        input_mapping_id,
                        required_inputs: Vec::new(),
                        max_retries: None,
                        retry_delay: None,
                        timeout: None,
                    });
                }
            }
        }
        Step::WaitForSignal(step) => {
            if let Some(on_wait) = &step.on_wait {
                nested_graphs.push(DirectNestedGraphManifest {
                    role: "waitForSignal.onWait".to_string(),
                    graph: Box::new(graph_manifest(
                        on_wait,
                        inherited_durable,
                        state,
                        agent_catalog,
                    )?),
                });
            }
        }
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

fn canonicalize_direct_agent_id(agent_id: &str) -> String {
    agent_id.to_lowercase().replace('_', "-")
}

/// Canonicalize a step's resolvable `connection_ref` (a `MappingValue`) into the
/// manifest `Value` the stdlib `resolve-connection-id` evaluates against the
/// execution source. `None` for a literal / connectionless step — the manifest
/// `connection_id` literal then carries the (same-tenant) binding as before.
fn connection_ref_json(
    connection_ref: Option<&MappingValue>,
) -> Result<Option<serde_json::Value>, DirectManifestError> {
    connection_ref.map(canonical_json).transpose()
}

/// The AiAgent's memory provider: the agent id, literal connection id, and
/// resolvable `connection_ref` of the Agent step on the `memory`-labelled edge,
/// if any. Both connection forms are carried so a memory-storage connection can
/// be a caller-supplied / rotated ref, not only a compile-time literal.
type MemoryProvider = (String, Option<String>, Option<MappingValue>);

fn ai_agent_memory_provider(graph: &ExecutionGraph, step_id: &str) -> Option<MemoryProvider> {
    let edge = graph
        .execution_plan
        .iter()
        .find(|edge| edge.from_step == step_id && edge.label.as_deref() == Some("memory"))?;
    match graph.steps.get(&edge.to_step) {
        Some(Step::Agent(agent)) => Some((
            agent.agent_id.clone(),
            agent.connection_id.clone(),
            agent.connection_ref.clone(),
        )),
        _ => None,
    }
}

/// The AiAgent's MCP tool edges as `(toolset_id, target_step_id, connection_id,
/// connection_ref)`. An `mcp.<toolset>` edge targets an Agent step with
/// `agent_id == "mcp"`; each becomes two synthetic LLM tools
/// (`<toolset>_search` / `<toolset>_invoke`). The provider's connection may be a
/// literal or a resolvable ref.
type McpEdge = (String, String, Option<String>, Option<MappingValue>);

fn ai_agent_mcp_edges(graph: &ExecutionGraph, step_id: &str) -> Vec<McpEdge> {
    graph
        .execution_plan
        .iter()
        .filter(|edge| edge.from_step == step_id)
        .filter_map(|edge| {
            let label = edge.label.as_deref()?;
            let toolset = label.strip_prefix("mcp.").filter(|s| !s.is_empty())?;
            let (connection_id, connection_ref) = match graph.steps.get(&edge.to_step) {
                Some(Step::Agent(agent)) => {
                    (agent.connection_id.clone(), agent.connection_ref.clone())
                }
                _ => return None,
            };
            Some((
                toolset.to_string(),
                edge.to_step.clone(),
                connection_id,
                connection_ref,
            ))
        })
        .collect()
}

/// The two synthetic tool definitions advertised to the LLM for one MCP toolset:
/// `<toolset>_search` (discover tools) and `<toolset>_invoke` (call one).
/// Mirrors the generated `mcp_tool_def_tokens`.
fn ai_agent_mcp_tool_defs(toolset: &str) -> [serde_json::Value; 2] {
    let search = serde_json::json!({
        "name": format!("{toolset}_search"),
        "description": format!(
            "Search the `{toolset}` MCP toolset for tools matching a free-text query. \
             Use this before `{toolset}_invoke` to discover tool names and argument shapes."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Free-text description of what you need." },
                "limit": { "type": "integer", "description": "Maximum number of tools to return (default 5, max 20)." }
            },
            "required": ["query"]
        }
    });
    let invoke = serde_json::json!({
        "name": format!("{toolset}_invoke"),
        "description": format!(
            "Invoke a specific tool from the `{toolset}` MCP toolset. The `tool_name` must be \
             one returned by `{toolset}_search`; `args` must match its input schema."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "tool_name": { "type": "string", "description": "Exact tool name from the search result." },
                "args": { "type": "object", "description": "Tool arguments matching the tool's input schema." }
            },
            "required": ["tool_name", "args"]
        }
    });
    [search, invoke]
}

/// The system-prompt suffix appended when an AiAgent has MCP edges, listing the
/// toolsets and the search→invoke pattern. Mirrors the generated
/// `mcp_prompt_addition`.
fn ai_agent_mcp_prompt_addition(toolsets: &[String]) -> String {
    let names = toolsets
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n\nExternal toolsets are available: {names}. To use one, first call \
         `<toolset>_search` with a description of what you need to find available \
         tools, then call `<toolset>_invoke` with the exact tool name and args \
         from the search result. Do not guess tool names."
    )
}

/// The AiAgent's tool edges as `(label, target_step_id)` pairs: labelled
/// outgoing edges other than `next`/`onError`/`memory`/`mcp.*`.
fn ai_agent_tool_edges(graph: &ExecutionGraph, step_id: &str) -> Vec<(String, String)> {
    graph
        .execution_plan
        .iter()
        .filter(|edge| edge.from_step == step_id)
        .filter_map(|edge| {
            let label = edge.label.as_deref()?;
            if label == "next"
                || label == "onError"
                || label == "memory"
                || label.starts_with("mcp.")
            {
                return None;
            }
            Some((label.to_string(), edge.to_step.clone()))
        })
        .collect()
}

fn agent_capability_rate_limited(
    agent_catalog: Option<&AgentCatalog>,
    agent_id: &str,
    capability_id: &str,
) -> bool {
    agent_catalog
        .and_then(|catalog| catalog.capability(agent_id, capability_id))
        .map(|capability| capability.rate_limited)
        .unwrap_or(false)
}

fn required_agent_inputs(
    agent_catalog: Option<&AgentCatalog>,
    agent_id: &str,
    capability_id: &str,
) -> Vec<DirectAgentRequiredInputManifest> {
    agent_catalog
        .and_then(|catalog| catalog.capability(agent_id, capability_id))
        .map(|capability| {
            capability
                .inputs
                .iter()
                .filter(|field| field.required && field.name != "_connection")
                .map(|field| DirectAgentRequiredInputManifest {
                    name: field.name.clone(),
                    field_type: field.type_name.clone(),
                    description: field.description.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
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
            "delay_simple" => include_str!("../../tests/fixtures/delay_simple.json"),
            "log" => include_str!("../../tests/fixtures/log_no_context.json"),
            "error" => include_str!("../../tests/fixtures/error_direct_simple.json"),
            "edge_condition" => include_str!("../../tests/fixtures/edge_condition_priority.json"),
            "transform" => include_str!("../../tests/fixtures/transform_workflow.json"),
            "split" => include_str!("../../tests/fixtures/split_workflow.json"),
            "while_simple" => include_str!("../../tests/fixtures/while_simple.json"),
            "wait" => include_str!("../../tests/fixtures/wait_for_signal_with_callback.json"),
            "embed_workflow" => include_str!("../../tests/fixtures/embed_workflow_workflow.json"),
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
        assert_eq!(first.graph.rate_limit_budget_ms, graph.rate_limit_budget_ms);
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
    fn manifest_assigns_split_id_and_nested_graph() {
        let manifest = build_direct_workflow_manifest(&fixture("split")).expect("manifest");

        assert_eq!(manifest.graph.splits.len(), 1);
        let split = &manifest.graph.splits[0];
        assert_eq!(split.id, 0);
        assert_eq!(split.step_id, "split");
        assert_eq!(split.step_type, "Split");
        assert_eq!(split.purpose, "split.config");
        assert!(split.durable);
        assert_eq!(split.value["value"]["valueType"], "reference");
        assert_eq!(split.value["value"]["value"], "data.items");
        assert_eq!(split.value["sequential"], true);
        assert_eq!(split.input_schema, serde_json::json!({}));
        assert_eq!(split.output_schema, serde_json::json!({}));

        let split_step = manifest
            .graph
            .steps
            .iter()
            .find(|step| step.id == "split")
            .expect("split step");
        assert_eq!(split_step.nested_graphs.len(), 1);
        assert_eq!(split_step.nested_graphs[0].role, "split.subgraph");
        assert_eq!(split_step.nested_graphs[0].graph.entry_point, "transform");
    }

    #[test]
    fn manifest_assigns_while_id_condition_and_nested_graph() {
        let manifest = build_direct_workflow_manifest(&fixture("while_simple")).expect("manifest");

        assert_eq!(manifest.graph.whiles.len(), 1);
        let while_step = &manifest.graph.whiles[0];
        assert_eq!(while_step.id, 0);
        assert_eq!(while_step.step_id, "loop");
        assert_eq!(while_step.name.as_deref(), Some("Counter Loop"));
        assert_eq!(while_step.step_type, "While");
        assert_eq!(while_step.purpose, "while.config");
        assert_eq!(while_step.value["maxIterations"], 10);
        assert_eq!(while_step.condition["type"], "operation");
        assert_eq!(while_step.condition["op"], "LT");

        let step = manifest
            .graph
            .steps
            .iter()
            .find(|step| step.id == "loop")
            .expect("while step");
        assert_eq!(step.nested_graphs.len(), 1);
        assert_eq!(step.nested_graphs[0].role, "while.subgraph");
        assert_eq!(step.nested_graphs[0].graph.entry_point, "increment");
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
    fn manifest_assigns_delay_id() {
        let manifest = build_direct_workflow_manifest(&fixture("delay_simple")).expect("manifest");

        assert_eq!(manifest.graph.delays.len(), 1);
        let delay = &manifest.graph.delays[0];
        assert_eq!(delay.id, 0);
        assert_eq!(delay.step_id, "delay");
        assert_eq!(delay.name.as_deref(), Some("Wait 1 second"));
        assert_eq!(delay.step_type, "Delay");
        assert_eq!(delay.purpose, "delay.config");
        assert!(delay.durable);
        assert_eq!(delay.duration_ms["valueType"], "immediate");
        assert_eq!(delay.duration_ms["value"], 1000);
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
        assert!(!agent.rate_limited);
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
        assert_eq!(mapping.value["source_data"]["valueType"], "reference");
        assert_eq!(mapping.value["source_data"]["value"], "data");
        // `mappings` is { source_path: target_field } for the map-fields capability.
        assert_eq!(
            mapping.value["mappings"]["value"]["$.input_field"],
            "output_field"
        );
    }

    #[test]
    fn manifest_serializes_agent_required_inputs_from_catalog() {
        let catalog =
            AgentCatalog::from_json(include_str!("../../tests/catalog/agent_catalog.json"))
                .expect("agent_catalog.json fixture should parse");
        let manifest = build_direct_workflow_manifest_with_agent_catalog(
            &fixture("transform"),
            Some(&catalog),
        )
        .expect("manifest");

        let required_inputs = &manifest.graph.agents[0].required_inputs;
        assert!(
            required_inputs
                .iter()
                .any(|input| input.name == "source_data")
        );
        assert!(required_inputs.iter().any(|input| input.name == "mappings"));
        assert!(
            required_inputs
                .iter()
                .all(|input| !input.field_type.is_empty())
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
    fn manifest_captures_static_child_workflow_graphs_with_shared_mapping_ids() {
        let parent = fixture("embed_workflow");
        let child = fixture("simple");
        let manifest = build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
            &parent,
            &[DirectManifestChildWorkflowInput {
                step_id: "call_child",
                workflow_id: "child_workflow",
                version_requested: "latest",
                version_resolved: 3,
                execution_graph: &child,
            }],
            None,
        )
        .expect("manifest");

        assert_eq!(manifest.child_workflows.len(), 1);
        let child_manifest = &manifest.child_workflows[0];
        assert_eq!(child_manifest.step_id, "call_child");
        assert_eq!(child_manifest.workflow_id, "child_workflow");
        assert_eq!(child_manifest.version_requested, "latest");
        assert_eq!(child_manifest.version_resolved, 3);
        assert_eq!(child_manifest.graph.entry_point, "finish");

        let root_embed_mapping = manifest
            .graph
            .mappings
            .iter()
            .find(|mapping| mapping.step_id == "call_child")
            .expect("root EmbedWorkflow mapping");
        assert_eq!(root_embed_mapping.id, 0);
        assert_eq!(root_embed_mapping.step_type, "EmbedWorkflow");
        assert_eq!(root_embed_mapping.purpose, "embedWorkflow.inputMapping");
        assert_eq!(
            root_embed_mapping.value["childInput"]["value"],
            "data.input"
        );

        let root_finish_mapping = manifest
            .graph
            .mappings
            .iter()
            .find(|mapping| mapping.step_id == "finish")
            .expect("root Finish mapping");
        assert_eq!(root_finish_mapping.id, 1);

        let child_finish_mapping = child_manifest
            .graph
            .mappings
            .iter()
            .find(|mapping| mapping.step_id == "finish")
            .expect("child Finish mapping");
        assert_eq!(child_finish_mapping.id, 2);
        assert_eq!(child_finish_mapping.purpose, "finish.inputMapping");
        assert_eq!(child_manifest.feature_summary.total_steps, 1);
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
    #[test]
    fn ai_agent_manifest_threads_retry_config() {
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "ai",
              "executionPlan": [
                {"fromStep":"ai","toStep":"finish","label":"next"}
              ],
              "steps": {
                "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
                  "systemPrompt":{"valueType":"immediate","value":"sys"},
                  "userPrompt":{"valueType":"immediate","value":"go"},
                  "provider":"openai",
                  "maxRetries":3,
                  "retryDelay":10
                }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .expect("graph parses");

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest builds");
        let agent = manifest
            .graph
            .agents
            .iter()
            .find(|agent| agent.step_id == "ai")
            .expect("ai agent entry");
        assert_eq!(agent.capability_id, "chat-completion");
        assert_eq!(agent.max_retries, Some(3));
        assert_eq!(agent.retry_delay, Some(10));
    }

    #[test]
    fn agent_connection_ref_lands_in_manifest() {
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "call",
              "executionPlan": [{"fromStep":"call","toStep":"finish"}],
              "inputSchema": {
                "crm": {"type":"connection","integration":"hubspot","required":true}
              },
              "steps": {
                "call": {"id":"call","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                  "connectionRef":{"valueType":"reference","value":"data.crm"},
                  "inputMapping":{"url":{"valueType":"immediate","value":"https://example.test"}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .expect("graph parses");

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest builds");
        let agent = manifest
            .graph
            .agents
            .iter()
            .find(|agent| agent.step_id == "call")
            .expect("agent entry");
        // The ref rides the manifest verbatim (the stdlib resolves it against the
        // execution source at the invoke boundary); no literal id here.
        assert_eq!(agent.connection_id, None);
        let connection_ref = agent.connection_ref.as_ref().expect("connection_ref");
        assert_eq!(connection_ref["valueType"], "reference");
        assert_eq!(connection_ref["value"], "data.crm");

        // The ref is NOT injected into the input mapping — the author's own
        // mapping is untouched.
        let mapping = manifest
            .graph
            .mappings
            .iter()
            .find(|mapping| mapping.id == agent.input_mapping_id)
            .expect("input mapping");
        assert_eq!(mapping.value["url"]["value"], "https://example.test");
        assert!(mapping.value.get("_connection").is_none());
    }

    #[test]
    fn literal_connection_id_leaves_manifest_ref_unset() {
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "call",
              "executionPlan": [{"fromStep":"call","toStep":"finish"}],
              "steps": {
                "call": {"id":"call","stepType":"Agent","agentId":"http","capabilityId":"http-request",
                  "connectionId":"conn-123",
                  "inputMapping":{"url":{"valueType":"immediate","value":"https://x.test"}}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .expect("graph parses");

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest builds");
        let agent = manifest
            .graph
            .agents
            .iter()
            .find(|agent| agent.step_id == "call")
            .expect("agent entry");
        // Back-compat: the literal id rides the manifest, no ref.
        assert_eq!(agent.connection_id.as_deref(), Some("conn-123"));
        assert!(agent.connection_ref.is_none());
    }

    #[test]
    fn ai_agent_connection_ref_lands_in_manifest() {
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "ai",
              "executionPlan": [{"fromStep":"ai","toStep":"finish","label":"next"}],
              "inputSchema": {
                "llm": {"type":"connection","integration":"openai","required":true}
              },
              "steps": {
                "ai": {"id":"ai","stepType":"AiAgent",
                  "connectionRef":{"valueType":"reference","value":"data.llm"},
                  "config":{
                    "systemPrompt":{"valueType":"immediate","value":"sys"},
                    "userPrompt":{"valueType":"immediate","value":"go"},
                    "provider":"openai"
                  }},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .expect("graph parses");

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest builds");
        let agent = manifest
            .graph
            .agents
            .iter()
            .find(|agent| agent.step_id == "ai")
            .expect("ai agent entry");
        assert_eq!(agent.connection_id, None);
        let connection_ref = agent.connection_ref.as_ref().expect("connection_ref");
        assert_eq!(connection_ref["value"], "data.llm");
    }

    #[test]
    fn memory_and_mcp_provider_connection_refs_land_in_manifest() {
        // An AiAgent whose OWN connection is a ref, plus a memory-storage
        // provider and an MCP-tool provider each bound via their own ref. All
        // three must ride the manifest so they resolve uniformly at the invoke
        // boundary — the memory/MCP agents' input is not a mapping, so this is
        // the only way their connection can be a caller-supplied / rotated ref.
        let graph: runtara_dsl::ExecutionGraph = serde_json::from_str(
            r##"{
              "entryPoint": "ai",
              "executionPlan": [
                {"fromStep":"ai","toStep":"finish","label":"next"},
                {"fromStep":"ai","toStep":"mem","label":"memory"},
                {"fromStep":"ai","toStep":"mcp1","label":"mcp.tools"}
              ],
              "steps": {
                "ai": {"id":"ai","stepType":"AiAgent",
                  "connectionRef":{"valueType":"reference","value":"data.llm"},
                  "config":{
                    "systemPrompt":{"valueType":"immediate","value":"sys"},
                    "userPrompt":{"valueType":"immediate","value":"go"},
                    "provider":"openai",
                    "memory":{
                      "conversationId":{"valueType":"immediate","value":"c1"},
                      "compaction":{"maxMessages":2,"strategy":"summarize"}
                    }
                  }},
                "mem": {"id":"mem","stepType":"Agent","agentId":"object_model",
                  "capabilityId":"load-memory",
                  "connectionRef":{"valueType":"reference","value":"data.memconn"}},
                "mcp1": {"id":"mcp1","stepType":"Agent","agentId":"mcp",
                  "capabilityId":"mcp-tool-search",
                  "connectionRef":{"valueType":"reference","value":"data.mcpconn"}},
                "finish": {"id":"finish","stepType":"Finish"}
              }
            }"##,
        )
        .expect("graph parses");

        let manifest = build_direct_workflow_manifest(&graph).expect("manifest builds");
        let ref_value = |purpose: &str| -> String {
            manifest
                .graph
                .agents
                .iter()
                .find(|a| a.purpose == purpose)
                .unwrap_or_else(|| panic!("agent with purpose {purpose}"))
                .connection_ref
                .as_ref()
                .unwrap_or_else(|| panic!("connection_ref for {purpose}"))["value"]
                .as_str()
                .expect("string")
                .to_string()
        };

        // Memory load/save resolve the memory provider's ref…
        assert_eq!(ref_value("memory.load"), "data.memconn");
        assert_eq!(ref_value("memory.save"), "data.memconn");
        // …summarize reuses the AiAgent's own provider ref…
        assert_eq!(ref_value("memory.summarize"), "data.llm");
        // …and each MCP tool provider resolves the MCP provider's ref.
        assert_eq!(ref_value("agent.tool.mcp"), "data.mcpconn");
    }
}
