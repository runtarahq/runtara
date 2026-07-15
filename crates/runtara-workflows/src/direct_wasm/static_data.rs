// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Static data layout for directly emitted workflow core Wasm modules.
//!
//! Because the module is emitted byte-by-byte with no allocator or string table,
//! every constant must live at a statically known address. `DirectCoreStaticData`
//! walks a growing offset (starting past a reserved 256-byte low-memory scratch
//! prefix), laying each constant — the manifest JSON, default variables,
//! event-kind tags, interned step ids/edge labels, per-agent capability/connection/
//! integration ids — as a data segment addressed by `(offset, len)`, which is the
//! Wasm-native way to pass a string across the stdlib ABI. Segments are re-aligned
//! as the offset advances; the final offset becomes `heap_base` and rounds up to
//! `memory_min_pages`, telling the module how much memory to declare and where its
//! runtime bump heap may safely begin.

use std::collections::{BTreeMap, BTreeSet};

use super::error::DirectCompileError;
use super::manifest::{DirectChildWorkflowGraphManifest, DirectGraphManifest};

pub(super) const DIRECT_EMPTY_STEPS_CONTEXT: &[u8] = b"{}";
const DIRECT_EMPTY_SPLIT_RESULTS: &[u8] = b"[]";
pub(super) const DIRECT_WORKFLOW_LOG_KIND: &[u8] = b"workflow_log";
pub(super) const DIRECT_WORKFLOW_ERROR_KIND: &[u8] = b"workflow_error";
pub(super) const DIRECT_STEP_DEBUG_START_KIND: &[u8] = b"step_debug_start";
pub(super) const DIRECT_STEP_DEBUG_END_KIND: &[u8] = b"step_debug_end";
const DIRECT_BREAKPOINT_HIT_KIND: &[u8] = b"breakpoint_hit";
const DIRECT_BREAKPOINT_HIT_STATE: &[u8] = b"\"breakpoint_hit\"";
const DIRECT_EXTERNAL_INPUT_REQUESTED_KIND: &[u8] = b"external_input_requested";
const DIRECT_AGENT_EMPTY_INTEGRATION_ID: &[u8] = b"";
const DIRECT_AGENT_EMPTY_PARAMETERS: &[u8] = b"{}";
/// Workflow output for an implicit finish (a terminal step with no explicit
/// `Finish`); matches the generated compiler's `Ok(Value::Null)`.
const DIRECT_OUTPUT_NULL: &[u8] = b"null";
pub(super) const DIRECT_AGENT_RATE_LIMIT_WAIT: &[u8] = b"rate_limit_wait";
/// Structured failure payload emitted when a `While` step exceeds its configured
/// timeout. Generated Rust parses `WhileConfig.timeout` but does not enforce it;
/// direct mode is the first to honor the documented "if exceeded, step fails"
/// behavior, so this payload is owned by the direct emitter rather than mirrored
/// from generated code.
pub(super) const DIRECT_WHILE_TIMEOUT_ERROR: &[u8] = br#"{"code":"WHILE_TIMEOUT","message":"While step exceeded its configured timeout","category":"timeout","severity":"error"}"#;
/// Structured failure payload emitted when a `Split` step exceeds its configured
/// timeout. As with `While`, generated Rust parses `SplitConfig.timeout` without
/// enforcing it; direct mode owns this payload as the first correct
/// implementation rather than mirroring the non-enforcing baseline.
pub(super) const DIRECT_SPLIT_TIMEOUT_ERROR: &[u8] = br#"{"code":"SPLIT_TIMEOUT","message":"Split step exceeded its configured timeout","category":"timeout","severity":"error"}"#;

pub(super) const WASM_PAGE_SIZE: i32 = 65_536;
const DIRECT_STATIC_DATA_OFFSET: i32 = 256;

