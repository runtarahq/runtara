// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! JSON semantics used by direct-emitted workflow components.
//!
//! This module is the pure Rust implementation behind the
//! `runtara:workflow-stdlib/json` WIT contract. The component wrapper can keep
//! a parsed [`DirectJsonManifest`] after `init-manifest` and delegate the WIT
//! functions here.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::agent_input_validation::{
    AgentInputMissingReason, AgentInputValidationError, MissingAgentInput,
};
use crate::conditions::{is_truthy, to_number, values_equal};
use crate::switch_helpers::process_switch_output;
use crate::template::render_template;

/// Parsed direct-workflow manifest data needed by JSON stdlib calls.
#[derive(Debug, Clone)]
pub struct DirectJsonManifest {
    steps: BTreeMap<String, DirectJsonStep>,
    child_workflows: BTreeMap<String, DirectJsonChildWorkflow>,
    mappings: BTreeMap<u32, DirectJsonMapping>,
    conditions: BTreeMap<u32, DirectJsonCondition>,
    splits: BTreeMap<u32, DirectJsonSplit>,
    whiles: BTreeMap<u32, DirectJsonWhile>,
    filters: BTreeMap<u32, DirectJsonFilter>,
    switches: BTreeMap<u32, DirectJsonSwitch>,
    group_bys: BTreeMap<u32, DirectJsonGroupBy>,
    delays: BTreeMap<u32, DirectJsonDelay>,
    logs: BTreeMap<u32, DirectJsonLog>,
    errors: BTreeMap<u32, DirectJsonError>,
    agents: BTreeMap<u32, DirectJsonAgent>,
    debug_start_ms: RefCell<BTreeMap<String, i64>>,
}

/// Raw Agent retry payload plus generated-Rust-compatible retry classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectJsonAgentRetryError {
    pub payload: Vec<u8>,
    pub retryable: bool,
    pub rate_limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectJsonWorkflowRetryInfo {
    retryable: bool,
    rate_limited: bool,
    retry_after_ms: Option<u64>,
}

