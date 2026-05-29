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

impl DirectJsonManifest {
    /// Parse direct manifest JSON emitted by `runtara-workflows`.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: ManifestWire = serde_json::from_slice(bytes)
            .map_err(|err| format!("failed to parse direct manifest: {err}"))?;
        let mut collections = DirectJsonManifestCollections::default();
        collect_graph_manifest(&manifest.graph, &mut collections)?;
        Ok(Self {
            steps: collections.steps,
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

    /// Build the generated-code-compatible durable sleep key for an Agent retry.
    pub fn agent_retry_sleep_key(checkpoint_id: &str, attempt_number: u32) -> Vec<u8> {
        format!("{checkpoint_id}::retry_sleep::{attempt_number}").into_bytes()
    }

    /// Compute the generated-code-compatible delay for the next Agent retry.
    pub fn agent_retry_delay_ms(
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphWire {
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