pub(super) fn direct_core_variables_json(
    variables: &serde_json::Value,
    workflow_id: Option<&str>,
) -> Result<Vec<u8>, DirectCompileError> {
    // Declared workflow variables arrive as `{name: {"type": ..., "value": ...}}`
    // (the DSL `Variable` struct). Workflow logic references them as
    // `variables.<name>` and expects the resolved *value*, so flatten each
    // declaration to its `value`. Entries that are not `{type, value}` structs
    // are already bare values and kept unchanged.
    let mut variables = flatten_declared_variables(variables);

    let Some(workflow_id) = workflow_id else {
        return serde_json::to_vec(&variables).map_err(DirectCompileError::Serialize);
    };

    match &mut variables {
        serde_json::Value::Object(map) => {
            map.insert(
                "_workflow_id".to_string(),
                serde_json::Value::String(workflow_id.to_string()),
            );
        }
        _ => {
            let mut map = serde_json::Map::new();
            map.insert(
                "_workflow_id".to_string(),
                serde_json::Value::String(workflow_id.to_string()),
            );
            map.insert("_variables".to_string(), variables);
            variables = serde_json::Value::Object(map);
        }
    }

    serde_json::to_vec(&variables).map_err(DirectCompileError::Serialize)
}

/// Flatten declared workflow variables (`{name: {"type", "value"}}`, the DSL
/// `Variable` struct) to `{name: value}` so `variables.<name>` references resolve
/// to the value rather than the declaration struct. Entries that are not
/// `{type, value}` structs are already bare values and returned unchanged.
fn flatten_declared_variables(variables: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(map) = variables else {
        return variables.clone();
    };
    serde_json::Value::Object(
        map.iter()
            .map(|(name, decl)| {
                let value = match decl {
                    serde_json::Value::Object(fields)
                        if fields.contains_key("type") && fields.contains_key("value") =>
                    {
                        fields
                            .get("value")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)
                    }
                    other => other.clone(),
                };
                (name.clone(), value)
            })
            .collect(),
    )
}

#[derive(Debug, Clone)]
pub(super) struct DirectCoreStaticData {
    pub(super) manifest: DirectDataSegment,
    pub(super) variables: DirectDataSegment,
    pub(super) steps: DirectDataSegment,
    pub(super) split_empty_results: DirectDataSegment,
    pub(super) workflow_log_kind: DirectDataSegment,
    pub(super) workflow_error_kind: DirectDataSegment,
    pub(super) step_debug_start_kind: DirectDataSegment,
    pub(super) step_debug_end_kind: DirectDataSegment,
    pub(super) breakpoint_hit_kind: DirectDataSegment,
    pub(super) breakpoint_hit_state: DirectDataSegment,
    pub(super) external_input_requested_kind: DirectDataSegment,
    pub(super) agent_empty_integration_id: DirectDataSegment,
    pub(super) agent_empty_parameters: DirectDataSegment,
    pub(super) output_null: DirectDataSegment,
    pub(super) agent_rate_limit_wait: DirectDataSegment,
    pub(super) while_timeout_error: DirectDataSegment,
    pub(super) split_timeout_error: DirectDataSegment,
    step_ids: BTreeMap<String, DirectDataSegment>,
    agent_capability_ids: BTreeMap<u32, DirectDataSegment>,
    /// Agents with a literal `connection_id`. Not baked — the stdlib injects the
    /// connection from the manifest (`agent-connection-input`); this only gates
    /// the pre-invoke injection call.
    agent_connection_literals: BTreeSet<u32>,
    /// Agents whose connection is a resolvable `connection_ref` (evaluated at
    /// runtime), not a baked literal. The invoke path resolves these via the
    /// stdlib `resolve-connection-id` instead of reading a static segment.
    agent_connection_refs: BTreeSet<u32>,
    pub(super) heap_base: i32,
    pub(super) memory_min_pages: u64,
}

impl DirectCoreStaticData {
    #[cfg(test)]
    pub(super) fn new(
        graph: &DirectGraphManifest,
        manifest_json: &[u8],
        variables_json: &[u8],
        steps_json: &[u8],
    ) -> Result<Self, DirectCompileError> {
        Self::new_with_child_workflows(graph, &[], manifest_json, variables_json, steps_json)
    }