impl DirectJsonManifest {
    /// Parse direct manifest JSON emitted by `runtara-workflows`.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: ManifestWire = serde_json::from_slice(bytes)
            .map_err(|err| format!("failed to parse direct manifest: {err}"))?;
        let mut collections = DirectJsonManifestCollections::default();
        collect_graph_manifest(&manifest.graph, &mut collections)?;
        for child in &manifest.child_workflows {
            if collections
                .child_workflows
                .insert(
                    child.step_id.clone(),
                    DirectJsonChildWorkflow {
                        step_id: child.step_id.clone(),
                        workflow_id: child.workflow_id.clone(),
                        variables: child.graph.variables.clone(),
                        input_schema: child.graph.input_schema.clone(),
                    },
                )
                .is_some()
            {
                return Err(format!(
                    "duplicate direct child workflow step id '{}'",
                    child.step_id
                ));
            }
            collect_graph_manifest(&child.graph, &mut collections)?;
        }
        Ok(Self {
            steps: collections.steps,
            child_workflows: collections.child_workflows,
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
            debug_start_ms: RefCell::new(BTreeMap::new()),
        })
    }

    /// Apply a manifest mapping to a source JSON envelope.
    pub fn apply_mapping(&self, mapping_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse mapping source: {err}"))?;
        let mapping = self
            .mappings
            .get(&mapping_id)
            .ok_or_else(|| format!("unknown direct mapping id {mapping_id}"))?;
        let mut output = apply_input_mapping(&mapping.value, &source)?;
        if mapping.purpose == "finish.inputMapping" {
            output = output.get("outputs").cloned().unwrap_or(output);
        }
        serde_json::to_vec(&output)
            .map_err(|err| format!("failed to serialize mapping output: {err}"))
    }

    /// Evaluate a manifest condition against a source JSON envelope.
    pub fn eval_condition(&self, condition_id: u32, source: &[u8]) -> Result<bool, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse condition source: {err}"))?;
        let condition = self
            .conditions
            .get(&condition_id)
            .ok_or_else(|| format!("unknown direct condition id {condition_id}"))?;
        eval_condition_expression(&condition.value, &source)
    }

    /// Execute a manifest routing Switch config and return the selected route.
    pub fn process_switch(&self, switch_id: u32, source: &[u8]) -> Result<String, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse process-switch source: {err}"))?;
        let switch = self
            .switches
            .get(&switch_id)
            .ok_or_else(|| format!("unknown direct Switch id {switch_id}"))?;
        apply_switch(&switch.value, &source).map(|result| result.route)
    }

    /// Resolve and normalize a Split step's iteration items.
    pub fn split_items(&self, split_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse split source: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let items = split_items(split, &source)?;
        serde_json::to_vec(&items).map_err(|err| format!("failed to serialize Split items: {err}"))
    }

    /// Return the normalized Split iteration item count.
    pub fn split_item_count(&self, split_id: u32, source: &[u8]) -> Result<u32, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse split source: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let items = split_items(split, &source)?;
        let count = items
            .as_array()
            .map(Vec::len)
            .expect("split_items always returns a JSON array");
        u32::try_from(count).map_err(|_| {
            format!(
                "Split step '{}' produced too many iteration items for direct Wasm",
                split.step_id
            )
        })
    }

    /// Return one normalized Split iteration item by index.
    pub fn split_item(&self, split_id: u32, source: &[u8], index: u32) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse split source: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let items = split_items(split, &source)?;
        let items = items
            .as_array()
            .expect("split_items always returns a JSON array");
        let item = items.get(index as usize).ok_or_else(|| {
            format!(
                "Split step '{}' item index {index} is out of bounds for {} item(s)",
                split.step_id,
                items.len()
            )
        })?;
        serde_json::to_vec(item).map_err(|err| format!("failed to serialize Split item: {err}"))
    }

    /// Build generated-code-compatible variables for one Split iteration.
    pub fn split_iteration_variables(
        &self,
        split_id: u32,
        source: &[u8],
        item: &[u8],
        index: u32,
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse split source: {err}"))?;
        let item: Value = serde_json::from_slice(item)
            .map_err(|err| format!("failed to parse Split item: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let variables = split_iteration_variables(split, &source, item, index)?;
        serde_json::to_vec(&Value::Object(variables))
            .map_err(|err| format!("failed to serialize Split iteration variables: {err}"))
    }

    /// Validate one Split iteration input item against the Split input schema.
    pub fn split_validate_input(
        &self,
        split_id: u32,
        item: &[u8],
        index: u32,
    ) -> Result<(), String> {
        let item: Value = serde_json::from_slice(item)
            .map_err(|err| format!("failed to parse Split item: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        validate_split_schema(
            &item,
            &split.input_schema,
            &format!("Split '{}' iteration {index}: input", split.step_id),
        )
    }

    /// Validate one Split iteration output against the Split output schema.
    pub fn split_validate_output(
        &self,
        split_id: u32,
        output: &[u8],
        index: u32,
    ) -> Result<(), String> {
        let output: Value = serde_json::from_slice(output)
            .map_err(|err| format!("failed to parse Split iteration output: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        validate_split_schema(
            &output,
            &split.output_schema,
            &format!("Split '{}' iteration {index}: output", split.step_id),
        )
    }

    /// Build a Split result accumulator matching the step's failure policy.
    pub fn split_initial_results(&self, split_id: u32) -> Result<Vec<u8>, String> {
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let value = if split_dont_stop_on_failed(split) {
            serde_json::json!({
                "success": [],
                "error": [],
                "aborted": [],
                "unknown": [],
                "skipped": []
            })
        } else {
            Value::Array(Vec::new())
        };
        serde_json::to_vec(&value)
            .map_err(|err| format!("failed to serialize Split result accumulator: {err}"))
    }

    /// Append one Split iteration output to a JSON result array.
    pub fn split_append_output(
        &self,
        split_id: u32,
        results: &[u8],
        output: &[u8],
    ) -> Result<Vec<u8>, String> {
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let mut results: Value = serde_json::from_slice(results)
            .map_err(|err| format!("failed to parse Split results: {err}"))?;
        let output: Value = serde_json::from_slice(output)
            .map_err(|err| format!("failed to parse Split iteration output: {err}"))?;
        if split_dont_stop_on_failed(split) {
            split_accumulator_array_mut(&mut results, split, "success")?.push(output);
            serde_json::to_vec(&results)
        } else {
            let results = results.as_array_mut().ok_or_else(|| {
                format!(
                    "Split step '{}' internal result accumulator must be an array",
                    split.step_id
                )
            })?;
            results.push(output);
            serde_json::to_vec(results)
        }
        .map_err(|err| format!("failed to serialize Split result accumulator: {err}"))
    }

    /// Append one generated-code-compatible Split iteration failure.
    pub fn split_append_error(
        &self,
        split_id: u32,
        results: &[u8],
        error: String,
        index: u32,
    ) -> Result<Vec<u8>, String> {
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        if !split_dont_stop_on_failed(split) {
            return Err(format!(
                "Split step '{}' cannot append errors when dontStopOnFailed is false",
                split.step_id
            ));
        }
        let mut results: Value = serde_json::from_slice(results)
            .map_err(|err| format!("failed to parse Split results: {err}"))?;
        split_accumulator_array_mut(&mut results, split, "error")?.push(serde_json::json!({
            "error": error,
            "index": index
        }));
        serde_json::to_vec(&results)
            .map_err(|err| format!("failed to serialize Split result accumulator: {err}"))
    }

    /// Store Split iteration results in the generated-code-compatible steps context.
    pub fn split_output(
        &self,
        split_id: u32,
        source: &[u8],
        results: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse split source: {err}"))?;
        let results: Value = serde_json::from_slice(results)
            .map_err(|err| format!("failed to parse Split results: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let steps = if split_dont_stop_on_failed(split) {
            let mut steps = source
                .get("steps")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            steps.insert(
                split.step_id.clone(),
                split_dont_stop_result(split, &source, results)?,
            );
            steps
        } else {
            insert_step_output(
                &source,
                &split.step_id,
                split.name.as_deref(),
                "Split",
                results,
                None,
            )
        };
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize Split steps context: {err}"))
    }

    /// Build the generated-code-compatible durable cache key for a Split step.
    pub fn split_cache_key(&self, split_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse Split cache-key source: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;

        Ok(split_cache_key(split, &source).into_bytes())
    }

    /// Build the final generated-code-compatible Split step result.
    pub fn split_result(
        &self,
        split_id: u32,
        source: &[u8],
        results: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse Split result source: {err}"))?;
        let results: Value = serde_json::from_slice(results)
            .map_err(|err| format!("failed to parse Split results: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let result = split_result(split, &source, results)?;
        serde_json::to_vec(&result)
            .map_err(|err| format!("failed to serialize Split step result: {err}"))
    }

    /// Store a previously computed Split step result in the steps context.
    pub fn split_output_from_result(
        &self,
        split_id: u32,
        source: &[u8],
        step_result: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse Split output source: {err}"))?;
        let step_result: Value = serde_json::from_slice(step_result)
            .map_err(|err| format!("failed to parse Split step result: {err}"))?;
        let split = self
            .splits
            .get(&split_id)
            .ok_or_else(|| format!("unknown direct Split id {split_id}"))?;
        let mut steps = source
            .get("steps")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        steps.insert(split.step_id.clone(), step_result);
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize Split steps context: {err}"))
    }

    /// Return the configured While maximum iterations, or the generated-code default.
    pub fn while_max_iterations(&self, while_id: u32) -> Result<u32, String> {
        let while_step = self
            .whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        while_max_iterations(while_step)
    }

    /// Build the initial generated-code-compatible While state.
    pub fn while_initial_state(&self, while_id: u32) -> Result<Vec<u8>, String> {
        self.whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        serde_json::to_vec(&serde_json::json!({
            "index": 0,
            "outputs": Value::Null,
        }))
        .map_err(|err| format!("failed to serialize While initial state: {err}"))
    }

    /// Inject the current generated-code-compatible `loop` context into a source envelope.
    pub fn while_condition_source(
        &self,
        while_id: u32,
        source: &[u8],
        state: &[u8],
    ) -> Result<Vec<u8>, String> {
        let while_step = self
            .whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        let mut source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse While condition source: {err}"))?;
        let state = parse_while_state(while_step, state)?;
        let source = source.as_object_mut().ok_or_else(|| {
            format!(
                "While step '{}' condition source must be a JSON object",
                while_step.step_id
            )
        })?;
        source.insert("loop".to_string(), while_loop_context(&state));
        serde_json::to_vec(&Value::Object(source.clone()))
            .map_err(|err| format!("failed to serialize While condition source: {err}"))
    }

    /// Evaluate a While condition against a source envelope that already has loop context.
    pub fn while_condition(&self, while_id: u32, source: &[u8]) -> Result<bool, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse While condition source: {err}"))?;
        let while_step = self
            .whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        eval_condition_expression(&while_step.condition, &source)
    }

    /// Build generated-code-compatible variables for one While iteration.
    pub fn while_iteration_variables(
        &self,
        while_id: u32,
        variables: &[u8],
        state: &[u8],
    ) -> Result<Vec<u8>, String> {
        let while_step = self
            .whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        let variables: Value = serde_json::from_slice(variables)
            .map_err(|err| format!("failed to parse While variables: {err}"))?;
        let state = parse_while_state(while_step, state)?;
        let variables = while_iteration_variables(while_step, variables, &state);
        serde_json::to_vec(&Value::Object(variables))
            .map_err(|err| format!("failed to serialize While iteration variables: {err}"))
    }

    /// Advance a While state after one successful subgraph iteration output.
    pub fn while_advance_state(
        &self,
        while_id: u32,
        state: &[u8],
        output: &[u8],
    ) -> Result<Vec<u8>, String> {
        let while_step = self
            .whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        let mut state = parse_while_state(while_step, state)?;
        let output: Value = serde_json::from_slice(output)
            .map_err(|err| format!("failed to parse While iteration output: {err}"))?;
        state.index = state.index.checked_add(1).ok_or_else(|| {
            format!(
                "While step '{}' iteration index overflowed u32",
                while_step.step_id
            )
        })?;
        state.outputs = output;
        serde_json::to_vec(&state.to_value())
            .map_err(|err| format!("failed to serialize While state: {err}"))
    }

    /// Store final While loop outputs in the generated-code-compatible steps context.
    pub fn while_output(
        &self,
        while_id: u32,
        source: &[u8],
        state: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse While source: {err}"))?;
        let while_step = self
            .whiles
            .get(&while_id)
            .ok_or_else(|| format!("unknown direct While id {while_id}"))?;
        let state = parse_while_state(while_step, state)?;
        let output = serde_json::json!({
            "iterations": state.index,
            "outputs": state.outputs,
        });
        let steps = insert_step_output(
            &source,
            &while_step.step_id,
            while_step.name.as_deref(),
            "While",
            output,
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize While steps context: {err}"))
    }

    /// Execute a manifest Filter config and return an updated steps context.
    pub fn filter(&self, filter_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse filter source: {err}"))?;
        let filter = self
            .filters
            .get(&filter_id)
            .ok_or_else(|| format!("unknown direct Filter id {filter_id}"))?;
        let output = apply_filter(&filter.value, &source)?;
        let steps = insert_step_output(
            &source,
            &filter.step_id,
            filter.name.as_deref(),
            "Filter",
            output,
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize filter steps context: {err}"))
    }

    /// Execute a manifest value Switch config and return an updated steps context.
    pub fn value_switch(&self, switch_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse value-switch source: {err}"))?;
        let switch = self
            .switches
            .get(&switch_id)
            .ok_or_else(|| format!("unknown direct Switch id {switch_id}"))?;
        let result = apply_switch(&switch.value, &source)?;
        let route = switch_is_routing(&switch.value).then_some(result.route.as_str());
        let steps = insert_step_output(
            &source,
            &switch.step_id,
            switch.name.as_deref(),
            "Switch",
            result.output,
            route,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize value-switch steps context: {err}"))
    }

    /// Execute a manifest GroupBy config and return an updated steps context.
    pub fn group_by(&self, group_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse group-by source: {err}"))?;
        let group_by = self
            .group_bys
            .get(&group_id)
            .ok_or_else(|| format!("unknown direct GroupBy id {group_id}"))?;
        let output = apply_group_by(&group_by.value, &source)?;
        let steps = insert_step_output(
            &source,
            &group_by.step_id,
            group_by.name.as_deref(),
            "GroupBy",
            output,
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize group-by steps context: {err}"))
    }

    /// Resolve a Delay duration mapping to milliseconds.
    pub fn delay_duration_ms(&self, delay_id: u32, source: &[u8]) -> Result<u64, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse delay source: {err}"))?;
        let delay = self
            .delays
            .get(&delay_id)
            .ok_or_else(|| format!("unknown direct Delay id {delay_id}"))?;
        let duration = apply_mapping_value(&delay.duration_ms, &source)?;
        duration
            .as_u64()
            .or_else(|| duration.as_f64().map(|ms| ms as u64))
            .ok_or_else(|| {
                format!(
                    "Delay step '{}': duration_ms must be a number, got: {}",
                    delay.step_id, duration
                )
            })
    }

    /// Store a Delay output in the generated-code-compatible steps context.
    pub fn delay(&self, delay_id: u32, source: &[u8], duration_ms: u64) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse delay source: {err}"))?;
        let delay = self
            .delays
            .get(&delay_id)
            .ok_or_else(|| format!("unknown direct Delay id {delay_id}"))?;
        let mut steps = source
            .get("steps")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        steps.insert(delay.step_id.clone(), delay_step_value(delay, duration_ms));
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize delay steps context: {err}"))
    }

    /// Build the generated-code-compatible checkpoint key for a step breakpoint.
    pub fn breakpoint_key(&self, step_id: &str, source: &[u8]) -> Result<String, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse breakpoint-key source: {err}"))?;
        self.steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct breakpoint step '{step_id}'"))?;

        let loop_indices = source
            .get("variables")
            .and_then(Value::as_object)
            .and_then(|vars| vars.get("_loop_indices"))
            .and_then(Value::as_array)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|index| index.to_string())
                    .collect::<Vec<_>>()
                    .join("_")
            })
            .unwrap_or_default();
        if loop_indices.is_empty() {
            Ok(format!("breakpoint::{step_id}"))
        } else {
            Ok(format!("breakpoint::{step_id}::{loop_indices}"))
        }
    }

    /// Build the generated-code-compatible custom event payload for a step breakpoint.
    pub fn breakpoint_event(&self, step_id: &str, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse breakpoint-event source: {err}"))?;
        let step = self
            .steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct breakpoint step '{step_id}'"))?;
        let steps_context = source
            .get("steps")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let inputs = match step.step_type.as_str() {
            "Conditional" => source.clone(),
            "Finish" => self
                .finish_mapping(step.id.as_str())
                .map(|mapping| apply_input_mapping(&mapping.value, &source))
                .transpose()?
                .unwrap_or_else(|| Value::Object(Map::new())),
            "Filter" => {
                let filter = self
                    .filter_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Filter config for '{}'", step.id))?;
                filter
                    .value
                    .get("value")
                    .ok_or_else(|| "Filter config missing value".to_string())
                    .and_then(|value| apply_mapping_value(value, &source))?
            }
            "Switch" => {
                let switch = self
                    .switch_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Switch config for '{}'", step.id))?;
                switch_debug_inputs(&switch.value, &source)?
            }
            "GroupBy" => {
                let group_by = self
                    .group_by_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct GroupBy config for '{}'", step.id))?;
                group_by
                    .value
                    .get("value")
                    .ok_or_else(|| "GroupBy config missing value".to_string())
                    .and_then(|value| apply_mapping_value(value, &source))?
            }
            "Split" => {
                let split = self
                    .split_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Split config for '{}'", step.id))?;
                split_debug_inputs(split, &source)?
            }
            "While" => {
                let while_step = self
                    .while_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct While config for '{}'", step.id))?;
                while_debug_inputs(while_step)?
            }
            "Log" => {
                let log = self
                    .log_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Log config for '{}'", step.id))?;
                apply_log(&log.value, &source)?.context
            }
            "Error" => {
                let error = self
                    .error_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Error config for '{}'", step.id))?;
                apply_error(&error.value, &source)?.context
            }
            "Agent" => {
                let agent = self
                    .agent_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Agent config for '{}'", step.id))?;
                let mapping = self.mappings.get(&agent.input_mapping_id).ok_or_else(|| {
                    format!(
                        "missing direct Agent input mapping {} for '{}'",
                        agent.input_mapping_id, step.id
                    )
                })?;
                apply_input_mapping(&mapping.value, &source)?
            }
            "EmbedWorkflow" => source.get("data").cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        };

        serde_json::to_vec(&serde_json::json!({
            "step_id": step.id.clone(),
            "step_name": step.name.clone(),
            "step_type": step.step_type.clone(),
            "inputs": inputs,
            "steps_context": Value::Object(steps_context),
        }))
        .map_err(|err| format!("failed to serialize breakpoint event payload: {err}"))
    }

    /// Build the deterministic signal id used by generated WaitForSignal code.
    pub fn wait_signal_id(
        &self,
        step_id: &str,
        instance_id: &str,
        source: &[u8],
    ) -> Result<String, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-signal-id source: {err}"))?;
        self.wait_step(step_id)?;
        let workflow_id = source
            .get("variables")
            .and_then(Value::as_object)
            .and_then(|vars| vars.get("_workflow_id"))
            .and_then(Value::as_str)
            .unwrap_or("root");
        let indices_suffix = wait_loop_indices_suffix(&source);
        Ok(format!(
            "{instance_id}/{workflow_id}/{step_id}{indices_suffix}"
        ))
    }

    /// Resolve an optional WaitForSignal timeout mapping to milliseconds.
    pub fn wait_timeout_ms(&self, step_id: &str, source: &[u8]) -> Result<Option<u64>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-timeout source: {err}"))?;
        let step = self.wait_step(step_id)?;
        let Some(timeout_mapping) = step.body.get("timeoutMs") else {
            return Ok(None);
        };
        let timeout = apply_mapping_value(timeout_mapping, &source)?;
        match timeout {
            Value::Null => Ok(None),
            Value::Number(number) => Ok(number
                .as_u64()
                .or_else(|| number.as_f64().map(|ms| ms as u64))),
            other => Err(format!(
                "WaitForSignal step '{step_id}': timeout_ms must be a number, got: {other}"
            )),
        }
    }

    /// Build the generated-code-compatible timeout failure string.
    pub fn wait_timeout_error(
        &self,
        step_id: &str,
        signal_id: &str,
        timeout_ms: u64,
    ) -> Result<Vec<u8>, String> {
        self.wait_step(step_id)?;
        Ok(format!(
            "WaitForSignal step '{step_id}' timed out after {timeout_ms}ms waiting for signal '{signal_id}'"
        )
        .into_bytes())
    }

    /// Build generated-code-compatible inputs for a WaitForSignal `onWait` graph.
    pub fn wait_on_wait_variables(
        &self,
        step_id: &str,
        instance_id: &str,
        signal_id: &str,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.wait_step(step_id)?;
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-on-wait source: {err}"))?;
        let mut variables = source
            .get("variables")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        variables.insert(
            "_signal_id".to_string(),
            Value::String(signal_id.to_string()),
        );
        variables.insert(
            "_instance_id".to_string(),
            Value::String(instance_id.to_string()),
        );
        serde_json::to_vec(&Value::Object(variables))
            .map_err(|err| format!("failed to serialize wait-on-wait variables: {err}"))
    }

    /// Wrap a nested `onWait` graph failure exactly like generated Rust code.
    pub fn wait_on_wait_error(&self, step_id: &str, error: &[u8]) -> Result<Vec<u8>, String> {
        self.wait_step(step_id)?;
        let error = String::from_utf8_lossy(error);
        Ok(format!("WaitForSignal step '{step_id}' on_wait failed: {error}").into_bytes())
    }

    /// Return the configured WaitForSignal poll interval, defaulting to 1000ms.
    pub fn wait_poll_interval_ms(&self, step_id: &str) -> Result<u64, String> {
        let step = self.wait_step(step_id)?;
        match step.body.get("pollIntervalMs") {
            Some(Value::Number(number)) => number.as_u64().ok_or_else(|| {
                format!(
                    "WaitForSignal step '{step_id}': poll_interval_ms must be a non-negative integer"
                )
            }),
            Some(other) => Err(format!(
                "WaitForSignal step '{step_id}': poll_interval_ms must be a number, got: {other}"
            )),
            None => Ok(1000),
        }
    }

    /// Build the generated-code-compatible custom event payload for a wait step.
    pub fn wait_event(
        &self,
        step_id: &str,
        signal_id: &str,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-event source: {err}"))?;
        let step = self.wait_step(step_id)?;
        let action = step.body.get("action").and_then(Value::as_object);
        let action_key = action
            .and_then(|action| action.get("key"))
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(|key| Value::String(key.to_string()))
            .unwrap_or(Value::Null);
        let correlation = wait_action_mapping(action, "correlation", &source)?;
        let context = wait_action_mapping(action, "context", &source)?;

        let event = serde_json::json!({
            "type": "external_input_requested",
            "signal_id": signal_id,
            "step_id": step.id,
            "step_name": step.name.as_deref().unwrap_or("Unnamed"),
            "response_schema": step.body.get("responseSchema").cloned().unwrap_or(Value::Null),
            "action_key": action_key,
            "correlation": correlation,
            "context": context,
        });
        serde_json::to_vec(&event)
            .map_err(|err| format!("failed to serialize wait event payload: {err}"))
    }

    /// Build a generated-code-compatible `step_debug_start` payload for WaitForSignal.
    pub fn wait_debug_start(
        &self,
        step_id: &str,
        signal_id: &str,
        timeout_ms: Option<u64>,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let _source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-debug-start source: {err}"))?;
        let step = self.wait_step(step_id)?;
        let timestamp = timestamp_ms();
        self.debug_start_ms
            .borrow_mut()
            .insert(step_id.to_string(), timestamp);

        let mut payload = debug_event_base(step, timestamp);
        payload.insert(
            "inputs".to_string(),
            serde_json::json!({
                "signal_id": signal_id,
                "timeout_ms": timeout_ms,
                "poll_interval_ms": self.wait_poll_interval_ms(step_id)?,
                "response_schema": step.body.get("responseSchema").cloned().unwrap_or(Value::Null),
            }),
        );

        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize wait-debug-start payload: {err}"))
    }

    /// Store a WaitForSignal payload in the generated-code-compatible steps context.
    pub fn wait_output(
        &self,
        step_id: &str,
        signal_id: &str,
        signal_payload: &[u8],
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-output source: {err}"))?;
        let step = self.wait_step(step_id)?;
        let signal_payload = serde_json::from_slice(signal_payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(signal_payload).to_string()));
        let mut steps = source
            .get("steps")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        steps.insert(
            step.id.clone(),
            wait_step_value(step, signal_id, signal_payload),
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize wait steps context: {err}"))
    }

    /// Build the generated-code-compatible durable cache key for an
    /// `EmbedWorkflow` call site.
    pub fn embed_workflow_cache_key(
        &self,
        step_id: &str,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse EmbedWorkflow source: {err}"))?;
        self.embed_workflow_step(step_id)?;
        Ok(embed_workflow_cache_key(step_id, &source).into_bytes())
    }

    /// Build isolated child variables for an `EmbedWorkflow` call site and
    /// validate mapped child inputs against the child graph input schema.
    pub fn embed_workflow_variables(
        &self,
        step_id: &str,
        source: &[u8],
        child_input: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse EmbedWorkflow source: {err}"))?;
        let child_input: Value = serde_json::from_slice(child_input)
            .map_err(|err| format!("failed to parse EmbedWorkflow child input: {err}"))?;
        let child = self.child_workflow(step_id)?;
        validate_embed_child_inputs(child, &child_input)?;
        let variables = embed_child_variables(step_id, child, &source);
        serde_json::to_vec(&Value::Object(variables))
            .map_err(|err| format!("failed to serialize EmbedWorkflow child variables: {err}"))
    }

    /// Wrap a child workflow output in the generated-code-compatible
    /// `EmbedWorkflow` step result envelope.
    pub fn embed_workflow_result(
        &self,
        step_id: &str,
        _source: &[u8],
        child_output: &[u8],
    ) -> Result<Vec<u8>, String> {
        let child_output: Value = serde_json::from_slice(child_output)
            .map_err(|err| format!("failed to parse EmbedWorkflow child output: {err}"))?;
        let step = self.embed_workflow_step(step_id)?;
        let child = self.child_workflow(step_id)?;
        let result = embed_workflow_step_value(step, child, child_output);
        serde_json::to_vec(&result)
            .map_err(|err| format!("failed to serialize EmbedWorkflow result: {err}"))
    }

    /// Insert a generated-code-compatible `EmbedWorkflow` step result into the
    /// parent steps context.
    pub fn embed_workflow_output_from_result(
        &self,
        step_id: &str,
        source: &[u8],
        step_result: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse EmbedWorkflow source: {err}"))?;
        let step_result: Value = serde_json::from_slice(step_result)
            .map_err(|err| format!("failed to parse EmbedWorkflow result: {err}"))?;
        self.embed_workflow_step(step_id)?;
        let mut steps = source
            .get("steps")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        steps.insert(step_id.to_string(), step_result);
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize EmbedWorkflow steps context: {err}"))
    }

    /// Wrap a child workflow failure exactly like generated `EmbedWorkflow`
    /// code before propagating it to the parent context.
    pub fn embed_workflow_error(
        &self,
        step_id: &str,
        child_error: &[u8],
    ) -> Result<Vec<u8>, String> {
        let child_error: Value = serde_json::from_slice(child_error)
            .map_err(|err| format!("failed to parse EmbedWorkflow child error: {err}"))?;
        let step = self.embed_workflow_step(step_id)?;
        let child = self.child_workflow(step_id)?;
        let result = embed_workflow_error_value(step, child, child_error);
        serde_json::to_vec(&result)
            .map_err(|err| format!("failed to serialize EmbedWorkflow child error: {err}"))
    }

    /// Store an Agent capability output in the generated-code-compatible steps context.
    pub fn agent_output(
        &self,
        agent_id: u32,
        source: &[u8],
        output: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse agent-output source: {err}"))?;
        let output: Value = serde_json::from_slice(output)
            .map_err(|err| format!("failed to parse Agent output: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let steps = insert_step_output(
            &source,
            &agent.step_id,
            agent.name.as_deref(),
            "Agent",
            output,
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize Agent steps context: {err}"))
    }

    /// Build an Ai Agent step output context from a `chat-completion` capability
    /// result `{choice, usage}`. Extracts the final assistant text from the
    /// choice (single-shot: one iteration, no tool calls) and wraps it in the
    /// generated-code-compatible `{response, iterations, toolCalls}` envelope.
    /// Mirrors the generated `__step_output_envelope` payload for AiAgent.
    pub fn ai_agent_output(
        &self,
        agent_id: u32,
        source: &[u8],
        output: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-agent-output source: {err}"))?;
        let output: Value = serde_json::from_slice(output)
            .map_err(|err| format!("failed to parse AiAgent output: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;

        // With a structured output schema the capability parses the response as
        // JSON and returns it under `structured_output`; use it as the response.
        // Otherwise the response is the final assistant text (a JSON string).
        let response = match output.get("structured_output") {
            Some(value) if !value.is_null() => value.clone(),
            _ => Value::String(extract_ai_final_text(
                output.get("choice").unwrap_or(&Value::Null),
            )),
        };
        let outputs = serde_json::json!({
            "response": response,
            "iterations": 1,
            "toolCalls": [],
        });

        let steps = insert_step_output(
            &source,
            &agent.step_id,
            agent.name.as_deref(),
            "AiAgent",
            outputs,
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize AiAgent steps context: {err}"))
    }

    // ===== Ai Agent tool-loop helpers (drive the `chat-turn` capability) =====

    /// Build the next `chat-turn` capability input by merging the constant base
    /// config (system/user prompts, provider, model, tools, ...) with the loop
    /// state carried in the previous turn output (`chatHistory`, `iterations`,
    /// `toolCallLog`) and the pending tool results from this round's dispatch.
    /// For the first turn pass an empty turn output and empty pending list.
    pub fn ai_turn_next_input(
        base: &[u8],
        turn_out: &[u8],
        pending: &[u8],
    ) -> Result<Vec<u8>, String> {
        let base: Value = serde_json::from_slice(base)
            .map_err(|err| format!("failed to parse ai-turn base: {err}"))?;
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        let pending: Value = serde_json::from_slice(pending)
            .map_err(|err| format!("failed to parse ai-turn pending: {err}"))?;
        let mut input = base.as_object().cloned().unwrap_or_default();
        input.insert(
            "chat_history".to_string(),
            turn_out
                .get("chat_history")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        );
        input.insert(
            "iterations".to_string(),
            turn_out
                .get("iterations")
                .cloned()
                .unwrap_or(Value::from(0)),
        );
        input.insert(
            "tool_call_log".to_string(),
            turn_out
                .get("tool_call_log")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        );
        input.insert("pending_tool_results".to_string(), pending);
        serde_json::to_vec(&Value::Object(input))
            .map_err(|err| format!("failed to serialize ai-turn input: {err}"))
    }

    /// True when the turn output's `action` is `complete`.
    pub fn ai_turn_is_complete(turn_out: &[u8]) -> Result<bool, String> {
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        Ok(turn_out.get("action").and_then(Value::as_str) == Some("complete"))
    }

    /// Number of tool calls the turn requested.
    pub fn ai_turn_tool_count(turn_out: &[u8]) -> Result<u32, String> {
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        Ok(turn_out
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| calls.len() as u32)
            .unwrap_or(0))
    }

    /// The arguments object for the `index`-th tool call, serialized as the tool
    /// agent's input payload.
    pub fn ai_turn_tool_args(turn_out: &[u8], index: u32) -> Result<Vec<u8>, String> {
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        let args = turn_out
            .get("tool_calls")
            .and_then(Value::as_array)
            .and_then(|calls| calls.get(index as usize))
            .and_then(|call| call.get("arguments"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        serde_json::to_vec(&args)
            .map_err(|err| format!("failed to serialize ai-turn tool args: {err}"))
    }

    /// The resolved tool index for the `index`-th tool call (its position in the
    /// advertised tools). Returns `u32::MAX` when the model named an unknown
    /// tool, which the loop dispatches as a no-op.
    pub fn ai_turn_tool_index(turn_out: &[u8], index: u32) -> Result<u32, String> {
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        Ok(turn_out
            .get("tool_calls")
            .and_then(Value::as_array)
            .and_then(|calls| calls.get(index as usize))
            .and_then(|call| call.get("tool_index"))
            .and_then(Value::as_u64)
            .map(|value| value as u32)
            .unwrap_or(u32::MAX))
    }

    /// Append a dispatched tool result (paired with the `index`-th tool call's
    /// id) to the pending-results list for the next turn.
    pub fn ai_turn_add_result(
        pending: &[u8],
        turn_out: &[u8],
        index: u32,
        result: &[u8],
    ) -> Result<Vec<u8>, String> {
        let mut pending: Vec<Value> = serde_json::from_slice(pending)
            .map_err(|err| format!("failed to parse ai-turn pending: {err}"))?;
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        let tool_call_id = turn_out
            .get("tool_calls")
            .and_then(Value::as_array)
            .and_then(|calls| calls.get(index as usize))
            .and_then(|call| call.get("tool_call_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let content: Value = serde_json::from_slice(result)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(result).into_owned()));
        pending.push(serde_json::json!({
            "tool_call_id": tool_call_id,
            "content": content,
        }));
        serde_json::to_vec(&Value::Array(pending))
            .map_err(|err| format!("failed to serialize ai-turn pending: {err}"))
    }

    /// Build the initial loop state from a `load-memory` result: the loaded
    /// conversation becomes the starting `chat_history`.
    pub fn ai_memory_initial_state(load_output: &[u8]) -> Result<Vec<u8>, String> {
        let load: Value = serde_json::from_slice(load_output)
            .map_err(|err| format!("failed to parse load-memory output: {err}"))?;
        let messages = load
            .get("messages")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let state = serde_json::json!({
            "chat_history": messages,
            "iterations": 0,
            "tool_call_log": [],
        });
        serde_json::to_vec(&state)
            .map_err(|err| format!("failed to serialize ai-memory state: {err}"))
    }

    /// Build the `save-memory` input from the final loop state and the resolved
    /// conversation: `{conversation_id, messages}`.
    pub fn ai_memory_save_input(
        conversation: &[u8],
        final_state: &[u8],
    ) -> Result<Vec<u8>, String> {
        let conversation: Value = serde_json::from_slice(conversation)
            .map_err(|err| format!("failed to parse ai-memory conversation: {err}"))?;
        let state: Value = serde_json::from_slice(final_state)
            .map_err(|err| format!("failed to parse ai-memory state: {err}"))?;
        let input = serde_json::json!({
            "conversation_id": conversation.get("conversation_id").cloned().unwrap_or(Value::Null),
            "messages": state
                .get("chat_history")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        });
        serde_json::to_vec(&input)
            .map_err(|err| format!("failed to serialize ai-memory save input: {err}"))
    }

    /// Sliding-window compaction: if the loop state's `chat_history` exceeds
    /// `max_messages`, drop the oldest `len - max_messages` messages so only the
    /// most recent `max_messages` remain. Mirrors the generated SlidingWindow
    /// path (`__chat_history.drain(0..excess)`), which runs before the memory
    /// save whenever memory is configured (default max 50). Below the threshold
    /// the state is returned unchanged.
    pub fn ai_memory_compact_sliding(state: &[u8], max_messages: u32) -> Result<Vec<u8>, String> {
        let mut state: Value = serde_json::from_slice(state)
            .map_err(|err| format!("failed to parse ai-memory state: {err}"))?;
        let max = max_messages as usize;
        if let Some(history) = state
            .get_mut("chat_history")
            .and_then(|value| value.as_array_mut())
            && history.len() > max
        {
            let excess = history.len() - max;
            history.drain(0..excess);
        }
        serde_json::to_vec(&state)
            .map_err(|err| format!("failed to serialize ai-memory state: {err}"))
    }

    /// Build the AiAgent step output context from a completed turn: the
    /// `{response, iterations, toolCalls}` envelope inserted under the step id.
    pub fn ai_turn_output(
        &self,
        agent_id: u32,
        source: &[u8],
        turn_out: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-turn source: {err}"))?;
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let outputs = serde_json::json!({
            "response": turn_out.get("response").cloned().unwrap_or(Value::Null),
            "iterations": turn_out.get("iterations").cloned().unwrap_or(Value::from(0)),
            "toolCalls": turn_out
                .get("tool_call_log")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        });
        let steps = insert_step_output(
            &source,
            &agent.step_id,
            agent.name.as_deref(),
            "AiAgent",
            outputs,
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize AiAgent steps context: {err}"))
    }

    /// Validate resolved Agent inputs and return a generated-code-compatible
    /// validation error string when required fields are missing/null.
    pub fn agent_validate_input(&self, agent_id: u32, input: &[u8]) -> Result<Vec<u8>, String> {
        let input: Value = serde_json::from_slice(input)
            .map_err(|err| format!("failed to parse Agent input: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let input_obj = input.as_object();
        let mut missing_inputs = Vec::new();

        for field in &agent.required_inputs {
            let value = input_obj.and_then(|obj| obj.get(field.name.as_str()));
            let reason = match value {
                None => Some(AgentInputMissingReason::NotProvided),
                Some(Value::Null) => Some(AgentInputMissingReason::WasNull),
                Some(_) => None,
            };

            if let Some(reason) = reason {
                missing_inputs.push(MissingAgentInput {
                    name: field.name.clone(),
                    field_type: field.field_type.clone(),
                    description: field.description.clone(),
                    reason,
                });
            }
        }

        if missing_inputs.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(AgentInputValidationError {
                step_id: agent.step_id.clone(),
                step_name: agent.name.clone(),
                agent_id: agent.agent_id.clone(),
                capability_id: agent.capability_id.clone(),
                missing_inputs,
            }
            .to_json_string()
            .into_bytes())
        }
    }

    /// Inject generated-code-compatible connection fields into Agent JSON input.
    pub fn agent_connection_input(&self, agent_id: u32, input: &[u8]) -> Result<Vec<u8>, String> {
        let mut input: Value = serde_json::from_slice(input)
            .map_err(|err| format!("failed to parse Agent input for connection: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let Some(connection_id) = agent.connection_id.as_deref() else {
            return serde_json::to_vec(&input)
                .map_err(|err| format!("failed to serialize Agent input: {err}"));
        };

        if let Value::Object(ref mut map) = input {
            map.insert(
                "connection_id".to_string(),
                Value::String(connection_id.to_string()),
            );
            map.insert(
                "_connection".to_string(),
                serde_json::json!({
                    "connection_id": connection_id,
                    "integration_id": "",
                    "parameters": {}
                }),
            );
        }

        serde_json::to_vec(&input).map_err(|err| format!("failed to serialize Agent input: {err}"))
    }

    /// Build the generated-code-compatible durable cache key for an Agent step.
    pub fn agent_cache_key(&self, agent_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse Agent cache-key source: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;

        Ok(agent_cache_key(agent, &source).into_bytes())
    }

    /// Build the generated-code-compatible durable sleep key for a retry.
    pub fn retry_sleep_key(checkpoint_id: &str, attempt_number: u32) -> Vec<u8> {
        format!("{checkpoint_id}::retry_sleep::{attempt_number}").into_bytes()
    }

    /// Compute the generated-code-compatible delay for the next retry.
    pub fn retry_delay_ms(
        attempt_number: u32,
        total_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
        retry_after_ms: Option<u64>,
    ) -> u64 {
        if let Some(retry_after_ms) = retry_after_ms {
            return retry_after_ms.min(max_delay_ms);
        }

        let backoff_attempt = attempt_number.min(total_attempts);
        let delay_multiplier = 2u64.pow(backoff_attempt.saturating_sub(2));
        base_delay_ms
            .saturating_mul(delay_multiplier)
            .min(max_delay_ms)
    }

    /// Build the generated-code-compatible durable sleep key for an Agent retry.
    pub fn agent_retry_sleep_key(checkpoint_id: &str, attempt_number: u32) -> Vec<u8> {
        Self::retry_sleep_key(checkpoint_id, attempt_number)
    }

    /// Compute the generated-code-compatible delay for the next Agent retry.
    pub fn agent_retry_delay_ms(
        attempt_number: u32,
        total_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
        retry_after_ms: Option<u64>,
    ) -> u64 {
        Self::retry_delay_ms(
            attempt_number,
            total_attempts,
            base_delay_ms,
            max_delay_ms,
            retry_after_ms,
        )
    }

    /// Return whether a workflow error should consume the normal retry path.
    pub fn workflow_error_retryable(error: &[u8]) -> bool {
        workflow_retry_info(error).retryable
    }

    /// Return whether a workflow error should use the rate-limit retry budget.
    pub fn workflow_error_rate_limited(error: &[u8]) -> bool {
        workflow_retry_info(error).rate_limited
    }

    /// Extract a generated-code-compatible retry-after override from a workflow error.
    pub fn workflow_error_retry_after_ms(error: &[u8]) -> Option<u64> {
        workflow_retry_info(error).retry_after_ms
    }

    /// Convert a WIT `error-info` into the raw JSON envelope used for retries.
    #[allow(clippy::too_many_arguments)]
    pub fn agent_error_info(
        code: &str,
        message: &str,
        category: &str,
        severity: &str,
        retryable: bool,
        retry_after_ms: Option<u64>,
        attributes: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        Ok(Self::agent_retry_error_info(
            code,
            message,
            category,
            severity,
            retryable,
            retry_after_ms,
            attributes,
        )?
        .payload)
    }

    /// Convert a WIT `error-info` into retry payload and retry classification.
    #[allow(clippy::too_many_arguments)]
    pub fn agent_retry_error_info(
        code: &str,
        message: &str,
        category: &str,
        severity: &str,
        retryable: bool,
        retry_after_ms: Option<u64>,
        attributes: Option<&str>,
    ) -> Result<DirectJsonAgentRetryError, String> {
        Ok(DirectJsonAgentRetryError {
            payload: agent_error_info_envelope(
                code,
                message,
                category,
                severity,
                retryable,
                retry_after_ms,
                attributes,
            )
            .into_bytes(),
            retryable: retryable && category != "permanent",
            rate_limited: agent_error_code_is_rate_limited(code),
        })
    }

    /// Convert a WIT `error-info` into the current Agent failure string shape.
    #[allow(clippy::too_many_arguments)]
    pub fn agent_error(
        &self,
        agent_id: u32,
        code: &str,
        message: &str,
        category: &str,
        severity: &str,
        retryable: bool,
        retry_after_ms: Option<u64>,
        attributes: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let raw = String::from_utf8(Self::agent_error_info(
            code,
            message,
            category,
            severity,
            retryable,
            retry_after_ms,
            attributes,
        )?)
        .map_err(|error| format!("Agent error-info JSON was not UTF-8: {error}"))?;
        Ok(format!(
            "Step {} failed: Agent {}::{}: {}",
            agent.step_id, agent.agent_id, agent.capability_id, raw
        )
        .into_bytes())
    }

    /// Convert a raw Agent error-info payload into the current failure string shape.
    pub fn agent_error_from_info(
        &self,
        agent_id: u32,
        error_info: &[u8],
    ) -> Result<Vec<u8>, String> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let raw = String::from_utf8(error_info.to_vec())
            .map_err(|error| format!("Agent error-info JSON was not UTF-8: {error}"))?;
        Ok(format!(
            "Step {} failed: Agent {}::{}: {}",
            agent.step_id, agent.agent_id, agent.capability_id, raw
        )
        .into_bytes())
    }

    /// Build generated-code-compatible Agent `step_debug_end` payload for failures.
    pub fn agent_debug_error(&self, agent_id: u32, error: &[u8]) -> Result<Vec<u8>, String> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let step = self
            .steps
            .get(&agent.step_id)
            .ok_or_else(|| format!("unknown direct Agent step '{}'", agent.step_id))?;
        let timestamp = timestamp_ms();
        let duration_ms = self
            .debug_start_ms
            .borrow_mut()
            .remove(&agent.step_id)
            .map(|start| timestamp.saturating_sub(start).max(0))
            .unwrap_or(0);
        let error = String::from_utf8_lossy(error).to_string();

        let mut payload = debug_event_base(step, timestamp);
        payload.insert(
            "outputs".to_string(),
            serde_json::json!({
                "_error": true,
                "error": error,
            }),
        );
        payload.insert(
            "duration_ms".to_string(),
            Value::Number(serde_json::Number::from(duration_ms)),
        );

        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize Agent step-debug-end payload: {err}"))
    }

    /// Build the payload for a manifest Log step's runtime custom event.
    pub fn log_event(&self, log_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse log-event source: {err}"))?;
        let log = self
            .logs
            .get(&log_id)
            .ok_or_else(|| format!("unknown direct Log id {log_id}"))?;
        let details = apply_log(&log.value, &source)?;
        serde_json::to_vec(&serde_json::json!({
            "step_id": log.step_id,
            "step_name": log.name.as_deref().unwrap_or("Unnamed"),
            "level": details.level,
            "message": details.message,
            "context": details.context,
            "timestamp_ms": timestamp_ms(),
        }))
        .map_err(|err| format!("failed to serialize log event payload: {err}"))
    }

    /// Execute a manifest Log step and return an updated steps context.
    pub fn log(&self, log_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse log source: {err}"))?;
        let log = self
            .logs
            .get(&log_id)
            .ok_or_else(|| format!("unknown direct Log id {log_id}"))?;
        let details = apply_log(&log.value, &source)?;
        let steps = insert_step_output(
            &source,
            &log.step_id,
            log.name.as_deref(),
            "Log",
            serde_json::json!({
                "level": details.level,
                "message": details.message,
            }),
            None,
        );
        serde_json::to_vec(&Value::Object(steps))
            .map_err(|err| format!("failed to serialize log steps context: {err}"))
    }

    /// Build the payload for a manifest Error step's runtime custom event.
    pub fn error_event(&self, error_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse error-event source: {err}"))?;
        let error = self
            .errors
            .get(&error_id)
            .ok_or_else(|| format!("unknown direct Error id {error_id}"))?;
        let details = apply_error(&error.value, &source)?;
        serde_json::to_vec(&serde_json::json!({
            "step_id": error.step_id,
            "step_name": error.name.as_deref().unwrap_or("Unnamed"),
            "category": details.category,
            "code": details.code,
            "message": details.message,
            "severity": details.severity,
            "context": details.context,
            "timestamp_ms": timestamp_ms(),
        }))
        .map_err(|err| format!("failed to serialize error event payload: {err}"))
    }

    /// Build the structured failure payload for a manifest Error step.
    pub fn error(&self, error_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse error source: {err}"))?;
        let error = self
            .errors
            .get(&error_id)
            .ok_or_else(|| format!("unknown direct Error id {error_id}"))?;
        let details = apply_error(&error.value, &source)?;
        serde_json::to_vec(&serde_json::json!({
            "stepId": error.step_id,
            "stepName": error.name.as_deref().unwrap_or("Unnamed"),
            "category": details.category,
            "code": details.code,
            "message": details.message,
            "severity": details.severity,
            "context": details.context,
        }))
        .map_err(|err| format!("failed to serialize error failure payload: {err}"))
    }

    /// Insert generated-code-compatible `onError` context into the steps map.
    pub fn error_steps(
        &self,
        step_id: &str,
        error: &[u8],
        steps: &[u8],
    ) -> Result<Vec<u8>, String> {
        error_steps(step_id, error, steps)
    }

    /// Build a generated-code-compatible `step_debug_start` payload.
    pub fn step_debug_start(&self, step_id: &str, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse step-debug-start source: {err}"))?;
        let step = self
            .steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct debug step '{step_id}'"))?;
        let timestamp = timestamp_ms();
        self.debug_start_ms
            .borrow_mut()
            .insert(step_id.to_string(), timestamp);

        let mut payload = debug_event_base(step, timestamp);
        let (inputs, input_mapping) = self.debug_start_data(step, &source)?;
        payload.insert("inputs".to_string(), inputs);
        if let Some(input_mapping) = input_mapping {
            payload.insert("input_mapping".to_string(), input_mapping);
        }

        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize step-debug-start payload: {err}"))
    }

    /// Build a generated-code-compatible `step_debug_end` payload.
    pub fn step_debug_end(&self, step_id: &str, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse step-debug-end source: {err}"))?;
        let step = self
            .steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct debug step '{step_id}'"))?;
        let timestamp = timestamp_ms();
        let duration_ms = self
            .debug_start_ms
            .borrow_mut()
            .remove(step_id)
            .map(|start| timestamp.saturating_sub(start).max(0))
            .unwrap_or(0);

        let mut payload = debug_event_base(step, timestamp);
        payload.insert("outputs".to_string(), self.debug_end_output(step, &source)?);
        payload.insert(
            "duration_ms".to_string(),
            Value::Number(serde_json::Number::from(duration_ms)),
        );

        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize step-debug-end payload: {err}"))
    }

    fn debug_start_data(
        &self,
        step: &DirectJsonStep,
        source: &Value,
    ) -> Result<(Value, Option<Value>), String> {
        match step.step_type.as_str() {
            "Finish" => {
                let mapping = self.finish_mapping(step.id.as_str());
                Ok((
                    serde_json::json!({ "finishing": true }),
                    mapping.and_then(|mapping| {
                        (!mapping.value.as_object().is_some_and(Map::is_empty))
                            .then(|| mapping.value.clone())
                    }),
                ))
            }
            "Conditional" => {
                let condition = self.conditional_condition(step.id.as_str());
                Ok((
                    serde_json::json!({ "condition": "evaluating" }),
                    condition.cloned(),
                ))
            }
            "Filter" => {
                let filter = self
                    .filter_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Filter config for '{}'", step.id))?;
                let input = filter
                    .value
                    .get("value")
                    .ok_or_else(|| "Filter config missing value".to_string())
                    .and_then(|value| apply_mapping_value(value, source))?;
                Ok((input, filter.value.get("condition").cloned()))
            }
            "Switch" => {
                let switch = self
                    .switch_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Switch config for '{}'", step.id))?;
                Ok((
                    switch_debug_inputs(&switch.value, source)?,
                    Some(switch.value.clone()),
                ))
            }
            "GroupBy" => {
                let group_by = self
                    .group_by_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct GroupBy config for '{}'", step.id))?;
                let input = group_by
                    .value
                    .get("value")
                    .ok_or_else(|| "GroupBy config missing value".to_string())
                    .and_then(|value| apply_mapping_value(value, source))?;
                Ok((input, None))
            }
            "Delay" => {
                let delay = self
                    .delay_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Delay config for '{}'", step.id))?;
                let duration_ms = apply_mapping_value(&delay.duration_ms, source)?;
                Ok((
                    serde_json::json!({ "duration_ms": duration_ms }),
                    Some(delay.duration_ms.clone()),
                ))
            }
            "Agent" => {
                let agent = self
                    .agent_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Agent config for '{}'", step.id))?;
                let mapping = self.mappings.get(&agent.input_mapping_id).ok_or_else(|| {
                    format!(
                        "missing direct Agent input mapping {} for '{}'",
                        agent.input_mapping_id, step.id
                    )
                })?;
                let inputs = apply_input_mapping(&mapping.value, source)?;
                Ok((
                    inputs,
                    (!mapping.value.as_object().is_some_and(Map::is_empty))
                        .then(|| mapping.value.clone()),
                ))
            }
            "EmbedWorkflow" => {
                let mapping = self.embed_workflow_mapping(step.id.as_str());
                let inputs = mapping
                    .map(|mapping| apply_input_mapping(&mapping.value, source))
                    .transpose()?
                    .unwrap_or_else(|| Value::Object(Map::new()));
                Ok((
                    inputs,
                    mapping.and_then(|mapping| {
                        (!mapping.value.as_object().is_some_and(Map::is_empty))
                            .then(|| mapping.value.clone())
                    }),
                ))
            }
            "Error" => Ok((Value::Null, None)),
            "Log" => {
                let log = self
                    .log_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Log config for '{}'", step.id))?;
                let details = apply_log(&log.value, source)?;
                Ok((details.context, None))
            }
            other => Err(format!(
                "direct step-debug-start does not support step type '{other}'"
            )),
        }
    }

    fn debug_end_output(&self, step: &DirectJsonStep, source: &Value) -> Result<Value, String> {
        match step.step_type.as_str() {
            "Finish" => {
                let mapping = self
                    .finish_mapping(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Finish mapping for '{}'", step.id))?;
                let mut output = apply_input_mapping(&mapping.value, source)?;
                output = output.get("outputs").cloned().unwrap_or(output);
                Ok(step_output_envelope(step, output, None))
            }
            "Conditional" => {
                let condition = self
                    .conditional_condition(step.id.as_str())
                    .ok_or_else(|| {
                        format!("missing direct Conditional condition for '{}'", step.id)
                    })?;
                let result = eval_condition_expression(condition, source)?;
                Ok(step_output_envelope(
                    step,
                    serde_json::json!({ "result": result }),
                    None,
                ))
            }
            "Filter" => {
                let filter = self
                    .filter_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Filter config for '{}'", step.id))?;
                let output = apply_filter(&filter.value, source)?;
                Ok(step_output_envelope(step, output, None))
            }
            "Switch" => {
                let switch = self
                    .switch_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Switch config for '{}'", step.id))?;
                let result = apply_switch(&switch.value, source)?;
                let route = switch_is_routing(&switch.value).then_some(result.route.as_str());
                Ok(step_output_envelope(step, result.output, route))
            }
            "GroupBy" => {
                let group_by = self
                    .group_by_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct GroupBy config for '{}'", step.id))?;
                let output = apply_group_by(&group_by.value, source)?;
                Ok(step_output_envelope(step, output, None))
            }
            "Delay" => source
                .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                .cloned()
                .or_else(|| {
                    self.delay_by_step(step.id.as_str()).and_then(|delay| {
                        let duration = apply_mapping_value(&delay.duration_ms, source).ok()?;
                        let duration_ms = duration
                            .as_u64()
                            .or_else(|| duration.as_f64().map(|ms| ms as u64))?;
                        Some(delay_step_value(delay, duration_ms))
                    })
                })
                .ok_or_else(|| format!("missing direct Delay output for '{}'", step.id)),
            "Log" => {
                let log = self
                    .log_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Log config for '{}'", step.id))?;
                let details = apply_log(&log.value, source)?;
                Ok(step_output_envelope(
                    step,
                    serde_json::json!({
                        "level": details.level,
                        "message": details.message,
                    }),
                    None,
                ))
            }
            "Agent" => source
                .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                .cloned()
                .ok_or_else(|| format!("missing direct Agent output for '{}'", step.id)),
            "EmbedWorkflow" => source
                .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                .cloned()
                .ok_or_else(|| format!("missing direct EmbedWorkflow output for '{}'", step.id)),
            "WaitForSignal" => source
                .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                .cloned()
                .ok_or_else(|| format!("missing direct WaitForSignal output for '{}'", step.id)),
            "Error" => {
                let error = self
                    .error_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Error config for '{}'", step.id))?;
                let details = apply_error(&error.value, source)?;
                Ok(serde_json::json!({
                    "_error": true,
                    "category": details.category,
                    "code": details.code,
                    "message": details.message,
                    "severity": details.severity,
                }))
            }
            other => Err(format!(
                "direct step-debug-end does not support step type '{other}'"
            )),
        }
    }

    fn finish_mapping(&self, step_id: &str) -> Option<&DirectJsonMapping> {
        self.mappings
            .values()
            .find(|mapping| mapping.step_id == step_id && mapping.purpose == "finish.inputMapping")
    }

    fn embed_workflow_mapping(&self, step_id: &str) -> Option<&DirectJsonMapping> {
        self.mappings.values().find(|mapping| {
            mapping.step_id == step_id && mapping.purpose == "embedWorkflow.inputMapping"
        })
    }

    fn conditional_condition(&self, step_id: &str) -> Option<&Value> {
        self.conditions
            .values()
            .find(|condition| {
                condition.owner_id == step_id && condition.purpose == "conditional.condition"
            })
            .map(|condition| &condition.value)
    }

    fn filter_by_step(&self, step_id: &str) -> Option<&DirectJsonFilter> {
        self.filters
            .values()
            .find(|filter| filter.step_id == step_id)
    }

    fn split_by_step(&self, step_id: &str) -> Option<&DirectJsonSplit> {
        self.splits.values().find(|split| split.step_id == step_id)
    }

    fn while_by_step(&self, step_id: &str) -> Option<&DirectJsonWhile> {
        self.whiles
            .values()
            .find(|while_step| while_step.step_id == step_id)
    }

    fn switch_by_step(&self, step_id: &str) -> Option<&DirectJsonSwitch> {
        self.switches
            .values()
            .find(|switch| switch.step_id == step_id)
    }

    fn group_by_by_step(&self, step_id: &str) -> Option<&DirectJsonGroupBy> {
        self.group_bys
            .values()
            .find(|group_by| group_by.step_id == step_id)
    }

    fn delay_by_step(&self, step_id: &str) -> Option<&DirectJsonDelay> {
        self.delays.values().find(|delay| delay.step_id == step_id)
    }

    fn log_by_step(&self, step_id: &str) -> Option<&DirectJsonLog> {
        self.logs.values().find(|log| log.step_id == step_id)
    }

    fn error_by_step(&self, step_id: &str) -> Option<&DirectJsonError> {
        self.errors.values().find(|error| error.step_id == step_id)
    }

    fn agent_by_step(&self, step_id: &str) -> Option<&DirectJsonAgent> {
        self.agents.values().find(|agent| agent.step_id == step_id)
    }

    fn wait_step(&self, step_id: &str) -> Result<&DirectJsonStep, String> {
        let step = self
            .steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct WaitForSignal step '{step_id}'"))?;
        if step.step_type == "WaitForSignal" {
            Ok(step)
        } else {
            Err(format!(
                "direct step '{step_id}' is {}, not WaitForSignal",
                step.step_type
            ))
        }
    }

    fn embed_workflow_step(&self, step_id: &str) -> Result<&DirectJsonStep, String> {
        let step = self
            .steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct EmbedWorkflow step '{step_id}'"))?;
        if step.step_type == "EmbedWorkflow" {
            Ok(step)
        } else {
            Err(format!(
                "direct step '{step_id}' is {}, not EmbedWorkflow",
                step.step_type
            ))
        }
    }

    fn child_workflow(&self, step_id: &str) -> Result<&DirectJsonChildWorkflow, String> {
        self.child_workflows
            .get(step_id)
            .ok_or_else(|| format!("missing direct child workflow graph for step '{step_id}'"))
    }
}

/// Build the source envelope consumed by direct mapping/condition helpers.
pub fn build_source(data: &[u8], variables: &[u8], steps: &[u8]) -> Result<Vec<u8>, String> {
    let data: Value =
        serde_json::from_slice(data).map_err(|err| format!("failed to parse data: {err}"))?;
    let variables: Value = serde_json::from_slice(variables)
        .map_err(|err| format!("failed to parse variables: {err}"))?;
    let steps: Value =
        serde_json::from_slice(steps).map_err(|err| format!("failed to parse steps: {err}"))?;

    let mut source = Map::new();
    source.insert("data".to_string(), data.clone());
    source.insert("variables".to_string(), variables.clone());
    source.insert("steps".to_string(), steps);

    let mut workflow_inputs = Map::new();
    workflow_inputs.insert("data".to_string(), data);
    workflow_inputs.insert("variables".to_string(), variables.clone());
    source.insert(
        "workflow".to_string(),
        serde_json::json!({ "inputs": Value::Object(workflow_inputs) }),
    );

    if let Some(loop_ctx) = variables.as_object().and_then(|vars| vars.get("_loop")) {
        source.insert("loop".to_string(), loop_ctx.clone());
    }
    if let Some(item) = variables.as_object().and_then(|vars| vars.get("_item")) {
        source.insert("item".to_string(), item.clone());
    }

    serde_json::to_vec(&Value::Object(source))
        .map_err(|err| format!("failed to serialize source: {err}"))
}

/// Insert generated-code-compatible `onError` context into the steps map.
pub fn error_steps(step_id: &str, error: &[u8], steps: &[u8]) -> Result<Vec<u8>, String> {
    let mut steps: Map<String, Value> = serde_json::from_slice::<Value>(steps)
        .map_err(|err| format!("failed to parse error steps context: {err}"))?
        .as_object()
        .cloned()
        .ok_or_else(|| "error steps context must be a JSON object".to_string())?;
    let error = serde_json::from_slice::<Value>(error).unwrap_or_else(|_| {
        serde_json::json!({
            "message": String::from_utf8_lossy(error).to_string(),
            "stepId": step_id,
            "code": null,
            "category": "unknown",
            "severity": "error"
        })
    });

    steps.insert("__error".to_string(), error.clone());
    steps.insert("error".to_string(), error);

    serde_json::to_vec(&Value::Object(steps))
        .map_err(|err| format!("failed to serialize error steps context: {err}"))
}

fn agent_cache_key(agent: &DirectJsonAgent, source: &Value) -> String {
    let variables = source.get("variables").and_then(Value::as_object);
    let prefix = variables
        .and_then(|vars| vars.get("_cache_key_prefix"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let indices_suffix = variables
        .and_then(|vars| vars.get("_loop_indices"))
        .and_then(Value::as_array)
        .filter(|indices| !indices.is_empty())
        .map(|indices| {
            let indices = indices.iter().map(Value::to_string).collect::<Vec<_>>();
            format!("::[{}]", indices.join(","))
        })
        .unwrap_or_default();
    let base = format!(
        "agent::{}::{}::{}",
        agent.agent_id, agent.capability_id, agent.step_id
    );

    if prefix.is_empty() {
        let workflow_id = variables
            .and_then(|vars| vars.get("_workflow_id"))
            .and_then(Value::as_str)
            .unwrap_or("root");
        format!("{workflow_id}::{base}{indices_suffix}")
    } else {
        format!("{prefix}::{base}{indices_suffix}")
    }
}

fn split_cache_key(split: &DirectJsonSplit, source: &Value) -> String {
    let variables = source.get("variables").and_then(Value::as_object);
    let prefix = variables
        .and_then(|vars| vars.get("_cache_key_prefix"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let indices_suffix = variables
        .and_then(|vars| vars.get("_loop_indices"))
        .and_then(Value::as_array)
        .filter(|indices| !indices.is_empty())
        .map(|indices| {
            let indices = indices.iter().map(Value::to_string).collect::<Vec<_>>();
            format!("::[{}]", indices.join(","))
        })
        .unwrap_or_default();
    let base = format!("split::{}", split.step_id);

    if prefix.is_empty() {
        let workflow_id = variables
            .and_then(|vars| vars.get("_workflow_id"))
            .and_then(Value::as_str)
            .unwrap_or("root");
        format!("{workflow_id}::{base}{indices_suffix}")
    } else {
        format!("{prefix}::{base}{indices_suffix}")
    }
}

#[derive(Default)]
struct DirectJsonManifestCollections {
    steps: BTreeMap<String, DirectJsonStep>,
    child_workflows: BTreeMap<String, DirectJsonChildWorkflow>,
    mappings: BTreeMap<u32, DirectJsonMapping>,
    conditions: BTreeMap<u32, DirectJsonCondition>,
    splits: BTreeMap<u32, DirectJsonSplit>,
    whiles: BTreeMap<u32, DirectJsonWhile>,
    filters: BTreeMap<u32, DirectJsonFilter>,
    switches: BTreeMap<u32, DirectJsonSwitch>,
    group_bys: BTreeMap<u32, DirectJsonGroupBy>,
    delays: BTreeMap<u32, DirectJsonDelay>,
    logs: BTreeMap<u32, DirectJsonLog>,
    errors: BTreeMap<u32, DirectJsonError>,
    agents: BTreeMap<u32, DirectJsonAgent>,
}

fn collect_graph_manifest(
    graph: &GraphWire,
    collections: &mut DirectJsonManifestCollections,
) -> Result<(), String> {
    for step in &graph.steps {
        collections
            .steps
            .entry(step.id.clone())
            .or_insert_with(|| DirectJsonStep {
                id: step.id.clone(),
                step_type: step.step_type.clone(),
                name: step.name.clone(),
                body: step.body.clone(),
            });
    }
    for mapping in &graph.mappings {
        if collections
            .mappings
            .insert(
                mapping.id,
                DirectJsonMapping {
                    step_id: mapping.step_id.clone(),
                    purpose: mapping.purpose.clone(),
                    value: mapping.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct mapping id {}", mapping.id));
        }
    }
    for condition in &graph.conditions {
        if collections
            .conditions
            .insert(
                condition.id,
                DirectJsonCondition {
                    owner_id: condition.owner_id.clone(),
                    purpose: condition.purpose.clone(),
                    value: condition.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct condition id {}", condition.id));
        }
    }
    for split in &graph.splits {
        if collections
            .splits
            .insert(
                split.id,
                DirectJsonSplit {
                    step_id: split.step_id.clone(),
                    name: split.name.clone(),
                    value: split.value.clone(),
                    input_schema: split.input_schema.clone(),
                    output_schema: split.output_schema.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Split id {}", split.id));
        }
    }
    for while_step in &graph.whiles {
        if collections
            .whiles
            .insert(
                while_step.id,
                DirectJsonWhile {
                    step_id: while_step.step_id.clone(),
                    name: while_step.name.clone(),
                    value: while_step.value.clone(),
                    condition: while_step.condition.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct While id {}", while_step.id));
        }
    }
    for filter in &graph.filters {
        if collections
            .filters
            .insert(
                filter.id,
                DirectJsonFilter {
                    step_id: filter.step_id.clone(),
                    name: filter.name.clone(),
                    value: filter.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Filter id {}", filter.id));
        }
    }
    for switch in &graph.switches {
        if collections
            .switches
            .insert(
                switch.id,
                DirectJsonSwitch {
                    step_id: switch.step_id.clone(),
                    name: switch.name.clone(),
                    value: switch.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Switch id {}", switch.id));
        }
    }
    for group_by in &graph.group_bys {
        if collections
            .group_bys
            .insert(
                group_by.id,
                DirectJsonGroupBy {
                    step_id: group_by.step_id.clone(),
                    name: group_by.name.clone(),
                    value: group_by.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct GroupBy id {}", group_by.id));
        }
    }
    for delay in &graph.delays {
        if collections
            .delays
            .insert(
                delay.id,
                DirectJsonDelay {
                    step_id: delay.step_id.clone(),
                    name: delay.name.clone(),
                    duration_ms: delay.duration_ms.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Delay id {}", delay.id));
        }
    }
    for log in &graph.logs {
        if collections
            .logs
            .insert(
                log.id,
                DirectJsonLog {
                    step_id: log.step_id.clone(),
                    name: log.name.clone(),
                    value: log.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Log id {}", log.id));
        }
    }
    for error in &graph.errors {
        if collections
            .errors
            .insert(
                error.id,
                DirectJsonError {
                    step_id: error.step_id.clone(),
                    name: error.name.clone(),
                    value: error.value.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Error id {}", error.id));
        }
    }
    for agent in &graph.agents {
        if collections
            .agents
            .insert(
                agent.id,
                DirectJsonAgent {
                    step_id: agent.step_id.clone(),
                    name: agent.name.clone(),
                    agent_id: agent.agent_id.clone(),
                    capability_id: agent.capability_id.clone(),
                    connection_id: agent.connection_id.clone(),
                    input_mapping_id: agent.input_mapping_id,
                    required_inputs: agent.required_inputs.clone(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate direct Agent id {}", agent.id));
        }
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_graph_manifest(&nested.graph, collections)?;
        }
    }
    Ok(())
}

fn split_items(split: &DirectJsonSplit, source: &Value) -> Result<Value, String> {
    let value_mapping = split
        .value
        .get("value")
        .ok_or_else(|| format!("Split step '{}' config missing value", split.step_id))?;
    let input = apply_mapping_value(value_mapping, source)?;
    let allow_null = split_bool_config(&split.value, "allowNull");
    let convert_single_value = split_bool_config(&split.value, "convertSingleValue");

    let mut items = match input {
        Value::Array(items) => items,
        Value::Null => {
            if allow_null {
                Vec::new()
            } else {
                return Err(format!(
                    "Split step '{}' received null value. Set 'allowNull: true' to allow empty iterations, or use 'transform/ensure-array' agent.",
                    split.step_id
                ));
            }
        }
        other => {
            if convert_single_value {
                vec![other]
            } else {
                return Err(format!(
                    "Split step '{}' expected array, got {}. Set 'convertSingleValue: true' to auto-wrap, or use 'transform/ensure-array' agent.",
                    split.step_id,
                    json_type_name(&other)
                ));
            }
        }
    };

    let batch_size = split
        .value
        .get("batchSize")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if batch_size > 0 {
        items = items
            .chunks(batch_size)
            .map(|chunk| Value::Array(chunk.to_vec()))
            .collect();
    }

    Ok(Value::Array(items))
}

fn split_debug_inputs(split: &DirectJsonSplit, source: &Value) -> Result<Value, String> {
    let value_mapping = split
        .value
        .get("value")
        .ok_or_else(|| format!("Split step '{}' config missing value", split.step_id))?;
    let mut inputs = Map::new();
    inputs.insert(
        "value".to_string(),
        apply_mapping_value(value_mapping, source)?,
    );
    inputs.insert(
        "parallelism".to_string(),
        serde_json::json!(
            split
                .value
                .get("parallelism")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
    );
    inputs.insert(
        "sequential".to_string(),
        Value::Bool(split_bool_config(&split.value, "sequential")),
    );
    inputs.insert(
        "dontStopOnFailed".to_string(),
        Value::Bool(split_bool_config(&split.value, "dontStopOnFailed")),
    );
    inputs.insert(
        "allowNull".to_string(),
        Value::Bool(split_bool_config(&split.value, "allowNull")),
    );
    inputs.insert(
        "convertSingleValue".to_string(),
        Value::Bool(split_bool_config(&split.value, "convertSingleValue")),
    );
    inputs.insert(
        "batchSize".to_string(),
        serde_json::json!(
            split
                .value
                .get("batchSize")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
    );
    if let Some(extra_variables_mapping) = split.value.get("variables") {
        inputs.insert(
            "variables".to_string(),
            apply_input_mapping(extra_variables_mapping, source)?,
        );
    }
    Ok(Value::Object(inputs))
}

fn split_iteration_variables(
    split: &DirectJsonSplit,
    source: &Value,
    item: Value,
    index: u32,
) -> Result<Map<String, Value>, String> {
    let mut variables = source
        .get("variables")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if let Some(extra_variables_mapping) = split.value.get("variables") {
        let extra_variables = apply_input_mapping(extra_variables_mapping, source)?;
        if let Value::Object(extra_variables) = extra_variables {
            variables.extend(extra_variables);
        }
    }

    let parent_indices = variables
        .get("_loop_indices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut loop_indices = parent_indices;
    loop_indices.push(serde_json::json!(index));
    variables.insert("_loop_indices".to_string(), Value::Array(loop_indices));
    variables.insert("_index".to_string(), serde_json::json!(index));
    variables.insert("_item".to_string(), item);

    let scope_id = variables
        .get("_scope_id")
        .and_then(Value::as_str)
        .map(|parent| format!("{}_{}_{}", parent, split.step_id, index))
        .unwrap_or_else(|| format!("sc_{}_{}", split.step_id, index));
    variables.insert("_scope_id".to_string(), Value::String(scope_id));

    Ok(variables)
}

fn split_dont_stop_on_failed(split: &DirectJsonSplit) -> bool {
    split_bool_config(&split.value, "dontStopOnFailed")
}

fn split_accumulator_array_mut<'a>(
    results: &'a mut Value,
    split: &DirectJsonSplit,
    key: &str,
) -> Result<&'a mut Vec<Value>, String> {
    results
        .as_object_mut()
        .and_then(|object| object.get_mut(key))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            format!(
                "Split step '{}' internal result accumulator must contain a '{key}' array",
                split.step_id
            )
        })
}

fn split_dont_stop_result(
    split: &DirectJsonSplit,
    source: &Value,
    results: Value,
) -> Result<Value, String> {
    let object = results.as_object().ok_or_else(|| {
        format!(
            "Split step '{}' internal result accumulator must be an object",
            split.step_id
        )
    })?;
    let success = object
        .get("success")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "Split step '{}' internal result accumulator must contain a 'success' array",
                split.step_id
            )
        })?;
    let error = object
        .get("error")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "Split step '{}' internal result accumulator must contain an 'error' array",
                split.step_id
            )
        })?;
    let aborted = object
        .get("aborted")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let unknown = object
        .get("unknown")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let skipped = object
        .get("skipped")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = split_items(split, source)?
        .as_array()
        .map(Vec::len)
        .expect("split_items always returns a JSON array");

    Ok(serde_json::json!({
        "stepId": split.step_id,
        "stepName": split.name.as_deref().unwrap_or("Unnamed"),
        "stepType": "Split",
        "data": {
            "success": success,
            "error": error,
            "aborted": aborted,
            "unknown": unknown,
            "skipped": skipped
        },
        "stats": {
            "success": success.len(),
            "error": error.len(),
            "aborted": aborted.len(),
            "unknown": unknown.len(),
            "skipped": skipped.len(),
            "total": total
        },
        "outputs": success
    }))
}

fn split_result(split: &DirectJsonSplit, source: &Value, results: Value) -> Result<Value, String> {
    if split_dont_stop_on_failed(split) {
        split_dont_stop_result(split, source, results)
    } else {
        let step = DirectJsonStep {
            id: split.step_id.clone(),
            step_type: "Split".to_string(),
            name: split.name.clone(),
            body: Value::Null,
        };
        Ok(step_output_envelope(&step, results, None))
    }
}

#[derive(Debug, Clone)]
struct DirectJsonWhileState {
    index: u32,
    outputs: Value,
}

impl DirectJsonWhileState {
    fn to_value(&self) -> Value {
        serde_json::json!({
            "index": self.index,
            "outputs": self.outputs,
        })
    }
}

fn while_max_iterations(while_step: &DirectJsonWhile) -> Result<u32, String> {
    let Some(max_iterations) = while_step.value.get("maxIterations") else {
        return Ok(10);
    };
    let max_iterations = max_iterations.as_u64().ok_or_else(|| {
        format!(
            "While step '{}' maxIterations must be an unsigned integer",
            while_step.step_id
        )
    })?;
    u32::try_from(max_iterations).map_err(|_| {
        format!(
            "While step '{}' maxIterations exceeds u32 range",
            while_step.step_id
        )
    })
}

fn while_debug_inputs(while_step: &DirectJsonWhile) -> Result<Value, String> {
    Ok(serde_json::json!({
        "maxIterations": while_max_iterations(while_step)?,
    }))
}

fn parse_while_state(
    while_step: &DirectJsonWhile,
    state: &[u8],
) -> Result<DirectJsonWhileState, String> {
    let state: Value = serde_json::from_slice(state)
        .map_err(|err| format!("failed to parse While state: {err}"))?;
    let state = state.as_object().ok_or_else(|| {
        format!(
            "While step '{}' internal state must be a JSON object",
            while_step.step_id
        )
    })?;
    let index = state.get("index").and_then(Value::as_u64).ok_or_else(|| {
        format!(
            "While step '{}' internal state missing numeric index",
            while_step.step_id
        )
    })?;
    let index = u32::try_from(index).map_err(|_| {
        format!(
            "While step '{}' internal state index exceeds u32 range",
            while_step.step_id
        )
    })?;
    let outputs = state.get("outputs").cloned().unwrap_or(Value::Null);

    Ok(DirectJsonWhileState { index, outputs })
}

fn while_loop_context(state: &DirectJsonWhileState) -> Value {
    serde_json::json!({
        "index": state.index,
        "outputs": state.outputs,
    })
}

fn while_iteration_variables(
    while_step: &DirectJsonWhile,
    variables: Value,
    state: &DirectJsonWhileState,
) -> Map<String, Value> {
    let mut variables = variables.as_object().cloned().unwrap_or_default();
    let parent_indices = variables
        .get("_loop_indices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut loop_indices = parent_indices;
    loop_indices.push(serde_json::json!(state.index));
    variables.insert("_loop_indices".to_string(), Value::Array(loop_indices));
    variables.insert("_index".to_string(), serde_json::json!(state.index));
    if !state.outputs.is_null() {
        variables.insert("_previousOutputs".to_string(), state.outputs.clone());
    }
    variables.insert("_loop".to_string(), while_loop_context(state));

    let scope_id = variables
        .get("_scope_id")
        .and_then(Value::as_str)
        .map(|parent| format!("{}_{}_{}", parent, while_step.step_id, state.index))
        .unwrap_or_else(|| format!("sc_{}_{}", while_step.step_id, state.index));
    variables.insert("_scope_id".to_string(), Value::String(scope_id));

    variables
}

fn validate_split_schema(value: &Value, schema: &Value, ctx: &str) -> Result<(), String> {
    let schema_obj = match schema.as_object() {
        Some(schema_obj) if !schema_obj.is_empty() => schema_obj,
        _ => return Ok(()),
    };
    let value_obj = value
        .as_object()
        .ok_or_else(|| format!("{ctx}: expected object, got {}", json_type_name(value)))?;

    let mut missing = Vec::new();
    let mut wrong_type = Vec::new();
    for (field_name, field_schema) in schema_obj {
        let required = field_schema
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let field_type = field_schema
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        match value_obj.get(field_name) {
            None if required => missing.push(field_name.clone()),
            None => {}
            Some(actual_value) if !field_type.is_empty() && !actual_value.is_null() => {
                if !split_schema_type_matches(field_type, actual_value) {
                    wrong_type.push(format!(
                        "'{}' (expected {}, got {})",
                        field_name,
                        field_type,
                        json_type_name(actual_value)
                    ));
                }
            }
            Some(_) => {}
        }
    }

    if missing.is_empty() && wrong_type.is_empty() {
        return Ok(());
    }

    let mut parts = Vec::new();
    if !missing.is_empty() {
        let mut got = value_obj.keys().cloned().collect::<Vec<_>>();
        got.sort();
        parts.push(format!(
            "required field(s) [{}] missing (got fields: [{}])",
            missing.join(", "),
            got.join(", ")
        ));
    }
    if !wrong_type.is_empty() {
        parts.push(format!("type mismatches: {}", wrong_type.join(", ")));
    }

    Err(format!("{ctx}: {}", parts.join("; ")))
}

fn split_schema_type_matches(field_type: &str, value: &Value) -> bool {
    match field_type {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn split_bool_config(config: &Value, key: &str) -> bool {
    config.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "object",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Array(_) => "array",
        Value::Null => "null",
    }
}

fn apply_filter(config: &Value, source: &Value) -> Result<Value, String> {
    let input = config
        .get("value")
        .ok_or_else(|| "Filter config missing value".to_string())
        .and_then(|value| apply_mapping_value(value, source))?;
    let items = input.as_array().cloned().unwrap_or_default();
    let condition = config
        .get("condition")
        .ok_or_else(|| "Filter config missing condition".to_string())?;
    let mut source = source.clone();
    if !source.is_object() {
        return Err("filter source must be a JSON object".to_string());
    }

    let mut filtered = Vec::new();
    for item in items {
        source
            .as_object_mut()
            .expect("filter source was checked as object")
            .insert("item".to_string(), item.clone());
        if eval_condition_expression(condition, &source)? {
            filtered.push(item);
        }
    }

    Ok(serde_json::json!({
        "items": filtered,
        "count": filtered.len(),
    }))
}

#[derive(Debug, Clone)]
struct DirectSwitchResult {
    output: Value,
    route: String,
}

fn apply_switch(config: &Value, source: &Value) -> Result<DirectSwitchResult, String> {
    let Some(switch_value) = config.get("value") else {
        let default = config
            .get("default")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        return Ok(DirectSwitchResult {
            output: process_switch_output(&default, source),
            route: "default".to_string(),
        });
    };

    if let Some(cases) = config.get("cases").and_then(Value::as_array) {
        for case in cases {
            let condition = switch_case_condition(switch_value, case)?;
            if eval_condition_expression(&condition, source)? {
                let output = case
                    .get("output")
                    .ok_or_else(|| "Switch case missing output".to_string())?;
                return Ok(DirectSwitchResult {
                    output: process_switch_output(output, source),
                    route: case
                        .get("route")
                        .and_then(Value::as_str)
                        .unwrap_or("default")
                        .to_string(),
                });
            }
        }
    }

    let default = config
        .get("default")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    Ok(DirectSwitchResult {
        output: process_switch_output(&default, source),
        route: "default".to_string(),
    })
}

fn switch_is_routing(config: &Value) -> bool {
    config
        .get("cases")
        .and_then(Value::as_array)
        .is_some_and(|cases| cases.iter().any(|case| case.get("route").is_some()))
}

fn switch_case_condition(switch_value: &Value, case: &Value) -> Result<Value, String> {
    let match_type = case
        .get("matchType")
        .and_then(Value::as_str)
        .ok_or_else(|| "Switch case missing matchType".to_string())?;
    let match_value = case.get("match").cloned().unwrap_or(Value::Null);
    let right = serde_json::json!({
        "valueType": "immediate",
        "value": match_value,
    });

    match match_type {
        "EQ" if case.get("match").is_some_and(Value::is_array) => {
            Ok(binary_condition("IN", switch_value.clone(), right))
        }
        "EQ" | "NE" | "GT" | "GTE" | "LT" | "LTE" | "STARTS_WITH" | "ENDS_WITH" | "CONTAINS"
        | "IN" | "NOT_IN" => Ok(binary_condition(match_type, switch_value.clone(), right)),
        "IS_DEFINED" | "IS_EMPTY" | "IS_NOT_EMPTY" => {
            Ok(unary_condition(match_type, switch_value.clone()))
        }
        "BETWEEN" => Ok(build_between_condition(switch_value, &match_value)),
        "RANGE" => Ok(build_range_condition(switch_value, &match_value)),
        other => Err(format!("unsupported Switch matchType '{other}'")),
    }
}

fn binary_condition(op: &str, left: Value, right: Value) -> Value {
    serde_json::json!({
        "type": "operation",
        "op": op,
        "arguments": [left, right],
    })
}

fn unary_condition(op: &str, value: Value) -> Value {
    serde_json::json!({
        "type": "operation",
        "op": op,
        "arguments": [value],
    })
}

fn value_condition(value: bool) -> Value {
    serde_json::json!({
        "type": "value",
        "valueType": "immediate",
        "value": value,
    })
}

fn build_between_condition(switch_value: &Value, match_value: &Value) -> Value {
    let Some(bounds) = match_value.as_array().filter(|bounds| bounds.len() >= 2) else {
        return value_condition(false);
    };

    serde_json::json!({
        "type": "operation",
        "op": "AND",
        "arguments": [
            binary_condition(
                "GTE",
                switch_value.clone(),
                serde_json::json!({ "valueType": "immediate", "value": bounds[0].clone() }),
            ),
            binary_condition(
                "LTE",
                switch_value.clone(),
                serde_json::json!({ "valueType": "immediate", "value": bounds[1].clone() }),
            ),
        ],
    })
}

fn build_range_condition(switch_value: &Value, match_value: &Value) -> Value {
    let Some(bounds) = match_value.as_object() else {
        return value_condition(true);
    };

    let mut conditions = Vec::new();
    for (key, op) in [("gte", "GTE"), ("gt", "GT"), ("lte", "LTE"), ("lt", "LT")] {
        if let Some(value) = bounds.get(key) {
            conditions.push(binary_condition(
                op,
                switch_value.clone(),
                serde_json::json!({ "valueType": "immediate", "value": value.clone() }),
            ));
        }
    }

    match conditions.len() {
        0 => value_condition(true),
        1 => conditions.remove(0),
        _ => serde_json::json!({
            "type": "operation",
            "op": "AND",
            "arguments": conditions,
        }),
    }
}

fn apply_group_by(config: &Value, source: &Value) -> Result<Value, String> {
    let input = config
        .get("value")
        .ok_or_else(|| "GroupBy config missing value".to_string())
        .and_then(|value| apply_mapping_value(value, source))?;
    let items = input.as_array().cloned().unwrap_or_default();
    let key = config
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| "GroupBy config missing key".to_string())?;
    let pointer = path_to_json_pointer(key);

    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    let mut counts = BTreeMap::<String, usize>::new();
    if let Some(expected_keys) = config.get("expectedKeys").and_then(Value::as_array) {
        for key in expected_keys.iter().filter_map(Value::as_str) {
            groups.entry(key.to_string()).or_default();
            counts.entry(key.to_string()).or_insert(0);
        }
    }

    for item in items {
        let key = item.pointer(&pointer).cloned().unwrap_or(Value::Null);
        let key = group_key_string(&key);
        groups.entry(key.clone()).or_default().push(item);
        *counts.entry(key).or_insert(0) += 1;
    }

    Ok(serde_json::json!({
        "groups": groups,
        "counts": counts,
        "total_groups": groups.len(),
    }))
}

#[derive(Debug, Clone)]
struct DirectLogResult {
    level: String,
    message: String,
    context: Value,
}

fn apply_log(config: &Value, source: &Value) -> Result<DirectLogResult, String> {
    let level = config
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("info")
        .to_string();
    let message = config
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| "Log step missing message".to_string())?
        .to_string();
    let context = config
        .get("context")
        .and_then(Value::as_object)
        .filter(|context| !context.is_empty())
        .map(|context| apply_input_mapping(&Value::Object(context.clone()), source))
        .transpose()?
        .unwrap_or_else(|| Value::Object(Map::new()));

    Ok(DirectLogResult {
        level,
        message,
        context,
    })
}

#[derive(Debug, Clone)]
struct DirectErrorResult {
    category: String,
    code: String,
    message: String,
    severity: String,
    context: Value,
}

fn apply_error(config: &Value, source: &Value) -> Result<DirectErrorResult, String> {
    let category = config
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("permanent")
        .to_string();
    let code = config
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| "Error step missing code".to_string())?
        .to_string();
    let message = config
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| "Error step missing message".to_string())?
        .to_string();
    let severity = config
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    let context = config
        .get("context")
        .and_then(Value::as_object)
        .filter(|context| !context.is_empty())
        .map(|context| apply_input_mapping(&Value::Object(context.clone()), source))
        .transpose()?
        .unwrap_or_else(|| Value::Object(Map::new()));

    Ok(DirectErrorResult {
        category,
        code,
        message,
        severity,
        context,
    })
}

fn agent_error_info_envelope(
    code: &str,
    message: &str,
    category: &str,
    severity: &str,
    retryable: bool,
    retry_after_ms: Option<u64>,
    attributes: Option<&str>,
) -> String {
    let mut object = Map::new();
    object.insert("code".to_string(), Value::String(code.to_string()));
    object.insert("message".to_string(), Value::String(message.to_string()));
    object.insert("category".to_string(), Value::String(category.to_string()));
    object.insert("severity".to_string(), Value::String(severity.to_string()));
    object.insert("retryable".to_string(), Value::Bool(retryable));
    if let Some(retry_after_ms) = retry_after_ms {
        object.insert(
            "retryAfterMs".to_string(),
            Value::Number(serde_json::Number::from(retry_after_ms)),
        );
    }
    if let Some(attributes) = attributes
        && let Ok(parsed) = serde_json::from_str::<Value>(attributes)
    {
        object.insert("attributes".to_string(), parsed);
    }

    Value::Object(object).to_string()
}

fn workflow_retry_info(error: &[u8]) -> DirectJsonWorkflowRetryInfo {
    let Ok(parsed) = serde_json::from_slice::<Value>(error) else {
        return DirectJsonWorkflowRetryInfo {
            retryable: true,
            rate_limited: false,
            retry_after_ms: None,
        };
    };

    let category = parsed.get("category").and_then(Value::as_str);
    let code = parsed.get("code").and_then(Value::as_str).unwrap_or("");
    let rate_limited = agent_error_code_is_rate_limited(code) || code == "HTTP_RATE_LIMITED";
    let retry_after_ms = parsed.get("retryAfterMs").and_then(Value::as_u64);
    let auto_retry_429 = std::env::var("AUTO_RETRY_ON_429")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(true);

    DirectJsonWorkflowRetryInfo {
        retryable: category != Some("permanent") && (!rate_limited || auto_retry_429),
        rate_limited,
        retry_after_ms,
    }
}

fn agent_error_code_is_rate_limited(code: &str) -> bool {
    code.contains("RATE_LIMITED")
}

fn timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn debug_event_base(step: &DirectJsonStep, timestamp_ms: i64) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("step_id".to_string(), Value::String(step.id.clone()));
    payload.insert(
        "step_name".to_string(),
        step.name
            .as_ref()
            .map(|name| Value::String(name.clone()))
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "step_type".to_string(),
        Value::String(step.step_type.clone()),
    );
    payload.insert("scope_id".to_string(), Value::Null);
    payload.insert("parent_scope_id".to_string(), Value::Null);
    payload.insert("loop_indices".to_string(), Value::Array(Vec::new()));
    payload.insert(
        "timestamp_ms".to_string(),
        Value::Number(serde_json::Number::from(timestamp_ms)),
    );
    payload
}

fn switch_debug_inputs(config: &Value, source: &Value) -> Result<Value, String> {
    let value = config
        .get("value")
        .map(|value| apply_mapping_value(value, source))
        .transpose()?
        .unwrap_or(Value::Null);
    let mut inputs = Map::new();
    inputs.insert("value".to_string(), value);
    inputs.insert(
        "cases".to_string(),
        config
            .get("cases")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    inputs.insert(
        "default".to_string(),
        config
            .get("default")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    Ok(Value::Object(inputs))
}

fn step_output_envelope(step: &DirectJsonStep, output: Value, route: Option<&str>) -> Value {
    let mut envelope = serde_json::json!({
        "stepId": step.id,
        "stepName": step.name.as_deref().unwrap_or_else(|| default_step_name(&step.step_type)),
        "stepType": step.step_type,
        "outputs": output,
    });
    if let Some(route) = route
        && let Some(envelope) = envelope.as_object_mut()
    {
        envelope.insert("route".to_string(), Value::String(route.to_string()));
    }
    envelope
}

fn escape_json_pointer_token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn default_step_name(step_type: &str) -> &str {
    if step_type == "Finish" {
        "Finish"
    } else {
        "Unnamed"
    }
}

/// Extract the final assistant text from a serialized `chat-completion` choice.
///
/// The choice is `runtara_ai::OneOrMany<AssistantContent>`, which serializes as
/// a JSON array of untagged `AssistantContent`: a `Text` is `{"text": "..."}`
/// and a `ToolCall` is `{"id": ..., "function": ...}`. This returns the first
/// element carrying a `text` string — matching how the generated Ai Agent loop
/// sets `__final_response` from `AssistantContent::Text`. Tool-call content is
/// ignored (single-shot); returns an empty string when no text is present.
///
/// Parsed by hand rather than via `runtara_ai` types: the direct-component
/// stdlib build intentionally does not link `runtara-ai`.
fn extract_ai_final_text(choice: &Value) -> String {
    if let Some(items) = choice.as_array() {
        for item in items {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
        }
    }
    String::new()
}

fn insert_step_output(
    source: &Value,
    step_id: &str,
    step_name: Option<&str>,
    step_type: &str,
    output: Value,
    route: Option<&str>,
) -> Map<String, Value> {
    let mut steps = source
        .get("steps")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let step = DirectJsonStep {
        id: step_id.to_string(),
        step_type: step_type.to_string(),
        name: step_name.map(str::to_string),
        body: Value::Null,
    };
    steps.insert(
        step_id.to_string(),
        step_output_envelope(&step, output, route),
    );
    steps
}

fn delay_step_value(delay: &DirectJsonDelay, duration_ms: u64) -> Value {
    serde_json::json!({
        "stepId": delay.step_id,
        "stepName": delay.name.as_deref().unwrap_or("Unnamed"),
        "stepType": "Delay",
        "duration_ms": duration_ms,
    })
}

fn wait_loop_indices_suffix(source: &Value) -> String {
    source
        .get("variables")
        .and_then(Value::as_object)
        .and_then(|vars| vars.get("_loop_indices"))
        .and_then(Value::as_array)
        .filter(|indices| !indices.is_empty())
        .map(|indices| {
            let indices = indices.iter().map(Value::to_string).collect::<Vec<_>>();
            format!("/[{}]", indices.join(","))
        })
        .unwrap_or_default()
}

fn embed_workflow_cache_key(step_id: &str, source: &Value) -> String {
    let variables = source.get("variables").and_then(Value::as_object);
    let prefix = variables
        .and_then(|vars| vars.get("_cache_key_prefix"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let indices_suffix = variables
        .and_then(|vars| vars.get("_loop_indices"))
        .and_then(Value::as_array)
        .filter(|indices| !indices.is_empty())
        .map(|indices| {
            let indices = indices.iter().map(Value::to_string).collect::<Vec<_>>();
            format!("::[{}]", indices.join(","))
        })
        .unwrap_or_default();
    let base = format!("embed_workflow::{step_id}");
    if prefix.is_empty() {
        format!("{base}{indices_suffix}")
    } else {
        format!("{prefix}::{base}{indices_suffix}")
    }
}

fn embed_child_variables(
    step_id: &str,
    child: &DirectJsonChildWorkflow,
    source: &Value,
) -> Map<String, Value> {
    let parent_variables = source.get("variables").and_then(Value::as_object);
    let parent_scope_id = parent_variables
        .and_then(|vars| vars.get("_scope_id"))
        .and_then(Value::as_str);
    let child_scope_id = parent_scope_id
        .map(|parent| format!("{parent}_{step_id}"))
        .unwrap_or_else(|| format!("sc_{step_id}"));

    let mut variables = Map::new();
    variables.insert("_scope_id".to_string(), Value::String(child_scope_id));

    if let Some(workflow_id) = parent_variables
        .and_then(|vars| vars.get("_workflow_id"))
        .and_then(Value::as_str)
    {
        variables.insert(
            "_workflow_id".to_string(),
            Value::String(workflow_id.to_string()),
        );
    }
    if let Some(instance_id) = parent_variables.and_then(|vars| vars.get("_instance_id")) {
        variables.insert("_instance_id".to_string(), instance_id.clone());
    }
    if let Some(tenant_id) = parent_variables.and_then(|vars| vars.get("_tenant_id")) {
        variables.insert("_tenant_id".to_string(), tenant_id.clone());
    }

    let loop_indices_suffix = parent_variables
        .and_then(|vars| vars.get("_loop_indices"))
        .and_then(Value::as_array)
        .filter(|indices| !indices.is_empty())
        .map(|indices| {
            let indices = indices.iter().map(Value::to_string).collect::<Vec<_>>();
            format!("[{}]", indices.join(","))
        })
        .unwrap_or_default();
    let child_cache_prefix = match parent_variables
        .and_then(|vars| vars.get("_cache_key_prefix"))
        .and_then(Value::as_str)
    {
        Some(prefix) if !prefix.is_empty() => {
            format!("{prefix}__{step_id}{loop_indices_suffix}")
        }
        _ => {
            let workflow_id = parent_variables
                .and_then(|vars| vars.get("_workflow_id"))
                .and_then(Value::as_str)
                .unwrap_or("root");
            format!("{workflow_id}::{step_id}{loop_indices_suffix}")
        }
    };
    variables.insert(
        "_cache_key_prefix".to_string(),
        Value::String(child_cache_prefix),
    );

    if let Some(defaults) = child.variables.as_object() {
        for (name, variable) in defaults {
            if name.starts_with('_') {
                continue;
            }
            let value = variable
                .get("value")
                .cloned()
                .unwrap_or_else(|| variable.clone());
            variables.entry(name.clone()).or_insert(value);
        }
    }

    variables
}

fn validate_embed_child_inputs(
    child: &DirectJsonChildWorkflow,
    child_input: &Value,
) -> Result<(), String> {
    let input_object = child_input.as_object();
    let mut missing = Vec::new();
    if let Some(schema) = child.input_schema.as_object() {
        for (name, field) in schema {
            if !field
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let reason = match input_object.and_then(|input| input.get(name)) {
                None => Some("not provided"),
                Some(Value::Null) => Some("was null"),
                Some(_) => None,
            };
            if let Some(reason) = reason {
                let field_type = field
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let description = field.get("description").and_then(Value::as_str);
                missing.push((name.as_str(), field_type, description, reason));
            }
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let mut message = format!(
        "EmbedWorkflow step '{}' is missing required inputs for child workflow '{}':\n",
        child.step_id, child.workflow_id
    );
    for (name, field_type, description, reason) in missing {
        message.push_str(&format!("  - {name} ({field_type})"));
        if let Some(description) = description {
            message.push_str(&format!(": {description}"));
        }
        message.push_str(&format!(" [{reason}]\n"));
    }
    Err(message)
}

fn embed_workflow_step_value(
    step: &DirectJsonStep,
    child: &DirectJsonChildWorkflow,
    output: Value,
) -> Value {
    serde_json::json!({
        "stepId": step.id,
        "stepName": step.name.as_deref().unwrap_or("Unnamed"),
        "stepType": "EmbedWorkflow",
        "childWorkflowId": child.workflow_id,
        "outputs": output,
    })
}

fn embed_workflow_error_value(
    step: &DirectJsonStep,
    child: &DirectJsonChildWorkflow,
    child_error: Value,
) -> Value {
    let category = child_error
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("transient");
    let severity = child_error
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("error");
    serde_json::json!({
        "stepId": step.id,
        "stepName": step.name.as_deref().unwrap_or("Unnamed"),
        "stepType": "EmbedWorkflow",
        "code": "CHILD_WORKFLOW_FAILED",
        "message": format!("Child workflow {} failed", child.workflow_id),
        "category": category,
        "severity": severity,
        "childWorkflowId": child.workflow_id,
        "childError": child_error,
    })
}

fn wait_action_mapping(
    action: Option<&Map<String, Value>>,
    field: &str,
    source: &Value,
) -> Result<Value, String> {
    let Some(mapping) = action.and_then(|action| action.get(field)) else {
        return Ok(Value::Object(Map::new()));
    };
    apply_input_mapping(mapping, source)
}

fn wait_step_value(step: &DirectJsonStep, signal_id: &str, signal_payload: Value) -> Value {
    serde_json::json!({
        "stepId": step.id,
        "stepName": step.name.as_deref().unwrap_or("Unnamed"),
        "stepType": "WaitForSignal",
        "signal_id": signal_id,
        "outputs": signal_payload,
    })
}

fn group_key_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "_null".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "_invalid".to_string()),
    }
}

fn eval_condition_expression(expr: &Value, source: &Value) -> Result<bool, String> {
    if is_condition_operation(expr) {
        eval_condition_operation(expr, source)
    } else {
        eval_condition_value(expr, source).map(|value| is_truthy(&value))
    }
}

fn is_condition_operation(expr: &Value) -> bool {
    expr.get("op").is_some()
        || expr
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "operation")
}

fn eval_condition_operation(expr: &Value, source: &Value) -> Result<bool, String> {
    let op = expr
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "condition operation missing op".to_string())?;
    let args = expr
        .get("arguments")
        .and_then(Value::as_array)
        .ok_or_else(|| "condition operation missing arguments".to_string())?;

    match op {
        "AND" => args.iter().try_fold(true, |acc, arg| {
            if !acc {
                Ok(false)
            } else {
                eval_condition_argument_as_bool(arg, source)
            }
        }),
        "OR" => args.iter().try_fold(false, |acc, arg| {
            if acc {
                Ok(true)
            } else {
                eval_condition_argument_as_bool(arg, source)
            }
        }),
        "NOT" => args
            .first()
            .map(|arg| eval_condition_argument_as_bool(arg, source).map(|value| !value))
            .unwrap_or(Ok(true)),
        "GT" | "GTE" | "LT" | "LTE" => eval_comparison(op, args, source),
        "EQ" | "NE" => eval_equality(op, args, source),
        "STARTS_WITH" | "ENDS_WITH" => eval_string_match(op, args, source),
        "CONTAINS" | "IN" | "NOT_IN" => eval_array_match(op, args, source),
        "LENGTH" => eval_length_as_value(args, source).map(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().map(|value| value as i64))
                .unwrap_or(0)
                > 0
        }),
        "IS_DEFINED" => args
            .first()
            .map(|arg| eval_condition_argument_as_value(arg, source).map(|value| !value.is_null()))
            .unwrap_or(Ok(false)),
        "IS_EMPTY" => args
            .first()
            .map(|arg| {
                eval_condition_argument_as_value(arg, source).map(|value| match value {
                    Value::Array(value) => value.is_empty(),
                    Value::String(value) => value.is_empty(),
                    Value::Object(value) => value.is_empty(),
                    Value::Null => true,
                    _ => false,
                })
            })
            .unwrap_or(Ok(true)),
        "IS_NOT_EMPTY" => args
            .first()
            .map(|arg| {
                eval_condition_argument_as_value(arg, source).map(|value| match value {
                    Value::Array(value) => !value.is_empty(),
                    Value::String(value) => !value.is_empty(),
                    Value::Object(value) => !value.is_empty(),
                    Value::Null => false,
                    _ => true,
                })
            })
            .unwrap_or(Ok(false)),
        "SIMILARITY_GTE" | "MATCH" | "COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE" => Ok(false),
        other => Err(format!("unsupported condition operator '{other}'")),
    }
}

fn eval_condition_argument_as_bool(arg: &Value, source: &Value) -> Result<bool, String> {
    if is_condition_operation(arg) {
        eval_condition_expression(arg, source)
    } else {
        eval_condition_value(arg, source).map(|value| is_truthy(&value))
    }
}

fn eval_condition_argument_as_value(arg: &Value, source: &Value) -> Result<Value, String> {
    if is_condition_operation(arg) {
        if arg.get("op").and_then(Value::as_str) == Some("LENGTH") {
            let args = arg
                .get("arguments")
                .and_then(Value::as_array)
                .ok_or_else(|| "LENGTH condition missing arguments".to_string())?;
            eval_length_as_value(args, source)
        } else {
            eval_condition_expression(arg, source).map(Value::Bool)
        }
    } else {
        eval_condition_value(arg, source)
    }
}

fn eval_condition_value(value: &Value, source: &Value) -> Result<Value, String> {
    if value.get("type").and_then(Value::as_str) == Some("value") {
        if value.get("valueType").is_some() {
            return apply_mapping_value(value, source);
        }
        if let Some(inner) = value.get("value") {
            return apply_mapping_value(inner, source);
        }
    }
    apply_mapping_value(value, source)
}

fn eval_comparison(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let Some(left) = to_number(&left) else {
        return Ok(false);
    };
    let Some(right) = to_number(&right) else {
        return Ok(false);
    };
    Ok(match op {
        "GT" => left > right,
        "GTE" => left >= right,
        "LT" => left < right,
        "LTE" => left <= right,
        _ => false,
    })
}

fn eval_equality(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let equal = values_equal(&left, &right);
    Ok(if op == "NE" { !equal } else { equal })
}

fn eval_string_match(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let Some(left) = left.as_str() else {
        return Ok(false);
    };
    let Some(right) = right.as_str() else {
        return Ok(false);
    };
    Ok(if op == "STARTS_WITH" {
        left.starts_with(right)
    } else {
        left.ends_with(right)
    })
}

fn eval_array_match(op: &str, args: &[Value], source: &Value) -> Result<bool, String> {
    if args.len() < 2 {
        return Ok(false);
    }
    let left = eval_condition_argument_as_value(&args[0], source)?;
    let right = eval_condition_argument_as_value(&args[1], source)?;
    let matched = match op {
        "CONTAINS" => left
            .as_array()
            .is_some_and(|items| items.iter().any(|item| values_equal(item, &right))),
        "IN" | "NOT_IN" => right
            .as_array()
            .is_some_and(|items| items.iter().any(|item| values_equal(&left, item))),
        _ => false,
    };
    Ok(if op == "NOT_IN" { !matched } else { matched })
}

fn eval_length_as_value(args: &[Value], source: &Value) -> Result<Value, String> {
    let Some(arg) = args.first() else {
        return Ok(Value::Number(0.into()));
    };
    let value = eval_condition_argument_as_value(arg, source)?;
    let len = match &value {
        Value::String(value) => value.len() as i64,
        Value::Array(value) => value.len() as i64,
        Value::Object(value) => value.len() as i64,
        Value::Null => 0,
        _ => 1,
    };
    Ok(Value::Number(len.into()))
}

fn apply_input_mapping(mapping: &Value, source: &Value) -> Result<Value, String> {
    let Value::Object(entries) = mapping else {
        return Err("input mapping must be a JSON object".to_string());
    };

    let mut output = Map::new();
    for (key, value) in entries {
        let value = apply_mapping_value(value, source)?;
        insert_nested(&mut output, key, value);
    }
    Ok(Value::Object(output))
}

fn apply_mapping_value(value: &Value, source: &Value) -> Result<Value, String> {
    let Value::Object(map) = value else {
        return Err("mapping value must be an object".to_string());
    };
    let value_type = map
        .get("valueType")
        .and_then(Value::as_str)
        .ok_or_else(|| "mapping value missing valueType".to_string())?;

    match value_type {
        "reference" => apply_reference(map, source),
        "immediate" => Ok(map.get("value").cloned().unwrap_or(Value::Null)),
        "composite" => apply_composite(map.get("value").unwrap_or(&Value::Null), source),
        "template" => {
            let template = map
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| "template mapping value must be a string".to_string())?;
            render_template(template, source).map(Value::String)
        }
        other => Err(format!("unsupported mapping valueType '{other}'")),
    }
}

fn apply_reference(map: &Map<String, Value>, source: &Value) -> Result<Value, String> {
    let path = map
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| "reference mapping value must be a string path".to_string())?;
    let default = map.get("default").cloned();
    let value = match lookup_source_path(source, path) {
        Some(Value::Null) | None => default.unwrap_or(Value::Null),
        Some(value) => value,
    };
    Ok(apply_type_hint(
        value,
        map.get("type").and_then(Value::as_str),
    ))
}

fn apply_composite(value: &Value, source: &Value) -> Result<Value, String> {
    match value {
        Value::Object(map) => {
            let mut output = Map::new();
            for (key, child) in map {
                output.insert(key.clone(), apply_mapping_value(child, source)?);
            }
            Ok(Value::Object(output))
        }
        Value::Array(items) => items
            .iter()
            .map(|item| apply_mapping_value(item, source))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err("composite mapping value must be an object or array".to_string()),
    }
}

fn apply_type_hint(value: Value, type_hint: Option<&str>) -> Value {
    match type_hint {
        Some("string") => match value {
            Value::String(_) | Value::Null => value,
            Value::Number(number) => Value::String(number.to_string()),
            Value::Bool(boolean) => Value::String(boolean.to_string()),
            other => Value::String(other.to_string()),
        },
        Some("integer") => value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64))
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
            .or_else(|| value.as_bool().map(|value| if value { 1 } else { 0 }))
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|| {
                if value.is_null() {
                    Value::Null
                } else {
                    Value::Number(0.into())
                }
            }),
        Some("number") => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| {
                if value.is_null() {
                    Value::Null
                } else {
                    Value::Number(0.into())
                }
            }),
        Some("boolean") => match value {
            Value::Bool(_) | Value::Null => value,
            Value::String(value) => Value::Bool(value == "true" || value == "1"),
            Value::Number(value) => Value::Bool(value.as_i64().is_some_and(|value| value != 0)),
            Value::Array(value) => Value::Bool(!value.is_empty()),
            Value::Object(value) => Value::Bool(!value.is_empty()),
        },
        Some("json" | "file") | None => value,
        Some(_) => value,
    }
}

fn insert_nested(output: &mut Map<String, Value>, key: &str, value: Value) {
    let mut parts = key.split('.').peekable();
    let Some(first) = parts.next() else {
        return;
    };
    if parts.peek().is_none() {
        output.insert(first.to_string(), value);
        return;
    }

    let mut current = output
        .entry(first.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    while let Some(part) = parts.next() {
        let is_last = parts.peek().is_none();
        if is_last {
            if let Value::Object(map) = current {
                map.insert(part.to_string(), value);
            }
            return;
        }

        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        current = current
            .as_object_mut()
            .expect("current was just forced to object")
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

fn lookup_source_path(source: &Value, path: &str) -> Option<Value> {
    let pointer = path_to_json_pointer(path);
    source.pointer(&pointer).cloned()
}

fn path_to_json_pointer(path: &str) -> String {
    let normalized = path
        .replace("['", ".")
        .replace("']", "")
        .replace("[\"", ".")
        .replace("\"]", "");

    let mut dotted = String::new();
    let mut chars = normalized.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            let mut index = String::new();
            while let Some(&next_ch) = chars.peek() {
                if next_ch == ']' {
                    chars.next();
                    break;
                }
                index.push(chars.next().expect("peeked character exists"));
            }
            if index.chars().all(|c| c.is_ascii_digit()) {
                dotted.push('.');
                dotted.push_str(&index);
            } else {
                dotted.push('[');
                dotted.push_str(&index);
                dotted.push(']');
            }
        } else {
            dotted.push(ch);
        }
    }

    let mut out = String::with_capacity(dotted.len() + 4);
    for segment in dotted.split('.') {
        out.push('/');
        for ch in segment.chars() {
            match ch {
                '~' => out.push_str("~0"),
                '/' => out.push_str("~1"),
                _ => out.push(ch),
            }
        }
    }
    out
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestWire {
    graph: GraphWire,
    #[serde(default)]
    child_workflows: Vec<ChildWorkflowWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChildWorkflowWire {
    step_id: String,
    workflow_id: String,
    graph: GraphWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphWire {
    #[serde(default)]
    variables: Value,
    #[serde(default)]
    input_schema: Value,
    #[serde(default)]
    mappings: Vec<MappingWire>,
    #[serde(default)]
    conditions: Vec<ConditionWire>,
    #[serde(default)]
    splits: Vec<SplitWire>,
    #[serde(default)]
    whiles: Vec<WhileWire>,
    #[serde(default)]
    filters: Vec<FilterWire>,
    #[serde(default)]
    switches: Vec<SwitchWire>,
    #[serde(default)]
    group_bys: Vec<GroupByWire>,
    #[serde(default)]
    delays: Vec<DelayWire>,
    #[serde(default)]
    logs: Vec<LogWire>,
    #[serde(default)]
    errors: Vec<ErrorWire>,
    #[serde(default)]
    agents: Vec<AgentWire>,
    #[serde(default)]
    steps: Vec<StepWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepWire {
    id: String,
    #[serde(rename = "stepType")]
    step_type: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    nested_graphs: Vec<NestedGraphWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NestedGraphWire {
    graph: GraphWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MappingWire {
    id: u32,
    #[serde(rename = "stepId")]
    step_id: String,
    purpose: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConditionWire {
    id: u32,
    owner_id: String,
    purpose: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SplitWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
    #[serde(default)]
    input_schema: Value,
    #[serde(default)]
    output_schema: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WhileWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
    condition: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupByWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DelayWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    duration_ms: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentWire {
    id: u32,
    step_id: String,
    #[serde(default)]
    name: Option<String>,
    agent_id: String,
    capability_id: String,
    #[serde(default)]
    connection_id: Option<String>,
    input_mapping_id: u32,
    #[serde(default)]
    required_inputs: Vec<DirectJsonRequiredAgentInput>,
}

#[derive(Debug, Clone)]
struct DirectJsonMapping {
    step_id: String,
    purpose: String,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonCondition {
    owner_id: String,
    purpose: String,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonSplit {
    step_id: String,
    name: Option<String>,
    value: Value,
    input_schema: Value,
    output_schema: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonWhile {
    step_id: String,
    name: Option<String>,
    value: Value,
    condition: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonStep {
    id: String,
    step_type: String,
    name: Option<String>,
    body: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonChildWorkflow {
    step_id: String,
    workflow_id: String,
    variables: Value,
    input_schema: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonFilter {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonSwitch {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonGroupBy {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonDelay {
    step_id: String,
    name: Option<String>,
    duration_ms: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonLog {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonError {
    step_id: String,
    name: Option<String>,
    value: Value,
}

#[derive(Debug, Clone)]
struct DirectJsonAgent {
    step_id: String,
    name: Option<String>,
    agent_id: String,
    capability_id: String,
    connection_id: Option<String>,
    input_mapping_id: u32,
    required_inputs: Vec<DirectJsonRequiredAgentInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectJsonRequiredAgentInput {
    name: String,
    field_type: String,
    #[serde(default)]
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(mapping_value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "finish",
                    "stepType": "Finish",
                    "purpose": "finish.inputMapping",
                    "value": mapping_value
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn condition_manifest(condition_value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "conditions": [{
                    "id": 0,
                    "ownerId": "check",
                    "ownerType": "Conditional",
                    "purpose": "conditional.condition",
                    "value": condition_value
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn filter_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "filters": [{
                    "id": 0,
                    "stepId": "filter",
                    "name": "Filter Active Items",
                    "stepType": "Filter",
                    "purpose": "filter.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn split_manifest(config: Value) -> Vec<u8> {
        split_manifest_with_schemas(config, json!({}), json!({}))
    }

    fn split_manifest_with_schemas(
        config: Value,
        input_schema: Value,
        output_schema: Value,
    ) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "splits": [{
                    "id": 0,
                    "stepId": "split",
                    "name": "Process Items",
                    "stepType": "Split",
                    "purpose": "split.config",
                    "durable": true,
                    "value": config,
                    "inputSchema": input_schema,
                    "outputSchema": output_schema
                }],
                "steps": [{
                    "id": "split",
                    "stepType": "Split",
                    "name": "Process Items",
                    "body": {
                        "id": "split",
                        "stepType": "Split",
                        "name": "Process Items"
                    }
                }]
            }
        }))
        .expect("manifest json")
    }

    fn while_manifest(config: Value, condition: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "whiles": [{
                    "id": 0,
                    "stepId": "loop",
                    "name": "Counter Loop",
                    "stepType": "While",
                    "purpose": "while.config",
                    "value": config,
                    "condition": condition
                }],
                "steps": [{
                    "id": "loop",
                    "stepType": "While",
                    "name": "Counter Loop",
                    "body": {
                        "id": "loop",
                        "stepType": "While",
                        "name": "Counter Loop"
                    }
                }]
            }
        }))
        .expect("manifest json")
    }

    fn switch_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "switches": [{
                    "id": 0,
                    "stepId": "switch",
                    "name": "Classify Status",
                    "stepType": "Switch",
                    "purpose": "switch.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn group_by_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "groupBys": [{
                    "id": 0,
                    "stepId": "group",
                    "name": "Group by Status",
                    "stepType": "GroupBy",
                    "purpose": "groupBy.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn delay_manifest(duration_ms: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "delays": [{
                    "id": 0,
                    "stepId": "delay",
                    "name": "Wait",
                    "stepType": "Delay",
                    "purpose": "delay.config",
                    "durationMs": duration_ms
                }],
                "steps": [{
                    "id": "delay",
                    "stepType": "Delay",
                    "name": "Wait",
                    "body": {
                        "id": "delay",
                        "stepType": "Delay",
                        "name": "Wait"
                    }
                }]
            }
        }))
        .expect("manifest json")
    }

    fn wait_manifest(body: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "steps": [{
                    "id": "wait",
                    "stepType": "WaitForSignal",
                    "name": "Review Input",
                    "body": body
                }]
            }
        }))
        .expect("manifest json")
    }

    fn log_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "logs": [{
                    "id": 0,
                    "stepId": "log",
                    "name": "Log Start",
                    "stepType": "Log",
                    "purpose": "log.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn error_manifest(config: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "errors": [{
                    "id": 0,
                    "stepId": "fail",
                    "name": "Fail Fast",
                    "stepType": "Error",
                    "purpose": "error.config",
                    "value": config
                }],
                "steps": []
            }
        }))
        .expect("manifest json")
    }

    fn agent_manifest(input_mapping: Value) -> Vec<u8> {
        agent_manifest_with_required_inputs(input_mapping, json!([]))
    }

    fn agent_manifest_with_required_inputs(
        input_mapping: Value,
        required_inputs: Value,
    ) -> Vec<u8> {
        agent_manifest_with_required_inputs_and_connection(input_mapping, required_inputs, None)
    }

    fn agent_manifest_with_required_inputs_and_connection(
        input_mapping: Value,
        required_inputs: Value,
        connection_id: Option<&str>,
    ) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "agent",
                    "stepType": "Agent",
                    "purpose": "agent.inputMapping",
                    "value": input_mapping
                }],
                "agents": [{
                    "id": 0,
                    "stepId": "agent",
                    "name": "Normalize Data",
                    "stepType": "Agent",
                    "purpose": "agent.config",
                    "agentId": "utils",
                    "capabilityId": "normalize",
                    "inputMappingId": 0,
                    "requiredInputs": required_inputs,
                    "connectionId": connection_id
                }],
                "steps": [{
                    "id": "agent",
                    "stepType": "Agent",
                    "name": "Normalize Data",
                    "body": {
                        "id": "agent",
                        "stepType": "Agent",
                        "name": "Normalize Data"
                    }
                }]
            }
        }))
        .expect("manifest json")
    }

    fn debug_manifest(
        step_type: &str,
        step_id: &str,
        name: Option<&str>,
        collections: Value,
    ) -> Vec<u8> {
        let mut graph = collections.as_object().cloned().unwrap_or_default();
        graph.insert(
            "steps".to_string(),
            json!([{
                "id": step_id,
                "stepType": step_type,
                "name": name,
                "body": {
                    "id": step_id,
                    "stepType": step_type,
                    "name": name
                }
            }]),
        );
        serde_json::to_vec(&json!({ "graph": graph })).expect("manifest json")
    }

    #[test]
    fn build_source_matches_generated_workflow_shape() {
        let source = build_source(
            br#"{"input":"hello"}"#,
            br#"{"tenant":"t1","_item":{"id":7}}"#,
            br#"{"previous":{"outputs":{"ok":true}}}"#,
        )
        .expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");

        assert_eq!(source["data"]["input"], "hello");
        assert_eq!(source["variables"]["tenant"], "t1");
        assert_eq!(source["steps"]["previous"]["outputs"]["ok"], true);
        assert_eq!(source["workflow"]["inputs"]["data"]["input"], "hello");
        assert_eq!(source["workflow"]["inputs"]["variables"]["tenant"], "t1");
        assert_eq!(source["item"]["id"], 7);
    }

    #[test]
    fn parse_allows_duplicate_step_ids_across_nested_graphs() {
        let manifest = serde_json::to_vec(&json!({
            "graph": {
                "steps": [{
                    "id": "split",
                    "stepType": "Split",
                    "body": { "id": "split", "stepType": "Split" },
                    "nestedGraphs": [{
                        "role": "split.subgraph",
                        "graph": {
                            "steps": [{
                                "id": "finish",
                                "stepType": "Finish",
                                "body": { "id": "finish", "stepType": "Finish" }
                            }]
                        }
                    }]
                }, {
                    "id": "finish",
                    "stepType": "Finish",
                    "body": { "id": "finish", "stepType": "Finish" }
                }]
            }
        }))
        .expect("manifest json");

        DirectJsonManifest::parse(&manifest).expect("duplicate nested step ids are graph-scoped");
    }

    #[test]
    fn parse_collects_static_child_workflow_graph_mappings() {
        let manifest = serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "call_child",
                    "stepType": "EmbedWorkflow",
                    "purpose": "embedWorkflow.inputMapping",
                    "value": {
                        "childInput": {
                            "valueType": "reference",
                            "value": "data.input"
                        }
                    }
                }],
                "steps": [{
                    "id": "call_child",
                    "stepType": "EmbedWorkflow",
                    "body": { "id": "call_child", "stepType": "EmbedWorkflow" }
                }]
            },
            "childWorkflows": [{
                "stepId": "call_child",
                "workflowId": "child_workflow",
                "versionRequested": "latest",
                "versionResolved": 3,
                "graph": {
                    "mappings": [{
                        "id": 1,
                        "stepId": "finish",
                        "stepType": "Finish",
                        "purpose": "finish.inputMapping",
                        "value": {
                            "result": {
                                "valueType": "reference",
                                "value": "data.input"
                            }
                        }
                    }],
                    "steps": [{
                        "id": "finish",
                        "stepType": "Finish",
                        "body": { "id": "finish", "stepType": "Finish" }
                    }]
                }
            }]
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest).expect("manifest");

        let parent_source = build_source(br#"{"input":"parent"}"#, b"{}", b"{}").expect("source");
        let child_input = manifest
            .apply_mapping(0, &parent_source)
            .expect("parent-to-child mapping");
        let child_input: Value = serde_json::from_slice(&child_input).expect("child input json");
        assert_eq!(child_input, json!({ "childInput": "parent" }));

        let child_source = build_source(br#"{"input":"child"}"#, b"{}", b"{}").expect("source");
        let output = manifest
            .apply_mapping(1, &child_source)
            .expect("child finish mapping");
        let output: Value = serde_json::from_slice(&output).expect("output json");
        assert_eq!(output, json!({ "result": "child" }));
    }

    #[test]
    fn embed_workflow_helpers_build_child_scope_and_parent_step_result() {
        let manifest = serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "call_child",
                    "stepType": "EmbedWorkflow",
                    "purpose": "embedWorkflow.inputMapping",
                    "value": {
                        "childInput": {
                            "valueType": "reference",
                            "value": "data.input"
                        }
                    }
                }],
                "steps": [{
                    "id": "call_child",
                    "stepType": "EmbedWorkflow",
                    "name": "Call child",
                    "body": {
                        "id": "call_child",
                        "stepType": "EmbedWorkflow",
                        "name": "Call child"
                    }
                }]
            },
            "childWorkflows": [{
                "stepId": "call_child",
                "workflowId": "child_workflow",
                "versionRequested": "latest",
                "versionResolved": 3,
                "graph": {
                    "variables": {
                        "child_default": {
                            "type": "string",
                            "value": "from-child"
                        },
                        "_internal": {
                            "type": "string",
                            "value": "ignored"
                        }
                    },
                    "inputSchema": {
                        "childInput": {
                            "type": "string",
                            "required": true,
                            "description": "Child input"
                        }
                    },
                    "steps": [{
                        "id": "finish",
                        "stepType": "Finish",
                        "body": { "id": "finish", "stepType": "Finish" }
                    }]
                }
            }]
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest).expect("manifest");
        let source = build_source(
            br#"{"input":"parent"}"#,
            br#"{
                "_scope_id":"scope",
                "_workflow_id":"parent_workflow",
                "_instance_id":"instance-1",
                "_tenant_id":"tenant-1",
                "_cache_key_prefix":"root-prefix",
                "_loop_indices":[2,3]
            }"#,
            b"{}",
        )
        .expect("source");
        let child_input = manifest
            .apply_mapping(0, &source)
            .expect("child input mapping");

        let cache_key = manifest
            .embed_workflow_cache_key("call_child", &source)
            .expect("cache key");
        assert_eq!(
            std::str::from_utf8(&cache_key).expect("cache key utf8"),
            "root-prefix::embed_workflow::call_child::[2,3]"
        );

        let child_variables = manifest
            .embed_workflow_variables("call_child", &source, &child_input)
            .expect("child variables");
        let child_variables: Value =
            serde_json::from_slice(&child_variables).expect("child variables json");
        assert_eq!(child_variables["_scope_id"], "scope_call_child");
        assert_eq!(child_variables["_workflow_id"], "parent_workflow");
        assert_eq!(child_variables["_instance_id"], "instance-1");
        assert_eq!(child_variables["_tenant_id"], "tenant-1");
        assert_eq!(
            child_variables["_cache_key_prefix"],
            "root-prefix__call_child[2,3]"
        );
        assert_eq!(child_variables["child_default"], "from-child");
        assert!(child_variables.get("_internal").is_none());

        let step_result = manifest
            .embed_workflow_result("call_child", &source, br#"{"result":"child-ok"}"#)
            .expect("step result");
        let step_result_json: Value =
            serde_json::from_slice(&step_result).expect("step result json");
        assert_eq!(step_result_json["stepId"], "call_child");
        assert_eq!(step_result_json["stepName"], "Call child");
        assert_eq!(step_result_json["stepType"], "EmbedWorkflow");
        assert_eq!(step_result_json["childWorkflowId"], "child_workflow");
        assert_eq!(step_result_json["outputs"]["result"], "child-ok");

        let steps = manifest
            .embed_workflow_output_from_result("call_child", &source, &step_result)
            .expect("parent steps");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        assert_eq!(steps["call_child"], step_result_json);

        let child_error = br#"{
            "stepId": "fail",
            "stepName": "Child Failure",
            "category": "permanent",
            "code": "CHILD_FAILED",
            "message": "Child workflow failed",
            "severity": "critical",
            "context": { "childInput": "child-ok" }
        }"#;
        let wrapped_error = manifest
            .embed_workflow_error("call_child", child_error)
            .expect("wrapped child error");
        let wrapped_error: Value =
            serde_json::from_slice(&wrapped_error).expect("wrapped child error json");
        assert_eq!(wrapped_error["stepId"], "call_child");
        assert_eq!(wrapped_error["stepName"], "Call child");
        assert_eq!(wrapped_error["stepType"], "EmbedWorkflow");
        assert_eq!(wrapped_error["code"], "CHILD_WORKFLOW_FAILED");
        assert_eq!(
            wrapped_error["message"],
            "Child workflow child_workflow failed"
        );
        assert_eq!(wrapped_error["category"], "permanent");
        assert_eq!(wrapped_error["severity"], "critical");
        assert_eq!(wrapped_error["childWorkflowId"], "child_workflow");
        assert_eq!(wrapped_error["childError"]["code"], "CHILD_FAILED");

        let err = manifest
            .embed_workflow_variables("call_child", &source, b"{}")
            .expect_err("missing child input should fail");
        assert!(err.contains("missing required inputs"));
        assert!(err.contains("childInput (string): Child input [not provided]"));
    }

    #[test]
    fn apply_finish_mapping_resolves_simple_passthrough() {
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "result": { "valueType": "reference", "value": "data.input" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"input":"hello"}"#, b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "result": "hello" }));
    }

    #[test]
    fn finish_mapping_unwraps_outputs_field_after_dotted_insert() {
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "outputs.value": { "valueType": "immediate", "value": 7 }
        })))
        .expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "value": 7 }));
    }

    #[test]
    fn apply_mapping_handles_defaults_templates_and_composites() {
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "fallback": {
                "valueType": "reference",
                "value": "data.missing",
                "type": "string",
                "default": 42
            },
            "nullFallback": {
                "valueType": "reference",
                "value": "data.nullish",
                "default": "defaulted"
            },
            "message": {
                "valueType": "template",
                "value": "hello {{ data.name }}"
            },
            "nested": {
                "valueType": "composite",
                "value": {
                    "first": { "valueType": "reference", "value": "steps.prev.outputs.first" },
                    "items": {
                        "valueType": "composite",
                        "value": [
                            { "valueType": "reference", "value": "workflow.inputs.data.name" },
                            { "valueType": "immediate", "value": true }
                        ]
                    }
                }
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"name":"Ada","nullish":null}"#,
            b"{}",
            br#"{"prev":{"outputs":{"first":"alpha"}}}"#,
        )
        .expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(
            output,
            json!({
                "fallback": "42",
                "nullFallback": "defaulted",
                "message": "hello Ada",
                "nested": {
                    "first": "alpha",
                    "items": ["Ada", true]
                }
            })
        );
    }

    #[test]
    fn eval_condition_handles_equality_against_source() {
        let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "data.flag" },
                { "valueType": "immediate", "value": true }
            ]
        })))
        .expect("manifest");
        let source = build_source(br#"{"flag":true}"#, b"{}", b"{}").expect("source");

        assert!(manifest.eval_condition(0, &source).expect("condition"));
    }

    #[test]
    fn eval_condition_handles_length_comparison() {
        let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
            "type": "operation",
            "op": "GT",
            "arguments": [
                {
                    "type": "operation",
                    "op": "LENGTH",
                    "arguments": [
                        { "valueType": "reference", "value": "data.description" }
                    ]
                },
                { "valueType": "immediate", "value": 3 }
            ]
        })))
        .expect("manifest");
        let short = build_source(br#"{"description":"hey"}"#, b"{}", b"{}").expect("source");
        let long = build_source(br#"{"description":"hello"}"#, b"{}", b"{}").expect("source");

        assert!(!manifest.eval_condition(0, &short).expect("short"));
        assert!(manifest.eval_condition(0, &long).expect("long"));
    }

    #[test]
    fn eval_condition_handles_truthy_value_expression() {
        let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
            "type": "value",
            "valueType": "reference",
            "value": "data.present"
        })))
        .expect("manifest");
        let source = build_source(br#"{"present":"yes"}"#, b"{}", b"{}").expect("source");

        assert!(manifest.eval_condition(0, &source).expect("condition"));
    }

    #[test]
    fn split_items_normalizes_arrays_single_values_nulls_and_batches() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "allowNull": true,
            "convertSingleValue": true,
            "batchSize": 2
        })))
        .expect("manifest");
        let array_source = build_source(br#"{"items":[1,2,3]}"#, b"{}", b"{}").expect("source");
        let single_source = build_source(br#"{"items":"one"}"#, b"{}", b"{}").expect("source");
        let null_source = build_source(br#"{"items":null}"#, b"{}", b"{}").expect("source");

        let array_items = manifest.split_items(0, &array_source).expect("array items");
        let array_items: Value = serde_json::from_slice(&array_items).expect("array json");
        let single_items = manifest
            .split_items(0, &single_source)
            .expect("single items");
        let single_items: Value = serde_json::from_slice(&single_items).expect("single json");
        let null_items = manifest.split_items(0, &null_source).expect("null items");
        let null_items: Value = serde_json::from_slice(&null_items).expect("null json");

        assert_eq!(array_items, json!([[1, 2], [3]]));
        assert_eq!(single_items, json!([["one"]]));
        assert_eq!(null_items, json!([]));
    }

    #[test]
    fn split_item_count_and_item_use_normalized_items() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "batchSize": 2
        })))
        .expect("manifest");
        let source = build_source(br#"{"items":[1,2,3]}"#, b"{}", b"{}").expect("source");

        assert_eq!(
            manifest
                .split_item_count(0, &source)
                .expect("split item count"),
            2
        );
        let first = manifest
            .split_item(0, &source, 0)
            .expect("first split item");
        let second = manifest
            .split_item(0, &source, 1)
            .expect("second split item");
        let first: Value = serde_json::from_slice(&first).expect("first item json");
        let second: Value = serde_json::from_slice(&second).expect("second item json");

        assert_eq!(first, json!([1, 2]));
        assert_eq!(second, json!([3]));
    }

    #[test]
    fn split_item_rejects_out_of_bounds_index() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"items":[1]}"#, b"{}", b"{}").expect("source");

        let err = manifest
            .split_item(0, &source, 1)
            .expect_err("index should fail");

        assert_eq!(
            err,
            "Split step 'split' item index 1 is out of bounds for 1 item(s)"
        );
    }

    #[test]
    fn split_items_rejects_null_and_non_array_without_flags() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");
        let null_source = build_source(br#"{"items":null}"#, b"{}", b"{}").expect("source");
        let object_source = build_source(br#"{"items":{"id":1}}"#, b"{}", b"{}").expect("source");

        let null_err = manifest
            .split_items(0, &null_source)
            .expect_err("null should fail");
        let object_err = manifest
            .split_items(0, &object_source)
            .expect_err("object should fail");

        assert_eq!(
            null_err,
            "Split step 'split' received null value. Set 'allowNull: true' to allow empty iterations, or use 'transform/ensure-array' agent."
        );
        assert_eq!(
            object_err,
            "Split step 'split' expected array, got object. Set 'convertSingleValue: true' to auto-wrap, or use 'transform/ensure-array' agent."
        );
    }

    #[test]
    fn split_iteration_variables_match_generated_scope_shape() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "variables": {
                "tenant": { "valueType": "reference", "value": "data.tenant" }
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"tenant":"acme","items":[{"id":7}]}"#,
            br#"{"_workflow_id":"wf-1","_scope_id":"parent","_loop_indices":[2],"existing":true}"#,
            b"{}",
        )
        .expect("source");

        let variables = manifest
            .split_iteration_variables(0, &source, br#"{"id":7}"#, 3)
            .expect("iteration variables");
        let variables: Value = serde_json::from_slice(&variables).expect("variables json");

        assert_eq!(variables["_workflow_id"], json!("wf-1"));
        assert_eq!(variables["existing"], json!(true));
        assert_eq!(variables["tenant"], json!("acme"));
        assert_eq!(variables["_loop_indices"], json!([2, 3]));
        assert_eq!(variables["_index"], json!(3));
        assert_eq!(variables["_item"], json!({ "id": 7 }));
        assert_eq!(variables["_scope_id"], json!("parent_split_3"));
    }

    #[test]
    fn split_schema_validation_accepts_matching_input_and_output() {
        let manifest = DirectJsonManifest::parse(&split_manifest_with_schemas(
            json!({
                "value": { "valueType": "reference", "value": "data.items" }
            }),
            json!({
                "value": { "type": "string", "required": true },
                "count": { "type": "integer", "required": false }
            }),
            json!({
                "processed": { "type": "object", "required": true }
            }),
        ))
        .expect("manifest");

        manifest
            .split_validate_input(0, br#"{"value":"ok","count":2}"#, 1)
            .expect("input schema");
        manifest
            .split_validate_output(0, br#"{"processed":{"ok":true}}"#, 1)
            .expect("output schema");
    }

    #[test]
    fn split_schema_validation_reports_missing_and_wrong_type_fields() {
        let manifest = DirectJsonManifest::parse(&split_manifest_with_schemas(
            json!({
                "value": { "valueType": "reference", "value": "data.items" }
            }),
            json!({
                "value": { "type": "string", "required": true },
                "count": { "type": "integer", "required": true }
            }),
            json!({}),
        ))
        .expect("manifest");

        let err = manifest
            .split_validate_input(0, br#"{"value":7}"#, 2)
            .expect_err("schema should fail");

        assert_eq!(
            err,
            "Split 'split' iteration 2: input: required field(s) [count] missing (got fields: [value]); type mismatches: 'value' (expected string, got number)"
        );
    }

    #[test]
    fn split_schema_validation_rejects_non_object_value() {
        let manifest = DirectJsonManifest::parse(&split_manifest_with_schemas(
            json!({
                "value": { "valueType": "reference", "value": "data.items" }
            }),
            json!({
                "value": { "type": "string", "required": true }
            }),
            json!({}),
        ))
        .expect("manifest");

        let err = manifest
            .split_validate_input(0, br#""not-object""#, 0)
            .expect_err("schema should fail");

        assert_eq!(
            err,
            "Split 'split' iteration 0: input: expected object, got string"
        );
    }

    #[test]
    fn split_append_output_accumulates_iteration_outputs() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");

        let results = manifest
            .split_append_output(0, b"[]", br#"{"id":1}"#)
            .expect("first append");
        let results = manifest
            .split_append_output(0, &results, br#"{"id":2}"#)
            .expect("second append");
        let results: Value = serde_json::from_slice(&results).expect("results json");

        assert_eq!(results, json!([{ "id": 1 }, { "id": 2 }]));
    }

    #[test]
    fn split_dont_stop_accumulator_records_successes_and_errors() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "dontStopOnFailed": true
        })))
        .expect("manifest");

        let results = manifest
            .split_initial_results(0)
            .expect("initial accumulator");
        let results = manifest
            .split_append_output(0, &results, br#"{"id":1}"#)
            .expect("success append");
        let results = manifest
            .split_append_error(0, &results, "bad item".to_string(), 1)
            .expect("error append");
        let results: Value = serde_json::from_slice(&results).expect("results json");

        assert_eq!(results["success"], json!([{ "id": 1 }]));
        assert_eq!(
            results["error"],
            json!([{ "error": "bad item", "index": 1 }])
        );
        assert_eq!(results["aborted"], json!([]));
        assert_eq!(results["unknown"], json!([]));
        assert_eq!(results["skipped"], json!([]));
    }

    #[test]
    fn split_dont_stop_output_records_generated_step_result_shape() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "dontStopOnFailed": true
        })))
        .expect("manifest");
        let source = build_source(br#"{"items":[1,2]}"#, b"{}", br#"{"prev":{"outputs":1}}"#)
            .expect("source");

        let results = manifest
            .split_initial_results(0)
            .expect("initial accumulator");
        let results = manifest
            .split_append_output(0, &results, br#"{"id":1}"#)
            .expect("success append");
        let results = manifest
            .split_append_error(0, &results, "bad item".to_string(), 1)
            .expect("error append");
        let steps = manifest
            .split_output(0, &source, &results)
            .expect("Split steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["prev"]["outputs"], json!(1));
        assert_eq!(steps["split"]["stepId"], json!("split"));
        assert_eq!(steps["split"]["stepName"], json!("Process Items"));
        assert_eq!(steps["split"]["stepType"], json!("Split"));
        assert_eq!(steps["split"]["data"]["success"], json!([{ "id": 1 }]));
        assert_eq!(
            steps["split"]["data"]["error"],
            json!([{ "error": "bad item", "index": 1 }])
        );
        assert_eq!(steps["split"]["stats"]["success"], json!(1));
        assert_eq!(steps["split"]["stats"]["error"], json!(1));
        assert_eq!(steps["split"]["stats"]["total"], json!(2));
        assert_eq!(steps["split"]["outputs"], json!([{ "id": 1 }]));
    }

    #[test]
    fn split_append_output_rejects_non_array_accumulator() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");

        let err = manifest
            .split_append_output(0, br#"{"not":"array"}"#, br#"{"id":1}"#)
            .expect_err("non-array accumulator should fail");

        assert_eq!(
            err,
            "Split step 'split' internal result accumulator must be an array"
        );
    }

    #[test]
    fn split_output_records_generated_step_envelope() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");
        let source =
            build_source(br#"{"items":[1]}"#, b"{}", br#"{"prev":{"outputs":1}}"#).expect("source");

        let steps = manifest
            .split_output(0, &source, br#"[{"ok":true}]"#)
            .expect("Split steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["prev"]["outputs"], json!(1));
        assert_eq!(steps["split"]["stepId"], json!("split"));
        assert_eq!(steps["split"]["stepName"], json!("Process Items"));
        assert_eq!(steps["split"]["stepType"], json!("Split"));
        assert_eq!(steps["split"]["outputs"], json!([{ "ok": true }]));
    }

    #[test]
    fn split_cache_key_uses_workflow_id_prefix_and_loop_indices() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"items":[{"id":1}]}"#,
            br#"{"_workflow_id":"wf-42","_loop_indices":[1,"x"]}"#,
            b"{}",
        )
        .expect("source");

        let key = manifest.split_cache_key(0, &source).expect("cache key");

        assert_eq!(
            String::from_utf8(key).expect("utf8"),
            "wf-42::split::split::[1,\"x\"]"
        );
    }

    #[test]
    fn split_result_can_be_inserted_into_steps_context() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"items":[{"id":1}]}"#, b"{}", b"{}").expect("source");

        let result = manifest
            .split_result(0, &source, br#"[{"ok":true}]"#)
            .expect("Split result");
        let steps = manifest
            .split_output_from_result(0, &source, &result)
            .expect("Split steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["split"]["stepId"], json!("split"));
        assert_eq!(steps["split"]["stepType"], json!("Split"));
        assert_eq!(steps["split"]["outputs"], json!([{ "ok": true }]));
        assert_eq!(
            serde_json::from_slice::<Value>(&result).expect("result json"),
            steps["split"]
        );
    }

    #[test]
    fn while_default_max_iterations_is_generated_code_default() {
        let manifest = DirectJsonManifest::parse(&while_manifest(
            json!({}),
            json!({ "valueType": "immediate", "value": false }),
        ))
        .expect("manifest");

        assert_eq!(
            manifest.while_max_iterations(0).expect("max iterations"),
            10
        );
    }

    #[test]
    fn while_helpers_match_generated_state_condition_and_output_shape() {
        let manifest = DirectJsonManifest::parse(&while_manifest(
            json!({ "maxIterations": 3 }),
            json!({
                "type": "operation",
                "op": "LT",
                "arguments": [
                    { "valueType": "reference", "value": "loop.index" },
                    { "valueType": "immediate", "value": 2 }
                ]
            }),
        ))
        .expect("manifest");
        let source = build_source(
            br#"{"input":"hello"}"#,
            br#"{"_workflow_id":"wf-1","_scope_id":"parent","_loop_indices":[4],"keep":true}"#,
            br#"{"prev":{"outputs":1}}"#,
        )
        .expect("source");

        assert_eq!(manifest.while_max_iterations(0).expect("max iterations"), 3);
        let state = manifest.while_initial_state(0).expect("initial state");
        let state_value: Value = serde_json::from_slice(&state).expect("state json");
        assert_eq!(state_value, json!({ "index": 0, "outputs": null }));

        let condition_source = manifest
            .while_condition_source(0, &source, &state)
            .expect("condition source");
        let condition_source: Value =
            serde_json::from_slice(&condition_source).expect("condition source json");
        assert_eq!(
            condition_source["loop"],
            json!({ "index": 0, "outputs": null })
        );
        assert!(
            manifest
                .while_condition(
                    0,
                    &serde_json::to_vec(&condition_source).expect("condition source bytes")
                )
                .expect("condition")
        );

        let iteration_variables = manifest
            .while_iteration_variables(
                0,
                br#"{"_workflow_id":"wf-1","_scope_id":"parent","_loop_indices":[4],"keep":true}"#,
                &state,
            )
            .expect("iteration variables");
        let iteration_variables: Value =
            serde_json::from_slice(&iteration_variables).expect("variables json");
        assert_eq!(iteration_variables["_workflow_id"], json!("wf-1"));
        assert_eq!(iteration_variables["keep"], json!(true));
        assert_eq!(iteration_variables["_loop_indices"], json!([4, 0]));
        assert_eq!(iteration_variables["_index"], json!(0));
        assert_eq!(
            iteration_variables["_loop"],
            json!({ "index": 0, "outputs": null })
        );
        assert!(iteration_variables.get("_previousOutputs").is_none());
        assert_eq!(iteration_variables["_scope_id"], json!("parent_loop_0"));

        let state = manifest
            .while_advance_state(0, &state, br#"{"counter":1}"#)
            .expect("advanced state");
        let iteration_variables = manifest
            .while_iteration_variables(
                0,
                br#"{"_workflow_id":"wf-1","_scope_id":"parent","_loop_indices":[4]}"#,
                &state,
            )
            .expect("iteration variables");
        let iteration_variables: Value =
            serde_json::from_slice(&iteration_variables).expect("variables json");
        assert_eq!(iteration_variables["_loop_indices"], json!([4, 1]));
        assert_eq!(iteration_variables["_index"], json!(1));
        assert_eq!(
            iteration_variables["_previousOutputs"],
            json!({ "counter": 1 })
        );
        assert_eq!(
            iteration_variables["_loop"],
            json!({ "index": 1, "outputs": { "counter": 1 } })
        );

        let condition_source = manifest
            .while_condition_source(0, &source, &state)
            .expect("condition source");
        assert!(
            manifest
                .while_condition(0, &condition_source)
                .expect("second condition")
        );
        let state = manifest
            .while_advance_state(0, &state, br#"{"counter":2}"#)
            .expect("second advanced state");
        let condition_source = manifest
            .while_condition_source(0, &source, &state)
            .expect("condition source");
        assert!(
            !manifest
                .while_condition(0, &condition_source)
                .expect("final condition")
        );

        let steps = manifest
            .while_output(0, &source, &state)
            .expect("While steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["prev"]["outputs"], json!(1));
        assert_eq!(steps["loop"]["stepId"], json!("loop"));
        assert_eq!(steps["loop"]["stepName"], json!("Counter Loop"));
        assert_eq!(steps["loop"]["stepType"], json!("While"));
        assert_eq!(
            steps["loop"]["outputs"],
            json!({ "iterations": 2, "outputs": { "counter": 2 } })
        );
    }

    #[test]
    fn filter_keeps_items_matching_condition() {
        let manifest = DirectJsonManifest::parse(&filter_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "item.status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"failed"},{"id":3,"status":"active"}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.filter(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["filter"]["outputs"];

        assert_eq!(output["count"], json!(2));
        assert_eq!(output["items"][0]["id"], json!(1));
        assert_eq!(output["items"][1]["id"], json!(3));
        assert_eq!(steps["filter"]["stepName"], json!("Filter Active Items"));
        assert_eq!(steps["filter"]["stepType"], json!("Filter"));
    }

    #[test]
    fn filter_supports_nested_boolean_conditions() {
        let manifest = DirectJsonManifest::parse(&filter_manifest(json!({
            "value": { "valueType": "reference", "value": "data.users" },
            "condition": {
                "type": "operation",
                "op": "OR",
                "arguments": [
                    {
                        "type": "operation",
                        "op": "AND",
                        "arguments": [
                            {
                                "type": "operation",
                                "op": "EQ",
                                "arguments": [
                                    { "valueType": "reference", "value": "item.status" },
                                    { "valueType": "immediate", "value": "active" }
                                ]
                            },
                            {
                                "type": "operation",
                                "op": "GT",
                                "arguments": [
                                    { "valueType": "reference", "value": "item.age" },
                                    { "valueType": "immediate", "value": 18 }
                                ]
                            }
                        ]
                    },
                    {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "item.role" },
                            { "valueType": "immediate", "value": "admin" }
                        ]
                    }
                ]
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"users":[{"id":1,"status":"active","age":19,"role":"user"},{"id":2,"status":"active","age":17,"role":"user"},{"id":3,"status":"disabled","age":15,"role":"admin"}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.filter(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["filter"]["outputs"];

        assert_eq!(output["count"], json!(2));
        assert_eq!(output["items"][0]["id"], json!(1));
        assert_eq!(output["items"][1]["id"], json!(3));
    }

    #[test]
    fn filter_treats_non_array_input_as_empty_array() {
        let manifest = DirectJsonManifest::parse(&filter_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "item.status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        })))
        .expect("manifest");
        let source =
            build_source(br#"{"items":{"status":"active"}}"#, b"{}", b"{}").expect("source");

        let steps = manifest.filter(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["filter"]["outputs"];

        assert_eq!(output["count"], json!(0));
        assert_eq!(output["items"], json!([]));
    }

    #[test]
    fn value_switch_selects_first_matching_case() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [
                {
                    "matchType": "EQ",
                    "match": "active",
                    "output": {
                        "bucket": { "valueType": "immediate", "value": "ready" },
                        "echo": { "valueType": "reference", "value": "data.status" }
                    }
                },
                {
                    "matchType": "EQ",
                    "match": ["active", "retry"],
                    "output": { "bucket": "array-match" }
                }
            ],
            "default": { "bucket": "other" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"status":"active"}"#, b"{}", b"{}").expect("source");

        let steps = manifest.value_switch(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["switch"]["outputs"];

        assert_eq!(output, &json!({ "bucket": "ready", "echo": "active" }));
        assert_eq!(steps["switch"]["stepName"], json!("Classify Status"));
        assert_eq!(steps["switch"]["stepType"], json!("Switch"));
        assert!(steps["switch"].get("route").is_none());
    }

    #[test]
    fn value_switch_supports_array_match_and_default() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [
                {
                    "matchType": "EQ",
                    "match": ["queued", "retry"],
                    "output": { "bucket": "pending" }
                }
            ],
            "default": { "bucket": "other" }
        })))
        .expect("manifest");
        let queued = build_source(br#"{"status":"queued"}"#, b"{}", b"{}").expect("source");
        let unknown = build_source(br#"{"status":"done"}"#, b"{}", b"{}").expect("source");

        let queued_steps = manifest.value_switch(0, &queued).expect("queued steps");
        let queued_steps: Value = serde_json::from_slice(&queued_steps).expect("queued json");
        assert_eq!(
            queued_steps["switch"]["outputs"],
            json!({ "bucket": "pending" })
        );

        let unknown_steps = manifest.value_switch(0, &unknown).expect("unknown steps");
        let unknown_steps: Value = serde_json::from_slice(&unknown_steps).expect("unknown json");
        assert_eq!(
            unknown_steps["switch"]["outputs"],
            json!({ "bucket": "other" })
        );
    }

    #[test]
    fn value_switch_supports_between_and_range_cases() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.score" },
            "cases": [
                {
                    "matchType": "BETWEEN",
                    "match": [80, 100],
                    "output": { "grade": "high" }
                },
                {
                    "matchType": "RANGE",
                    "match": { "gte": 50, "lt": 80 },
                    "output": { "grade": "mid" }
                }
            ],
            "default": { "grade": "low" }
        })))
        .expect("manifest");

        for (input, expected) in [
            (br#"{"score":90}"#.as_slice(), json!({ "grade": "high" })),
            (br#"{"score":65}"#.as_slice(), json!({ "grade": "mid" })),
            (br#"{"score":20}"#.as_slice(), json!({ "grade": "low" })),
        ] {
            let source = build_source(input, b"{}", b"{}").expect("source");
            let steps = manifest.value_switch(0, &source).expect("steps context");
            let steps: Value = serde_json::from_slice(&steps).expect("steps json");
            assert_eq!(steps["switch"]["outputs"], expected);
        }
    }

    #[test]
    fn routing_switch_returns_route_and_records_route_in_steps_context() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [
                {
                    "matchType": "EQ",
                    "match": "active",
                    "output": {
                        "bucket": { "valueType": "immediate", "value": "ready" },
                        "echo": { "valueType": "reference", "value": "data.status" }
                    },
                    "route": "active"
                },
                {
                    "matchType": "EQ",
                    "match": ["queued", "retry"],
                    "output": { "bucket": "pending" },
                    "route": "pending"
                }
            ],
            "default": { "bucket": "other" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"status":"active"}"#, b"{}", b"{}").expect("source");

        let route = manifest.process_switch(0, &source).expect("switch route");
        let steps = manifest.value_switch(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(route, "active");
        assert_eq!(
            steps["switch"]["outputs"],
            json!({ "bucket": "ready", "echo": "active" })
        );
        assert_eq!(steps["switch"]["route"], json!("active"));
    }

    #[test]
    fn routing_switch_default_route_is_recorded() {
        let manifest = DirectJsonManifest::parse(&switch_manifest(json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [
                {
                    "matchType": "EQ",
                    "match": "active",
                    "output": { "bucket": "ready" },
                    "route": "active"
                }
            ],
            "default": { "bucket": "other" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"status":"done"}"#, b"{}", b"{}").expect("source");

        let route = manifest.process_switch(0, &source).expect("switch route");
        let steps = manifest.value_switch(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(route, "default");
        assert_eq!(steps["switch"]["outputs"], json!({ "bucket": "other" }));
        assert_eq!(steps["switch"]["route"], json!("default"));
    }

    #[test]
    fn group_by_groups_items_by_simple_key() {
        let manifest = DirectJsonManifest::parse(&group_by_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "key": "status"
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"inactive"},{"id":3,"status":"active"}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.group_by(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["group"]["outputs"];

        assert_eq!(output["counts"], json!({ "active": 2, "inactive": 1 }));
        assert_eq!(output["total_groups"], json!(2));
        assert_eq!(output["groups"]["active"][0]["id"], json!(1));
        assert_eq!(output["groups"]["active"][1]["id"], json!(3));
        assert_eq!(steps["group"]["stepName"], json!("Group by Status"));
        assert_eq!(steps["group"]["stepType"], json!("GroupBy"));
    }

    #[test]
    fn group_by_handles_nested_keys_null_and_expected_keys() {
        let manifest = DirectJsonManifest::parse(&group_by_manifest(json!({
            "value": { "valueType": "reference", "value": "data.users" },
            "key": "profile.role",
            "expectedKeys": ["admin", "viewer", "missing"]
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"users":[{"id":1,"profile":{"role":"admin"}},{"id":2,"profile":{"role":"viewer"}},{"id":3,"profile":{}}]}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let steps = manifest.group_by(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["group"]["outputs"];

        assert_eq!(
            output["counts"],
            json!({ "_null": 1, "admin": 1, "missing": 0, "viewer": 1 })
        );
        assert_eq!(output["groups"]["missing"], json!([]));
        assert_eq!(output["groups"]["_null"][0]["id"], json!(3));
        assert_eq!(output["total_groups"], json!(4));
    }

    #[test]
    fn group_by_treats_non_array_input_as_empty_array() {
        let manifest = DirectJsonManifest::parse(&group_by_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "key": "status",
            "expectedKeys": ["active"]
        })))
        .expect("manifest");
        let source =
            build_source(br#"{"items":{"status":"active"}}"#, b"{}", b"{}").expect("source");

        let steps = manifest.group_by(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let output = &steps["group"]["outputs"];

        assert_eq!(output["counts"], json!({ "active": 0 }));
        assert_eq!(output["groups"], json!({ "active": [] }));
        assert_eq!(output["total_groups"], json!(1));
    }

    #[test]
    fn delay_resolves_duration_and_stores_generated_step_shape() {
        let manifest = DirectJsonManifest::parse(&delay_manifest(json!({
            "valueType": "reference",
            "value": "data.waitTime"
        })))
        .expect("manifest");
        let source = build_source(br#"{"waitTime":250}"#, b"{}", b"{}").expect("source");

        let duration_ms = manifest
            .delay_duration_ms(0, &source)
            .expect("delay duration");
        let steps = manifest
            .delay(0, &source, duration_ms)
            .expect("Delay steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(duration_ms, 250);
        assert_eq!(steps["delay"]["stepId"], json!("delay"));
        assert_eq!(steps["delay"]["stepName"], json!("Wait"));
        assert_eq!(steps["delay"]["stepType"], json!("Delay"));
        assert_eq!(steps["delay"]["duration_ms"], json!(250));
        assert!(steps["delay"].get("outputs").is_none());
    }

    #[test]
    fn delay_rejects_non_numeric_duration() {
        let manifest = DirectJsonManifest::parse(&delay_manifest(json!({
            "valueType": "reference",
            "value": "data.waitTime"
        })))
        .expect("manifest");
        let source = build_source(br#"{"waitTime":"slow"}"#, b"{}", b"{}").expect("source");

        let err = manifest
            .delay_duration_ms(0, &source)
            .expect_err("string delay should fail");

        assert_eq!(
            err,
            "Delay step 'delay': duration_ms must be a number, got: \"slow\""
        );
    }

    #[test]
    fn delay_debug_end_uses_generated_step_shape() {
        let manifest = DirectJsonManifest::parse(&delay_manifest(json!({
            "valueType": "immediate",
            "value": 10
        })))
        .expect("manifest");
        let source = build_source(br#"{}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("delay", &source)
            .expect("debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        let end = manifest
            .step_debug_end("delay", &source)
            .expect("debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");

        assert_eq!(start["inputs"], json!({ "duration_ms": 10 }));
        assert_eq!(start["input_mapping"]["value"], json!(10));
        assert_eq!(end["outputs"]["duration_ms"], json!(10));
        assert!(end["outputs"].get("outputs").is_none());
    }

    #[test]
    fn breakpoint_key_and_event_match_generated_shape() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input",
            "breakpoint": true
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"case_id":"case-42"}"#,
            br#"{"_loop_indices":[1,2,"ignored"]}"#,
            br#"{"before":{"stepId":"before","outputs":1}}"#,
        )
        .expect("source");

        let key = manifest
            .breakpoint_key("wait", &source)
            .expect("breakpoint key");
        let event = manifest
            .breakpoint_event("wait", &source)
            .expect("breakpoint event");
        let event: Value = serde_json::from_slice(&event).expect("event json");

        assert_eq!(key, "breakpoint::wait::1_2");
        assert_eq!(event["step_id"], json!("wait"));
        assert_eq!(event["step_name"], json!("Review Input"));
        assert_eq!(event["step_type"], json!("WaitForSignal"));
        assert_eq!(event["inputs"], Value::Null);
        assert_eq!(event["steps_context"]["before"]["outputs"], json!(1));
    }

    #[test]
    fn finish_breakpoint_event_uses_raw_resolved_outputs() {
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "Finish",
            "finish",
            Some("Done"),
            json!({
                "mappings": [{
                    "id": 0,
                    "stepId": "finish",
                    "stepType": "Finish",
                    "purpose": "finish.inputMapping",
                    "value": {
                        "outputs.value": {
                            "valueType": "reference",
                            "value": "data.input"
                        }
                    }
                }]
            }),
        ))
        .expect("manifest");
        let source = build_source(
            br#"{"input":"mapped"}"#,
            br#"{"_loop_indices":[3]}"#,
            br#"{"before":{"outputs":true}}"#,
        )
        .expect("source");

        let key = manifest
            .breakpoint_key("finish", &source)
            .expect("breakpoint key");
        let event = manifest
            .breakpoint_event("finish", &source)
            .expect("breakpoint event");
        let event: Value = serde_json::from_slice(&event).expect("event json");

        assert_eq!(key, "breakpoint::finish::3");
        assert_eq!(event["step_id"], json!("finish"));
        assert_eq!(event["step_name"], json!("Done"));
        assert_eq!(event["step_type"], json!("Finish"));
        assert_eq!(event["inputs"], json!({ "outputs": { "value": "mapped" } }));
        assert_eq!(event["steps_context"]["before"]["outputs"], json!(true));
    }

    #[test]
    fn direct_control_breakpoint_events_use_generated_step_inputs() {
        let source = build_source(
            br#"{"status":"active","items":[{"status":"active"},{"status":"archived"}],"input":"hello"}"#,
            br#"{"tenant":"t1"}"#,
            br#"{"before":{"outputs":true}}"#,
        )
        .expect("source");

        let cases = [
            (
                "Conditional",
                "check",
                json!({
                    "conditions": [{
                        "id": 0,
                        "ownerId": "check",
                        "ownerType": "Conditional",
                        "purpose": "conditional.condition",
                        "value": {
                            "type": "operation",
                            "op": "EQ",
                            "arguments": [
                                { "valueType": "reference", "value": "data.status" },
                                { "valueType": "immediate", "value": "active" }
                            ]
                        }
                    }]
                }),
                json!({
                    "data": {
                        "status": "active",
                        "items": [
                            { "status": "active" },
                            { "status": "archived" }
                        ],
                        "input": "hello"
                    },
                    "variables": { "tenant": "t1" },
                    "steps": { "before": { "outputs": true } },
                    "workflow": {
                        "inputs": {
                            "data": {
                                "status": "active",
                                "items": [
                                    { "status": "active" },
                                    { "status": "archived" }
                                ],
                                "input": "hello"
                            },
                            "variables": { "tenant": "t1" }
                        }
                    }
                }),
            ),
            (
                "Filter",
                "filter",
                json!({
                    "filters": [{
                        "id": 0,
                        "stepId": "filter",
                        "name": "Filter Active Items",
                        "stepType": "Filter",
                        "purpose": "filter.config",
                        "value": {
                            "value": { "valueType": "reference", "value": "data.items" },
                            "condition": {
                                "type": "operation",
                                "op": "EQ",
                                "arguments": [
                                    { "valueType": "reference", "value": "item.status" },
                                    { "valueType": "immediate", "value": "active" }
                                ]
                            }
                        }
                    }]
                }),
                json!([
                    { "status": "active" },
                    { "status": "archived" }
                ]),
            ),
            (
                "Switch",
                "switch",
                json!({
                    "switches": [{
                        "id": 0,
                        "stepId": "switch",
                        "name": "Classify Status",
                        "stepType": "Switch",
                        "purpose": "switch.config",
                        "value": {
                            "value": { "valueType": "reference", "value": "data.status" },
                            "cases": [{
                                "matchType": "EQ",
                                "match": "active",
                                "output": { "bucket": "ready" }
                            }],
                            "default": { "bucket": "other" }
                        }
                    }]
                }),
                json!({
                    "value": "active",
                    "cases": [{
                        "matchType": "EQ",
                        "match": "active",
                        "output": { "bucket": "ready" }
                    }],
                    "default": { "bucket": "other" }
                }),
            ),
            (
                "GroupBy",
                "group",
                json!({
                    "groupBys": [{
                        "id": 0,
                        "stepId": "group",
                        "name": "Group by Status",
                        "stepType": "GroupBy",
                        "purpose": "groupBy.config",
                        "value": {
                            "value": { "valueType": "reference", "value": "data.items" },
                            "key": "status"
                        }
                    }]
                }),
                json!([
                    { "status": "active" },
                    { "status": "archived" }
                ]),
            ),
            (
                "Split",
                "split",
                json!({
                    "splits": [{
                        "id": 0,
                        "stepId": "split",
                        "name": "Split Items",
                        "stepType": "Split",
                        "purpose": "split.config",
                        "value": {
                            "value": { "valueType": "reference", "value": "data.items" },
                            "parallelism": 2,
                            "sequential": true,
                            "dontStopOnFailed": true,
                            "allowNull": true,
                            "convertSingleValue": true,
                            "batchSize": 10,
                            "variables": {
                                "tenant": { "valueType": "reference", "value": "variables.tenant" }
                            }
                        }
                    }]
                }),
                json!({
                    "value": [
                        { "status": "active" },
                        { "status": "archived" }
                    ],
                    "parallelism": 2,
                    "sequential": true,
                    "dontStopOnFailed": true,
                    "allowNull": true,
                    "convertSingleValue": true,
                    "batchSize": 10,
                    "variables": { "tenant": "t1" }
                }),
            ),
            (
                "While",
                "loop",
                json!({
                    "whiles": [{
                        "id": 0,
                        "stepId": "loop",
                        "name": "Loop Items",
                        "stepType": "While",
                        "purpose": "while.config",
                        "value": {
                            "maxIterations": 5
                        },
                        "condition": {
                            "type": "operation",
                            "op": "LT",
                            "arguments": [
                                { "valueType": "reference", "value": "loop.index" },
                                { "valueType": "reference", "value": "data.count" }
                            ]
                        }
                    }]
                }),
                json!({ "maxIterations": 5 }),
            ),
            (
                "Log",
                "log",
                json!({
                    "logs": [{
                        "id": 0,
                        "stepId": "log",
                        "name": "Log Start",
                        "stepType": "Log",
                        "purpose": "log.config",
                        "value": {
                            "id": "log",
                            "stepType": "Log",
                            "message": "Starting workflow",
                            "context": {
                                "input": { "valueType": "reference", "value": "data.input" },
                                "static": { "valueType": "immediate", "value": 42 }
                            }
                        }
                    }]
                }),
                json!({ "input": "hello", "static": 42 }),
            ),
            (
                "Error",
                "fail",
                json!({
                    "errors": [{
                        "id": 0,
                        "stepId": "fail",
                        "name": "Fail Fast",
                        "stepType": "Error",
                        "purpose": "error.config",
                        "value": {
                            "id": "fail",
                            "stepType": "Error",
                            "code": "TEMPORARY_FAILURE",
                            "message": "Try again later",
                            "context": {
                                "input": { "valueType": "reference", "value": "data.input" },
                                "static": { "valueType": "immediate", "value": 42 }
                            }
                        }
                    }]
                }),
                json!({ "input": "hello", "static": 42 }),
            ),
        ];

        for (step_type, step_id, collections, expected_inputs) in cases {
            let manifest =
                DirectJsonManifest::parse(&debug_manifest(step_type, step_id, None, collections))
                    .expect("manifest");
            let event = manifest
                .breakpoint_event(step_id, &source)
                .expect("breakpoint event");
            let event: Value = serde_json::from_slice(&event).expect("event json");

            assert_eq!(
                event["inputs"], expected_inputs,
                "{step_type} breakpoint inputs should match generated code"
            );
            assert_eq!(event["step_type"], json!(step_type));
        }
    }

    #[test]
    fn agent_breakpoint_event_uses_mapped_inputs_before_connection_injection() {
        let manifest =
            DirectJsonManifest::parse(&agent_manifest_with_required_inputs_and_connection(
                json!({
                    "value": { "valueType": "reference", "value": "data.value" },
                    "tenant": { "valueType": "reference", "value": "variables.tenant" }
                }),
                json!([]),
                Some("crm-main"),
            ))
            .expect("manifest");
        let source = build_source(
            br#"{"value":"agent-input"}"#,
            br#"{"tenant":"t1"}"#,
            br#"{"before":{"outputs":true}}"#,
        )
        .expect("source");

        let event = manifest
            .breakpoint_event("agent", &source)
            .expect("breakpoint event");
        let event: Value = serde_json::from_slice(&event).expect("event json");

        assert_eq!(event["step_id"], json!("agent"));
        assert_eq!(event["step_name"], json!("Normalize Data"));
        assert_eq!(event["step_type"], json!("Agent"));
        assert_eq!(
            event["inputs"],
            json!({ "value": "agent-input", "tenant": "t1" })
        );
        assert!(
            event["inputs"].get("connection").is_none(),
            "Agent breakpoint payload should match generated mapped inputs before connection injection"
        );
        assert_eq!(event["steps_context"]["before"]["outputs"], json!(true));
    }

    #[test]
    fn embed_workflow_breakpoint_event_uses_resolved_child_inputs() {
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "EmbedWorkflow",
            "call_child",
            Some("Call child"),
            json!({}),
        ))
        .expect("manifest");
        let source = build_source(
            br#"{"childInput":"mapped","count":2}"#,
            br#"{"_loop_indices":[4],"tenant":"t1"}"#,
            br#"{"before":{"outputs":true}}"#,
        )
        .expect("source");

        let key = manifest
            .breakpoint_key("call_child", &source)
            .expect("breakpoint key");
        let event = manifest
            .breakpoint_event("call_child", &source)
            .expect("breakpoint event");
        let event: Value = serde_json::from_slice(&event).expect("event json");

        assert_eq!(key, "breakpoint::call_child::4");
        assert_eq!(event["step_id"], json!("call_child"));
        assert_eq!(event["step_name"], json!("Call child"));
        assert_eq!(event["step_type"], json!("EmbedWorkflow"));
        assert_eq!(
            event["inputs"],
            json!({ "childInput": "mapped", "count": 2 })
        );
        assert_eq!(event["steps_context"]["before"]["outputs"], json!(true));
    }

    #[test]
    fn wait_signal_id_matches_generated_shape_with_loop_indices() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input"
        })))
        .expect("manifest");
        let source = build_source(
            br#"{}"#,
            br#"{"_workflow_id":"child","_loop_indices":[0,2]}"#,
            b"{}",
        )
        .expect("source");

        let signal_id = manifest
            .wait_signal_id("wait", "inst-1", &source)
            .expect("signal id");

        assert_eq!(signal_id, "inst-1/child/wait/[0,2]");
    }

    #[test]
    fn wait_timeout_and_poll_interval_use_step_body() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input",
            "timeoutMs": {
                "valueType": "reference",
                "value": "data.timeout_ms"
            },
            "pollIntervalMs": 250
        })))
        .expect("manifest");
        let source = build_source(br#"{"timeout_ms":60000}"#, b"{}", b"{}").expect("source");

        assert_eq!(
            manifest.wait_timeout_ms("wait", &source).expect("timeout"),
            Some(60000)
        );
        assert_eq!(
            manifest
                .wait_poll_interval_ms("wait")
                .expect("poll interval"),
            250
        );
    }

    #[test]
    fn wait_timeout_error_matches_generated_failure_message() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input"
        })))
        .expect("manifest");

        let error = manifest
            .wait_timeout_error("wait", "inst-1/root/wait", 500)
            .expect("timeout error");

        assert_eq!(
            String::from_utf8(error).expect("utf8 error"),
            "WaitForSignal step 'wait' timed out after 500ms waiting for signal 'inst-1/root/wait'"
        );
    }

    #[test]
    fn wait_on_wait_variables_match_generated_input_shape() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input"
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"value":"in"}"#,
            br#"{"tenant":"t1","_signal_id":"old","_instance_id":"old"}"#,
            b"{}",
        )
        .expect("source");

        let variables = manifest
            .wait_on_wait_variables("wait", "inst-1", "inst-1/root/wait", &source)
            .expect("on-wait variables");
        let variables: Value = serde_json::from_slice(&variables).expect("variables json");

        assert_eq!(variables["tenant"], json!("t1"));
        assert_eq!(variables["_signal_id"], json!("inst-1/root/wait"));
        assert_eq!(variables["_instance_id"], json!("inst-1"));
    }

    #[test]
    fn wait_on_wait_error_wraps_nested_failure_like_generated_code() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input"
        })))
        .expect("manifest");
        let nested = br#"{"stepId":"fail","code":"ON_WAIT_FAILED"}"#;

        let error = manifest
            .wait_on_wait_error("wait", nested)
            .expect("on-wait error");
        let error = String::from_utf8(error).expect("utf8 error");

        assert_eq!(
            error,
            "WaitForSignal step 'wait' on_wait failed: {\"stepId\":\"fail\",\"code\":\"ON_WAIT_FAILED\"}"
        );
    }

    #[test]
    fn wait_event_evaluates_action_metadata_and_schema() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input",
            "responseSchema": {
                "approved": { "type": "boolean", "required": true }
            },
            "action": {
                "key": "case_review_decision",
                "correlation": {
                    "case_id": {
                        "valueType": "reference",
                        "value": "data.case_id"
                    }
                },
                "context": {
                    "summary": {
                        "valueType": "reference",
                        "value": "data.summary"
                    }
                }
            }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"case_id":"case-42","summary":"Needs approval"}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let event = manifest
            .wait_event("wait", "inst-1/root/wait", &source)
            .expect("wait event");
        let event: Value = serde_json::from_slice(&event).expect("event json");

        assert_eq!(event["type"], json!("external_input_requested"));
        assert_eq!(event["signal_id"], json!("inst-1/root/wait"));
        assert_eq!(event["step_id"], json!("wait"));
        assert_eq!(event["step_name"], json!("Review Input"));
        assert_eq!(event["action_key"], json!("case_review_decision"));
        assert_eq!(event["correlation"], json!({ "case_id": "case-42" }));
        assert_eq!(event["context"], json!({ "summary": "Needs approval" }));
        assert_eq!(
            event["response_schema"]["approved"]["type"],
            json!("boolean")
        );
    }

    #[test]
    fn wait_output_stores_generated_step_shape_and_preserves_existing_steps() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input"
        })))
        .expect("manifest");
        let source = build_source(
            br#"{}"#,
            b"{}",
            br#"{"before":{"stepId":"before","outputs":1}}"#,
        )
        .expect("source");

        let steps = manifest
            .wait_output("wait", "inst-1/root/wait", br#"{"approved":true}"#, &source)
            .expect("wait output");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["before"]["outputs"], json!(1));
        assert_eq!(steps["wait"]["stepId"], json!("wait"));
        assert_eq!(steps["wait"]["stepName"], json!("Review Input"));
        assert_eq!(steps["wait"]["stepType"], json!("WaitForSignal"));
        assert_eq!(steps["wait"]["signal_id"], json!("inst-1/root/wait"));
        assert_eq!(steps["wait"]["outputs"], json!({ "approved": true }));
    }

    #[test]
    fn wait_debug_start_and_end_match_generated_shape() {
        let manifest = DirectJsonManifest::parse(&wait_manifest(json!({
            "id": "wait",
            "stepType": "WaitForSignal",
            "name": "Review Input",
            "pollIntervalMs": 250,
            "responseSchema": {
                "approved": { "type": "boolean", "required": true }
            }
        })))
        .expect("manifest");
        let source = build_source(br#"{"case_id":"case-42"}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .wait_debug_start("wait", "inst-1/root/wait", Some(500), &source)
            .expect("wait debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");

        assert_eq!(start["step_id"], json!("wait"));
        assert_eq!(start["step_name"], json!("Review Input"));
        assert_eq!(start["step_type"], json!("WaitForSignal"));
        assert_eq!(start["inputs"]["signal_id"], json!("inst-1/root/wait"));
        assert_eq!(start["inputs"]["timeout_ms"], json!(500));
        assert_eq!(start["inputs"]["poll_interval_ms"], json!(250));
        assert_eq!(
            start["inputs"]["response_schema"]["approved"]["type"],
            json!("boolean")
        );

        let steps = manifest
            .wait_output("wait", "inst-1/root/wait", br#"{"approved":true}"#, &source)
            .expect("wait output");
        let source_after_wait =
            build_source(br#"{"case_id":"case-42"}"#, b"{}", &steps).expect("source after wait");
        let end = manifest
            .step_debug_end("wait", &source_after_wait)
            .expect("wait debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");

        assert_eq!(
            end["outputs"],
            json!({
                "stepId": "wait",
                "stepName": "Review Input",
                "stepType": "WaitForSignal",
                "signal_id": "inst-1/root/wait",
                "outputs": { "approved": true }
            })
        );
        assert!(end["duration_ms"].as_i64().is_some_and(|value| value >= 0));
    }

    #[test]
    fn agent_output_stores_generated_code_compatible_step_envelope() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "value": { "valueType": "reference", "value": "data.value" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"value":"in"}"#, b"{}", b"{}").expect("source");

        let steps = manifest
            .agent_output(0, &source, br#"{"value":"out","ok":true}"#)
            .expect("Agent steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["agent"]["stepId"], json!("agent"));
        assert_eq!(steps["agent"]["stepName"], json!("Normalize Data"));
        assert_eq!(steps["agent"]["stepType"], json!("Agent"));
        assert_eq!(
            steps["agent"]["outputs"],
            json!({ "value": "out", "ok": true })
        );
    }

    #[test]
    fn ai_agent_output_builds_single_shot_envelope() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "value": { "valueType": "reference", "value": "data.value" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"value":"in"}"#, b"{}", b"{}").expect("source");

        // A chat-completion result: choice = serialized OneOrMany<AssistantContent>
        // (a JSON array; an untagged Text content is `{"text": ...}`).
        let output = json!({ "choice": [{ "text": "Hello!" }] });
        let output_bytes = serde_json::to_vec(&output).unwrap();

        let steps = manifest
            .ai_agent_output(0, &source, &output_bytes)
            .expect("AiAgent steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["agent"]["stepType"], json!("AiAgent"));
        assert_eq!(steps["agent"]["outputs"]["response"], json!("Hello!"));
        assert_eq!(steps["agent"]["outputs"]["iterations"], json!(1));
        assert_eq!(steps["agent"]["outputs"]["toolCalls"], json!([]));
    }

    #[test]
    fn ai_turn_loop_helpers_drive_a_tool_round() {
        // First turn: empty state + empty pending.
        let base = br#"{"system_prompt":"sys","user_prompt":"hi","provider":"openai","tools":[]}"#;
        let empty_turn = br#"{"chat_history":[],"iterations":0,"tool_call_log":[]}"#;
        let empty_pending = b"[]";
        let input =
            DirectJsonManifest::ai_turn_next_input(base, empty_turn, empty_pending).expect("input");
        let input: Value = serde_json::from_slice(&input).unwrap();
        assert_eq!(input["system_prompt"], json!("sys"));
        assert_eq!(input["iterations"], json!(0));
        assert_eq!(input["pending_tool_results"], json!([]));

        // A turn output requesting one tool call.
        let turn_out = serde_json::to_vec(&json!({
            "action": "tools",
            "chat_history": [{"text":"hi"}],
            "iterations": 1,
            "tool_call_log": [{"tool_name":"echo","arguments":{"v":1}}],
            "tool_calls": [{"tool_call_id":"call-1","name":"echo","arguments":{"v":1}}]
        }))
        .unwrap();
        assert!(!DirectJsonManifest::ai_turn_is_complete(&turn_out).unwrap());
        assert_eq!(
            DirectJsonManifest::ai_turn_tool_count(&turn_out).unwrap(),
            1
        );
        let args = DirectJsonManifest::ai_turn_tool_args(&turn_out, 0).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&args).unwrap(),
            json!({"v":1})
        );

        // Dispatch result → pending for the next turn.
        let pending =
            DirectJsonManifest::ai_turn_add_result(b"[]", &turn_out, 0, br#"{"ok":true}"#).unwrap();
        let pending: Value = serde_json::from_slice(&pending).unwrap();
        assert_eq!(
            pending,
            json!([{"tool_call_id":"call-1","content":{"ok":true}}])
        );

        // A completed turn → output envelope.
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");
        let done = serde_json::to_vec(&json!({
            "action": "complete",
            "chat_history": [],
            "iterations": 2,
            "tool_call_log": [{"tool_name":"echo"}],
            "response": "done"
        }))
        .unwrap();
        assert!(DirectJsonManifest::ai_turn_is_complete(&done).unwrap());
        let steps = manifest.ai_turn_output(0, &source, &done).expect("output");
        let steps: Value = serde_json::from_slice(&steps).unwrap();
        assert_eq!(steps["agent"]["stepType"], json!("AiAgent"));
        assert_eq!(steps["agent"]["outputs"]["response"], json!("done"));
        assert_eq!(steps["agent"]["outputs"]["iterations"], json!(2));
        assert_eq!(
            steps["agent"]["outputs"]["toolCalls"],
            json!([{"tool_name":"echo"}])
        );
    }

    #[test]
    fn ai_memory_helpers_round_trip_history() {
        // load-memory output → initial loop state seeds chat_history.
        let load = json!({ "success": true, "messages": [{"text":"prior"}], "message_count": 1 });
        let state =
            DirectJsonManifest::ai_memory_initial_state(&serde_json::to_vec(&load).unwrap())
                .expect("initial state");
        let state_value: Value = serde_json::from_slice(&state).unwrap();
        assert_eq!(state_value["chat_history"], json!([{"text":"prior"}]));
        assert_eq!(state_value["iterations"], json!(0));

        // final state + conversation → save-memory input.
        let conversation = json!({ "conversation_id": "c-1" });
        let final_state =
            json!({ "chat_history": [{"text":"prior"},{"text":"new"}], "iterations": 2 });
        let save = DirectJsonManifest::ai_memory_save_input(
            &serde_json::to_vec(&conversation).unwrap(),
            &serde_json::to_vec(&final_state).unwrap(),
        )
        .expect("save input");
        let save_value: Value = serde_json::from_slice(&save).unwrap();
        assert_eq!(save_value["conversation_id"], json!("c-1"));
        assert_eq!(
            save_value["messages"],
            json!([{"text":"prior"},{"text":"new"}])
        );
    }

    #[test]
    fn ai_memory_compact_sliding_drops_oldest_over_threshold() {
        let state = json!({
            "chat_history": [{"i":0},{"i":1},{"i":2},{"i":3},{"i":4}],
            "iterations": 5,
        });
        // Over threshold: keep only the most recent 2.
        let compacted =
            DirectJsonManifest::ai_memory_compact_sliding(&serde_json::to_vec(&state).unwrap(), 2)
                .expect("compact");
        let value: Value = serde_json::from_slice(&compacted).unwrap();
        assert_eq!(value["chat_history"], json!([{"i":3},{"i":4}]));
        // Untouched fields survive.
        assert_eq!(value["iterations"], json!(5));

        // At/under threshold: unchanged.
        let kept =
            DirectJsonManifest::ai_memory_compact_sliding(&serde_json::to_vec(&state).unwrap(), 5)
                .expect("compact");
        let kept_value: Value = serde_json::from_slice(&kept).unwrap();
        assert_eq!(kept_value["chat_history"], state["chat_history"]);
    }

    #[test]
    fn ai_agent_output_uses_structured_output_when_present() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "value": { "valueType": "reference", "value": "data.value" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"value":"in"}"#, b"{}", b"{}").expect("source");

        // With a structured output schema the capability returns the parsed JSON
        // under `structured_output`; the response becomes that object, not text.
        let output = json!({
            "choice": [{ "text": "{\"sentiment\":\"positive\"}" }],
            "structured_output": { "sentiment": "positive" }
        });
        let output_bytes = serde_json::to_vec(&output).unwrap();

        let steps = manifest
            .ai_agent_output(0, &source, &output_bytes)
            .expect("AiAgent steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(
            steps["agent"]["outputs"]["response"],
            json!({ "sentiment": "positive" })
        );
    }

    #[test]
    fn agent_validate_input_accepts_present_required_fields() {
        let manifest = DirectJsonManifest::parse(&agent_manifest_with_required_inputs(
            json!({}),
            json!([{
                "name": "value",
                "fieldType": "string",
                "description": "Value to normalize"
            }]),
        ))
        .expect("manifest");

        let validation = manifest
            .agent_validate_input(0, br#"{"value":"present"}"#)
            .expect("validation");

        assert!(validation.is_empty());
    }

    #[test]
    fn agent_validate_input_returns_generated_json_error() {
        let manifest = DirectJsonManifest::parse(&agent_manifest_with_required_inputs(
            json!({}),
            json!([
                {
                    "name": "value",
                    "fieldType": "string",
                    "description": "Value to normalize"
                },
                {
                    "name": "other",
                    "fieldType": "number"
                }
            ]),
        ))
        .expect("manifest");

        let validation = manifest
            .agent_validate_input(0, br#"{"value":null}"#)
            .expect("validation");
        let validation: Value = serde_json::from_slice(&validation).expect("validation json");

        assert_eq!(validation["code"], json!("STEP_REQUIRED_INPUT_NULL"));
        assert_eq!(validation["stepId"], json!("agent"));
        assert_eq!(validation["stepName"], json!("Normalize Data"));
        assert_eq!(validation["stepType"], json!("Agent"));
        assert_eq!(validation["agentId"], json!("utils"));
        assert_eq!(validation["capabilityId"], json!("normalize"));
        assert_eq!(validation["missingInputs"][0]["field"], json!("value"));
        assert_eq!(validation["missingInputs"][0]["reason"], json!("was null"));
        assert_eq!(
            validation["missingInputs"][1]["code"],
            json!("STEP_REQUIRED_INPUT_MISSING")
        );
    }

    #[test]
    fn agent_connection_input_matches_generated_injection_shape() {
        let manifest =
            DirectJsonManifest::parse(&agent_manifest_with_required_inputs_and_connection(
                json!({}),
                json!([]),
                Some("shopify-main"),
            ))
            .expect("manifest");

        let input = manifest
            .agent_connection_input(0, br#"{"value":"present"}"#)
            .expect("connection input");
        let input: Value = serde_json::from_slice(&input).expect("input json");

        assert_eq!(input["connection_id"], json!("shopify-main"));
        assert_eq!(
            input["_connection"],
            json!({
                "connection_id": "shopify-main",
                "integration_id": "",
                "parameters": {}
            })
        );
        assert_eq!(input["value"], json!("present"));
    }

    #[test]
    fn agent_cache_key_matches_generated_default_root_shape() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let source = build_source(br#"{"value":"in"}"#, b"{}", b"{}").expect("source");

        let key = manifest.agent_cache_key(0, &source).expect("cache key");

        assert_eq!(
            String::from_utf8(key).expect("utf8"),
            "root::agent::utils::normalize::agent"
        );
    }

    #[test]
    fn agent_cache_key_uses_workflow_id_and_loop_indices() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let source = build_source(
            br#"{"value":"in"}"#,
            br#"{"_workflow_id":"wf-42","_loop_indices":[0,2,"x"]}"#,
            b"{}",
        )
        .expect("source");

        let key = manifest.agent_cache_key(0, &source).expect("cache key");

        assert_eq!(
            String::from_utf8(key).expect("utf8"),
            "wf-42::agent::utils::normalize::agent::[0,2,\"x\"]"
        );
    }

    #[test]
    fn agent_cache_key_prefers_parent_cache_prefix() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let source = build_source(
            br#"{"value":"in"}"#,
            br#"{"_workflow_id":"wf-42","_cache_key_prefix":"parent::child","_loop_indices":[1]}"#,
            b"{}",
        )
        .expect("source");

        let key = manifest.agent_cache_key(0, &source).expect("cache key");

        assert_eq!(
            String::from_utf8(key).expect("utf8"),
            "parent::child::agent::utils::normalize::agent::[1]"
        );
    }

    #[test]
    fn agent_retry_sleep_key_matches_generated_retry_shape() {
        let key = DirectJsonManifest::agent_retry_sleep_key(
            "wf-42::agent::utils::normalize::agent::[1]",
            2,
        );

        assert_eq!(
            String::from_utf8(key).expect("utf8"),
            "wf-42::agent::utils::normalize::agent::[1]::retry_sleep::2"
        );
    }

    #[test]
    fn workflow_error_retry_info_matches_resilient_macro_classification() {
        let transient = br#"{"category":"transient","code":"TEMPORARY"}"#;
        assert!(DirectJsonManifest::workflow_error_retryable(transient));
        assert!(!DirectJsonManifest::workflow_error_rate_limited(transient));
        assert_eq!(
            DirectJsonManifest::workflow_error_retry_after_ms(transient),
            None
        );

        let permanent = br#"{"category":"permanent","code":"BAD_INPUT"}"#;
        assert!(!DirectJsonManifest::workflow_error_retryable(permanent));

        let rate_limited =
            br#"{"category":"transient","code":"HTTP_RATE_LIMITED","retryAfterMs":1500}"#;
        assert!(DirectJsonManifest::workflow_error_retryable(rate_limited));
        assert!(DirectJsonManifest::workflow_error_rate_limited(
            rate_limited
        ));
        assert_eq!(
            DirectJsonManifest::workflow_error_retry_after_ms(rate_limited),
            Some(1_500)
        );

        assert!(DirectJsonManifest::workflow_error_retryable(b"not-json"));
    }

    #[test]
    fn agent_retry_delay_matches_generated_backoff_shape() {
        assert_eq!(
            DirectJsonManifest::agent_retry_delay_ms(2, 4, 1_000, 60_000, None),
            1_000
        );
        assert_eq!(
            DirectJsonManifest::agent_retry_delay_ms(3, 4, 1_000, 60_000, None),
            2_000
        );
        assert_eq!(
            DirectJsonManifest::agent_retry_delay_ms(4, 4, 1_000, 60_000, None),
            4_000
        );
        assert_eq!(
            DirectJsonManifest::agent_retry_delay_ms(10, 4, 1_000, 3_000, None),
            3_000
        );
        assert_eq!(
            DirectJsonManifest::agent_retry_delay_ms(2, 4, 1_000, 60_000, Some(1_500)),
            1_500
        );
        assert_eq!(
            DirectJsonManifest::agent_retry_delay_ms(2, 4, 1_000, 1_000, Some(1_500)),
            1_000
        );
    }

    #[test]
    fn error_steps_inserts_structured_error_context() {
        let steps = error_steps(
            "agent",
            br#"{"code":"BAD","category":"permanent","message":"bad"}"#,
            br#"{"previous":{"outputs":{"ok":true}}}"#,
        )
        .expect("error steps");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["previous"]["outputs"]["ok"], json!(true));
        assert_eq!(steps["__error"]["code"], json!("BAD"));
        assert_eq!(steps["__error"]["category"], json!("permanent"));
        assert_eq!(steps["error"], steps["__error"]);
    }

    #[test]
    fn error_steps_matches_generated_fallback_error_context() {
        let steps = error_steps("agent", b"Step agent failed", b"{}").expect("error steps");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(steps["__error"]["message"], json!("Step agent failed"));
        assert_eq!(steps["__error"]["stepId"], json!("agent"));
        assert_eq!(steps["__error"]["code"], Value::Null);
        assert_eq!(steps["__error"]["category"], json!("unknown"));
        assert_eq!(steps["__error"]["severity"], json!("error"));
        assert_eq!(steps["error"], steps["__error"]);
    }

    #[test]
    fn agent_debug_payloads_use_mapping_and_stored_output() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "value": { "valueType": "reference", "value": "data.value" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"value":"in"}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("agent", &source)
            .expect("debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["inputs"], json!({ "value": "in" }));
        assert_eq!(
            start["input_mapping"],
            json!({ "value": { "valueType": "reference", "value": "data.value" } })
        );

        let steps = manifest
            .agent_output(0, &source, br#"{"value":"out"}"#)
            .expect("Agent steps context");
        let source = build_source(br#"{"value":"in"}"#, b"{}", &steps).expect("source");

        let end = manifest
            .step_debug_end("agent", &source)
            .expect("debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["stepId"], json!("agent"));
        assert_eq!(end["outputs"]["stepType"], json!("Agent"));
        assert_eq!(end["outputs"]["outputs"], json!({ "value": "out" }));
    }

    #[test]
    fn agent_error_formats_error_info_like_component_dispatch() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");

        let raw = DirectJsonManifest::agent_error_info(
            "CAPABILITY_ERROR",
            "bad request",
            "permanent",
            "error",
            false,
            Some(1500),
            Some(r#"{"field":"value"}"#),
        )
        .expect("Agent error-info");
        let raw: Value = serde_json::from_slice(&raw).expect("raw json");
        assert_eq!(raw["code"], json!("CAPABILITY_ERROR"));
        assert_eq!(raw["message"], json!("bad request"));
        assert_eq!(raw["category"], json!("permanent"));
        assert_eq!(raw["severity"], json!("error"));
        assert_eq!(raw["retryable"], json!(false));
        assert_eq!(raw["retryAfterMs"], json!(1500));
        assert_eq!(raw["attributes"], json!({ "field": "value" }));

        let error = manifest
            .agent_error(
                0,
                "CAPABILITY_ERROR",
                "bad request",
                "permanent",
                "error",
                false,
                Some(1500),
                Some(r#"{"field":"value"}"#),
            )
            .expect("Agent error");
        let error = String::from_utf8(error).expect("utf8 error");

        assert!(error.starts_with("Step agent failed: Agent utils::normalize: "));
        let raw = error
            .strip_prefix("Step agent failed: Agent utils::normalize: ")
            .expect("raw envelope");
        let raw: Value = serde_json::from_str(raw).expect("raw json");
        assert_eq!(raw["code"], json!("CAPABILITY_ERROR"));
        assert_eq!(raw["message"], json!("bad request"));
        assert_eq!(raw["category"], json!("permanent"));
        assert_eq!(raw["severity"], json!("error"));
        assert_eq!(raw["retryable"], json!(false));
        assert_eq!(raw["retryAfterMs"], json!(1500));
        assert_eq!(raw["attributes"], json!({ "field": "value" }));
    }

    #[test]
    fn agent_retry_error_info_classifies_rate_limited_codes() {
        let retry = DirectJsonManifest::agent_retry_error_info(
            "HTTP_RATE_LIMITED",
            "try later",
            "transient",
            "error",
            true,
            None,
            None,
        )
        .expect("Agent retry error-info");
        let raw: Value = serde_json::from_slice(&retry.payload).expect("raw json");

        assert!(retry.retryable);
        assert!(retry.rate_limited);
        assert_eq!(raw["code"], json!("HTTP_RATE_LIMITED"));
        assert_eq!(raw.get("retryAfterMs"), None);

        let permanent = DirectJsonManifest::agent_retry_error_info(
            "CAPABILITY_RATE_LIMITED",
            "bad config",
            "permanent",
            "error",
            true,
            Some(1500),
            None,
        )
        .expect("Agent retry error-info");
        assert!(!permanent.retryable);
        assert!(permanent.rate_limited);
    }

    #[test]
    fn agent_error_from_info_formats_preserved_retry_payload() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let payload = br#"{"code":"HTTP_RATE_LIMITED","message":"try later"}"#;

        let error = manifest
            .agent_error_from_info(0, payload)
            .expect("Agent error");
        let error = String::from_utf8(error).expect("utf8 error");

        assert_eq!(
            error,
            "Step agent failed: Agent utils::normalize: {\"code\":\"HTTP_RATE_LIMITED\",\"message\":\"try later\"}"
        );
    }

    #[test]
    fn agent_debug_error_payload_matches_generated_shape() {
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");
        manifest
            .step_debug_start("agent", &source)
            .expect("debug start");

        let payload = manifest
            .agent_debug_error(0, b"Step agent failed")
            .expect("debug error");
        let payload: Value = serde_json::from_slice(&payload).expect("payload json");

        assert_eq!(payload["step_id"], json!("agent"));
        assert_eq!(payload["step_type"], json!("Agent"));
        assert_eq!(
            payload["outputs"],
            json!({ "_error": true, "error": "Step agent failed" })
        );
        assert!(payload["duration_ms"].as_i64().is_some());
    }

    #[test]
    fn log_event_builds_payload_and_records_step_output() {
        let manifest = DirectJsonManifest::parse(&log_manifest(json!({
            "id": "log",
            "stepType": "Log",
            "name": "Log Start",
            "level": "warn",
            "message": "Starting workflow",
            "context": {
                "input": { "valueType": "reference", "value": "data.input" },
                "static": { "valueType": "immediate", "value": 42 }
            }
        })))
        .expect("manifest");
        let source = build_source(br#"{"input":"hello"}"#, b"{}", b"{}").expect("source");

        let payload = manifest.log_event(0, &source).expect("log payload");
        let payload: Value = serde_json::from_slice(&payload).expect("payload json");
        assert_eq!(payload["step_id"], json!("log"));
        assert_eq!(payload["step_name"], json!("Log Start"));
        assert_eq!(payload["level"], json!("warn"));
        assert_eq!(payload["message"], json!("Starting workflow"));
        assert_eq!(
            payload["context"],
            json!({ "input": "hello", "static": 42 })
        );
        assert!(
            payload["timestamp_ms"]
                .as_i64()
                .is_some_and(|value| value > 0)
        );

        let steps = manifest.log(0, &source).expect("steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        assert_eq!(steps["log"]["stepName"], json!("Log Start"));
        assert_eq!(steps["log"]["stepType"], json!("Log"));
        assert_eq!(
            steps["log"]["outputs"],
            json!({ "level": "warn", "message": "Starting workflow" })
        );
    }

    #[test]
    fn log_defaults_to_info_and_empty_context() {
        let manifest = DirectJsonManifest::parse(&log_manifest(json!({
            "id": "log",
            "stepType": "Log",
            "message": "Default level"
        })))
        .expect("manifest");
        let source = build_source(br#"{"input":"hello"}"#, b"{}", b"{}").expect("source");

        let payload = manifest.log_event(0, &source).expect("log payload");
        let payload: Value = serde_json::from_slice(&payload).expect("payload json");
        assert_eq!(payload["level"], json!("info"));
        assert_eq!(payload["context"], json!({}));
    }

    #[test]
    fn error_event_builds_payload_and_failure_message() {
        let manifest = DirectJsonManifest::parse(&error_manifest(json!({
            "id": "fail",
            "stepType": "Error",
            "name": "Fail Fast",
            "category": "transient",
            "code": "TEMPORARY_FAILURE",
            "message": "Try again later",
            "severity": "warning",
            "context": {
                "input": { "valueType": "reference", "value": "data.input" },
                "static": { "valueType": "immediate", "value": 42 }
            }
        })))
        .expect("manifest");
        let source = build_source(br#"{"input":"hello"}"#, b"{}", b"{}").expect("source");

        let payload = manifest.error_event(0, &source).expect("error payload");
        let payload: Value = serde_json::from_slice(&payload).expect("payload json");
        assert_eq!(payload["step_id"], json!("fail"));
        assert_eq!(payload["step_name"], json!("Fail Fast"));
        assert_eq!(payload["category"], json!("transient"));
        assert_eq!(payload["code"], json!("TEMPORARY_FAILURE"));
        assert_eq!(payload["message"], json!("Try again later"));
        assert_eq!(payload["severity"], json!("warning"));
        assert_eq!(
            payload["context"],
            json!({ "input": "hello", "static": 42 })
        );
        assert!(
            payload["timestamp_ms"]
                .as_i64()
                .is_some_and(|value| value > 0)
        );

        let failure = manifest.error(0, &source).expect("failure payload");
        let failure: Value = serde_json::from_slice(&failure).expect("failure json");
        assert_eq!(
            failure,
            json!({
                "stepId": "fail",
                "stepName": "Fail Fast",
                "category": "transient",
                "code": "TEMPORARY_FAILURE",
                "message": "Try again later",
                "severity": "warning",
                "context": { "input": "hello", "static": 42 }
            })
        );
    }

    #[test]
    fn error_defaults_to_permanent_error_and_empty_context() {
        let manifest = DirectJsonManifest::parse(&error_manifest(json!({
            "id": "fail",
            "stepType": "Error",
            "code": "DEFAULT_FAILURE",
            "message": "Default failure"
        })))
        .expect("manifest");
        let source = build_source(br#"{"input":"hello"}"#, b"{}", b"{}").expect("source");

        let payload = manifest.error_event(0, &source).expect("error payload");
        let payload: Value = serde_json::from_slice(&payload).expect("payload json");
        assert_eq!(payload["category"], json!("permanent"));
        assert_eq!(payload["severity"], json!("error"));
        assert_eq!(payload["context"], json!({}));

        let failure = manifest.error(0, &source).expect("failure payload");
        let failure: Value = serde_json::from_slice(&failure).expect("failure json");
        assert_eq!(failure["category"], json!("permanent"));
        assert_eq!(failure["severity"], json!("error"));
        assert_eq!(failure["context"], json!({}));
    }

    #[test]
    fn step_debug_finish_payloads_match_generated_shape() {
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "Finish",
            "finish",
            Some("Done"),
            json!({
                "mappings": [{
                    "id": 0,
                    "stepId": "finish",
                    "stepType": "Finish",
                    "purpose": "finish.inputMapping",
                    "value": {
                        "outputs": {
                            "valueType": "reference",
                            "value": "data.value"
                        }
                    }
                }]
            }),
        ))
        .expect("manifest");
        let source = build_source(br#"{"value":"ok"}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("finish", &source)
            .expect("debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["step_id"], json!("finish"));
        assert_eq!(start["step_name"], json!("Done"));
        assert_eq!(start["step_type"], json!("Finish"));
        assert_eq!(start["scope_id"], Value::Null);
        assert_eq!(start["parent_scope_id"], Value::Null);
        assert_eq!(start["loop_indices"], json!([]));
        assert_eq!(start["inputs"], json!({ "finishing": true }));
        assert_eq!(
            start["input_mapping"],
            json!({
                "outputs": {
                    "valueType": "reference",
                    "value": "data.value"
                }
            })
        );
        assert!(
            start["timestamp_ms"]
                .as_i64()
                .is_some_and(|value| value > 0)
        );

        let end = manifest
            .step_debug_end("finish", &source)
            .expect("debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["step_id"], json!("finish"));
        assert_eq!(
            end["outputs"],
            json!({
                "stepId": "finish",
                "stepName": "Done",
                "stepType": "Finish",
                "outputs": "ok"
            })
        );
        assert!(end["duration_ms"].as_i64().is_some_and(|value| value >= 0));
    }

    #[test]
    fn step_debug_conditional_payloads_include_result() {
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "Conditional",
            "check",
            None,
            json!({
                "conditions": [{
                    "id": 0,
                    "ownerId": "check",
                    "ownerType": "Conditional",
                    "purpose": "conditional.condition",
                    "value": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            { "valueType": "reference", "value": "data.status" },
                            { "valueType": "immediate", "value": "active" }
                        ]
                    }
                }]
            }),
        ))
        .expect("manifest");
        let source = build_source(br#"{"status":"active"}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("check", &source)
            .expect("debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["inputs"], json!({ "condition": "evaluating" }));
        assert_eq!(start["input_mapping"]["op"], json!("EQ"));

        let end = manifest
            .step_debug_end("check", &source)
            .expect("debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(
            end["outputs"],
            json!({
                "stepId": "check",
                "stepName": "Unnamed",
                "stepType": "Conditional",
                "outputs": { "result": true }
            })
        );
    }

    #[test]
    fn step_debug_switch_payloads_include_inputs_and_route() {
        let config = json!({
            "value": { "valueType": "reference", "value": "data.status" },
            "cases": [{
                "matchType": "EQ",
                "match": "active",
                "route": "active",
                "output": { "selected": { "valueType": "immediate", "value": "yes" } }
            }],
            "default": { "selected": { "valueType": "immediate", "value": "no" } }
        });
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "Switch",
            "switch",
            Some("Classify"),
            json!({
                "switches": [{
                    "id": 0,
                    "stepId": "switch",
                    "name": "Classify",
                    "stepType": "Switch",
                    "purpose": "switch.config",
                    "value": config
                }]
            }),
        ))
        .expect("manifest");
        let source = build_source(br#"{"status":"active"}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("switch", &source)
            .expect("debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["inputs"]["value"], json!("active"));
        assert_eq!(start["inputs"]["cases"][0]["route"], json!("active"));
        assert_eq!(start["input_mapping"]["cases"][0]["match"], json!("active"));

        let end = manifest
            .step_debug_end("switch", &source)
            .expect("debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(
            end["outputs"],
            json!({
                "stepId": "switch",
                "stepName": "Classify",
                "stepType": "Switch",
                "outputs": { "selected": "yes" },
                "route": "active"
            })
        );
    }
}