    pub(super) fn new_with_child_workflows(
        graph: &DirectGraphManifest,
        child_workflows: &[DirectChildWorkflowGraphManifest],
        manifest_json: &[u8],
        variables_json: &[u8],
        steps_json: &[u8],
    ) -> Result<Self, DirectCompileError> {
        let mut offset = DIRECT_STATIC_DATA_OFFSET;
        let manifest = DirectDataSegment::new(offset, manifest_json);
        offset = align_i32(checked_offset_add(offset, manifest_json.len())?, 4);

        let variables = DirectDataSegment::new(offset, variables_json);
        offset = align_i32(checked_offset_add(offset, variables_json.len())?, 4);

        let steps = DirectDataSegment::new(offset, steps_json);
        offset = align_i32(checked_offset_add(offset, steps_json.len())?, 16);

        let split_empty_results = DirectDataSegment::new(offset, DIRECT_EMPTY_SPLIT_RESULTS);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_EMPTY_SPLIT_RESULTS.len())?,
            16,
        );

        let workflow_log_kind = DirectDataSegment::new(offset, DIRECT_WORKFLOW_LOG_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_WORKFLOW_LOG_KIND.len())?,
            16,
        );

        let workflow_error_kind = DirectDataSegment::new(offset, DIRECT_WORKFLOW_ERROR_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_WORKFLOW_ERROR_KIND.len())?,
            16,
        );

        let step_debug_start_kind = DirectDataSegment::new(offset, DIRECT_STEP_DEBUG_START_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_STEP_DEBUG_START_KIND.len())?,
            16,
        );

        let step_debug_end_kind = DirectDataSegment::new(offset, DIRECT_STEP_DEBUG_END_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_STEP_DEBUG_END_KIND.len())?,
            16,
        );

        let breakpoint_hit_kind = DirectDataSegment::new(offset, DIRECT_BREAKPOINT_HIT_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_BREAKPOINT_HIT_KIND.len())?,
            16,
        );

        let breakpoint_hit_state = DirectDataSegment::new(offset, DIRECT_BREAKPOINT_HIT_STATE);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_BREAKPOINT_HIT_STATE.len())?,
            16,
        );

        let external_input_requested_kind =
            DirectDataSegment::new(offset, DIRECT_EXTERNAL_INPUT_REQUESTED_KIND);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_EXTERNAL_INPUT_REQUESTED_KIND.len())?,
            16,
        );

        let agent_empty_integration_id =
            DirectDataSegment::new(offset, DIRECT_AGENT_EMPTY_INTEGRATION_ID);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_AGENT_EMPTY_INTEGRATION_ID.len())?,
            16,
        );

        let agent_empty_parameters = DirectDataSegment::new(offset, DIRECT_AGENT_EMPTY_PARAMETERS);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_AGENT_EMPTY_PARAMETERS.len())?,
            16,
        );

        let output_null = DirectDataSegment::new(offset, DIRECT_OUTPUT_NULL);
        offset = align_i32(checked_offset_add(offset, DIRECT_OUTPUT_NULL.len())?, 16);

        let agent_rate_limit_wait = DirectDataSegment::new(offset, DIRECT_AGENT_RATE_LIMIT_WAIT);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_AGENT_RATE_LIMIT_WAIT.len())?,
            16,
        );

        let while_timeout_error = DirectDataSegment::new(offset, DIRECT_WHILE_TIMEOUT_ERROR);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_WHILE_TIMEOUT_ERROR.len())?,
            16,
        );

        let split_timeout_error = DirectDataSegment::new(offset, DIRECT_SPLIT_TIMEOUT_ERROR);
        offset = align_i32(
            checked_offset_add(offset, DIRECT_SPLIT_TIMEOUT_ERROR.len())?,
            16,
        );

        let mut step_ids = BTreeMap::new();
        collect_static_step_ids(graph, &mut offset, &mut step_ids)?;
        for child in child_workflows {
            collect_static_step_ids(&child.graph, &mut offset, &mut step_ids)?;
        }

        let mut agent_capability_ids = BTreeMap::new();
        let mut agent_connection_literals = BTreeSet::new();
        let mut agent_connection_refs = BTreeSet::new();
        collect_static_agent_data(
            graph,
            &mut offset,
            &mut agent_capability_ids,
            &mut agent_connection_literals,
            &mut agent_connection_refs,
        )?;
        for child in child_workflows {
            collect_static_agent_data(
                &child.graph,
                &mut offset,
                &mut agent_capability_ids,
                &mut agent_connection_literals,
                &mut agent_connection_refs,
            )?;
        }

        let memory_min_pages = wasm_pages_for_bytes(offset)?;
        Ok(Self {
            manifest,
            variables,
            steps,
            split_empty_results,
            workflow_log_kind,
            workflow_error_kind,
            step_debug_start_kind,
            step_debug_end_kind,
            breakpoint_hit_kind,
            breakpoint_hit_state,
            external_input_requested_kind,
            agent_empty_integration_id,
            agent_empty_parameters,
            output_null,
            agent_rate_limit_wait,
            while_timeout_error,
            split_timeout_error,
            step_ids,
            agent_capability_ids,
            agent_connection_literals,
            agent_connection_refs,
            heap_base: offset,
            memory_min_pages,
        })
    }

    pub(super) fn step_id(&self, step_id: &str) -> Result<&DirectDataSegment, DirectCompileError> {
        self.step_ids.get(step_id).ok_or_else(|| {
            DirectCompileError::Component(format!("missing direct static step id '{step_id}'"))
        })
    }

    pub(super) fn agent_capability_id(
        &self,
        agent_id: u32,
    ) -> Result<&DirectDataSegment, DirectCompileError> {
        self.agent_capability_ids.get(&agent_id).ok_or_else(|| {
            DirectCompileError::Component(format!(
                "missing direct static Agent capability id {agent_id}"
            ))
        })
    }

    /// True when the Agent has a connection to inject into its input — a literal
    /// `connection_id` or a resolvable `connection_ref`. Gates the pre-invoke
    /// `agent-connection-input` call (the single connection channel).
    pub(super) fn agent_has_connection(&self, agent_id: u32) -> bool {
        self.agent_connection_literals.contains(&agent_id)
            || self.agent_connection_refs.contains(&agent_id)
    }

    pub(super) fn data_segments(&self) -> Vec<&DirectDataSegment> {
        let mut segments = vec![
            &self.manifest,
            &self.variables,
            &self.steps,
            &self.split_empty_results,
            &self.workflow_log_kind,
            &self.workflow_error_kind,
            &self.step_debug_start_kind,
            &self.step_debug_end_kind,
            &self.breakpoint_hit_kind,
            &self.breakpoint_hit_state,
            &self.external_input_requested_kind,
            &self.agent_empty_integration_id,
            &self.agent_empty_parameters,
            &self.output_null,
            &self.agent_rate_limit_wait,
            &self.while_timeout_error,
            &self.split_timeout_error,
        ];
        segments.extend(self.step_ids.values());
        segments.extend(self.agent_capability_ids.values());
        segments
    }
}

fn collect_static_step_ids(
    graph: &DirectGraphManifest,
    offset: &mut i32,
    step_ids: &mut BTreeMap<String, DirectDataSegment>,
) -> Result<(), DirectCompileError> {
    for step in &graph.steps {
        if !step_ids.contains_key(&step.id) {
            let segment = DirectDataSegment::new(*offset, step.id.as_bytes());
            *offset = align_i32(checked_offset_add(*offset, step.id.len())?, 16);
            step_ids.insert(step.id.clone(), segment);
        }
        for nested in &step.nested_graphs {
            collect_static_step_ids(&nested.graph, offset, step_ids)?;
        }
    }
    // Intern edge labels too (string-interning map). An AiAgent tool edge's label
    // is the advertised tool name; WaitForSignal-as-tool builds its per-call
    // signal id `…/{ai_step}.tool.{label}.{call}` and so needs the label segment.
    for edge in &graph.edges {
        if let Some(label) = &edge.label
            && !step_ids.contains_key(label)
        {
            let segment = DirectDataSegment::new(*offset, label.as_bytes());
            *offset = align_i32(checked_offset_add(*offset, label.len())?, 16);
            step_ids.insert(label.clone(), segment);
        }
    }
    Ok(())
}

fn collect_static_agent_data(
    graph: &DirectGraphManifest,
    offset: &mut i32,
    agent_capability_ids: &mut BTreeMap<u32, DirectDataSegment>,
    agent_connection_literals: &mut BTreeSet<u32>,
    agent_connection_refs: &mut BTreeSet<u32>,
) -> Result<(), DirectCompileError> {
    for agent in &graph.agents {
        let segment = DirectDataSegment::new(*offset, agent.capability_id.as_bytes());
        *offset = align_i32(checked_offset_add(*offset, agent.capability_id.len())?, 16);
        agent_capability_ids.insert(agent.id, segment);

        // Both connection forms are resolved from the manifest at the invoke
        // boundary (`agent-connection-input`) and injected into the input — no
        // id is baked. Record which agents have a connection so the emitter
        // knows to emit the injection call; a `connection_ref` wins at runtime.
        if agent.connection_ref.is_some() {
            agent_connection_refs.insert(agent.id);
        }
        if agent
            .connection_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
        {
            agent_connection_literals.insert(agent.id);
        }
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_static_agent_data(
                &nested.graph,
                offset,
                agent_capability_ids,
                agent_connection_literals,
                agent_connection_refs,
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct DirectDataSegment {
    pub(super) offset: i32,
    pub(super) data: Vec<u8>,
}

impl DirectDataSegment {
    fn new(offset: i32, data: &[u8]) -> Self {
        Self {
            offset,
            data: data.to_vec(),
        }
    }

    pub(super) fn len_i32(&self) -> i32 {
        i32::try_from(self.data.len()).expect("direct data length already checked")
    }
}

fn checked_offset_add(offset: i32, len: usize) -> Result<i32, DirectCompileError> {
    let len = i32::try_from(len).map_err(|_| {
        DirectCompileError::Component(
            "direct workflow static data exceeds i32 address space".into(),
        )
    })?;
    offset.checked_add(len).ok_or_else(|| {
        DirectCompileError::Component("direct workflow static data offset overflow".into())
    })
}

fn align_i32(value: i32, align: i32) -> i32 {
    debug_assert!(align > 0 && (align & (align - 1)) == 0);
    (value + align - 1) & !(align - 1)
}

fn wasm_pages_for_bytes(bytes: i32) -> Result<u64, DirectCompileError> {
    let bytes = u64::try_from(bytes)
        .map_err(|_| DirectCompileError::Component("negative direct memory size".into()))?;
    Ok(bytes.div_ceil(WASM_PAGE_SIZE as u64).max(1))
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{
        DirectAgentManifest, DirectGraphManifest, DirectNestedGraphManifest, DirectStepManifest,
    };
    use super::*;

    #[test]
    fn static_data_collects_nested_step_and_agent_segments() {
        let nested = graph(
            "nested",
            vec![
                step("shared", "Finish", vec![]),
                step("nested-only", "Finish", vec![]),
            ],
            vec![agent_manifest(2, "nested-capability", None)],
        );
        let root = graph(
            "root",
            vec![
                step(
                    "root",
                    "Split",
                    vec![DirectNestedGraphManifest {
                        role: "split.subgraph".to_string(),
                        graph: Box::new(nested),
                    }],
                ),
                step("shared", "Finish", vec![]),
            ],
            vec![agent_manifest(1, "root-capability", Some("conn-1"))],
        );

        let static_data =
            DirectCoreStaticData::new(&root, b"manifest", b"{\"v\":1}", DIRECT_EMPTY_STEPS_CONTEXT)
                .expect("static data");

        assert_eq!(static_data.step_ids.len(), 3);
        assert_eq!(static_data.step_id("root").expect("root").data, b"root");
        assert_eq!(
            static_data.step_id("nested-only").expect("nested").data,
            b"nested-only"
        );
        assert_eq!(
            static_data
                .agent_capability_id(1)
                .expect("root capability")
                .data,
            b"root-capability"
        );
        assert_eq!(
            static_data
                .agent_capability_id(2)
                .expect("nested capability")
                .data,
            b"nested-capability"
        );
        // Agent 1 has a literal connection (injected at the invoke boundary, not
        // baked); agent 2 (nested) has none.
        assert!(static_data.agent_has_connection(1));
        assert!(!static_data.agent_has_connection(2));
        assert_eq!(static_data.memory_min_pages, 1);
        assert_eq!(static_data.heap_base % 16, 0);
    }

    #[test]
    fn wasm_pages_for_bytes_rounds_up_and_uses_minimum_one_page() {
        assert_eq!(wasm_pages_for_bytes(0).expect("zero"), 1);
        assert_eq!(wasm_pages_for_bytes(WASM_PAGE_SIZE).expect("one page"), 1);
        assert_eq!(
            wasm_pages_for_bytes(WASM_PAGE_SIZE + 1).expect("two pages"),
            2
        );
        assert!(wasm_pages_for_bytes(-1).is_err());
    }

    #[test]
    fn variables_json_injects_workflow_id_and_wraps_non_object_variables() {
        let bytes = direct_core_variables_json(&serde_json::json!({"existing": true}), Some("wf"))
            .expect("object variables");
        let variables: serde_json::Value = serde_json::from_slice(&bytes).expect("object json");
        assert_eq!(variables["_workflow_id"], "wf");
        assert_eq!(variables["existing"], true);

        let bytes = direct_core_variables_json(&serde_json::json!(["value"]), Some("wf"))
            .expect("array variables");
        let variables: serde_json::Value = serde_json::from_slice(&bytes).expect("array json");
        assert_eq!(variables["_workflow_id"], "wf");
        assert_eq!(variables["_variables"], serde_json::json!(["value"]));
    }

    #[test]
    fn variables_json_preserves_variables_without_compile_workflow_id() {
        let bytes = direct_core_variables_json(&serde_json::json!({"user": "value"}), None)
            .expect("variables");
        let variables: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(variables, serde_json::json!({"user": "value"}));
    }

    #[test]
    fn variables_json_flattens_declared_variable_structs_to_values() {
        // Declared variables are `{name: {"type", "value"}}`; the static segment
        // must expose the value so `variables.<name>` resolves to it (not the
        // declaration struct). A variable whose value is itself an object with a
        // `value` field is flattened exactly one level.
        let declared = serde_json::json!({
            "greeting": { "type": "string", "value": "hello" },
            "count": { "type": "integer", "value": 3, "description": "n" },
            "nested": { "type": "object", "value": { "value": "inner", "k": 1 } }
        });
        let bytes = direct_core_variables_json(&declared, Some("wf")).expect("variables");
        let variables: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(variables["greeting"], "hello");
        assert_eq!(variables["count"], 3);
        assert_eq!(
            variables["nested"],
            serde_json::json!({ "value": "inner", "k": 1 })
        );
        assert_eq!(variables["_workflow_id"], "wf");
    }

    fn graph(
        entry_point: &str,
        steps: Vec<DirectStepManifest>,
        agents: Vec<DirectAgentManifest>,
    ) -> DirectGraphManifest {
        DirectGraphManifest {
            name: None,
            entry_point: entry_point.to_string(),
            durable: false,
            rate_limit_budget_ms: 0,
            variables: serde_json::json!({}),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            steps,
            mappings: vec![],
            conditions: vec![],
            splits: vec![],
            whiles: vec![],
            filters: vec![],
            switches: vec![],
            group_bys: vec![],
            delays: vec![],
            logs: vec![],
            errors: vec![],
            agents,
            edges: vec![],
        }
    }

    fn step(
        id: &str,
        step_type: &str,
        nested_graphs: Vec<DirectNestedGraphManifest>,
    ) -> DirectStepManifest {
        DirectStepManifest {
            id: id.to_string(),
            step_type: step_type.to_string(),
            name: None,
            body: serde_json::json!({}),
            nested_graphs,
        }
    }

    fn agent_manifest(
        id: u32,
        capability_id: &str,
        connection_id: Option<&str>,
    ) -> DirectAgentManifest {
        DirectAgentManifest {
            id,
            step_id: format!("agent-{id}"),
            name: None,
            step_type: "Agent".to_string(),
            purpose: "agent.config".to_string(),
            agent_id: "utils".to_string(),
            capability_id: capability_id.to_string(),
            connection_id: connection_id.map(ToOwned::to_owned),
            connection_ref: None,
            durable: false,
            rate_limited: false,
            input_mapping_id: 0,
            required_inputs: vec![],
            max_retries: None,
            retry_delay: None,
            timeout: None,
        }
    }
}
