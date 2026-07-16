// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! JSON semantics used by direct-emitted workflow components.
//!
//! This module is the pure Rust implementation behind the
//! `runtara:workflow-stdlib/json` WIT contract. The component wrapper can keep
//! a parsed [`DirectJsonManifest`] after `init-manifest` and delegate the WIT
//! functions here.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::agent_input_validation::{
    AgentInputMissingReason, AgentInputValidationError, MissingAgentInput,
};
use crate::conditions::{is_truthy, to_number, values_equal};
use crate::switch_helpers::process_switch_output;
use crate::template::render_template;

// ===========================================================================
// Value interning ("scope handles").
//
// The direct runtime passes the whole scope as serialized JSON across every
// host-call boundary. A loop carrying a large value (a While/Split accumulator,
// an AiAgent conversation) therefore re-serializes and re-parses that value on
// every step of every iteration — O(N^2) work, and a single parse of a large
// value into a `serde_json::Value` (~5-10x the byte size) can alone exhaust the
// per-instance guest memory cap. The old generated-Rust path held scope as
// native structures passed by reference and did not have this cost.
//
// To restore that, a large value is interned once: its raw JSON bytes are kept
// host-side in a per-run arena and, where it sits in the scope, replaced with a
// small handle `{"$wfref": <id>}`. Carrying the scope between steps then moves
// only the handle; the bytes are parsed only when a path actually reads into the
// value (`lookup_source_path` resolves handles as it traverses), and a value is
// fully reconstituted only when it leaves the stdlib for an external consumer.
// Storing raw bytes (not a parsed Value) keeps the arena at ~1x the data.
//
// Safety property the rest of the code relies on: only values whose estimated
// size is at least `WFREF_THRESHOLD_BYTES` are interned, so anything smaller is
// byte-identical to before — the change is invisible to small-payload workflows.
// ===========================================================================

/// Sentinel object key marking an interned-value handle: `{"$wfref": <id>}`.
const WFREF_KEY: &str = "$wfref";

/// Values whose estimated serialized size is at least this many bytes are
/// interned and carried by handle instead of inline.
const WFREF_THRESHOLD_BYTES: usize = 16 * 1024;

thread_local! {
    /// Per-run arena of interned values, keyed by id, holding raw JSON bytes (not
    /// a parsed Value, so the footprint stays ~1x the data). Reset at
    /// `init-manifest`; the stdlib component is instantiated per workflow run, so
    /// this never outlives a single execution.
    static VALUE_STORE: RefCell<ValueStore> = RefCell::new(ValueStore::default());
}

#[derive(Default)]
struct ValueStore {
    entries: HashMap<u64, StoreEntry>,
    /// Content hash -> id. An identical value re-interned across iterations (e.g.
    /// a constant loop variable resolved fresh each pass) reuses one arena entry
    /// instead of growing the arena per iteration.
    content_index: HashMap<u64, u64>,
    next_id: u64,
}

struct StoreEntry {
    bytes: Rc<Vec<u8>>,
    /// Handle ids referenced inside this value's bytes — followed during
    /// mark-sweep so reachability is transitive (a value can carry handles).
    nested: Vec<u64>,
}

/// Clear the interning arena. Called at run start (`init-manifest`) so a reused
/// component instance never sees a previous run's handles.
pub fn reset_value_store() {
    VALUE_STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.entries.clear();
        store.content_index.clear();
        store.next_id = 0;
    });
}

/// Free every interned value not reachable from `roots`. Called at a loop
/// iteration boundary with the loop's live roots (the parent source plus the
/// surviving accumulator/state), so the previous iteration's superseded values
/// (old accumulator, per-iteration scratch) are reclaimed while everything still
/// referenced is kept. Reachability is transitive via each entry's nested ids,
/// so a handle that carries other handles is marked correctly. This is the GC
/// that bounds a growing-accumulator loop to ~one live copy.
pub fn value_store_retain(roots: &[&[u8]]) {
    // Collect the root handle ids first. If any root fails to parse we cannot
    // determine reachability, so free nothing rather than risk dropping a live
    // value — GC is an optimization; never let it corrupt state.
    let mut work: Vec<u64> = Vec::new();
    for root in roots {
        match serde_json::from_slice::<Value>(root) {
            Ok(value) => collect_handle_ids(&value, &mut work),
            Err(_) => return,
        }
    }
    VALUE_STORE.with(|store| {
        let mut store = store.borrow_mut();
        if store.entries.is_empty() {
            return;
        }
        let mut marked: std::collections::HashSet<u64> = std::collections::HashSet::new();
        while let Some(id) = work.pop() {
            if marked.insert(id)
                && let Some(entry) = store.entries.get(&id)
            {
                work.extend(entry.nested.iter().copied());
            }
        }
        store.entries.retain(|id, _| marked.contains(id));
        store.content_index.retain(|_, id| marked.contains(id));
    });
}

/// Collect every `{"$wfref": id}` handle id reachable in `value` (one level of
/// JSON structure — nested handles inside *stored* values are followed via the
/// entry's recorded nested ids during the sweep, not here).
fn collect_handle_ids(value: &Value, out: &mut Vec<u64>) {
    if let Some(id) = wfref_id(value) {
        out.push(id);
        return;
    }
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_handle_ids(item, out)),
        Value::Object(map) => map
            .values()
            .for_each(|value| collect_handle_ids(value, out)),
        _ => {}
    }
}

fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn make_wfref(id: u64) -> Value {
    let mut handle = Map::with_capacity(1);
    handle.insert(WFREF_KEY.to_string(), Value::from(id));
    Value::Object(handle)
}

/// The interned-value id, if `value` is a `{"$wfref": <id>}` handle.
fn wfref_id(value: &Value) -> Option<u64> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object.get(WFREF_KEY).and_then(Value::as_u64)
}

/// Cheap structural size estimate (no allocation) used only to decide whether to
/// intern; it approximates serialized length closely enough for a threshold.
fn estimate_json_size(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(_) => 5,
        Value::Number(_) => 8,
        Value::String(text) => text.len() + 2,
        Value::Array(items) => {
            2 + items
                .iter()
                .map(|item| estimate_json_size(item) + 1)
                .sum::<usize>()
        }
        Value::Object(map) => {
            2 + map
                .iter()
                .map(|(key, value)| key.len() + 4 + estimate_json_size(value))
                .sum::<usize>()
        }
    }
}

/// Intern `value` if it is large, returning a `{"$wfref": id}` handle; otherwise
/// return it unchanged. An existing handle is returned as-is so an unchanged
/// value keeps its id across steps and is never re-interned or copied.
fn intern_if_large(value: Value) -> Value {
    if wfref_id(&value).is_some() || estimate_json_size(&value) < WFREF_THRESHOLD_BYTES {
        return value;
    }
    let Ok(bytes) = serde_json::to_vec(&value) else {
        // Serialization can't realistically fail for a parsed Value; keep inline.
        return value;
    };
    let hash = content_hash(&bytes);
    let mut nested = Vec::new();
    collect_handle_ids(&value, &mut nested);
    let id = VALUE_STORE.with(|store| {
        let mut store = store.borrow_mut();
        // Reuse an identical existing entry (verifying bytes to rule out the
        // astronomically-unlikely hash collision) so constant re-interned values
        // don't grow the arena.
        if let Some(&existing) = store.content_index.get(&hash)
            && store
                .entries
                .get(&existing)
                .is_some_and(|stored| stored.bytes.as_slice() == bytes.as_slice())
        {
            return existing;
        }
        let id = store.next_id;
        store.next_id += 1;
        store.entries.insert(
            id,
            StoreEntry {
                bytes: Rc::new(bytes),
                nested,
            },
        );
        store.content_index.insert(hash, id);
        id
    });
    make_wfref(id)
}

/// Intern each large top-level entry of a scope container in place, leaving the
/// structural skeleton and small entries inline so common lookups never touch
/// the arena.
fn intern_scope_entries(map: &mut Map<String, Value>) {
    for value in map.values_mut() {
        let taken = std::mem::replace(value, Value::Null);
        *value = intern_if_large(taken);
    }
}

/// Resolve a `{"$wfref": id}` handle to its concrete value (parsing the stored
/// bytes); borrow non-handle values unchanged.
fn deref_handle(value: &Value) -> Cow<'_, Value> {
    if let Some(id) = wfref_id(value)
        && let Some(bytes) =
            VALUE_STORE.with(|store| store.borrow().entries.get(&id).map(|e| e.bytes.clone()))
        && let Ok(inner) = serde_json::from_slice::<Value>(&bytes)
    {
        return Cow::Owned(inner);
    }
    Cow::Borrowed(value)
}

/// Fully resolve every `{"$wfref": id}` handle in `value`. Used at boundaries
/// that serialize a value for an external consumer (checkpoint blob, cache key,
/// final output) where a handle must never leak; ordinary reads go through
/// `lookup_source_path`, which resolves handles as it traverses.
fn materialize(value: Value) -> Value {
    if let Some(id) = wfref_id(&value) {
        // Resolve from the arena. A missing id is a dangling handle (its value
        // was collected) — return Null rather than recursing on the handle, which
        // would loop forever. A correct GC never frees a still-referenced value,
        // so this is only a fail-safe.
        let bytes =
            VALUE_STORE.with(|store| store.borrow().entries.get(&id).map(|e| e.bytes.clone()));
        return match bytes.and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) {
            Some(inner) => materialize(inner),
            None => Value::Null,
        };
    }
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(materialize).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, materialize(value)))
                .collect(),
        ),
        other => other,
    }
}

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
    /// Lazily-populated cache of compiled conditions, keyed by a stable string
    /// (`c{id}` Conditional/edge, `w{id}` While, `f{id}` Filter). A condition is
    /// compiled once on first evaluation and reused across every element /
    /// iteration within the run — the manifest (and this cache) is discarded
    /// with the per-run component instance, so there is no cross-run staleness.
    compiled_conditions: RefCell<BTreeMap<String, Rc<CompiledCondition>>>,
    /// Lazily-populated cache of compiled input mappings, keyed by mapping id.
    /// Agent input mappings inside a Split body are evaluated per iteration, so
    /// compiling once per run rather than per iteration is a real win there.
    compiled_mappings: RefCell<BTreeMap<u32, Rc<CompiledInputMapping>>>,
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
            compiled_conditions: RefCell::new(BTreeMap::new()),
            compiled_mappings: RefCell::new(BTreeMap::new()),
        })
    }

    /// Get-or-compile a condition, caching it by a stable key so each condition
    /// is compiled once per run and reused across all evaluations.
    fn compiled_condition(&self, key: &str, raw: &Value) -> Rc<CompiledCondition> {
        if let Some(compiled) = self.compiled_conditions.borrow().get(key) {
            return Rc::clone(compiled);
        }
        let compiled = Rc::new(compile_condition(raw));
        self.compiled_conditions
            .borrow_mut()
            .insert(key.to_string(), Rc::clone(&compiled));
        compiled
    }

    /// Get-or-compile an input mapping, caching it by mapping id.
    fn compiled_mapping(&self, mapping_id: u32, raw: &Value) -> Rc<CompiledInputMapping> {
        if let Some(compiled) = self.compiled_mappings.borrow().get(&mapping_id) {
            return Rc::clone(compiled);
        }
        let compiled = Rc::new(compile_input_mapping(raw));
        self.compiled_mappings
            .borrow_mut()
            .insert(mapping_id, Rc::clone(&compiled));
        compiled
    }

    /// Apply a manifest mapping to a source JSON envelope.
    pub fn apply_mapping(&self, mapping_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse mapping source: {err}"))?;
        let mapping = self
            .mappings
            .get(&mapping_id)
            .ok_or_else(|| format!("unknown direct mapping id {mapping_id}"))?;
        let mut output = self
            .compiled_mapping(mapping_id, &mapping.value)
            .eval(&source)?;
        if mapping.purpose == "finish.inputMapping" {
            output = unwrap_finish_outputs(output);
        }
        if mapping.purpose == "agent.inputMapping" {
            resolve_nested_references(&mut output, &source);
            output = unwrap_top_level_immediate_envelopes(output);
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
        self.compiled_condition(&format!("c{condition_id}"), &condition.value)
            .eval(&source)
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
        self.compiled_condition(&format!("w{while_id}"), &while_step.condition)
            .eval(&source)
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
        let input = filter
            .value
            .get("value")
            .ok_or_else(|| "Filter config missing value".to_string())
            .and_then(|value| apply_mapping_value(value, &source))?;
        let condition_raw = filter
            .value
            .get("condition")
            .ok_or_else(|| "Filter config missing condition".to_string())?;
        let condition = self.compiled_condition(&format!("f{filter_id}"), condition_raw);
        let output = apply_filter_compiled(input, &condition, &source)?;
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
    /// The inherited checkpoint-namespace prefix, when running as a child
    /// (embedded or composed). Empty at the top level — every builder that
    /// folds this stays byte-identical for plain workflows.
    fn source_cache_key_prefix(source: &Value) -> Option<String> {
        source
            .get("variables")
            .and_then(Value::as_object)
            .and_then(|vars| vars.get("_cache_key_prefix"))
            .and_then(Value::as_str)
            .filter(|prefix| !prefix.is_empty())
            .map(str::to_string)
    }

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
        let base = if loop_indices.is_empty() {
            format!("breakpoint::{step_id}")
        } else {
            format!("breakpoint::{step_id}::{loop_indices}")
        };
        Ok(match Self::source_cache_key_prefix(&source) {
            Some(prefix) => format!("{prefix}::{base}"),
            None => base,
        })
    }

    /// Per-scope durability key for a durable Delay's sleep checkpoint.
    ///
    /// The bare step id at top level — byte-identical to the legacy static
    /// key, so existing checkpoint rows and assertions are unaffected — and
    /// `{step_id}::{indices}` inside Split/While iterations (folding
    /// `variables._loop_indices` exactly like [`Self::breakpoint_key`]).
    /// Without the fold, per-item durable delays collide on one key. A child
    /// scope's `_cache_key_prefix` (embedded or composed) prepends as
    /// `{prefix}::` so a durable child's delays never collide with the
    /// parent's — or with another invocation of the same child.
    pub fn delay_sleep_key(&self, step_id: &str, source: &[u8]) -> Result<String, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse delay-sleep-key source: {err}"))?;
        self.steps
            .get(step_id)
            .ok_or_else(|| format!("unknown direct delay step '{step_id}'"))?;

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
        let base = if loop_indices.is_empty() {
            step_id.to_string()
        } else {
            format!("{step_id}::{loop_indices}")
        };
        Ok(match Self::source_cache_key_prefix(&source) {
            Some(prefix) => format!("{prefix}::{base}"),
            None => base,
        })
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
            "Agent" | "AiAgent" => {
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

    /// Build the per-call signal id for a WaitForSignal step used as an AiAgent
    /// tool, matching the generated tool arm's
    /// `{instance}/{workflow}/{step}.tool.{label}.{call}{indices}`.
    pub fn ai_wait_tool_signal_id(
        &self,
        step_id: &str,
        instance_id: &str,
        label: &str,
        call_counter: u32,
        source: &[u8],
    ) -> Result<String, String> {
        // `step_id` here is the AiAgent step id (the path component, matching the
        // generated tool arm), not a WaitForSignal step — so it is NOT validated
        // against the wait-step registry.
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-wait-tool-signal-id source: {err}"))?;
        let workflow_id = source
            .get("variables")
            .and_then(Value::as_object)
            .and_then(|vars| vars.get("_workflow_id"))
            .and_then(Value::as_str)
            .unwrap_or("root");
        let indices_suffix = wait_loop_indices_suffix(&source);
        Ok(format!(
            "{instance_id}/{workflow_id}/{step_id}.tool.{label}.{call_counter}{indices_suffix}"
        ))
    }

    /// Wrap a received WaitForSignal-tool signal payload as the tool result the
    /// model sees: `{ "status": "received", "human_response": <payload> }`,
    /// matching the generated tool arm.
    pub fn ai_wait_tool_result(&self, signal_payload: &[u8]) -> Result<Vec<u8>, String> {
        let payload: Value = serde_json::from_slice(signal_payload)
            .map_err(|err| format!("failed to parse ai-wait-tool-result payload: {err}"))?;
        let wrapped = serde_json::json!({
            "status": "received",
            "human_response": payload,
        });
        serde_json::to_vec(&wrapped)
            .map_err(|err| format!("failed to serialize ai-wait-tool-result: {err}"))
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

    /// Structured WAIT_TIMEOUT envelope for onError routing (GAP-14). The
    /// plain-string `wait_timeout_error` stays the /failed payload for parity
    /// with the generated path; routed handlers need a structured envelope so
    /// `steps.__error.code` / `.category` references resolve.
    pub fn wait_timeout_error_envelope(
        &self,
        step_id: &str,
        signal_id: &str,
        timeout_ms: u64,
    ) -> Result<Vec<u8>, String> {
        self.wait_step(step_id)?;
        serde_json::to_vec(&serde_json::json!({
            "code": "WAIT_TIMEOUT",
            "message": format!(
                "WaitForSignal step '{step_id}' timed out after {timeout_ms}ms waiting for signal '{signal_id}'"
            ),
            "category": "timeout",
            "severity": "error",
        }))
        .map_err(|err| format!("failed to serialize wait-timeout envelope: {err}"))
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
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse wait-debug-start source: {err}"))?;
        let step = self.wait_step(step_id)?;
        let timestamp = timestamp_ms();
        self.debug_start_ms
            .borrow_mut()
            .insert(step_id.to_string(), timestamp);

        let mut payload = debug_event_base(step, &source, timestamp);
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
    /// Per-turn durability: the checkpoint key for one AiAgent loop turn —
    /// `{step_id}.turn.{iteration}`, scoped by `variables._loop_indices` like
    /// the breakpoint and agent cache keys, so Split/While-nested loops get
    /// distinct keys per iteration scope.
    pub fn ai_turn_cache_key(
        step_id: &str,
        iteration: u32,
        source: &[u8],
    ) -> Result<String, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-turn-cache-key source: {err}"))?;
        let indices_suffix = wait_loop_indices_suffix(&source);
        let base = format!("{step_id}.turn.{iteration}{indices_suffix}");
        Ok(match Self::source_cache_key_prefix(&source) {
            Some(prefix) => format!("{prefix}::{base}"),
            None => base,
        })
    }

    /// Per-turn durability: wrap the post-turn loop state for the turn
    /// checkpoint. `state`/`pending` are the loop's JSON payloads; the
    /// monotonic tool-call counter must survive replay because
    /// WaitForSignal-tool signal ids embed it.
    pub fn ai_turn_snapshot(
        state: &[u8],
        pending: &[u8],
        tool_calls: u32,
        complete: bool,
    ) -> Result<Vec<u8>, String> {
        let state: Value = serde_json::from_slice(state)
            .map_err(|err| format!("failed to parse ai-turn snapshot state: {err}"))?;
        let pending: Value = serde_json::from_slice(pending)
            .map_err(|err| format!("failed to parse ai-turn snapshot pending: {err}"))?;
        serde_json::to_vec(&serde_json::json!({
            "state": state,
            "pending": pending,
            "toolCalls": tool_calls,
            "complete": complete,
        }))
        .map_err(|err| format!("failed to serialize ai-turn snapshot: {err}"))
    }

    /// Per-turn durability: unpack a snapshot field (0 = state, 1 = pending).
    pub fn ai_turn_snapshot_part(snapshot: &[u8], part: u32) -> Result<Vec<u8>, String> {
        let snapshot: Value = serde_json::from_slice(snapshot)
            .map_err(|err| format!("failed to parse ai-turn snapshot: {err}"))?;
        let value = match part {
            0 => snapshot.get("state").cloned().unwrap_or(Value::Null),
            1 => snapshot
                .get("pending")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
            other => return Err(format!("unknown ai-turn snapshot part {other}")),
        };
        serde_json::to_vec(&value)
            .map_err(|err| format!("failed to serialize ai-turn snapshot part: {err}"))
    }

    /// Per-turn durability: the snapshot's monotonic tool-call counter.
    pub fn ai_turn_snapshot_tool_calls(snapshot: &[u8]) -> Result<u32, String> {
        let snapshot: Value = serde_json::from_slice(snapshot)
            .map_err(|err| format!("failed to parse ai-turn snapshot: {err}"))?;
        Ok(snapshot
            .get("toolCalls")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32)
    }

    /// Per-turn durability: whether the snapshotted turn completed the loop.
    pub fn ai_turn_snapshot_complete(snapshot: &[u8]) -> Result<bool, String> {
        let snapshot: Value = serde_json::from_slice(snapshot)
            .map_err(|err| format!("failed to parse ai-turn snapshot: {err}"))?;
        Ok(snapshot
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

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

    /// Merge a `timeout_ms` field into a tool call's argument object so the
    /// dispatched tool capability applies the configured per-call timeout. The
    /// arguments are produced by the model, so this is where the emitter injects
    /// the tool's own Agent-step timeout. A non-object args value (unusual) is
    /// returned unchanged.
    pub fn ai_tool_args_with_timeout(args: &[u8], timeout_ms: u64) -> Result<Vec<u8>, String> {
        let mut args: Value = serde_json::from_slice(args)
            .map_err(|err| format!("failed to parse ai tool args: {err}"))?;
        if let Value::Object(map) = &mut args {
            map.insert("timeout_ms".to_string(), Value::from(timeout_ms));
        }
        serde_json::to_vec(&args)
            .map_err(|err| format!("failed to serialize ai tool args with timeout: {err}"))
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

    /// Build the `summarize-memory` capability input from the base chat-turn
    /// config (for the LLM provider/model), the final loop state, and the
    /// compaction threshold: `{provider, model, max_messages, state}`. The
    /// capability decides internally whether the conversation is over the
    /// threshold (mirroring the generated Summarize branch's guard).
    pub fn ai_summarize_input(
        base: &[u8],
        state: &[u8],
        max_messages: u32,
    ) -> Result<Vec<u8>, String> {
        let base: Value = serde_json::from_slice(base)
            .map_err(|err| format!("failed to parse ai-summarize base: {err}"))?;
        let state: Value = serde_json::from_slice(state)
            .map_err(|err| format!("failed to parse ai-summarize state: {err}"))?;
        let mut input = serde_json::Map::new();
        input.insert(
            "provider".to_string(),
            base.get("provider").cloned().unwrap_or(Value::Null),
        );
        if let Some(model) = base.get("model").filter(|model| !model.is_null()) {
            input.insert("model".to_string(), model.clone());
        }
        input.insert("max_messages".to_string(), Value::from(max_messages));
        input.insert("state".to_string(), state);
        serde_json::to_vec(&Value::Object(input))
            .map_err(|err| format!("failed to serialize ai-summarize input: {err}"))
    }

    /// Extract the compacted loop state from a `summarize-memory` result
    /// (`{state}`). Carries the conversation forward into the memory save.
    pub fn ai_summarize_output(result: &[u8]) -> Result<Vec<u8>, String> {
        let result: Value = serde_json::from_slice(result)
            .map_err(|err| format!("failed to parse summarize-memory output: {err}"))?;
        let state = result
            .get("state")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        serde_json::to_vec(&state)
            .map_err(|err| format!("failed to serialize ai-summarize state: {err}"))
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

    /// Build a `step_debug_start` payload for one dispatched AiAgent tool call.
    ///
    /// Mirrors the generated loop's synthetic tool-call step: id
    /// `{ai_step}.tool.{name}.{call_number}`, name `Tool: {name}`, type
    /// `AiAgentToolCall`, inputs `{tool_name, arguments, iteration,
    /// call_number}`. `call_counter` is the loop's 0-based monotonic counter;
    /// the visible call number is 1-based.
    #[allow(clippy::too_many_arguments)]
    pub fn ai_tool_debug_start(
        &self,
        agent_id: u32,
        turn_out: &[u8],
        index: u32,
        iteration: u32,
        call_counter: u32,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-tool-debug source: {err}"))?;
        let (step, tool_name, arguments) =
            self.ai_tool_call_step(agent_id, turn_out, index, call_counter)?;
        let timestamp = timestamp_ms();
        self.debug_start_ms
            .borrow_mut()
            .insert(step.id.clone(), timestamp);

        let mut payload = debug_event_base(&step, &source, timestamp);
        payload.insert(
            "inputs".to_string(),
            serde_json::json!({
                "tool_name": tool_name,
                "arguments": arguments,
                "iteration": iteration,
                "call_number": call_counter + 1,
            }),
        );
        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize ai-tool debug-start payload: {err}"))
    }

    /// Build the matching `step_debug_end` payload for a dispatched AiAgent
    /// tool call, with the result wrapped in the legacy output envelope
    /// `{tool_name, result, iteration, call_number}`.
    #[allow(clippy::too_many_arguments)]
    pub fn ai_tool_debug_end(
        &self,
        agent_id: u32,
        turn_out: &[u8],
        index: u32,
        iteration: u32,
        call_counter: u32,
        tool_result: &[u8],
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-tool-debug source: {err}"))?;
        let (step, tool_name, _arguments) =
            self.ai_tool_call_step(agent_id, turn_out, index, call_counter)?;
        let result: Value = serde_json::from_slice(tool_result)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(tool_result).into_owned()));
        let timestamp = timestamp_ms();
        let duration_ms = self
            .debug_start_ms
            .borrow_mut()
            .remove(&step.id)
            .map(|start| timestamp.saturating_sub(start).max(0))
            .unwrap_or(0);

        let mut payload = debug_event_base(&step, &source, timestamp);
        let outputs = step_output_envelope(
            &step,
            serde_json::json!({
                "tool_name": tool_name,
                "result": result,
                "iteration": iteration,
                "call_number": call_counter + 1,
            }),
            None,
        );
        payload.insert("outputs".to_string(), outputs);
        payload.insert(
            "duration_ms".to_string(),
            Value::Number(serde_json::Number::from(duration_ms)),
        );
        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize ai-tool debug-end payload: {err}"))
    }

    /// Build a `step_debug_start` payload for an AiAgent conversation-memory
    /// phase (load/save/compaction), mirroring the generated compiler's
    /// synthetic memory steps. Phases: 0 = load, 1 = save, 2 = sliding-window
    /// compaction, 3 = summarize compaction. The compaction phases return an
    /// empty payload when the history is at/below `max_messages` — the caller
    /// skips the event, matching the generated "only when exceeded" gate.
    pub fn ai_memory_debug_start(
        &self,
        agent_id: u32,
        phase: u32,
        conversation: &[u8],
        state: &[u8],
        max_messages: u32,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-memory-debug source: {err}"))?;
        let phase = AiMemoryDebugPhase::from_code(phase)?;
        let step = self.ai_memory_step(agent_id, phase)?;
        let conversation_id = ai_memory_conversation_id(conversation)?;
        let history = ai_memory_chat_history(state)?;

        let inputs = match phase {
            AiMemoryDebugPhase::Load => serde_json::json!({
                "conversation_id": conversation_id,
            }),
            AiMemoryDebugPhase::Save => serde_json::json!({
                "conversation_id": conversation_id,
                "message_count": history.len(),
            }),
            AiMemoryDebugPhase::CompactSliding | AiMemoryDebugPhase::CompactSummarize => {
                if history.len() <= max_messages as usize {
                    return Ok(Vec::new());
                }
                let excess = history.len() - max_messages as usize;
                let excess_key = match phase {
                    AiMemoryDebugPhase::CompactSliding => "messages_to_drop",
                    _ => "messages_to_compact",
                };
                let mut inputs = serde_json::json!({
                    "strategy": phase.strategy(),
                    "messages_before": history.len(),
                    "max_messages": max_messages,
                    "conversation_id": conversation_id,
                });
                if let Some(map) = inputs.as_object_mut() {
                    map.insert(excess_key.to_string(), Value::from(excess));
                }
                inputs
            }
        };

        let timestamp = timestamp_ms();
        self.debug_start_ms
            .borrow_mut()
            .insert(step.id.clone(), timestamp);
        let mut payload = debug_event_base(&step, &source, timestamp);
        payload.insert("inputs".to_string(), inputs);
        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize ai-memory debug-start payload: {err}"))
    }

    /// Build the matching `step_debug_end` payload for an AiAgent memory
    /// phase. `state` is the post-phase loop state; `prior_state` is the
    /// pre-compaction state (an empty object for load/save).
    #[allow(clippy::too_many_arguments)]
    pub fn ai_memory_debug_end(
        &self,
        agent_id: u32,
        phase: u32,
        conversation: &[u8],
        state: &[u8],
        prior_state: &[u8],
        max_messages: u32,
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse ai-memory-debug source: {err}"))?;
        let phase = AiMemoryDebugPhase::from_code(phase)?;
        let step = self.ai_memory_step(agent_id, phase)?;
        let conversation_id = ai_memory_conversation_id(conversation)?;
        let history = ai_memory_chat_history(state)?;

        let outputs = match phase {
            // Load/save end events carry raw outputs (not the step output
            // envelope), with truncated message previews — like the generated
            // loop. Failures take the agent-error branch and fail the step, so
            // an end event always reports success.
            AiMemoryDebugPhase::Load | AiMemoryDebugPhase::Save => {
                let previews: Vec<Value> = history.iter().map(ai_memory_message_preview).collect();
                serde_json::json!({
                    "success": true,
                    "conversation_id": conversation_id,
                    "message_count": history.len(),
                    "messages": previews,
                })
            }
            AiMemoryDebugPhase::CompactSliding | AiMemoryDebugPhase::CompactSummarize => {
                let before = ai_memory_chat_history(prior_state)?;
                if before.len() <= max_messages as usize {
                    return Ok(Vec::new());
                }
                let excess = before.len() - max_messages as usize;
                let details = match phase {
                    AiMemoryDebugPhase::CompactSliding => serde_json::json!({
                        "strategy": phase.strategy(),
                        "success": true,
                        "messages_before": before.len(),
                        "messages_after": history.len(),
                        "messages_dropped": excess,
                    }),
                    _ => serde_json::json!({
                        "strategy": phase.strategy(),
                        "success": true,
                        "messages_before": before.len(),
                        "messages_after": history.len(),
                        "messages_compacted": excess,
                        "summary": ai_memory_compaction_summary(&history),
                    }),
                };
                step_output_envelope(&step, details, None)
            }
        };

        let timestamp = timestamp_ms();
        let duration_ms = self
            .debug_start_ms
            .borrow_mut()
            .remove(&step.id)
            .map(|start| timestamp.saturating_sub(start).max(0))
            .unwrap_or(0);
        let mut payload = debug_event_base(&step, &source, timestamp);
        payload.insert("outputs".to_string(), outputs);
        payload.insert(
            "duration_ms".to_string(),
            Value::Number(serde_json::Number::from(duration_ms)),
        );
        serde_json::to_vec(&Value::Object(payload))
            .map_err(|err| format!("failed to serialize ai-memory debug-end payload: {err}"))
    }

    /// The synthetic step identity for an AiAgent memory phase.
    fn ai_memory_step(
        &self,
        agent_id: u32,
        phase: AiMemoryDebugPhase,
    ) -> Result<DirectJsonStep, String> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        Ok(DirectJsonStep {
            id: format!("{}{}", agent.step_id, phase.id_suffix()),
            step_type: phase.step_type().to_string(),
            name: Some(phase.step_name().to_string()),
            body: Value::Null,
        })
    }

    /// The synthetic step identity for the `index`-th tool call of a turn,
    /// plus the call's tool name and arguments.
    fn ai_tool_call_step(
        &self,
        agent_id: u32,
        turn_out: &[u8],
        index: u32,
        call_counter: u32,
    ) -> Result<(DirectJsonStep, String, Value), String> {
        let turn_out: Value = serde_json::from_slice(turn_out)
            .map_err(|err| format!("failed to parse ai-turn output: {err}"))?;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let call = turn_out
            .get("tool_calls")
            .and_then(Value::as_array)
            .and_then(|calls| calls.get(index as usize));
        let tool_name = call
            .and_then(|call| call.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let arguments = call
            .and_then(|call| call.get("arguments"))
            .cloned()
            .unwrap_or(Value::Null);
        let step = DirectJsonStep {
            id: format!("{}.tool.{}.{}", agent.step_id, tool_name, call_counter + 1),
            step_type: "AiAgentToolCall".to_string(),
            name: Some(format!("Tool: {tool_name}")),
            body: Value::Null,
        };
        Ok((step, tool_name, arguments))
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
    /// Inject the Agent's connection into its input as `_connection` (plus a
    /// top-level `connection_id`), evaluated against the execution `source`.
    ///
    /// This is the SINGLE connection channel: agents read `input._connection`
    /// directly (there is no out-of-band `connection` WIT argument anymore).
    /// The id is resolved by [`Self::resolve_connection_id`] — a resolvable
    /// `connection_ref` wins over the literal `connection_id`; an empty result
    /// (no connection, or a ref that resolves to null/absent) leaves the input
    /// untouched. `integration_id`/`parameters` stay empty: a connection is an
    /// opaque id, and the proxy resolves credentials by `(id, tenant)`
    /// server-side, so nothing secret ever rides the input.
    pub fn agent_connection_input(
        &self,
        agent_id: u32,
        input: &[u8],
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let mut input: Value = serde_json::from_slice(input)
            .map_err(|err| format!("failed to parse Agent input for connection: {err}"))?;

        let resolved = self.resolve_connection_id(agent_id, source)?;
        if !resolved.is_empty()
            && let Value::Object(ref mut map) = input
        {
            let connection_id = String::from_utf8(resolved)
                .map_err(|err| format!("resolved connection id is not valid UTF-8: {err}"))?;
            map.insert(
                "connection_id".to_string(),
                Value::String(connection_id.clone()),
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

    /// Wrap a composed workflow-agent child's input in the canonical
    /// `{data, variables}` envelope carrying the invocation-site checkpoint
    /// namespace.
    ///
    /// A durable workflow-agent child shares the PARENT instance's checkpoint
    /// store; its checkpoint ids were baked at its own compile time from its
    /// own step ids, so without a per-site scope they collide across
    /// invocations (Split over one child, the same child twice) and with the
    /// parent's own steps. The scope is [`child_cache_prefix`] — the exact
    /// compositional formula the EmbedWorkflow path injects — which the
    /// child's `build_source` whitelists back into its variables, prefixing
    /// every durable key it builds. Replay-stable by construction: step id is
    /// compile-time, `_loop_indices` are deterministic, the inherited parent
    /// prefix recurses (nested composition chains like nested embeds).
    ///
    /// The emitter calls this ONLY for workflow-agent targets, once per step
    /// (before the retry loop — the wrapped buffer feeds every attempt).
    /// Native agents never see an envelope-shaped input.
    pub fn agent_scope_input(
        &self,
        agent_id: u32,
        input: &[u8],
        source: &[u8],
    ) -> Result<Vec<u8>, String> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;
        let input: Value = serde_json::from_slice(input)
            .map_err(|err| format!("failed to parse Agent input for scoping: {err}"))?;
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse source for Agent scoping: {err}"))?;

        let envelope = serde_json::json!({
            "data": input,
            "variables": { "_cache_key_prefix": child_cache_prefix(&agent.step_id, &source) }
        });
        serde_json::to_vec(&envelope)
            .map_err(|err| format!("failed to serialize scoped Agent input: {err}"))
    }

    /// Resolve an Agent's connection to ONE concrete connection id, evaluated
    /// against the execution `source`.
    ///
    /// A resolvable `connection_ref` (a `MappingValue`) wins over the literal
    /// `connection_id`: it lets a step bind to a caller-supplied `connection`
    /// input, rotate connections, or select one per record. Returns the id as
    /// UTF-8 bytes, or an EMPTY vec when the agent has no connection or the ref
    /// resolves to null/absent. Used by [`Self::agent_connection_input`] to
    /// inject `input._connection` — uniformly for every agent kind (primary,
    /// memory, MCP-tool), whose input at runtime need not be a mapping.
    pub fn resolve_connection_id(&self, agent_id: u32, source: &[u8]) -> Result<Vec<u8>, String> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| format!("unknown direct Agent id {agent_id}"))?;

        if let Some(connection_ref) = agent.connection_ref.as_ref() {
            let source: Value = serde_json::from_slice(source).map_err(|err| {
                format!("failed to parse source for connection resolution: {err}")
            })?;
            let resolved = apply_mapping_value(connection_ref, &source)?;
            return Ok(match resolved {
                Value::String(id) => id.into_bytes(),
                // A ref that resolves to null/absent (e.g. an optional input the
                // caller did not supply) yields no connection.
                Value::Null => Vec::new(),
                other => {
                    return Err(format!(
                        "connection_ref for Agent {agent_id} must resolve to a string id, got {other}"
                    ));
                }
            });
        }

        Ok(match agent.connection_id.as_deref() {
            Some(id) if !id.is_empty() => id.as_bytes().to_vec(),
            _ => Vec::new(),
        })
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

    /// Durable per-attempt invoke-result key for an Agent retry. A distinct
    /// namespace ("::attempt::") from the durable-sleep key ("::retry_sleep::")
    /// and the write-only audit key ("::retry::") so the per-attempt result
    /// checkpoint never collides with either.
    pub fn agent_attempt_result_key(checkpoint_id: &str, attempt_number: u32) -> Vec<u8> {
        format!("{checkpoint_id}::attempt::{attempt_number}").into_bytes()
    }

    /// Encode a per-attempt invoke-result envelope. Fixed 12-byte header
    /// followed by the raw error-info payload:
    ///
    /// | offset | field           | type      |
    /// |--------|-----------------|-----------|
    /// | 0      | tag             | u8        |
    /// | 1      | retryable       | u8 (bool) |
    /// | 2      | rate_limited    | u8 (bool) |
    /// | 3      | retry_after_tag | u8 (bool) |
    /// | 4..12  | retry_after_ms  | u64 le    |
    /// | 12..   | payload         | bytes     |
    ///
    /// The emitter decodes the header by fixed offset from the checkpoint bytes;
    /// see `emit_agent_attempt_decode` in the direct core emitter.
    pub fn agent_attempt_envelope(
        tag: u8,
        retryable: bool,
        rate_limited: bool,
        retry_after_tag: bool,
        retry_after_ms: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut envelope = Vec::with_capacity(12 + payload.len());
        envelope.push(tag);
        envelope.push(retryable as u8);
        envelope.push(rate_limited as u8);
        envelope.push(retry_after_tag as u8);
        envelope.extend_from_slice(&retry_after_ms.to_le_bytes());
        envelope.extend_from_slice(payload);
        envelope
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
    pub fn agent_debug_error(
        &self,
        agent_id: u32,
        source: &[u8],
        error: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse agent-debug-error source: {err}"))?;
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

        let mut payload = debug_event_base(step, &source, timestamp);
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

    /// Build a generic `step_debug_end` payload for a failed step of any type.
    ///
    /// The [`agent_debug_error`](Self::agent_debug_error) analogue keyed by step
    /// id rather than agent id, so non-Agent steps (Finish, Conditional, Filter,
    /// While, …) can attribute an unhandled input-resolution failure to
    /// themselves with the same `{ outputs: { _error, error } }` shape the step
    /// summary recognizes.
    pub fn step_debug_error(
        &self,
        step_id: &str,
        source: &[u8],
        error: &[u8],
    ) -> Result<Vec<u8>, String> {
        let source: Value = serde_json::from_slice(source)
            .map_err(|err| format!("failed to parse step-debug-error source: {err}"))?;
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
        let error = String::from_utf8_lossy(error).to_string();

        let mut payload = debug_event_base(step, &source, timestamp);
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
            .map_err(|err| format!("failed to serialize step-debug-end error payload: {err}"))
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

        let mut payload = debug_event_base(step, &source, timestamp);
        let (inputs, input_mapping) = self.debug_start_data(step, &source)?;
        // Bound the resolved inputs: steps that operate over collections
        // (Filter/GroupBy/Agent/EmbedWorkflow/...) would otherwise persist the
        // entire input value verbatim on every iteration — multi-MB per event,
        // GB-scale per instance for a loop body. `bounded_debug_value` was only
        // wired into Split/While; apply it here so every step type is bounded.
        payload.insert("inputs".to_string(), bounded_debug_value(inputs));
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

        let mut payload = debug_event_base(step, &source, timestamp);
        payload.insert(
            "outputs".to_string(),
            bound_debug_output(self.debug_end_output(step, &source)?),
        );
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
                // Tolerate an unresolvable input (see the Agent arm): the failing
                // step's start must still emit so the resolution failure can be
                // attributed to it; show null inputs, the error rides the end.
                let input = filter
                    .value
                    .get("value")
                    .and_then(|value| apply_mapping_value(value, source).ok())
                    .unwrap_or(Value::Null);
                Ok((input, filter.value.get("condition").cloned()))
            }
            "Switch" => {
                let switch = self
                    .switch_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Switch config for '{}'", step.id))?;
                Ok((
                    switch_debug_inputs(&switch.value, source).unwrap_or(Value::Null),
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
                    .and_then(|value| apply_mapping_value(value, source).ok())
                    .unwrap_or(Value::Null);
                Ok((input, None))
            }
            "Delay" => {
                let delay = self
                    .delay_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Delay config for '{}'", step.id))?;
                // Tolerate an unresolvable duration (see the Agent arm) so the
                // start emits and the resolution failure is attributed to the step.
                let duration_ms =
                    apply_mapping_value(&delay.duration_ms, source).unwrap_or(Value::Null);
                Ok((
                    serde_json::json!({ "duration_ms": duration_ms }),
                    Some(delay.duration_ms.clone()),
                ))
            }
            "Agent" | "AiAgent" => {
                let agent = self
                    .agent_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Agent config for '{}'", step.id))?;
                let mapping = self.mappings.get(&agent.input_mapping_id).ok_or_else(|| {
                    format!(
                        "missing direct Agent input mapping {} for '{}'",
                        agent.input_mapping_id, step.id
                    )
                })?;
                // Tolerate an input-mapping that fails to resolve (e.g. a template
                // render error). A step-debug-start is diagnostic, and the emitter
                // fires one on the input-resolution failure path so the failing
                // step appears in the step summary with its error; if resolution
                // errored here too the start would abort and the step would stay
                // invisible. Show empty inputs — the paired error step-debug-end
                // carries the actual failure.
                let inputs = apply_input_mapping(&mapping.value, source)
                    .unwrap_or_else(|_| Value::Object(Map::new()));
                Ok((
                    inputs,
                    (!mapping.value.as_object().is_some_and(Map::is_empty))
                        .then(|| mapping.value.clone()),
                ))
            }
            "EmbedWorkflow" => {
                let mapping = self.embed_workflow_mapping(step.id.as_str());
                let inputs = mapping
                    .and_then(|mapping| apply_input_mapping(&mapping.value, source).ok())
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
                // Tolerate an unresolvable log payload (see the Agent arm): a Log
                // emits no start normally, but the emitter fires one on the
                // resolution-failure path to attribute the failure to the step.
                let context = apply_log(&log.value, source)
                    .map(|details| details.context)
                    .unwrap_or(Value::Null);
                Ok((context, None))
            }
            "Split" => {
                let split = self
                    .split_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Split config for '{}'", step.id))?;
                let inputs = split_debug_inputs(split, source).unwrap_or(Value::Null);
                Ok((inputs, None))
            }
            "While" => {
                let while_step = self
                    .while_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct While config for '{}'", step.id))?;
                let inputs = while_debug_inputs(while_step)?;
                Ok((inputs, None))
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
                let output = unwrap_finish_outputs(apply_input_mapping(&mapping.value, source)?);
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
            // Filter/Switch/GroupBy persist their output to `steps.<id>` during
            // execution (via `insert_step_output`), and the emitter now rebuilds
            // source before the debug-end event — so read that stored envelope
            // instead of recomputing the step. Recomputing made these steps run a
            // second time under track-events (~doubling cost for collection-heavy
            // steps like a Filter over a large array). The recompute stays as a
            // fallback for paths where the stored output isn't in scope (e.g. a
            // pre-execution breakpoint).
            "Filter" => {
                if let Some(stored) = source
                    .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                    .cloned()
                {
                    return Ok(stored);
                }
                let filter = self
                    .filter_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Filter config for '{}'", step.id))?;
                Ok(step_output_envelope(
                    step,
                    apply_filter(&filter.value, source)?,
                    None,
                ))
            }
            "Switch" => {
                if let Some(stored) = source
                    .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                    .cloned()
                {
                    return Ok(stored);
                }
                let switch = self
                    .switch_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct Switch config for '{}'", step.id))?;
                let result = apply_switch(&switch.value, source)?;
                let route = switch_is_routing(&switch.value).then_some(result.route.as_str());
                Ok(step_output_envelope(step, result.output, route))
            }
            "GroupBy" => {
                if let Some(stored) = source
                    .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                    .cloned()
                {
                    return Ok(stored);
                }
                let group_by = self
                    .group_by_by_step(step.id.as_str())
                    .ok_or_else(|| format!("missing direct GroupBy config for '{}'", step.id))?;
                Ok(step_output_envelope(
                    step,
                    apply_group_by(&group_by.value, source)?,
                    None,
                ))
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
            "Agent" | "AiAgent" => source
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
            "Split" => source
                .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                .cloned()
                .ok_or_else(|| format!("missing direct Split output for '{}'", step.id)),
            "While" => source
                .pointer(&format!("/steps/{}", escape_json_pointer_token(&step.id)))
                .cloned()
                .ok_or_else(|| format!("missing direct While output for '{}'", step.id)),
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

/// Inject the synthetic runtime-identity variables `_instance_id` and
/// `_tenant_id` that the generated compiler exposes on every `variables`
/// snapshot (codegen `emit_main`), read from the same env vars generated uses.
/// Only filled in when ABSENT so a child/iteration scope that already inherited
/// them is never clobbered.
///
/// `_workflow_id` is intentionally NOT injected here: it is already baked into
/// the manifest variables segment at compile time (`direct_core_variables_json`)
/// and is the agent cache-key prefix, so synthesizing it from env would alter
/// cache keys. `_instance_id`/`_tenant_id` do not participate in cache keys.
fn inject_runtime_identity_variables(variables: &mut Value) {
    let Some(obj) = variables.as_object_mut() else {
        return;
    };
    obj.entry("_instance_id".to_string()).or_insert_with(|| {
        Value::String(
            std::env::var("RUNTARA_INSTANCE_ID").unwrap_or_else(|_| "unknown".to_string()),
        )
    });
    obj.entry("_tenant_id".to_string()).or_insert_with(|| {
        Value::String(std::env::var("TENANT_ID").unwrap_or_else(|_| "unknown".to_string()))
    });
}

/// Build the source envelope consumed by direct mapping/condition helpers.
pub fn build_source(data: &[u8], variables: &[u8], steps: &[u8]) -> Result<Vec<u8>, String> {
    let mut data: Value =
        serde_json::from_slice(data).map_err(|err| format!("failed to parse data: {err}"))?;
    let mut variables: Value = serde_json::from_slice(variables)
        .map_err(|err| format!("failed to parse variables: {err}"))?;
    // The workflow start input arrives in the canonical envelope
    // `{"data": {...}, "variables": {...}}` (enforced at the API boundary) and is
    // stored verbatim as the instance input. Unwrap it so `data.*` resolves
    // against the inner payload, and merge the runtime `variables` over the
    // compile-time declared defaults (runtime wins). The `_`-prefixed identity
    // variables are never overridable from input — with ONE whitelisted
    // exception: `_cache_key_prefix`, the checkpoint-namespace a PARENT
    // injects when invoking this workflow as a composed agent (see
    // `child_cache_prefix`). It is a namespace hint, not identity — the worst
    // a caller can do by setting it is namespace its own child's durable
    // state, which is exactly the feature. Inputs with no `data` key
    // (low-level / direct runtime invocations) are used as-is.
    let inner_data = if let Value::Object(envelope) = &mut data {
        if envelope.contains_key("data") {
            if let Some(Value::Object(runtime_vars)) = envelope.remove("variables")
                && let Value::Object(defaults) = &mut variables
            {
                for (key, value) in runtime_vars {
                    if !key.starts_with('_') || key == "_cache_key_prefix" {
                        defaults.insert(key, value);
                    }
                }
            }
            envelope.remove("data")
        } else {
            None
        }
    } else {
        None
    };
    if let Some(inner) = inner_data {
        data = inner;
    }
    inject_runtime_identity_variables(&mut variables);
    let mut steps: Value =
        serde_json::from_slice(steps).map_err(|err| format!("failed to parse steps: {err}"))?;

    // Intern large scope values: replace each big `data`/`variables`/`steps`
    // entry with a `$wfref` handle so the serialized source — and every buffer
    // that carries it between steps and iterations — stays small. References
    // resolve handles transparently in `lookup_source_path`; only an actual read
    // of a big value materializes it. Incoming handles are preserved (an
    // unchanged value keeps its id and is never re-copied). This is the core of
    // bounding loop memory to ~one copy of the live data.
    if let Value::Object(map) = &mut data {
        intern_scope_entries(map);
    }
    if let Value::Object(map) = &mut variables {
        intern_scope_entries(map);
    }
    if let Value::Object(map) = &mut steps {
        intern_scope_entries(map);
    }

    // onError handlers inject the captured error envelope at `steps.__error`
    // (alias `steps.error`, see `error_steps`). Mirror it to the source root so
    // references using the historically-documented bare `__error.*` / `error.*`
    // form resolve in addition to the canonical `steps.__error.*`. Only present
    // during onError dispatch; absent (and thus a no-op) for normal steps.
    let error_alias = steps
        .as_object()
        .and_then(|map| map.get("__error"))
        .cloned();

    let mut source = Map::new();
    source.insert("data".to_string(), data.clone());
    source.insert("variables".to_string(), variables.clone());
    source.insert("steps".to_string(), steps);

    if let Some(error) = error_alias {
        source.insert("__error".to_string(), error.clone());
        source.insert("error".to_string(), error);
    }

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
    let error = parse_error_envelope(error, step_id);

    steps.insert("__error".to_string(), error.clone());
    steps.insert("error".to_string(), error);

    serde_json::to_vec(&Value::Object(steps))
        .map_err(|err| format!("failed to serialize error steps context: {err}"))
}

/// Recover a structured error envelope for the `onError` context.
///
/// Agent failures reach `error_steps` already wrapped by
/// [`DirectJsonManifest::agent_error`] as
/// `Step <id> failed: Agent <agent>::<cap>: {envelope-json}`, so a plain
/// `from_slice` of the whole string fails and the structured `code` /
/// `category` / `attributes` fields would be lost (handlers would only see the
/// wrapped text under `message`). Recover them by parsing the JSON envelope
/// embedded after the first `{` — mirroring how `why_execution_failed` unwraps
/// the same shape — so `steps.__error.code` etc. resolve. Falls back to a
/// synthesized envelope when there is no JSON to recover.
fn parse_error_envelope(error: &[u8], step_id: &str) -> Value {
    // Already a structured envelope (non-Agent failures, or a bare envelope).
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(error) {
        return ensure_step_id(Value::Object(map), step_id);
    }
    let text = String::from_utf8_lossy(error);
    // Agent failures: unwrap the trailing `{...}` envelope. The wrapping prefix
    // (`Step <id> failed: Agent <agent>::<cap>: `) never contains `{`, so the
    // first brace is the envelope's opening brace.
    if let Some(brace) = text.find('{')
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text[brace..].trim())
    {
        return ensure_step_id(Value::Object(map), step_id);
    }
    serde_json::json!({
        "message": text.to_string(),
        "stepId": step_id,
        "code": null,
        "category": "unknown",
        "severity": "error"
    })
}

/// Ensure the recovered envelope always exposes `stepId` for onError handlers.
fn ensure_step_id(mut value: Value, step_id: &str) -> Value {
    if let Value::Object(map) = &mut value
        && !map.contains_key("stepId")
    {
        map.insert("stepId".to_string(), Value::String(step_id.to_string()));
    }
    value
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
                    connection_ref: agent.connection_ref.clone(),
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

/// Cap a resolved value before it goes into a step-debug payload. A Split's
/// `value` (the whole list it fans out over) and its `variables` (large in-scope
/// references) can each be many MB; embedding them verbatim floods the event
/// stream and can blow the event HTTP body. Small values pass through unchanged;
/// large ones collapse to a compact summary that preserves type and size.
fn bounded_debug_value(value: Value) -> Value {
    const MAX_ITEMS: usize = 50;
    const MAX_BYTES: usize = 8 * 1024;
    // Cheap fast paths avoid serializing a huge array/string just to measure it.
    match &value {
        Value::Array(items) if items.len() > MAX_ITEMS => {
            return serde_json::json!({
                "_truncated": true,
                "_type": "array",
                "_length": items.len(),
            });
        }
        Value::String(text) if text.len() > MAX_BYTES => {
            return serde_json::json!({
                "_truncated": true,
                "_type": "string",
                "_length": text.len(),
            });
        }
        _ => {}
    }
    match serde_json::to_vec(&value) {
        Ok(bytes) if bytes.len() > MAX_BYTES => match value {
            Value::Object(map) => serde_json::json!({
                "_truncated": true,
                "_type": "object",
                "_keys": map.keys().cloned().collect::<Vec<_>>(),
                "_bytes": bytes.len(),
            }),
            Value::Array(items) => serde_json::json!({
                "_truncated": true,
                "_type": "array",
                "_length": items.len(),
                "_bytes": bytes.len(),
            }),
            other => other,
        },
        _ => value,
    }
}

/// Bound a step-debug-end `outputs` value. The step-output envelope keeps the
/// real payload under an `"outputs"` key alongside `stepId`/`stepName`/
/// `stepType`/`route` and the `_error` flag the step-summary status query reads;
/// bound only the heavy nested payload so the envelope and `_error` survive.
/// Non-envelope values (e.g. the Error step's bare `{_error,..}`) are bounded
/// whole — they are small, so `bounded_debug_value` leaves them unchanged.
fn bound_debug_output(value: Value) -> Value {
    match value {
        Value::Object(mut map) if map.contains_key("outputs") => {
            if let Some(inner) = map.remove("outputs") {
                map.insert("outputs".to_string(), bounded_debug_value(inner));
            }
            Value::Object(map)
        }
        other => bounded_debug_value(other),
    }
}

fn split_debug_inputs(split: &DirectJsonSplit, source: &Value) -> Result<Value, String> {
    let value_mapping = split
        .value
        .get("value")
        .ok_or_else(|| format!("Split step '{}' config missing value", split.step_id))?;
    let mut inputs = Map::new();
    inputs.insert(
        "value".to_string(),
        bounded_debug_value(apply_mapping_value(value_mapping, source)?),
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
            bounded_debug_value(apply_input_mapping(extra_variables_mapping, source)?),
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

    let parent_scope_id = variables.get("_scope_id").cloned().unwrap_or(Value::Null);
    let scope_id = parent_scope_id
        .as_str()
        .map(|parent| format!("{}_{}_{}", parent, split.step_id, index))
        .unwrap_or_else(|| format!("sc_{}_{}", split.step_id, index));
    variables.insert("_parent_scope_id".to_string(), parent_scope_id);
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
        "hasFailures": !error.is_empty(),
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

    let parent_scope_id = variables.get("_scope_id").cloned().unwrap_or(Value::Null);
    let scope_id = parent_scope_id
        .as_str()
        .map(|parent| format!("{}_{}_{}", parent, while_step.step_id, state.index))
        .unwrap_or_else(|| format!("sc_{}_{}", while_step.step_id, state.index));
    variables.insert("_parent_scope_id".to_string(), parent_scope_id);
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
    let condition = config
        .get("condition")
        .ok_or_else(|| "Filter config missing condition".to_string())?;
    apply_filter_compiled(input, &compile_condition(condition), source)
}

/// Filter a pre-resolved `input` array with a pre-compiled condition. The hot
/// path: `manifest.filter` passes the run-cached compiled condition so the
/// condition is compiled once per run, not re-walked/re-parsed per element.
fn apply_filter_compiled(
    input: Value,
    condition: &CompiledCondition,
    source: &Value,
) -> Result<Value, String> {
    // Consume the resolved input array (no clone) and reuse one scope object
    // across elements, MOVING each item into `scope["item"]` rather than cloning
    // it — the condition only borrows `item.*`, so a per-element clone of the
    // whole element (potentially a large record) is pure waste. Matched items
    // are moved back out via `Value::take`.
    let items = match input {
        Value::Array(items) => items,
        _ => Vec::new(),
    };
    let mut scope = source.clone();
    if !scope.is_object() {
        return Err("filter source must be a JSON object".to_string());
    }

    let mut filtered = Vec::new();
    for item in items {
        scope
            .as_object_mut()
            .expect("filter source was checked as object")
            .insert("item".to_string(), item);
        if condition.eval(&scope)? {
            let matched = scope
                .as_object_mut()
                .and_then(|obj| obj.get_mut("item"))
                .map(Value::take)
                .unwrap_or(Value::Null);
            filtered.push(matched);
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
            // Desugar (BETWEEN/RANGE/array-EQ) then evaluate via the compiled
            // path. Switch is a per-step (cold) site, so the condition is
            // compiled per call rather than cached — cheap, and keeps all
            // condition evaluation on one evaluator.
            let condition = switch_case_condition(switch_value, case)?;
            if compile_condition(&condition).eval(source)? {
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

/// Structured fields for the invoke export's `Err(error-info)` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectInvokeErrorFields {
    pub code: String,
    pub message: String,
    pub category: String,
    pub severity: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub attributes: Option<String>,
}

/// Best-effort decomposition of a terminal error payload into structured
/// error-info fields. A JSON envelope (the `agent_error_info_envelope` /
/// stdlib error-step shape: `{code, message, category, severity, retryable,
/// retryAfterMs, attributes}`) maps field-for-field; anything else — plain
/// strings, non-object JSON, invalid UTF-8 — rides `message` verbatim
/// (lossily decoded), matching what `runtime.fail` records. Infallible by
/// construction.
pub fn invoke_error_fields(error: &[u8]) -> DirectInvokeErrorFields {
    let raw = String::from_utf8_lossy(error).into_owned();
    let Ok(Value::Object(envelope)) = serde_json::from_slice::<Value>(error) else {
        return DirectInvokeErrorFields {
            code: String::new(),
            message: raw,
            category: String::new(),
            severity: String::new(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        };
    };
    let field = |name: &str| {
        envelope
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let message = match envelope.get("message").and_then(Value::as_str) {
        Some(message) => message.to_string(),
        // An object without a message string still surfaces everything.
        None => raw,
    };
    DirectInvokeErrorFields {
        code: field("code"),
        message,
        category: field("category"),
        severity: field("severity"),
        retryable: envelope
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        retry_after_ms: envelope.get("retryAfterMs").and_then(Value::as_u64),
        // Agent envelopes carry `attributes`; workflow Error-step envelopes
        // carry the resolved `context` — both surface as the attributes JSON.
        attributes: envelope
            .get("attributes")
            .or_else(|| envelope.get("context"))
            .filter(|value| !value.is_null())
            .map(|value| value.to_string()),
    }
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

fn debug_event_base(
    step: &DirectJsonStep,
    source: &Value,
    timestamp_ms: i64,
) -> Map<String, Value> {
    // Scope/loop context lives in the runtime variables: each Split/While
    // iteration sets a distinct `_scope_id` (plus `_parent_scope_id` /
    // `_loop_indices`). Surface them so step-debug events from parallel
    // iterations are distinguishable — otherwise the paired-summary query joins
    // start/end by (step_id, scope_id) alone and cross-products every
    // iteration's events (9 phantom rows + negative durations for 3 iterations).
    let variables = source.get("variables").and_then(Value::as_object);
    let scope_id = variables
        .and_then(|vars| vars.get("_scope_id"))
        .cloned()
        .unwrap_or(Value::Null);
    let parent_scope_id = variables
        .and_then(|vars| vars.get("_parent_scope_id"))
        .cloned()
        .unwrap_or(Value::Null);
    let loop_indices = variables
        .and_then(|vars| vars.get("_loop_indices"))
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

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
    payload.insert("scope_id".to_string(), scope_id);
    payload.insert("parent_scope_id".to_string(), parent_scope_id);
    payload.insert("loop_indices".to_string(), loop_indices);
    payload.insert(
        "timestamp_ms".to_string(),
        Value::Number(serde_json::Number::from(timestamp_ms)),
    );
    payload
}

/// An AiAgent conversation-memory phase whose debug events surface as a
/// synthetic step, mirroring the generated compiler (which used the
/// `.memory_load`/`.memory_save`/`.memory.compact` id suffixes verbatim).
#[derive(Clone, Copy, PartialEq)]
enum AiMemoryDebugPhase {
    Load,
    Save,
    CompactSliding,
    CompactSummarize,
}

impl AiMemoryDebugPhase {
    fn from_code(code: u32) -> Result<Self, String> {
        match code {
            0 => Ok(Self::Load),
            1 => Ok(Self::Save),
            2 => Ok(Self::CompactSliding),
            3 => Ok(Self::CompactSummarize),
            other => Err(format!("unknown ai-memory debug phase {other}")),
        }
    }

    fn id_suffix(self) -> &'static str {
        match self {
            Self::Load => ".memory_load",
            Self::Save => ".memory_save",
            Self::CompactSliding | Self::CompactSummarize => ".memory.compact",
        }
    }

    fn step_name(self) -> &'static str {
        match self {
            Self::Load => "Memory: Load",
            Self::Save => "Memory: Save",
            Self::CompactSliding => "Memory: Sliding Window",
            Self::CompactSummarize => "Memory: Summarize",
        }
    }

    fn step_type(self) -> &'static str {
        match self {
            Self::Load => "AiAgentMemoryLoad",
            Self::Save => "AiAgentMemorySave",
            Self::CompactSliding | Self::CompactSummarize => "AiAgentMemoryCompaction",
        }
    }

    fn strategy(self) -> &'static str {
        match self {
            Self::CompactSliding => "sliding_window",
            Self::CompactSummarize => "summarize",
            _ => "",
        }
    }
}

/// The resolved conversation object's id (string, or null when absent).
fn ai_memory_conversation_id(conversation: &[u8]) -> Result<Value, String> {
    let conversation: Value = serde_json::from_slice(conversation)
        .map_err(|err| format!("failed to parse ai-memory conversation: {err}"))?;
    Ok(conversation
        .get("conversation_id")
        .cloned()
        .unwrap_or(Value::Null))
}

/// The loop state's chat history (empty when absent).
fn ai_memory_chat_history(state: &[u8]) -> Result<Vec<Value>, String> {
    let state: Value = serde_json::from_slice(state)
        .map_err(|err| format!("failed to parse ai-memory state: {err}"))?;
    Ok(state
        .get("chat_history")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// A `{role, preview}` entry for one serialized chat message, with the
/// preview truncated to ~200 chars — the generated loop's debug-event shape.
fn ai_memory_message_preview(message: &Value) -> Value {
    let role = message.get("role").and_then(Value::as_str).unwrap_or("?");
    let parts: Vec<String> = match message.get("content") {
        Some(Value::Array(items)) => items.iter().filter_map(ai_memory_content_preview).collect(),
        Some(single) => ai_memory_content_preview(single).into_iter().collect(),
        None => Vec::new(),
    };
    let preview = parts.join(" ");
    let truncated = if preview.chars().count() > 200 {
        let cut: String = preview.chars().take(200).collect();
        format!("{cut}...")
    } else {
        preview
    };
    serde_json::json!({ "role": role, "preview": truncated })
}

/// Preview one content part: text verbatim, tool results/calls as markers.
fn ai_memory_content_preview(part: &Value) -> Option<String> {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    if part.get("type").and_then(Value::as_str) == Some("tool_result") {
        let id = part.get("id").and_then(Value::as_str).unwrap_or("?");
        return Some(format!("[tool_result:{id}]"));
    }
    if let Some(name) = part
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
    {
        return Some(format!("[tool_call:{name}]"));
    }
    None
}

/// Extract the summary text the summarize-memory capability prepended as the
/// first history message (`[Previous conversation summary]: …`).
fn ai_memory_compaction_summary(history: &[Value]) -> Value {
    const MARKER: &str = "[Previous conversation summary]: ";
    history
        .first()
        .and_then(|message| match message.get("content") {
            Some(Value::Array(items)) => items
                .iter()
                .find_map(|item| item.get("text").and_then(Value::as_str)),
            Some(single) => single.get("text").and_then(Value::as_str),
            None => None,
        })
        .and_then(|text| text.strip_prefix(MARKER))
        .map(|summary| Value::String(summary.to_string()))
        .unwrap_or(Value::Null)
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

/// The checkpoint-namespace prefix for a CHILD invoked at `step_id` of the
/// current scope — the compositional site scope every durable key builder
/// honors via `variables._cache_key_prefix`:
/// `{inherited_prefix}__{step_id}[loop,indices]`, falling back to
/// `{workflow_id}::{step_id}[...]` at the root. Replay-stable by construction
/// (compile-time step id + deterministic loop indices + recursion). One
/// definition shared by EmbedWorkflow children (inlined; prefix rides the
/// in-process variables) and composed workflow-agent children (prefix rides
/// the child's input envelope) — so both child kinds are indistinguishable in
/// checkpoint key-space. See docs/workflow-agent-checkpoint-namespace-plan.md.
pub fn child_cache_prefix(step_id: &str, source: &Value) -> String {
    let parent_variables = source.get("variables").and_then(Value::as_object);
    let loop_indices_suffix = parent_variables
        .and_then(|vars| vars.get("_loop_indices"))
        .and_then(Value::as_array)
        .filter(|indices| !indices.is_empty())
        .map(|indices| {
            let indices = indices.iter().map(Value::to_string).collect::<Vec<_>>();
            format!("[{}]", indices.join(","))
        })
        .unwrap_or_default();
    match parent_variables
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

    variables.insert(
        "_cache_key_prefix".to_string(),
        Value::String(child_cache_prefix(step_id, source)),
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
        // Query-only operators: evaluated server-side inside object-model
        // query conditions, never by the workflow runtime. Validation rejects
        // them up front (E027); erroring here (instead of silently returning
        // false) covers workflows compiled before that validation existed.
        "SIMILARITY_GTE" | "MATCH" | "COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE" => Err(format!(
            "condition operator '{op}' is only valid inside object-model query conditions; \
             the workflow runtime cannot evaluate it"
        )),
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

/// Resolve a Finish step's mapped output to the workflow's return value.
///
/// A Finish whose mapping is exactly `{ "outputs": X }` returns `X` directly —
/// the common single-output convention, so `outputs: steps.prev.outputs` yields
/// `prev`'s value rather than `{ "outputs": <value> }`. A multi-field mapping
/// that merely includes an `outputs` key (e.g. a Split `dontStopOnFailed`
/// aggregation envelope `{ data, stats, outputs }`) is returned whole;
/// unwrapping it would silently drop the sibling `data`/`stats` fields.
fn unwrap_finish_outputs(output: Value) -> Value {
    match &output {
        Value::Object(map) if map.len() == 1 && map.contains_key("outputs") => {
            map.get("outputs").cloned().unwrap_or(output)
        }
        _ => output,
    }
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
            // minijinja cannot resolve `$wfref` interning handles: a template
            // reaching into a large, interned scope value (e.g.
            // `loop.outputs.next_page` once the accumulated outputs cross the
            // intern threshold) would otherwise see the bare handle object and
            // raise "undefined value". References go through `lookup_source_path`,
            // which derefs handles as it traverses; rendering a template is the
            // same kind of boundary, so hand minijinja a fully materialized,
            // handle-free view of the source. `materialize` is a no-op when
            // nothing was interned.
            render_template(template, &materialize(source.clone())).map(Value::String)
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
    let value = resolve_lookup(
        lookup_segments_detailed(source, &path_to_segments(path)),
        default.clone(),
    )?;
    coerce_reference_value(
        value,
        map.get("type").and_then(Value::as_str),
        default.as_ref(),
    )
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

/// Resolve `{valueType: "reference", value: "<path>"}` envelopes buried inside
/// agent input payloads — e.g. `ConditionExpression` arguments or score
/// expressions nested in an immediate `condition` value.
///
/// Two positions intentionally stay as references because their string value
/// names an Object Model column rather than a workflow path:
/// - argument 0 of field-based condition operators (`EQ`, `IN`, ...);
/// - unqualified references inside `fn` call arguments (e.g. `SIMILARITY`).
///
/// Resolved references are rewritten as `{valueType: "immediate", value: X}`
/// rather than the bare value: condition arguments are typed at the agent
/// boundary as untagged `MappingValue` shapes, so a bare scalar there fails
/// deserialization. [`unwrap_top_level_immediate_envelopes`] strips exactly
/// one wrapper per top-level field so primitive-typed agent inputs still see
/// their bare value.
fn resolve_nested_references(value: &mut Value, source: &Value) {
    match value {
        Value::Object(map) => {
            if is_reference_envelope(map) {
                let resolved = apply_reference(map, source).unwrap_or(Value::Null);
                let mut wrapped = Map::with_capacity(2);
                wrapped.insert("valueType".to_string(), Value::String("immediate".into()));
                wrapped.insert("value".to_string(), resolved);
                *value = Value::Object(wrapped);
                if let Value::Object(map) = value
                    && let Some(inner) = map.get_mut("value")
                {
                    resolve_nested_references(inner, source);
                }
                return;
            }

            let is_immediate_envelope = matches!(
                map.get("valueType"),
                Some(Value::String(s)) if s == "immediate"
            );
            if is_immediate_envelope {
                if let Some(inner) = map.get_mut("value") {
                    resolve_nested_references(inner, source);
                }
                return;
            }

            if map.get("fn").and_then(Value::as_str).is_some()
                && let Some(args) = map.get_mut("arguments").and_then(Value::as_array_mut)
            {
                for arg in args.iter_mut() {
                    if arg
                        .as_object()
                        .is_some_and(is_unqualified_reference_envelope)
                    {
                        continue;
                    }
                    resolve_nested_references(arg, source);
                }
                return;
            }

            let condition_op = map.get("op").and_then(Value::as_str).map(str::to_owned);
            if let Some(op) = condition_op.as_deref()
                && let Some(args) = map.get_mut("arguments").and_then(Value::as_array_mut)
            {
                for (index, arg) in args.iter_mut().enumerate() {
                    if index == 0
                        && is_field_argument_operator(op)
                        && arg.as_object().is_some_and(is_reference_envelope)
                    {
                        continue;
                    }
                    resolve_nested_references(arg, source);
                }
                return;
            }

            for child in map.values_mut() {
                resolve_nested_references(child, source);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                resolve_nested_references(item, source);
            }
        }
        _ => {}
    }
}

/// Strip a single `{valueType: "immediate", value: X}` envelope from each
/// top-level field. Pairs with [`resolve_nested_references`]: the resolver
/// wraps resolved references as immediates, so a top-level field that was a
/// reference nested directly inside an immediate would otherwise reach the
/// agent still wrapped. Nested wrappers deeper in the payload survive intact.
fn unwrap_top_level_immediate_envelopes(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        for child in map.values_mut() {
            let Value::Object(child_map) = child else {
                continue;
            };
            let is_immediate = matches!(
                child_map.get("valueType"),
                Some(Value::String(s)) if s == "immediate"
            );
            if !is_immediate {
                continue;
            }
            if let Some(inner) = child_map.remove("value") {
                *child = inner;
            }
        }
    }
    value
}

fn is_reference_envelope(map: &Map<String, Value>) -> bool {
    matches!(
        map.get("valueType"),
        Some(Value::String(s)) if s == "reference"
    ) && matches!(map.get("value"), Some(Value::String(_)))
}

fn is_unqualified_reference_envelope(map: &Map<String, Value>) -> bool {
    let Some(path) = map.get("value").and_then(Value::as_str) else {
        return false;
    };
    is_reference_envelope(map) && !is_qualified_workflow_path(path)
}

fn is_qualified_workflow_path(path: &str) -> bool {
    matches!(
        path.split('.').next(),
        Some("data" | "variables" | "workflow" | "steps" | "loop" | "item")
    )
}

fn is_field_argument_operator(op: &str) -> bool {
    matches!(
        op.to_ascii_uppercase().as_str(),
        "EQ" | "NE"
            | "GT"
            | "GTE"
            | "LT"
            | "LTE"
            | "STARTS_WITH"
            | "ENDS_WITH"
            | "CONTAINS"
            | "IN"
            | "NOT_IN"
            | "IS_DEFINED"
            | "IS_EMPTY"
            | "IS_NOT_EMPTY"
            | "SIMILARITY_GTE"
            | "MATCH"
            | "COSINE_DISTANCE_LTE"
            | "L2_DISTANCE_LTE"
    )
}

/// Coerce a resolved reference value to its declared `type` hint.
///
/// String and boolean hints are total — every JSON value has a representation.
/// The numeric hints are partial: a `null` passes through as `null` (an absent
/// optional stays absent), but a present value that cannot be parsed as the
/// requested type is a hard `Err` rather than a silent `0`, so malformed data
/// fails at the step that produced it instead of flowing onward as a plausible
/// zero.
///
/// Authors opt back into a fallback via the reference's `default`: see
/// [`coerce_reference_value`], which substitutes the `default` when coercion
/// fails.
///
/// Any other hint is a pass-through. The `type` key is overloaded in the raw
/// manifest — a condition value expression carries `type: "value"` as a wrapper
/// marker, not a `ValueType` — so the runtime cannot treat every unrecognized
/// `type` as an error. Unknown *`ValueType`* hints on real reference mappings
/// are instead rejected by the typed authoring layer (`ReferenceValue.type_hint`
/// is `Option<ValueType>`) before a manifest is ever compiled.
fn apply_type_hint(value: Value, type_hint: Option<&str>) -> Result<Value, String> {
    let coerced = match type_hint {
        Some("string") => match value {
            Value::String(_) | Value::Null => value,
            Value::Number(number) => Value::String(number.to_string()),
            Value::Bool(boolean) => Value::String(boolean.to_string()),
            other => Value::String(other.to_string()),
        },
        Some("integer") => {
            let parsed = value
                .as_i64()
                .or_else(|| value.as_f64().map(|value| value as i64))
                .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
                .or_else(|| value.as_bool().map(|value| if value { 1 } else { 0 }));
            match parsed {
                Some(parsed) => Value::Number(parsed.into()),
                None => return coercion_result(value, "integer"),
            }
        }
        Some("number") => {
            let parsed = value
                .as_f64()
                .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
                .and_then(serde_json::Number::from_f64);
            match parsed {
                Some(parsed) => Value::Number(parsed),
                None => return coercion_result(value, "number"),
            }
        }
        Some("boolean") => match value {
            Value::Bool(_) | Value::Null => value,
            Value::String(value) => Value::Bool(value == "true" || value == "1"),
            Value::Number(value) => Value::Bool(value.as_i64().is_some_and(|value| value != 0)),
            Value::Array(value) => Value::Bool(!value.is_empty()),
            Value::Object(value) => Value::Bool(!value.is_empty()),
        },
        // `json`/`file`, no hint, and any unrecognized hint pass through
        // untouched — see the overload note above.
        _ => value,
    };
    Ok(coerced)
}

/// Outcome for a numeric hint that failed to parse its value: `null` stays
/// `null` (an absent optional is not corrupt), but any present value that would
/// not parse is a hard error rather than a silently substituted zero.
fn coercion_result(value: Value, type_name: &str) -> Result<Value, String> {
    if value.is_null() {
        Ok(Value::Null)
    } else {
        Err(format!("value {value} cannot be coerced to {type_name}"))
    }
}

/// Apply a reference's `type` hint, falling back to its declared `default` when
/// coercion fails. A value that will not coerce is a hard error, but the author
/// can opt into an explicit fallback by declaring a `default` — mirroring how
/// [`resolve_lookup`] honors a `default` over a shape mismatch. The `default` is
/// coerced through the same hint, so a `default` that itself fails to coerce
/// still surfaces as an error rather than being masked.
fn coerce_reference_value(
    value: Value,
    type_hint: Option<&str>,
    default: Option<&Value>,
) -> Result<Value, String> {
    match apply_type_hint(value, type_hint) {
        Ok(coerced) => Ok(coerced),
        Err(error) => match default {
            Some(default) => apply_type_hint(default.clone(), type_hint),
            None => Err(error),
        },
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

/// `Option`-returning path probe: `Some` on a hit (including a `null` leaf),
/// `None` on any miss (absent *or* shape mismatch). Now used only by tests as a
/// terse `Some/None` shim over the reference walk; production reference
/// resolution goes through [`lookup_segments_detailed`] so a shape mismatch can
/// fail loudly rather than collapse to `None`.
#[cfg(test)]
fn lookup_source_path(source: &Value, path: &str) -> Option<Value> {
    lookup_segments(source, &path_to_segments(path))
}

/// Pre-split a reference path into already-unescaped JSON-pointer segments.
///
/// This is the expensive half of a reference lookup (the `['..']`/`["..]`
/// bracket normalization, the `[N]` numeric-index scan, and the `~0`/`~1`
/// escape round-trip). Hoisting it into a compiled reference means a per-element
/// Filter/While/GroupBy reference parses its path **once** instead of on every
/// evaluation. [`lookup_segments_detailed`] is the cheap walk over the result.
fn path_to_segments(path: &str) -> Vec<String> {
    path_to_json_pointer(path)
        .split('/')
        .skip(1)
        .map(unescape_pointer_segment)
        .collect()
}

/// `Option`-returning walk over pre-split segments — a test-only `Some/None`
/// shim over [`lookup_segments_detailed`]. Reference resolution proper uses the
/// detailed walk so a shape mismatch can fail loudly instead of becoming `None`.
#[cfg(test)]
fn lookup_segments(source: &Value, segments: &[String]) -> Option<Value> {
    match lookup_segments_detailed(source, segments) {
        Lookup::Found(value) => Some(value),
        Lookup::Absent | Lookup::Mismatch(_) => None,
    }
}

/// Outcome of resolving a reference path against the runtime scope.
enum Lookup {
    /// The path resolved to a value (which may itself be `null`).
    Found(Value),
    /// A segment was legitimately absent — a missing object key, an out-of-range
    /// array index, or traversal through an explicit `null`. This is the
    /// possibly-optional case: it resolves to the reference's declared `default`
    /// (or `null`), preserving `ReferenceValue.default` semantics.
    Absent,
    /// A segment traversed into a value of the wrong *shape* — a non-numeric key
    /// indexed into an array, or any segment reaching into a scalar. Almost
    /// always an authoring mistake (e.g. `steps.split.outputs.result`, where
    /// `outputs` is the collected array, not an object). Carries a diagnostic;
    /// [`resolve_lookup`] surfaces it loudly when no `default` is declared.
    Mismatch(String),
}

/// Walk pre-split JSON-pointer segments, resolving any `$wfref` handle
/// encountered (at the root, mid-path, or the final node) so interned values are
/// transparent to references. The result is fully materialized, so callers never
/// see a handle. Only nodes actually traversed are parsed — carrying a large
/// value through scope without reading it never touches the arena.
///
/// Unlike a plain `Option` walk, this distinguishes an optional miss (`Absent`)
/// from a shape error (`Mismatch`) so a mistyped reference tail can fail loudly
/// instead of silently resolving to null. See [`descend`].
fn lookup_segments_detailed(source: &Value, segments: &[String]) -> Lookup {
    let mut current: Cow<Value> = Cow::Borrowed(source);
    if wfref_id(source).is_some() {
        current = Cow::Owned(deref_handle(source).into_owned());
    }
    for (depth, segment) in segments.iter().enumerate() {
        // Borrow the child through inline (non-handle) nodes; clone only when we
        // must deref a `$wfref` handle or when the parent is already owned (its
        // borrow can't outlive this step). The common case — an inline path like
        // `item.sku` — walks entirely by reference and clones once at the leaf.
        current = match current {
            Cow::Borrowed(parent) => match descend(parent, segment) {
                Descent::Child(child) => {
                    if wfref_id(child).is_some() {
                        Cow::Owned(deref_handle(child).into_owned())
                    } else {
                        Cow::Borrowed(child)
                    }
                }
                Descent::Absent => return Lookup::Absent,
                Descent::Mismatch => {
                    return Lookup::Mismatch(mismatch_message(segments, depth, parent));
                }
            },
            Cow::Owned(parent) => match descend(&parent, segment) {
                Descent::Child(child) => {
                    if wfref_id(child).is_some() {
                        Cow::Owned(deref_handle(child).into_owned())
                    } else {
                        Cow::Owned(child.clone())
                    }
                }
                Descent::Absent => return Lookup::Absent,
                Descent::Mismatch => {
                    return Lookup::Mismatch(mismatch_message(segments, depth, &parent));
                }
            },
        };
    }
    Lookup::Found(materialize(current.into_owned()))
}

/// Outcome of indexing one path segment into a value.
enum Descent<'a> {
    /// Segment resolved to a child node.
    Child(&'a Value),
    /// Segment is legitimately absent (missing object key, out-of-range array
    /// index, or a `null` intermediate) — an optional miss.
    Absent,
    /// The value is the wrong shape to index with this segment (a non-numeric key
    /// into an array, or any segment into a scalar) — a probable authoring bug.
    Mismatch,
}

/// Index one JSON-pointer segment into a value, distinguishing an optional miss
/// from a shape mismatch.
///
/// The ONLY hard `Mismatch` is a **non-numeric key indexed into an array** — the
/// reporter's `steps.split.outputs.result`, where `outputs` is the collected
/// array. An array is never a keyed object, so that access has zero legitimate
/// meaning and is surfaced loudly.
///
/// Everything else stays lenient (`Absent`): a missing object key, an
/// out-of-range array index, a `null` intermediate, AND traversal into a scalar.
/// Reaching a nested field on a scalar/absent value is a common, intentional
/// pattern (e.g. filtering a heterogeneous array by `item.status` where some
/// elements are scalars or lack the field) — those must keep resolving to
/// `null`/the declared default rather than aborting the run.
fn descend<'a>(value: &'a Value, segment: &str) -> Descent<'a> {
    match value {
        Value::Object(map) => match map.get(segment) {
            Some(child) => Descent::Child(child),
            None => Descent::Absent,
        },
        Value::Array(items) => {
            if is_array_index_token(segment) {
                // Numeric token: in-range hits a child, out-of-range is an
                // optional miss (mirrors the historical null/default fall-through
                // documented on `array_index`).
                match array_index(segment, items.len()).and_then(|index| items.get(index)) {
                    Some(child) => Descent::Child(child),
                    None => Descent::Absent,
                }
            } else {
                // A named key into an array — e.g. `.result` on a Split's
                // collected array. Never valid; surface it.
                Descent::Mismatch
            }
        }
        // Scalars and `null`: cannot traverse further, but treated as an optional
        // miss (lenient), not a hard error. See the doc above.
        _ => Descent::Absent,
    }
}

/// Build the diagnostic for a [`Descent::Mismatch`]: which reference, which
/// segment failed, and why. Computed purely from `segments` so the interpreter
/// (`apply_reference`) and compiled (`CompiledReference::resolve`) paths produce
/// byte-identical messages (the parity contract above).
fn mismatch_message(segments: &[String], depth: usize, parent: &Value) -> String {
    let path = segments.join(".");
    let base = segments[..depth].join(".");
    let at = if base.is_empty() {
        "the source".to_string()
    } else {
        format!("'{base}'")
    };
    let segment = &segments[depth];
    if parent.is_array() {
        format!(
            "reference '{path}' cannot be resolved: {at} is an array with no field '{segment}' \
             — address array elements by numeric index (e.g. '{base}.0')"
        )
    } else {
        format!(
            "reference '{path}' cannot be resolved: {at} is a {kind} with no field '{segment}'",
            kind = json_type_name(parent)
        )
    }
}

/// Apply the reference `default` policy to a [`Lookup`]. A found value is used
/// as-is; a found `null` or an optional `Absent` miss falls back to the declared
/// default (or `null`); a shape `Mismatch` is a hard error unless the author
/// declared a `default`, in which case their explicit fallback is honored.
///
/// This is the one place a mistyped reference tail (e.g.
/// `steps.split.outputs.result` on a Split whose `outputs` is the collected
/// array) turns into a loud failure instead of a silent null — closing the
/// "green run, wrong result" failure mode.
fn resolve_lookup(lookup: Lookup, default: Option<Value>) -> Result<Value, String> {
    match lookup {
        Lookup::Found(Value::Null) | Lookup::Absent => Ok(default.unwrap_or(Value::Null)),
        Lookup::Found(value) => Ok(value),
        Lookup::Mismatch(message) => default.ok_or(message),
    }
}

/// Resolve a path segment to a concrete array index, supporting Python-style
/// negative suffix indexing: `-1` is the last element, `-2` the second-to-last.
/// Non-numeric segments and out-of-range negatives return `None`, so an unmatched
/// index falls through to the resolver's null/default path exactly like an
/// out-of-range positive index does.
fn array_index(segment: &str, len: usize) -> Option<usize> {
    let raw: i64 = segment.parse().ok()?;
    if raw >= 0 {
        usize::try_from(raw).ok()
    } else {
        // `unsigned_abs` avoids overflow at `i64::MIN`.
        len.checked_sub(usize::try_from(raw.unsigned_abs()).ok()?)
    }
}

/// True when a `[..]` bracket body is an array index — an optional leading `-`
/// followed by one or more ASCII digits (e.g. `0`, `12`, `-1`).
fn is_array_index_token(token: &str) -> bool {
    let digits = token.strip_prefix('-').unwrap_or(token);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Reverse the JSON-pointer escaping applied by [`path_to_json_pointer`].
fn unescape_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
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
            if is_array_index_token(&index) {
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

// ===========================================================================
// Compiled expressions
//
// A condition/mapping/reference is parsed ONCE into a reusable evaluable form
// and evaluated per element/iteration with no JSON re-walk and no path
// re-parse. This is a structural mirror of the interpreter
// (`eval_condition_expression` / `apply_mapping_value` / `apply_reference` /
// `lookup_source_path`); leaf comparison/coercion delegate to the SAME helpers
// (`values_equal` / `is_truthy` / `to_number` / `apply_type_hint` /
// `render_template`), so results are bit-identical. Compilation is infallible:
// any error the interpreter would raise at eval time is carried as an
// `Error(String)` node holding the exact message and re-raised on eval, so
// error parity (incl. message text) is preserved.
// ===========================================================================

#[derive(Debug, Clone, Copy)]
enum CmpOp {
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, Copy)]
enum ArrKind {
    Contains,
    In,
    NotIn,
}

/// Compiled condition — mirrors `eval_condition_expression` (`-> bool`).
#[derive(Debug, Clone)]
enum CompiledCondition {
    Op(Box<CompiledOp>),
    /// Non-operation node: `is_truthy(eval_condition_value(..))`.
    Truthy(CompiledMapping),
}

/// Compiled operation — mirrors `eval_condition_operation`.
#[derive(Debug, Clone)]
enum CompiledOp {
    And(Vec<CompiledCondition>),
    Or(Vec<CompiledCondition>),
    Not(Option<Box<CompiledCondition>>),
    Compare {
        op: CmpOp,
        args: Vec<CompiledArgValue>,
    },
    Equality {
        ne: bool,
        args: Vec<CompiledArgValue>,
    },
    StringMatch {
        ends: bool,
        args: Vec<CompiledArgValue>,
    },
    ArrayMatch {
        kind: ArrKind,
        args: Vec<CompiledArgValue>,
    },
    LengthBool(Vec<CompiledArgValue>),
    IsDefined(Option<CompiledArgValue>),
    IsEmpty(Option<CompiledArgValue>),
    IsNotEmpty(Option<CompiledArgValue>),
    /// Deferred error (missing op/args, query-only operator, unknown op).
    Error(String),
}

/// Compiled condition argument evaluated as a value — mirrors
/// `eval_condition_argument_as_value`.
#[derive(Debug, Clone)]
enum CompiledArgValue {
    /// `LENGTH` operation used as a value (`eval_length_as_value`).
    /// `None` = the LENGTH op had no `arguments` array.
    LengthValue(Option<Vec<CompiledArgValue>>),
    /// A non-LENGTH operation used as a value: `Value::Bool(eval_condition(..))`.
    Condition(Box<CompiledCondition>),
    /// A plain value/mapping (`eval_condition_value`).
    Value(CompiledMapping),
}

/// Compiled mapping value — mirrors `apply_mapping_value`.
#[derive(Debug, Clone)]
enum CompiledMapping {
    Reference(CompiledReference),
    Immediate(Value),
    Composite(Box<CompiledComposite>),
    Template(String),
    /// Deferred error (not an object / missing valueType / bad reference path /
    /// non-string template / unsupported valueType / bad composite).
    Error(String),
}

#[derive(Debug, Clone)]
enum CompiledComposite {
    Object(Vec<(String, CompiledMapping)>),
    Array(Vec<CompiledMapping>),
}

/// Compiled reference — `lookup` path pre-split once. Mirrors `apply_reference`.
#[derive(Debug, Clone)]
struct CompiledReference {
    segments: Vec<String>,
    default: Option<Value>,
    type_hint: Option<String>,
}

// ---- Compilation (infallible; defers errors to Error nodes) ----

fn compile_condition(expr: &Value) -> CompiledCondition {
    if is_condition_operation(expr) {
        CompiledCondition::Op(Box::new(compile_op(expr)))
    } else {
        CompiledCondition::Truthy(compile_value(expr))
    }
}

fn compile_op(expr: &Value) -> CompiledOp {
    let Some(op) = expr.get("op").and_then(Value::as_str) else {
        return CompiledOp::Error("condition operation missing op".to_string());
    };
    let Some(args) = expr.get("arguments").and_then(Value::as_array) else {
        return CompiledOp::Error("condition operation missing arguments".to_string());
    };
    let vals = || args.iter().map(compile_arg_value).collect::<Vec<_>>();
    match op {
        "AND" => CompiledOp::And(args.iter().map(compile_condition).collect()),
        "OR" => CompiledOp::Or(args.iter().map(compile_condition).collect()),
        "NOT" => CompiledOp::Not(args.first().map(|a| Box::new(compile_condition(a)))),
        "GT" => CompiledOp::Compare {
            op: CmpOp::Gt,
            args: vals(),
        },
        "GTE" => CompiledOp::Compare {
            op: CmpOp::Gte,
            args: vals(),
        },
        "LT" => CompiledOp::Compare {
            op: CmpOp::Lt,
            args: vals(),
        },
        "LTE" => CompiledOp::Compare {
            op: CmpOp::Lte,
            args: vals(),
        },
        "EQ" => CompiledOp::Equality {
            ne: false,
            args: vals(),
        },
        "NE" => CompiledOp::Equality {
            ne: true,
            args: vals(),
        },
        "STARTS_WITH" => CompiledOp::StringMatch {
            ends: false,
            args: vals(),
        },
        "ENDS_WITH" => CompiledOp::StringMatch {
            ends: true,
            args: vals(),
        },
        "CONTAINS" => CompiledOp::ArrayMatch {
            kind: ArrKind::Contains,
            args: vals(),
        },
        "IN" => CompiledOp::ArrayMatch {
            kind: ArrKind::In,
            args: vals(),
        },
        "NOT_IN" => CompiledOp::ArrayMatch {
            kind: ArrKind::NotIn,
            args: vals(),
        },
        "LENGTH" => CompiledOp::LengthBool(vals()),
        "IS_DEFINED" => CompiledOp::IsDefined(args.first().map(compile_arg_value)),
        "IS_EMPTY" => CompiledOp::IsEmpty(args.first().map(compile_arg_value)),
        "IS_NOT_EMPTY" => CompiledOp::IsNotEmpty(args.first().map(compile_arg_value)),
        "SIMILARITY_GTE" | "MATCH" | "COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE" => {
            CompiledOp::Error(format!(
                "condition operator '{op}' is only valid inside object-model query conditions; \
                 the workflow runtime cannot evaluate it"
            ))
        }
        other => CompiledOp::Error(format!("unsupported condition operator '{other}'")),
    }
}

fn compile_arg_value(arg: &Value) -> CompiledArgValue {
    if is_condition_operation(arg) {
        if arg.get("op").and_then(Value::as_str) == Some("LENGTH") {
            let compiled = arg
                .get("arguments")
                .and_then(Value::as_array)
                .map(|args| args.iter().map(compile_arg_value).collect());
            CompiledArgValue::LengthValue(compiled)
        } else {
            CompiledArgValue::Condition(Box::new(compile_condition(arg)))
        }
    } else {
        CompiledArgValue::Value(compile_value(arg))
    }
}

/// Mirror `eval_condition_value`'s `{type:"value"}` unwrap, then compile the
/// resolved mapping object.
fn compile_value(value: &Value) -> CompiledMapping {
    if value.get("type").and_then(Value::as_str) == Some("value") {
        if value.get("valueType").is_some() {
            return compile_mapping(value);
        }
        if let Some(inner) = value.get("value") {
            return compile_mapping(inner);
        }
    }
    compile_mapping(value)
}

fn compile_mapping(value: &Value) -> CompiledMapping {
    let Value::Object(map) = value else {
        return CompiledMapping::Error("mapping value must be an object".to_string());
    };
    let Some(value_type) = map.get("valueType").and_then(Value::as_str) else {
        return CompiledMapping::Error("mapping value missing valueType".to_string());
    };
    match value_type {
        "reference" => match map.get("value").and_then(Value::as_str) {
            Some(path) => CompiledMapping::Reference(CompiledReference {
                segments: path_to_segments(path),
                default: map.get("default").cloned(),
                type_hint: map.get("type").and_then(Value::as_str).map(str::to_string),
            }),
            None => {
                CompiledMapping::Error("reference mapping value must be a string path".to_string())
            }
        },
        "immediate" => CompiledMapping::Immediate(map.get("value").cloned().unwrap_or(Value::Null)),
        "composite" => compile_composite(map.get("value").unwrap_or(&Value::Null)),
        "template" => match map.get("value").and_then(Value::as_str) {
            Some(template) => CompiledMapping::Template(template.to_string()),
            None => CompiledMapping::Error("template mapping value must be a string".to_string()),
        },
        other => CompiledMapping::Error(format!("unsupported mapping valueType '{other}'")),
    }
}

fn compile_composite(value: &Value) -> CompiledMapping {
    match value {
        Value::Object(map) => CompiledMapping::Composite(Box::new(CompiledComposite::Object(
            map.iter()
                .map(|(k, child)| (k.clone(), compile_mapping(child)))
                .collect(),
        ))),
        Value::Array(items) => CompiledMapping::Composite(Box::new(CompiledComposite::Array(
            items.iter().map(compile_mapping).collect(),
        ))),
        _ => {
            CompiledMapping::Error("composite mapping value must be an object or array".to_string())
        }
    }
}

// ---- Evaluation (delegates leaf ops to the shared helpers) ----

impl CompiledCondition {
    fn eval(&self, source: &Value) -> Result<bool, String> {
        match self {
            CompiledCondition::Op(op) => op.eval(source),
            CompiledCondition::Truthy(value) => value.eval(source).map(|v| is_truthy(&v)),
        }
    }
}

impl CompiledOp {
    fn eval(&self, source: &Value) -> Result<bool, String> {
        match self {
            CompiledOp::And(args) => args
                .iter()
                .try_fold(true, |acc, a| if !acc { Ok(false) } else { a.eval(source) }),
            CompiledOp::Or(args) => args
                .iter()
                .try_fold(false, |acc, a| if acc { Ok(true) } else { a.eval(source) }),
            CompiledOp::Not(arg) => match arg {
                Some(c) => c.eval(source).map(|v| !v),
                None => Ok(true),
            },
            CompiledOp::Compare { op, args } => {
                if args.len() < 2 {
                    return Ok(false);
                }
                let (Some(left), Some(right)) = (
                    to_number(&args[0].eval(source)?),
                    to_number(&args[1].eval(source)?),
                ) else {
                    return Ok(false);
                };
                Ok(match op {
                    CmpOp::Gt => left > right,
                    CmpOp::Gte => left >= right,
                    CmpOp::Lt => left < right,
                    CmpOp::Lte => left <= right,
                })
            }
            CompiledOp::Equality { ne, args } => {
                if args.len() < 2 {
                    return Ok(false);
                }
                let equal = values_equal(&args[0].eval(source)?, &args[1].eval(source)?);
                Ok(if *ne { !equal } else { equal })
            }
            CompiledOp::StringMatch { ends, args } => {
                if args.len() < 2 {
                    return Ok(false);
                }
                let left = args[0].eval(source)?;
                let right = args[1].eval(source)?;
                let (Some(left), Some(right)) = (left.as_str(), right.as_str()) else {
                    return Ok(false);
                };
                Ok(if *ends {
                    left.ends_with(right)
                } else {
                    left.starts_with(right)
                })
            }
            CompiledOp::ArrayMatch { kind, args } => {
                if args.len() < 2 {
                    return Ok(false);
                }
                let left = args[0].eval(source)?;
                let right = args[1].eval(source)?;
                let matched = match kind {
                    ArrKind::Contains => left
                        .as_array()
                        .is_some_and(|items| items.iter().any(|i| values_equal(i, &right))),
                    ArrKind::In | ArrKind::NotIn => right
                        .as_array()
                        .is_some_and(|items| items.iter().any(|i| values_equal(&left, i))),
                };
                Ok(if matches!(kind, ArrKind::NotIn) {
                    !matched
                } else {
                    matched
                })
            }
            CompiledOp::LengthBool(args) => {
                let value = compiled_length_value(args, source)?;
                Ok(value
                    .as_i64()
                    .or_else(|| value.as_u64().map(|v| v as i64))
                    .unwrap_or(0)
                    > 0)
            }
            CompiledOp::IsDefined(arg) => match arg {
                Some(a) => a.eval(source).map(|v| !v.is_null()),
                None => Ok(false),
            },
            CompiledOp::IsEmpty(arg) => match arg {
                Some(a) => a.eval(source).map(|v| match v {
                    Value::Array(v) => v.is_empty(),
                    Value::String(v) => v.is_empty(),
                    Value::Object(v) => v.is_empty(),
                    Value::Null => true,
                    _ => false,
                }),
                None => Ok(true),
            },
            CompiledOp::IsNotEmpty(arg) => match arg {
                Some(a) => a.eval(source).map(|v| match v {
                    Value::Array(v) => !v.is_empty(),
                    Value::String(v) => !v.is_empty(),
                    Value::Object(v) => !v.is_empty(),
                    Value::Null => false,
                    _ => true,
                }),
                None => Ok(false),
            },
            CompiledOp::Error(message) => Err(message.clone()),
        }
    }
}

impl CompiledArgValue {
    fn eval(&self, source: &Value) -> Result<Value, String> {
        match self {
            CompiledArgValue::LengthValue(Some(args)) => compiled_length_value(args, source),
            CompiledArgValue::LengthValue(None) => {
                Err("LENGTH condition missing arguments".to_string())
            }
            CompiledArgValue::Condition(cond) => cond.eval(source).map(Value::Bool),
            CompiledArgValue::Value(mapping) => mapping.eval(source),
        }
    }
}

fn compiled_length_value(args: &[CompiledArgValue], source: &Value) -> Result<Value, String> {
    let Some(arg) = args.first() else {
        return Ok(Value::Number(0.into()));
    };
    let len = match &arg.eval(source)? {
        Value::String(v) => v.len() as i64,
        Value::Array(v) => v.len() as i64,
        Value::Object(v) => v.len() as i64,
        Value::Null => 0,
        _ => 1,
    };
    Ok(Value::Number(len.into()))
}

impl CompiledMapping {
    fn eval(&self, source: &Value) -> Result<Value, String> {
        match self {
            CompiledMapping::Reference(reference) => reference.resolve(source),
            CompiledMapping::Immediate(value) => Ok(value.clone()),
            CompiledMapping::Composite(composite) => composite.eval(source),
            CompiledMapping::Template(template) => {
                render_template(template, &materialize(source.clone())).map(Value::String)
            }
            CompiledMapping::Error(message) => Err(message.clone()),
        }
    }
}

impl CompiledComposite {
    fn eval(&self, source: &Value) -> Result<Value, String> {
        match self {
            CompiledComposite::Object(entries) => {
                let mut output = Map::new();
                for (key, child) in entries {
                    output.insert(key.clone(), child.eval(source)?);
                }
                Ok(Value::Object(output))
            }
            CompiledComposite::Array(items) => items
                .iter()
                .map(|item| item.eval(source))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
        }
    }
}

impl CompiledReference {
    fn resolve(&self, source: &Value) -> Result<Value, String> {
        let value = resolve_lookup(
            lookup_segments_detailed(source, &self.segments),
            self.default.clone(),
        )?;
        coerce_reference_value(value, self.type_hint.as_deref(), self.default.as_ref())
    }
}

/// Compiled top-level input mapping — mirrors `apply_input_mapping`. Each entry
/// value is compiled once; per evaluation the dotted key is expanded with the
/// same `insert_nested` as the interpreter.
#[derive(Debug, Clone)]
enum CompiledInputMapping {
    Entries(Vec<(String, CompiledMapping)>),
    Error(String),
}

fn compile_input_mapping(mapping: &Value) -> CompiledInputMapping {
    match mapping {
        Value::Object(entries) => CompiledInputMapping::Entries(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), compile_mapping(value)))
                .collect(),
        ),
        _ => CompiledInputMapping::Error("input mapping must be a JSON object".to_string()),
    }
}

impl CompiledInputMapping {
    fn eval(&self, source: &Value) -> Result<Value, String> {
        match self {
            CompiledInputMapping::Entries(entries) => {
                let mut output = Map::new();
                for (key, value) in entries {
                    insert_nested(&mut output, key, value.eval(source)?);
                }
                Ok(Value::Object(output))
            }
            CompiledInputMapping::Error(message) => Err(message.clone()),
        }
    }
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
    /// Resolvable connection binding (a `MappingValue`), evaluated against the
    /// execution source at runtime; wins over `connection_id` when present.
    #[serde(default)]
    connection_ref: Option<Value>,
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
    connection_ref: Option<Value>,
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

    #[test]
    fn interning_round_trips_large_values() {
        reset_value_store();
        let big = json!({ "items": vec!["x".repeat(1000); 50] }); // ~50 KiB
        let handle = intern_if_large(big.clone());
        assert!(
            wfref_id(&handle).is_some(),
            "a value over the threshold should become a handle"
        );
        assert_eq!(materialize(handle), big, "materialize must round-trip");
    }

    #[test]
    fn interning_leaves_small_values_inline() {
        reset_value_store();
        let small = json!({ "a": 1, "b": "short" });
        assert_eq!(
            intern_if_large(small.clone()),
            small,
            "small values must be byte-identical (no handle)"
        );
    }

    /// SYN-448: negative array indices resolve Python-style (`-1` = last) instead
    /// of silently returning null. Covers the shared core walked by both the
    /// interpreter and the compiled resolver.
    #[test]
    fn lookup_resolves_negative_array_indices() {
        reset_value_store();
        let source = json!({ "items": ["a", "b", "c"] });

        // Dot form: -1 is the last element, -3 the first.
        assert_eq!(lookup_source_path(&source, "items.-1"), Some(json!("c")));
        assert_eq!(lookup_source_path(&source, "items.-2"), Some(json!("b")));
        assert_eq!(lookup_source_path(&source, "items.-3"), Some(json!("a")));
        // Positive indexing is unchanged.
        assert_eq!(lookup_source_path(&source, "items.0"), Some(json!("a")));
        // Bracket form normalizes the same way.
        assert_eq!(lookup_source_path(&source, "items[-1]"), Some(json!("c")));
        // Out-of-range negative misses (None), like an out-of-range positive index.
        assert_eq!(lookup_source_path(&source, "items.-4"), None);
        assert_eq!(lookup_source_path(&source, "items.3"), None);
    }

    /// SYN-448: the negative index must reach the leaf through both reference
    /// resolvers — the interpreter (`apply_mapping_value`) and the compiled form
    /// (`compile_mapping(..).eval`) — not just the raw path walk.
    #[test]
    fn reference_resolvers_honor_negative_index() {
        reset_value_store();
        let source =
            json!({ "steps": { "make_array": { "outputs": { "items": ["a", "b", "c"] } } } });
        let reference = json!({
            "valueType": "reference",
            "value": "steps.make_array.outputs.items.-1",
        });

        // Interpreter path.
        assert_eq!(
            apply_mapping_value(&reference, &source).unwrap(),
            json!("c"),
            "interpreter reference must resolve -1 to the last element"
        );
        // Compiled path (Filter/While/GroupBy hot loops).
        assert_eq!(
            compile_mapping(&reference).eval(&source).unwrap(),
            json!("c"),
            "compiled reference must resolve -1 to the last element"
        );
    }

    /// SYN-448: a real array element (`null` last) must not be confused with an
    /// out-of-range miss, and an out-of-range negative must fall back to the
    /// reference `default` rather than the wrong element.
    #[test]
    fn negative_index_distinguishes_null_element_from_miss() {
        reset_value_store();
        let source = json!({ "items": ["a", null] });

        // -1 points at a genuine null element.
        assert_eq!(lookup_source_path(&source, "items.-1"), Some(json!(null)));

        // Out-of-range negative falls through to the mapping's default.
        let with_default = json!({
            "valueType": "reference",
            "value": "items.-9",
            "default": "fallback",
        });
        assert_eq!(
            apply_mapping_value(&with_default, &source).unwrap(),
            json!("fallback")
        );
    }

    /// A named key indexed into an array (the reporter's `steps.split.outputs.result`
    /// on a Split whose `outputs` is the collected array) is a shape mismatch: with
    /// no `default` it must FAIL LOUD in both resolvers, not silently resolve to
    /// null. This is the fix for the "green run, wrong result" failure mode.
    #[test]
    fn named_key_into_array_with_no_default_fails_loud() {
        reset_value_store();
        let source = json!({
            "steps": { "split_users": { "outputs": ["a", "b", "c"] } }
        });
        let reference = json!({
            "valueType": "reference",
            "value": "steps.split_users.outputs.result",
        });

        // Interpreter path errors.
        let interp = apply_mapping_value(&reference, &source);
        assert!(interp.is_err(), "interpreter must fail on array.named_key");
        // Compiled path (Filter/While/GroupBy + condition args) errors identically.
        let compiled = compile_mapping(&reference).eval(&source);
        assert!(compiled.is_err(), "compiled must fail on array.named_key");
        assert_eq!(
            interp.unwrap_err(),
            compiled.unwrap_err(),
            "interpreter and compiled error messages must match (parity contract)"
        );

        // The message names the reference and explains the shape mismatch.
        let message = apply_mapping_value(&reference, &source).unwrap_err();
        assert!(
            message.contains("steps.split_users.outputs.result")
                && message.contains("is an array")
                && message.contains("'result'"),
            "unhelpful mismatch message: {message}"
        );
    }

    /// The same shape mismatch is silenced when the author declares an explicit
    /// `default` — their opt-out is honored, no error.
    #[test]
    fn shape_mismatch_with_explicit_default_is_honored() {
        reset_value_store();
        let source = json!({ "steps": { "s": { "outputs": ["a"] } } });
        let reference = json!({
            "valueType": "reference",
            "value": "steps.s.outputs.result",
            "default": "fallback",
        });
        assert_eq!(
            apply_mapping_value(&reference, &source).unwrap(),
            json!("fallback")
        );
        assert_eq!(
            compile_mapping(&reference).eval(&source).unwrap(),
            json!("fallback")
        );
    }

    /// Traversing INTO a scalar stays LENIENT (resolves to null / the declared
    /// default), NOT a hard error: `item.status` on a scalar element of a
    /// heterogeneous array is a legitimate filter pattern. Only a named key into
    /// an array is fail-loud (see `named_key_into_array_with_no_default_fails_loud`).
    #[test]
    fn traversing_into_scalar_stays_lenient() {
        reset_value_store();
        let source = json!({ "data": { "name": "alice" } });

        let no_default = json!({ "valueType": "reference", "value": "data.name.first" });
        assert_eq!(
            apply_mapping_value(&no_default, &source).unwrap(),
            json!(null)
        );
        assert_eq!(
            compile_mapping(&no_default).eval(&source).unwrap(),
            json!(null)
        );

        let with_default =
            json!({ "valueType": "reference", "value": "data.name.first", "default": "d" });
        assert_eq!(
            apply_mapping_value(&with_default, &source).unwrap(),
            json!("d")
        );
    }

    /// Optional references MUST stay lenient: a missing OBJECT key and an
    /// out-of-range NUMERIC index resolve to null (or the declared default)
    /// without erroring — these are the `ReferenceValue.default` cases the DSL
    /// documents, distinct from the array-named-key / scalar shape errors above.
    #[test]
    fn optional_misses_stay_lenient() {
        reset_value_store();
        let source = json!({ "data": { "present": 1, "nested": null }, "arr": ["x"] });

        // Missing object key, no default -> null (not an error).
        let missing_key = json!({ "valueType": "reference", "value": "data.absent" });
        assert_eq!(
            apply_mapping_value(&missing_key, &source).unwrap(),
            json!(null)
        );
        assert_eq!(
            compile_mapping(&missing_key).eval(&source).unwrap(),
            json!(null)
        );

        // Out-of-range numeric index, no default -> null (not an error).
        let oob_index = json!({ "valueType": "reference", "value": "arr.9" });
        assert_eq!(
            apply_mapping_value(&oob_index, &source).unwrap(),
            json!(null)
        );
        assert_eq!(
            compile_mapping(&oob_index).eval(&source).unwrap(),
            json!(null)
        );

        // A `null` intermediate (`data.nested` IS null) stays an optional chain
        // (lenient) rather than a scalar-traversal mismatch, so `.deeper` through
        // it falls back to the declared default.
        let via_null = json!({
            "valueType": "reference",
            "value": "data.nested.deeper",
            "default": "d",
        });
        assert_eq!(apply_mapping_value(&via_null, &source).unwrap(), json!("d"));
        assert_eq!(
            compile_mapping(&via_null).eval(&source).unwrap(),
            json!("d")
        );
    }

    #[test]
    fn lookup_resolves_through_handles() {
        reset_value_store();
        let big = json!({ "pages": vec![json!({ "sku": "S" }); 4000] }); // > 16 KiB
        let handle = intern_if_large(big.clone());
        assert!(wfref_id(&handle).is_some());
        let source = json!({ "variables": { "acc": handle } });
        // Path crossing a handle resolves the underlying value.
        assert_eq!(
            lookup_source_path(&source, "variables.acc.pages[0].sku"),
            Some(json!("S"))
        );
        // Reading the whole handle materializes it.
        assert_eq!(lookup_source_path(&source, "variables.acc"), Some(big));
    }

    #[test]
    fn build_source_interns_large_variable_and_resolves_it() {
        reset_value_store();
        let big = json!(vec!["y".repeat(100); 500]); // ~50 KiB array
        let data = serde_json::to_vec(&json!({})).unwrap();
        let variables = serde_json::to_vec(&json!({ "big": big, "small": 1 })).unwrap();
        let steps = serde_json::to_vec(&json!({})).unwrap();
        let source_bytes = build_source(&data, &variables, &steps).unwrap();
        // The serialized source is tiny because `big` became a handle, even though
        // it is referenced twice (source.variables and workflow.inputs.variables).
        assert!(
            source_bytes.len() < 2048,
            "interned source should be small, got {} bytes",
            source_bytes.len()
        );
        let source: Value = serde_json::from_slice(&source_bytes).unwrap();
        assert_eq!(
            lookup_source_path(&source, "variables.small"),
            Some(json!(1))
        );
        assert_eq!(lookup_source_path(&source, "variables.big"), Some(big));
    }

    #[test]
    fn value_store_retain_frees_unreachable() {
        reset_value_store();
        let keep = json!({ "rows": vec!["k".repeat(100); 500] }); // > 16 KiB
        let drop = json!({ "rows": vec!["d".repeat(100); 500] }); // > 16 KiB, distinct
        let keep_handle = intern_if_large(keep.clone());
        let drop_handle = intern_if_large(drop.clone());
        assert!(wfref_id(&keep_handle).is_some() && wfref_id(&drop_handle).is_some());
        // Retain with only the keep handle reachable from a root.
        let root = serde_json::to_vec(&json!({ "survivor": keep_handle.clone() })).unwrap();
        value_store_retain(&[root.as_slice()]);
        assert_eq!(
            materialize(keep_handle),
            keep,
            "reachable value must survive"
        );
        assert_eq!(
            materialize(drop_handle),
            Value::Null,
            "unreachable value must be collected"
        );
    }

    #[test]
    fn value_store_retain_marks_transitively() {
        reset_value_store();
        let inner = json!(vec!["i".repeat(100); 500]); // > 16 KiB
        let inner_handle = intern_if_large(inner.clone());
        // The outer value carries the inner handle, so collecting `outer` must
        // keep `inner` alive transitively.
        let outer = json!({ "inner": inner_handle, "pad": "x".repeat(20_000) });
        let outer_handle = intern_if_large(outer);
        let root = serde_json::to_vec(&json!({ "s": outer_handle.clone() })).unwrap();
        value_store_retain(&[root.as_slice()]);
        let materialized = materialize(outer_handle);
        assert_eq!(
            materialized["inner"], inner,
            "nested handle survives transitively"
        );
    }

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

    /// Build a manifest whose sole mapping resolves `data.count` under a `type`
    /// hint (and optional `default`), then run it against `data`.
    fn coerce_count(type_hint: &str, default: Option<Value>, data: &[u8]) -> Result<Value, String> {
        let mut reference = json!({
            "valueType": "reference",
            "value": "data.count",
            "type": type_hint,
        });
        if let Some(default) = default {
            reference["default"] = default;
        }
        let manifest =
            DirectJsonManifest::parse(&manifest(json!({ "result": reference }))).expect("manifest");
        let source = build_source(data, b"{}", b"{}").expect("source");
        manifest
            .apply_mapping(0, &source)
            .map(|bytes| serde_json::from_slice(&bytes).expect("output json"))
    }

    #[test]
    fn integer_hint_rejects_unparseable_value_instead_of_zero() {
        // A present, non-null value that will not parse must fail the step, not
        // become a plausible `0` flowing into downstream totals/thresholds.
        let error = coerce_count("integer", None, br#"{"count":"abc"}"#)
            .expect_err("unparseable integer should error");
        assert!(
            error.contains("cannot be coerced to integer"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn number_hint_rejects_unparseable_value_instead_of_zero() {
        let error = coerce_count("number", None, br#"{"count":"not-a-number"}"#)
            .expect_err("unparseable number should error");
        assert!(
            error.contains("cannot be coerced to number"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn integer_hint_default_rescues_unparseable_value() {
        // The author's `default` is the explicit escape hatch: an unparseable
        // value falls back to it rather than erroring or silently zeroing.
        let output = coerce_count("integer", Some(json!(7)), br#"{"count":"abc"}"#)
            .expect("default should rescue");
        assert_eq!(output, json!({ "result": 7 }));
    }

    #[test]
    fn integer_hint_default_is_coerced_through_the_hint() {
        // A string default is coerced by the same hint, matching the treatment
        // a resolved value would receive.
        let output = coerce_count("integer", Some(json!("42")), br#"{"count":"abc"}"#)
            .expect("string default should coerce");
        assert_eq!(output, json!({ "result": 42 }));
    }

    #[test]
    fn integer_hint_unparseable_default_still_errors() {
        // A `default` that itself will not coerce is an authoring error, not a
        // silent escape — it must surface rather than be masked.
        let error = coerce_count("integer", Some(json!("also-bad")), br#"{"count":"abc"}"#)
            .expect_err("unparseable default should error");
        assert!(
            error.contains("cannot be coerced to integer"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn integer_hint_passes_null_through() {
        // An absent/optional field is not corrupt data — `null` stays `null`.
        let output = coerce_count("integer", None, br#"{"count":null}"#).expect("null passes");
        assert_eq!(output, json!({ "result": null }));
    }

    #[test]
    fn integer_hint_still_coerces_parseable_values() {
        assert_eq!(
            coerce_count("integer", None, br#"{"count":"42"}"#).expect("string parses"),
            json!({ "result": 42 })
        );
        assert_eq!(
            coerce_count("integer", None, br#"{"count":42}"#).expect("number passes"),
            json!({ "result": 42 })
        );
    }

    #[test]
    fn unrecognized_type_hint_passes_value_through() {
        // The `type` key is overloaded in the raw manifest (a condition value
        // expression carries `type: "value"`), so an unrecognized hint must not
        // corrupt or reject the value — it passes through untouched. Unknown
        // `ValueType` hints on real reference mappings are caught earlier, by the
        // typed authoring layer.
        assert_eq!(
            coerce_count("value", None, br#"{"count":5}"#).expect("unknown hint passes through"),
            json!({ "result": 5 })
        );
    }

    #[test]
    fn apply_type_hint_string_and_boolean_are_total() {
        // String/boolean coercions never fail — every value has a representation.
        assert_eq!(
            apply_type_hint(json!(5), Some("string")).expect("string is total"),
            json!("5")
        );
        assert_eq!(
            apply_type_hint(json!("anything"), Some("boolean")).expect("boolean is total"),
            json!(false)
        );
        assert_eq!(
            apply_type_hint(json!("true"), Some("boolean")).expect("boolean is total"),
            json!(true)
        );
    }

    #[test]
    fn apply_type_hint_passthrough_hints_do_not_coerce() {
        // json/file/no-hint and any unrecognized hint leave the value untouched,
        // including values a numeric hint would reject.
        for hint in [Some("json"), Some("file"), Some("value"), None] {
            assert_eq!(
                apply_type_hint(json!("abc"), hint).expect("passthrough"),
                json!("abc")
            );
        }
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
    fn finish_mapping_keeps_envelope_when_outputs_is_one_of_several_fields() {
        // A multi-field Finish mapping that merely INCLUDES an `outputs` key (e.g.
        // a Split dontStop aggregation `{ data, stats, outputs }`) must be returned
        // whole — unwrapping `outputs` would silently drop the sibling fields.
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "outputs": { "valueType": "reference", "value": "data.outs" },
            "stats": { "valueType": "reference", "value": "data.st" },
            "marker": { "valueType": "immediate", "value": "X" }
        })))
        .expect("manifest");
        let source = build_source(
            br#"{"outs":[],"st":{"success":0,"error":2,"total":2}}"#,
            b"{}",
            b"{}",
        )
        .expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(
            output,
            json!({
                "outputs": [],
                "stats": { "success": 0, "error": 2, "total": 2 },
                "marker": "X"
            })
        );
    }

    #[test]
    fn finish_mapping_unwraps_sole_outputs_array() {
        // The single-output convention still applies when `outputs` is the only
        // field, even if its value is an array: `{ outputs: [] }` returns `[]`.
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "outputs": { "valueType": "reference", "value": "data.outs" }
        })))
        .expect("manifest");
        let source = build_source(br#"{"outs":[1,2,3]}"#, b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!([1, 2, 3]));
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
    fn resolve_connection_id_prefers_ref_then_literal_then_none() {
        // Three agents: one bound via a resolvable `connection_ref` to a
        // caller-supplied `connection` input, one with a literal id, one with
        // no connection. `resolve_connection_id` is the runtime source for the
        // agent-invoke `connection` argument, so this is what threads a rotated
        // / caller-supplied id to every agent kind uniformly.
        let manifest_json = serde_json::to_vec(&json!({
            "graph": {
                "agents": [
                    { "id": 0, "stepId": "a0", "stepType": "Agent", "purpose": "agent.config",
                      "agentId": "hubspot", "capabilityId": "create-contact", "inputMappingId": 0,
                      "connectionRef": { "valueType": "reference", "value": "data.crm" } },
                    { "id": 1, "stepId": "a1", "stepType": "Agent", "purpose": "agent.config",
                      "agentId": "hubspot", "capabilityId": "create-contact", "inputMappingId": 0,
                      "connectionId": "conn-literal" },
                    { "id": 2, "stepId": "a2", "stepType": "Agent", "purpose": "agent.config",
                      "agentId": "utils", "capabilityId": "noop", "inputMappingId": 0 }
                ],
                "mappings": [{ "id": 0, "stepId": "a0", "stepType": "Agent",
                    "purpose": "agent.inputMapping", "value": {} }],
                "steps": []
            }
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest_json).expect("manifest");
        let source = build_source(br#"{"crm":"conn-live-42"}"#, b"{}", b"{}").expect("source");

        // Ref resolves from the runtime input.
        assert_eq!(
            manifest
                .resolve_connection_id(0, &source)
                .expect("resolve ref"),
            b"conn-live-42".to_vec()
        );
        // Literal id passes through.
        assert_eq!(
            manifest
                .resolve_connection_id(1, &source)
                .expect("resolve literal"),
            b"conn-literal".to_vec()
        );
        // No connection → empty (caller writes no connection argument).
        assert!(
            manifest
                .resolve_connection_id(2, &source)
                .expect("resolve none")
                .is_empty()
        );

        // A ref that resolves to an absent input yields no connection.
        let empty_source = build_source(b"{}", b"{}", b"{}").expect("source");
        assert!(
            manifest
                .resolve_connection_id(0, &empty_source)
                .expect("resolve absent")
                .is_empty()
        );
    }

    #[test]
    fn template_resolves_through_interned_loop_outputs() {
        // Regression (interning stage 1, 8.0.19): a While iteration's accumulated
        // `loop.outputs` grows past the 16 KiB intern threshold, so build_source
        // carries `variables._loop` — and thus `source.loop` — as a `$wfref`
        // handle. References see through handles (lookup_source_path), but a
        // template reading *into* the handle (`loop.outputs.next_page`) saw the
        // bare handle object instead of the value: `loop.outputs` resolved to
        // undefined and minijinja raised "undefined value" on iteration 1+.
        reset_value_store();
        let big_pages = json!(vec![json!({ "sku": "S" }); 4000]); // > 16 KiB
        let loop_ctx = json!({
            "index": 1,
            "outputs": { "next_page": 2, "pages": big_pages }
        });
        let variables = serde_json::to_vec(&json!({ "_loop": loop_ctx })).unwrap();
        let source = build_source(b"{}", &variables, b"{}").expect("source");

        // Precondition: the large loop context is carried as a handle.
        let parsed: Value = serde_json::from_slice(&source).unwrap();
        assert!(
            wfref_id(&parsed["loop"]).is_some(),
            "precondition: large loop context should be interned to a handle"
        );

        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "page": {
                "valueType": "template",
                "value": "{% if loop.outputs.next_page %}{{ loop.outputs.next_page }}{% else %}1{% endif %}"
            }
        })))
        .expect("manifest");

        let output = manifest
            .apply_mapping(0, &source)
            .expect("template must render through the interned loop context");
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["page"], json!("2"));
    }

    #[test]
    fn template_resolves_nested_access_into_interned_scope_value() {
        // The same handle-opacity affects any template that reaches *into* a
        // large top-level scope entry (`data.*`, `variables.*`, `steps.*`), not
        // just `loop.outputs`. Here `data.big` is interned to a handle.
        reset_value_store();
        let big = json!({
            "marker": "found",
            "rows": vec!["x".repeat(100); 500] // > 16 KiB
        });
        let data = serde_json::to_vec(&json!({ "big": big })).unwrap();
        let source = build_source(&data, b"{}", b"{}").expect("source");

        let parsed: Value = serde_json::from_slice(&source).unwrap();
        assert!(
            wfref_id(&parsed["data"]["big"]).is_some(),
            "precondition: large data entry should be interned to a handle"
        );

        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "msg": {
                "valueType": "template",
                "value": "{{ data.big.marker }}"
            }
        })))
        .expect("manifest");

        let output = manifest
            .apply_mapping(0, &source)
            .expect("template must render through the interned data entry");
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["msg"], json!("found"));
    }

    #[test]
    fn agent_mapping_resolves_refs_nested_in_condition_payload() {
        // Regression (direct-wasm migration): references buried inside an
        // immediate condition payload — e.g. object-model query-instances
        // loading an instance back by the id a previous step produced — must
        // resolve against workflow scope like the generated compiler's
        // resolve_nested_references pass did. Without it the agent receives
        // the literal path string and the query silently matches nothing.
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "schema_name": { "valueType": "immediate", "value": "Invoice" },
            "condition": { "valueType": "immediate", "value": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "id" },
                    { "valueType": "reference", "value": "steps.create.outputs.id" }
                ]
            }}
        })))
        .expect("manifest");
        let source = build_source(
            b"{}",
            b"{}",
            br#"{"create":{"outputs":{"id":"5f0c9c2e-7e2b-4d27-9c0a-1a2b3c4d5e6f"}}}"#,
        )
        .expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(
            output,
            json!({
                "schema_name": "Invoice",
                "condition": {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                        // Field-position references name Object Model columns,
                        // not workflow paths, so they stay references.
                        { "valueType": "reference", "value": "id" },
                        {
                            "valueType": "immediate",
                            "value": "5f0c9c2e-7e2b-4d27-9c0a-1a2b3c4d5e6f"
                        }
                    ]
                }
            })
        );
    }

    #[test]
    fn agent_mapping_keeps_immediate_condition_literals_intact() {
        let condition = json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "id" },
                { "valueType": "immediate", "value": "literal-uuid" }
            ]
        });
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "condition": { "valueType": "immediate", "value": condition }
        })))
        .expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "condition": condition }));
    }

    #[test]
    fn agent_mapping_score_expression_keeps_column_refs() {
        // `fn` call arguments mix Object Model column refs (unqualified) and
        // workflow refs (qualified); only the latter resolve.
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "score_expression": { "valueType": "immediate", "value": {
                "alias": "sim",
                "expression": { "fn": "SIMILARITY", "arguments": [
                    { "valueType": "reference", "value": "commodity_title" },
                    { "valueType": "reference", "value": "data.customer_category" }
                ]}
            }}
        })))
        .expect("manifest");
        let source = build_source(br#"{"customer_category":"leather wallet"}"#, b"{}", b"{}")
            .expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(
            output["score_expression"]["expression"]["arguments"],
            json!([
                { "valueType": "reference", "value": "commodity_title" },
                { "valueType": "immediate", "value": "leather wallet" }
            ])
        );
    }

    #[test]
    fn agent_mapping_unwraps_top_level_ref_inside_immediate() {
        // A reference nested directly inside a top-level immediate gets
        // resolved + wrapped by the nested pass; the top-level unwrap strips
        // that single envelope so primitive-typed agent inputs see the bare
        // value (matches the generated compiler's
        // unwrap_top_level_immediate_envelopes).
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({
            "name": { "valueType": "immediate", "value": {
                "valueType": "reference", "value": "data.name"
            }}
        })))
        .expect("manifest");
        let source = build_source(br#"{"name":"Ada"}"#, b"{}", b"{}").expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "name": "Ada" }));
    }

    #[test]
    fn finish_mapping_leaves_nested_ref_envelopes_alone() {
        // The nested-reference pass is an agent-boundary behavior; other
        // mapping purposes pass immediate payloads through verbatim.
        let payload = json!({
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "id" },
                { "valueType": "reference", "value": "steps.create.outputs.id" }
            ]
        });
        let manifest = DirectJsonManifest::parse(&manifest(json!({
            "result": { "valueType": "immediate", "value": payload }
        })))
        .expect("manifest");
        let source =
            build_source(b"{}", b"{}", br#"{"create":{"outputs":{"id":"abc"}}}"#).expect("source");

        let output = manifest.apply_mapping(0, &source).expect("mapping output");
        let output: Value = serde_json::from_slice(&output).expect("output json");

        assert_eq!(output, json!({ "result": payload }));
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
    fn ai_turn_snapshot_round_trip_preserves_all_fields() {
        let state = br#"{"action":"tools","chat_history":[{"role":"assistant"}],"iterations":1}"#;
        let pending = br#"[{"tool_call_id":"call_1","content":"42"}]"#;

        let snapshot = DirectJsonManifest::ai_turn_snapshot(state, pending, 7, false)
            .expect("snapshot builds");

        let restored_state =
            DirectJsonManifest::ai_turn_snapshot_part(&snapshot, 0).expect("state part");
        assert_eq!(
            serde_json::from_slice::<Value>(&restored_state).unwrap(),
            serde_json::from_slice::<Value>(state).unwrap()
        );
        let restored_pending =
            DirectJsonManifest::ai_turn_snapshot_part(&snapshot, 1).expect("pending part");
        assert_eq!(
            serde_json::from_slice::<Value>(&restored_pending).unwrap(),
            serde_json::from_slice::<Value>(pending).unwrap()
        );
        assert_eq!(
            DirectJsonManifest::ai_turn_snapshot_tool_calls(&snapshot).unwrap(),
            7
        );
        assert!(!DirectJsonManifest::ai_turn_snapshot_complete(&snapshot).unwrap());

        let complete =
            DirectJsonManifest::ai_turn_snapshot(state, b"[]", 7, true).expect("complete snapshot");
        assert!(DirectJsonManifest::ai_turn_snapshot_complete(&complete).unwrap());
        assert!(DirectJsonManifest::ai_turn_snapshot_part(&snapshot, 2).is_err());
    }

    #[test]
    fn ai_turn_cache_key_scopes_loop_indices() {
        let plain = DirectJsonManifest::ai_turn_cache_key("ai", 3, br#"{"variables":{}}"#)
            .expect("plain key");
        assert_eq!(plain, "ai.turn.3");

        let scoped = DirectJsonManifest::ai_turn_cache_key(
            "ai",
            3,
            br#"{"variables":{"_loop_indices":[2,5]}}"#,
        )
        .expect("scoped key");
        assert!(
            scoped.starts_with("ai.turn.3") && scoped != plain,
            "loop-nested keys must differ from plain keys: {scoped}"
        );
    }

    /// The AiAgent loop's durable replay hinges on `ai-turn-cache-key` producing
    /// the *same* key across the original run and a resume. After interning, a
    /// large scope value is stored as a `$wfref` handle whose id is run-local — so
    /// if the key depended on the payload, an identical turn would key differently
    /// across runs and never replay. It must not: the key is built only from
    /// step id, iteration, and loop indices, all of which survive interning inline.
    /// This pins that interning is transparent to the durability key.
    #[test]
    fn ai_turn_cache_key_is_interning_transparent() {
        reset_value_store();
        let big = "x".repeat(64 * 1024); // > 16 KiB intern threshold
        let vars = br#"{"_loop_indices":[1,4]}"#;

        let source_big =
            build_source(format!(r#"{{"blob":"{big}"}}"#).as_bytes(), vars, b"{}").expect("big");
        let source_small = build_source(br#"{"blob":"x"}"#, vars, b"{}").expect("small");

        // The large value really interned (else the test proves nothing); the
        // small one stayed inline.
        let parsed_big: Value = serde_json::from_slice(&source_big).expect("parse big source");
        assert!(
            wfref_id(&parsed_big["data"]["blob"]).is_some(),
            "a >16 KiB scope value must intern to a $wfref handle"
        );
        let parsed_small: Value =
            serde_json::from_slice(&source_small).expect("parse small source");
        assert!(
            wfref_id(&parsed_small["data"]["blob"]).is_none(),
            "a tiny scope value must stay inline"
        );

        // Same turn, same loop indices → identical key, independent of whether the
        // scope payload was interned to a handle or left inline.
        let key_big = DirectJsonManifest::ai_turn_cache_key("ai", 2, &source_big).expect("key big");
        let key_small =
            DirectJsonManifest::ai_turn_cache_key("ai", 2, &source_small).expect("key small");
        assert_eq!(
            key_big, key_small,
            "durability key must not depend on interned payload size"
        );
        assert!(
            key_big.starts_with("ai.turn.2") && key_big != "ai.turn.2",
            "key carries step/iter and is scoped by loop indices: {key_big}"
        );
    }

    #[test]
    fn eval_condition_errors_on_query_only_operators() {
        // SIMILARITY_GTE / MATCH / COSINE_DISTANCE_LTE / L2_DISTANCE_LTE are
        // object-model query operators with no workflow-runtime evaluator.
        // They must error loudly (validation rejects them up front with E027;
        // this covers workflows compiled before that validation existed) —
        // never silently evaluate to false.
        for op in [
            "SIMILARITY_GTE",
            "MATCH",
            "COSINE_DISTANCE_LTE",
            "L2_DISTANCE_LTE",
        ] {
            let manifest = DirectJsonManifest::parse(&condition_manifest(json!({
                "type": "operation",
                "op": op,
                "arguments": [
                    { "valueType": "reference", "value": "data.text" },
                    { "valueType": "immediate", "value": "needle" },
                    { "valueType": "immediate", "value": 0.5 }
                ]
            })))
            .expect("manifest");
            let source = build_source(br#"{"text":"haystack"}"#, b"{}", b"{}").expect("source");

            let error = manifest
                .eval_condition(0, &source)
                .expect_err("query-only operator must error, not evaluate");
            assert!(
                error.contains(op) && error.contains("object-model"),
                "unexpected error for {op}: {error}"
            );
        }
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
        assert_eq!(
            steps["split"]["stats"],
            json!({
                "success": 1,
                "error": 1,
                "aborted": 0,
                "unknown": 0,
                "skipped": 0,
                "total": 2
            })
        );
        assert_eq!(steps["split"]["hasFailures"], json!(true));
        assert_eq!(steps["split"]["outputs"], json!([{ "id": 1 }]));
    }

    #[test]
    fn split_dont_stop_result_matches_split_output_and_flags_no_failures() {
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "dontStopOnFailed": true
        })))
        .expect("manifest");
        let source = build_source(br#"{"items":[1,2]}"#, b"{}", b"{}").expect("source");

        let results = manifest
            .split_initial_results(0)
            .expect("initial accumulator");
        let results = manifest
            .split_append_output(0, &results, br#"{"id":1}"#)
            .expect("success append");

        let result = manifest
            .split_result(0, &source, &results)
            .expect("Split result");
        let steps = manifest
            .split_output_from_result(0, &source, &result)
            .expect("durable Split steps context");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");
        let fresh = manifest
            .split_output(0, &source, &results)
            .expect("fresh Split steps context");
        let fresh: Value = serde_json::from_slice(&fresh).expect("steps json");

        assert_eq!(steps["split"], fresh["split"]);
        assert_eq!(steps["split"]["hasFailures"], json!(false));
        assert_eq!(steps["split"]["stats"]["error"], json!(0));
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

        for (step_type, step_id, collections, mut expected_inputs) in cases {
            // build_source injects the synthetic runtime-identity variables
            // `_instance_id`/`_tenant_id` (env unset in tests -> "unknown"), just
            // like the generated compiler. Mirror them into the expected inputs
            // so the comparison stays focused on the step inputs themselves. Only
            // the build_source-shaped envelope carries them (it has a `workflow`
            // key); a Split's breakpoint inputs are the raw step config, whose
            // `variables` field is not a build_source variables snapshot.
            if expected_inputs.get("workflow").is_some() {
                for path in ["/variables", "/workflow/inputs/variables"] {
                    if let Some(vars) = expected_inputs
                        .pointer_mut(path)
                        .and_then(Value::as_object_mut)
                    {
                        vars.insert("_instance_id".to_string(), json!("unknown"));
                        vars.insert("_tenant_id".to_string(), json!("unknown"));
                    }
                }
            }
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
    fn ai_summarize_input_carries_provider_and_state() {
        let base = json!({
            "provider": "openai",
            "model": "gpt-4o",
            "system_prompt": "be helpful",
            "chat_history": [],
        });
        let state = json!({ "chat_history": [{"i":0},{"i":1},{"i":2}], "iterations": 3 });
        let input = DirectJsonManifest::ai_summarize_input(
            &serde_json::to_vec(&base).unwrap(),
            &serde_json::to_vec(&state).unwrap(),
            2,
        )
        .expect("summarize input");
        let value: Value = serde_json::from_slice(&input).unwrap();
        assert_eq!(value["provider"], json!("openai"));
        assert_eq!(value["model"], json!("gpt-4o"));
        assert_eq!(value["max_messages"], json!(2));
        assert_eq!(value["state"]["iterations"], json!(3));

        // The capability returns `{state}`; ai_summarize_output unwraps it.
        let result = json!({ "state": { "chat_history": [{"summary":true}] } });
        let unwrapped =
            DirectJsonManifest::ai_summarize_output(&serde_json::to_vec(&result).unwrap())
                .expect("summarize output");
        let unwrapped_value: Value = serde_json::from_slice(&unwrapped).unwrap();
        assert_eq!(unwrapped_value["chat_history"], json!([{"summary":true}]));
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
    fn agent_scope_input_wraps_workflow_agent_child_envelope() {
        let manifest =
            DirectJsonManifest::parse(&agent_manifest_with_required_inputs(json!({}), json!([])))
                .expect("manifest");

        // Root invocation site: the scope derives from `_workflow_id`.
        let scoped = manifest
            .agent_scope_input(
                0,
                br#"{"message":"hi"}"#,
                br#"{"data":{},"variables":{"_workflow_id":"wf-1::inst-9"},"steps":{}}"#,
            )
            .expect("scoped input");
        let scoped: Value = serde_json::from_slice(&scoped).expect("scoped json");
        assert_eq!(scoped["data"], json!({ "message": "hi" }));
        assert_eq!(
            scoped["variables"],
            json!({ "_cache_key_prefix": "wf-1::inst-9::agent" })
        );

        // Nested site (this workflow itself running as a child): the inherited
        // prefix chains and loop indices append — the same compositional
        // formula as nested embeds, so composition depth is unbounded.
        let nested = manifest
            .agent_scope_input(
                0,
                b"{}",
                br#"{"variables":{"_cache_key_prefix":"wfP::call","_loop_indices":[2]}}"#,
            )
            .expect("nested scoped input");
        let nested: Value = serde_json::from_slice(&nested).expect("nested json");
        assert_eq!(
            nested["variables"]["_cache_key_prefix"],
            json!("wfP::call__agent[2]")
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
            .agent_connection_input(0, br#"{"value":"present"}"#, b"{}")
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
    fn agent_attempt_result_key_uses_distinct_namespace() {
        let base = "wf-42::agent::utils::normalize::agent::[1]";
        let attempt = DirectJsonManifest::agent_attempt_result_key(base, 3);
        assert_eq!(
            String::from_utf8(attempt).expect("utf8"),
            "wf-42::agent::utils::normalize::agent::[1]::attempt::3"
        );
        // Must never collide with the durable-sleep key or the audit key.
        let sleep = DirectJsonManifest::agent_retry_sleep_key(base, 3);
        assert_ne!(DirectJsonManifest::agent_attempt_result_key(base, 3), sleep);
        assert!(!String::from_utf8_lossy(&sleep).contains("::attempt::"));
    }

    #[test]
    fn agent_attempt_envelope_round_trips_header_and_payload() {
        let payload = br#"{"code":"HTTP_RATE_LIMITED","category":"transient","retryable":true}"#;
        let envelope = DirectJsonManifest::agent_attempt_envelope(
            1,    // tag = err
            true, // retryable
            true, // rate_limited
            true, // retry_after_tag
            1500, // retry_after_ms
            payload,
        );
        // Decode by the exact fixed offsets the emitter reads.
        assert_eq!(envelope[0], 1, "tag");
        assert_eq!(envelope[1], 1, "retryable");
        assert_eq!(envelope[2], 1, "rate_limited");
        assert_eq!(envelope[3], 1, "retry_after_tag");
        let retry_after_ms = u64::from_le_bytes(envelope[4..12].try_into().unwrap());
        assert_eq!(retry_after_ms, 1500);
        assert_eq!(&envelope[12..], payload);

        // No retry-after: tag byte 0, value 0, payload still recoverable.
        let envelope =
            DirectJsonManifest::agent_attempt_envelope(1, true, false, false, 0, payload);
        assert_eq!(envelope[3], 0, "retry_after_tag");
        assert_eq!(u64::from_le_bytes(envelope[4..12].try_into().unwrap()), 0);
        assert_eq!(&envelope[12..], payload);
        // Envelope is always non-empty even for an empty payload (leading header).
        assert!(
            !DirectJsonManifest::agent_attempt_envelope(1, false, false, false, 0, b"").is_empty()
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
    fn error_steps_recovers_structured_envelope_from_wrapped_agent_error() {
        // Agent failures arrive wrapped by `agent_error` as
        // `Step <id> failed: Agent <a>::<c>: {envelope}`. The onError context
        // must expose the structured fields, not just the wrapped text.
        let wrapped = br#"Step boom failed: Agent text::render-template: {"attributes":{"render_error":"invalid float literal"},"category":"permanent","code":"TEXT_TEMPLATE_RENDER_ERROR","message":"Template render error: invalid float literal","retryable":false,"severity":"error"}"#;
        let steps = error_steps("boom", wrapped, b"{}").expect("error steps");
        let steps: Value = serde_json::from_slice(&steps).expect("steps json");

        assert_eq!(
            steps["__error"]["code"],
            json!("TEXT_TEMPLATE_RENDER_ERROR")
        );
        assert_eq!(steps["__error"]["category"], json!("permanent"));
        assert_eq!(
            steps["__error"]["message"],
            json!("Template render error: invalid float literal")
        );
        assert_eq!(
            steps["__error"]["attributes"]["render_error"],
            json!("invalid float literal")
        );
        assert_eq!(steps["__error"]["stepId"], json!("boom"));
        assert_eq!(steps["error"], steps["__error"]);
    }

    #[test]
    fn build_source_mirrors_error_context_to_root_alias() {
        // onError dispatch injects the envelope at steps.__error / steps.error.
        let steps = error_steps(
            "agent",
            br#"{"code":"BAD","message":"boom","category":"permanent"}"#,
            b"{}",
        )
        .expect("error steps");

        let source = build_source(b"{}", b"{}", &steps).expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");

        // Canonical path remains intact.
        assert_eq!(source["steps"]["__error"]["message"], json!("boom"));
        // Back-compat bare-root aliases mirror the canonical envelope.
        assert_eq!(source["__error"], source["steps"]["__error"]);
        assert_eq!(source["error"], source["steps"]["__error"]);

        // A bare `__error.*` reference now resolves instead of falling to default.
        let resolved = apply_mapping_value(
            &json!({ "valueType": "reference", "value": "__error.message", "default": "fallback" }),
            &source,
        )
        .expect("resolve");
        assert_eq!(resolved, json!("boom"));
    }

    #[test]
    fn build_source_omits_error_alias_for_normal_steps() {
        let source =
            build_source(b"{}", b"{}", br#"{"prev":{"outputs":{"ok":true}}}"#).expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");
        assert!(source.get("__error").is_none());
        assert!(source.get("error").is_none());
    }

    #[test]
    fn build_source_unwraps_canonical_input_envelope() {
        // Regression: workflow inputs arrive as the canonical envelope
        // `{"data": {...}, "variables": {...}}` and are stored verbatim as the
        // instance input, so `data.*` references must resolve against the inner
        // `data` payload. Previously the whole envelope was used as `data`, so a
        // top-level `data.tpl` reference resolved to null (only the accidental
        // double-wrapped `data.data.tpl` path worked).
        let source = build_source(
            br#"{"data":{"tpl":"hello world"},"variables":{"_workflow_id":"wf1"}}"#,
            b"{}",
            b"{}",
        )
        .expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");
        assert_eq!(source["data"], json!({ "tpl": "hello world" }));
        let resolved = apply_mapping_value(
            &json!({ "valueType": "reference", "value": "data.tpl" }),
            &source,
        )
        .expect("resolve data.tpl");
        assert_eq!(resolved, json!("hello world"));

        // Bare data with no `data` key is used as-is (low-level/direct callers).
        let bare = build_source(br#"{"tpl":"bare"}"#, b"{}", b"{}").expect("bare source");
        let bare: Value = serde_json::from_slice(&bare).expect("bare json");
        assert_eq!(bare["data"], json!({ "tpl": "bare" }));
    }

    #[test]
    fn build_source_whitelists_cache_key_prefix_but_not_identity_vars() {
        // A parent invoking this workflow as a composed agent injects the
        // checkpoint-namespace through the input envelope. `_cache_key_prefix`
        // must survive the `_`-filter; the identity variables must NOT — a
        // caller can namespace its child's state but never spoof identity.
        let source = build_source(
            br#"{"data":{"x":1},"variables":{"_cache_key_prefix":"wfP::call[2]","_workflow_id":"spoofed","_tenant_id":"spoofed","plain":"ok"}}"#,
            br#"{"_workflow_id":"wf-real::inst-1"}"#,
            b"{}",
        )
        .expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");
        let vars = source["variables"].as_object().expect("variables");
        assert_eq!(vars["_cache_key_prefix"], json!("wfP::call[2]"));
        assert_eq!(
            vars["_workflow_id"],
            json!("wf-real::inst-1"),
            "identity variables must stay non-overridable from input"
        );
        assert_ne!(vars.get("_tenant_id"), Some(&json!("spoofed")));
        assert_eq!(vars["plain"], json!("ok"));
    }

    #[test]
    fn child_scoped_durable_keys_fold_the_cache_key_prefix() {
        // With `_cache_key_prefix` set (a child scope — embedded or composed),
        // every durable key builder prepends it; without it, keys stay
        // byte-identical to the legacy shapes (top-level workflows unaffected).
        let manifest =
            DirectJsonManifest::parse(&debug_manifest("Delay", "wait-step", None, json!({})))
                .expect("manifest");

        let plain = br#"{"data":{},"variables":{},"steps":{}}"#;
        let scoped = br#"{"data":{},"variables":{"_cache_key_prefix":"wfP::call[1]"},"steps":{}}"#;

        assert_eq!(
            manifest
                .delay_sleep_key("wait-step", plain)
                .expect("plain delay key"),
            "wait-step"
        );
        assert_eq!(
            manifest
                .delay_sleep_key("wait-step", scoped)
                .expect("scoped delay key"),
            "wfP::call[1]::wait-step"
        );
        assert_eq!(
            manifest
                .breakpoint_key("wait-step", scoped)
                .expect("scoped breakpoint key"),
            "wfP::call[1]::breakpoint::wait-step"
        );
        assert_eq!(
            DirectJsonManifest::ai_turn_cache_key("ai", 3, scoped).expect("scoped turn key"),
            "wfP::call[1]::ai.turn.3"
        );
        assert_eq!(
            DirectJsonManifest::ai_turn_cache_key("ai", 3, plain).expect("plain turn key"),
            "ai.turn.3"
        );
        // Retry/attempt keys derive from the (already-prefixed) base key.
        assert_eq!(
            DirectJsonManifest::agent_retry_sleep_key("wfP::call[1]::wait-step", 2),
            b"wfP::call[1]::wait-step::retry_sleep::2".to_vec()
        );
    }

    #[test]
    fn child_cache_prefix_matches_the_embed_formula() {
        // One shared definition for both child kinds: root level derives from
        // `_workflow_id`, nested levels chain with `__`, loop indices append.
        let root: Value = serde_json::from_str(r#"{"variables":{"_workflow_id":"wf-1::inst-9"}}"#)
            .expect("root source");
        assert_eq!(child_cache_prefix("call", &root), "wf-1::inst-9::call");

        let nested: Value = serde_json::from_str(
            r#"{"variables":{"_cache_key_prefix":"wf-1::inst-9::call","_loop_indices":[2,5]}}"#,
        )
        .expect("nested source");
        assert_eq!(
            child_cache_prefix("inner", &nested),
            "wf-1::inst-9::call__inner[2,5]"
        );

        let bare: Value = serde_json::from_str(r#"{"variables":{}}"#).expect("bare source");
        assert_eq!(child_cache_prefix("call", &bare), "root::call");
    }

    #[test]
    fn build_source_merges_runtime_variables_over_declared_defaults() {
        // The `variables` arg is the compile-time declared defaults (already
        // flattened to values). The canonical envelope's runtime `variables`
        // override those (runtime wins); declared-only variables keep their
        // default; `_`-prefixed identity vars are never overridable from input.
        let envelope =
            br#"{"data":{"x":1},"variables":{"greeting":"OVERRIDDEN","_workflow_id":"spoof"}}"#;
        let defaults = br#"{"greeting":"DEFAULT","mood":"happy","_workflow_id":"real"}"#;
        let source = build_source(envelope, defaults, b"{}").expect("source");
        let source: Value = serde_json::from_slice(&source).expect("source json");

        assert_eq!(source["data"], json!({ "x": 1 }));
        // Runtime override wins over the declared default.
        assert_eq!(source["variables"]["greeting"], "OVERRIDDEN");
        // Declared-only variable keeps its default.
        assert_eq!(source["variables"]["mood"], "happy");
        // `_`-prefixed identity vars are NOT overridable from runtime input.
        assert_eq!(source["variables"]["_workflow_id"], "real");

        let resolved = apply_mapping_value(
            &json!({ "valueType": "reference", "value": "variables.greeting" }),
            &source,
        )
        .expect("resolve variables.greeting");
        assert_eq!(resolved, json!("OVERRIDDEN"));
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
    fn ai_agent_debug_payloads_supported() {
        // Regression: a single-shot AiAgent step (`stepType: "AiAgent"`) must
        // build step-debug-start/end payloads like a regular Agent. These
        // previously hit the `other =>` arm and returned
        // `Err("...does not support step type 'AiAgent'")`; with track-events
        // enabled the emitter's debug-event guard turned that error into a silent
        // non-zero exit, so the instance was marked "crashed" with no diagnostic.
        let manifest_json = serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "agent",
                    "stepType": "AiAgent",
                    "purpose": "agent.inputMapping",
                    "value": { "user_prompt": { "valueType": "reference", "value": "data.value" } }
                }],
                "agents": [{
                    "id": 0,
                    "stepId": "agent",
                    "name": "Ask Model",
                    "stepType": "AiAgent",
                    "purpose": "agent.config",
                    "agentId": "ai-tools",
                    "capabilityId": "chat-completion",
                    "inputMappingId": 0,
                    "requiredInputs": [],
                    "connectionId": null
                }],
                "steps": [{
                    "id": "agent",
                    "stepType": "AiAgent",
                    "name": "Ask Model",
                    "body": { "id": "agent", "stepType": "AiAgent", "name": "Ask Model" }
                }]
            }
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest_json).expect("manifest");
        let source = build_source(br#"{"value":"in"}"#, b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("agent", &source)
            .expect("AiAgent debug start should be supported");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["inputs"], json!({ "user_prompt": "in" }));

        // Stored single-shot output (the chat-completion choice) feeds debug-end.
        let output = json!({ "choice": [{ "text": "Hello!" }] });
        let steps = manifest
            .ai_agent_output(0, &source, &serde_json::to_vec(&output).unwrap())
            .expect("AiAgent steps context");
        let source = build_source(br#"{"value":"in"}"#, b"{}", &steps).expect("source");

        let end = manifest
            .step_debug_end("agent", &source)
            .expect("AiAgent debug end should be supported");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["stepType"], json!("AiAgent"));
        assert_eq!(end["outputs"]["outputs"]["response"], json!("Hello!"));
    }

    #[test]
    fn ai_tool_debug_payloads_match_generated_shape() {
        // Tool calls dispatched by the AiAgent loop must surface as synthetic
        // `{step}.tool.{name}.{call}` steps of type AiAgentToolCall, matching
        // the generated compiler's per-tool-call debug events.
        let manifest_json = serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "agent",
                    "stepType": "AiAgent",
                    "purpose": "agent.inputMapping",
                    "value": {}
                }],
                "agents": [{
                    "id": 0,
                    "stepId": "agent",
                    "name": "Ask Model",
                    "stepType": "AiAgent",
                    "purpose": "agent.config",
                    "agentId": "ai-tools",
                    "capabilityId": "chat-turn",
                    "inputMappingId": 0,
                    "requiredInputs": [],
                    "connectionId": null
                }],
                "steps": [{
                    "id": "agent",
                    "stepType": "AiAgent",
                    "name": "Ask Model",
                    "body": { "id": "agent", "stepType": "AiAgent", "name": "Ask Model" }
                }]
            }
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest_json).expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");
        let turn_out = serde_json::to_vec(&json!({
            "action": "tools",
            "tool_calls": [{
                "tool_call_id": "call_1",
                "name": "echo",
                "tool_index": 0,
                "arguments": { "value": 42 }
            }]
        }))
        .expect("turn out");

        let start = manifest
            .ai_tool_debug_start(0, &turn_out, 0, 1, 0, &source)
            .expect("tool debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["step_id"], json!("agent.tool.echo.1"));
        assert_eq!(start["step_name"], json!("Tool: echo"));
        assert_eq!(start["step_type"], json!("AiAgentToolCall"));
        assert_eq!(
            start["inputs"],
            json!({
                "tool_name": "echo",
                "arguments": { "value": 42 },
                "iteration": 1,
                "call_number": 1
            })
        );

        let end = manifest
            .ai_tool_debug_end(0, &turn_out, 0, 1, 0, br#"{"echoed":42}"#, &source)
            .expect("tool debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["step_id"], json!("agent.tool.echo.1"));
        assert_eq!(end["step_type"], json!("AiAgentToolCall"));
        assert_eq!(end["outputs"]["stepId"], json!("agent.tool.echo.1"));
        assert_eq!(end["outputs"]["stepName"], json!("Tool: echo"));
        assert_eq!(end["outputs"]["stepType"], json!("AiAgentToolCall"));
        assert_eq!(
            end["outputs"]["outputs"],
            json!({
                "tool_name": "echo",
                "result": { "echoed": 42 },
                "iteration": 1,
                "call_number": 1
            })
        );
        assert!(end["duration_ms"].is_number());

        // A non-JSON tool result (raw bytes) is wrapped as a string.
        let end = manifest
            .ai_tool_debug_end(0, &turn_out, 0, 1, 0, b"plain text", &source)
            .expect("tool debug end with raw result");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["outputs"]["result"], json!("plain text"));
    }

    #[test]
    fn ai_memory_debug_payloads_match_generated_shape() {
        // The memory load/save/compaction phases must surface as synthetic
        // AiAgentMemory* steps with the generated compiler's payload shapes,
        // and the compaction phases must skip (empty payload) below the
        // threshold.
        let manifest_json = serde_json::to_vec(&json!({
            "graph": {
                "mappings": [{
                    "id": 0,
                    "stepId": "agent",
                    "stepType": "AiAgent",
                    "purpose": "agent.inputMapping",
                    "value": {}
                }],
                "agents": [{
                    "id": 0,
                    "stepId": "agent",
                    "name": "Ask Model",
                    "stepType": "AiAgent",
                    "purpose": "agent.config",
                    "agentId": "ai-tools",
                    "capabilityId": "chat-turn",
                    "inputMappingId": 0,
                    "requiredInputs": [],
                    "connectionId": null
                }],
                "steps": [{
                    "id": "agent",
                    "stepType": "AiAgent",
                    "name": "Ask Model",
                    "body": { "id": "agent", "stepType": "AiAgent", "name": "Ask Model" }
                }]
            }
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest_json).expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");
        let conversation = serde_json::to_vec(&json!({ "conversation_id": "conv-42" })).unwrap();
        let state = serde_json::to_vec(&json!({
            "chat_history": [
                { "role": "user", "content": [{ "type": "text", "text": "hi there" }] },
                { "role": "assistant", "content": [{ "id": "c1", "function": { "name": "echo", "arguments": {} } }] },
                { "role": "user", "content": [{ "type": "tool_result", "id": "c1", "content": [] }] }
            ],
            "iterations": 1,
            "tool_call_log": []
        }))
        .unwrap();
        let empty = b"{}".to_vec();

        // Load: start carries the conversation id; end carries count + previews.
        let start = manifest
            .ai_memory_debug_start(0, 0, &conversation, &empty, 0, &source)
            .expect("load debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["step_id"], json!("agent.memory_load"));
        assert_eq!(start["step_name"], json!("Memory: Load"));
        assert_eq!(start["step_type"], json!("AiAgentMemoryLoad"));
        assert_eq!(start["inputs"], json!({ "conversation_id": "conv-42" }));

        let end = manifest
            .ai_memory_debug_end(0, 0, &conversation, &state, &empty, 0, &source)
            .expect("load debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["success"], json!(true));
        assert_eq!(end["outputs"]["message_count"], json!(3));
        assert_eq!(
            end["outputs"]["messages"][0],
            json!({ "role": "user", "preview": "hi there" })
        );
        assert_eq!(
            end["outputs"]["messages"][1],
            json!({ "role": "assistant", "preview": "[tool_call:echo]" })
        );
        assert_eq!(
            end["outputs"]["messages"][2],
            json!({ "role": "user", "preview": "[tool_result:c1]" })
        );

        // Save: start carries the message count too.
        let start = manifest
            .ai_memory_debug_start(0, 1, &conversation, &state, 0, &source)
            .expect("save debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["step_id"], json!("agent.memory_save"));
        assert_eq!(start["step_type"], json!("AiAgentMemorySave"));
        assert_eq!(
            start["inputs"],
            json!({ "conversation_id": "conv-42", "message_count": 3 })
        );

        // Sliding-window compaction over the threshold: legacy id/name/fields.
        let compacted = serde_json::to_vec(&json!({
            "chat_history": [
                { "role": "user", "content": [{ "type": "text", "text": "hi there" }] }
            ]
        }))
        .unwrap();
        let start = manifest
            .ai_memory_debug_start(0, 2, &conversation, &state, 1, &source)
            .expect("compact debug start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["step_id"], json!("agent.memory.compact"));
        assert_eq!(start["step_name"], json!("Memory: Sliding Window"));
        assert_eq!(start["step_type"], json!("AiAgentMemoryCompaction"));
        assert_eq!(
            start["inputs"],
            json!({
                "strategy": "sliding_window",
                "messages_before": 3,
                "messages_to_drop": 2,
                "max_messages": 1,
                "conversation_id": "conv-42"
            })
        );
        let end = manifest
            .ai_memory_debug_end(0, 2, &conversation, &compacted, &state, 1, &source)
            .expect("compact debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["stepType"], json!("AiAgentMemoryCompaction"));
        assert_eq!(
            end["outputs"]["outputs"],
            json!({
                "strategy": "sliding_window",
                "success": true,
                "messages_before": 3,
                "messages_after": 1,
                "messages_dropped": 2
            })
        );

        // Below the threshold both compaction payloads are empty (event skipped).
        let skipped = manifest
            .ai_memory_debug_start(0, 2, &conversation, &state, 50, &source)
            .expect("below-threshold start");
        assert!(skipped.is_empty());
        let skipped = manifest
            .ai_memory_debug_end(0, 2, &conversation, &state, &state, 50, &source)
            .expect("below-threshold end");
        assert!(skipped.is_empty());

        // Summarize compaction surfaces the prepended summary message.
        let summarized = serde_json::to_vec(&json!({
            "chat_history": [
                { "role": "user", "content": [{ "type": "text", "text": "[Previous conversation summary]: user said hi" }] },
                { "role": "user", "content": [{ "type": "tool_result", "id": "c1", "content": [] }] }
            ]
        }))
        .unwrap();
        let end = manifest
            .ai_memory_debug_end(0, 3, &conversation, &summarized, &state, 2, &source)
            .expect("summarize debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["step_name"], json!("Memory: Summarize"));
        assert_eq!(
            end["outputs"]["outputs"],
            json!({
                "strategy": "summarize",
                "success": true,
                "messages_before": 3,
                "messages_after": 2,
                "messages_compacted": 1,
                "summary": "user said hi"
            })
        );
    }

    #[test]
    fn split_debug_payloads_supported() {
        // Regression: a Split step (`stepType: "Split"`) must build
        // step-debug-start/end payloads like the other step types. These
        // previously hit the `other =>` arm and returned
        // `Err("...does not support step type 'Split'")`; with track-events
        // enabled the emitter's debug-event guard turned that error into a
        // silent non-zero exit, so the instance failed the moment the Split was
        // due to start — no `step_debug_start` and no scope events were emitted.
        let manifest = DirectJsonManifest::parse(&split_manifest(json!({
            "value": { "valueType": "immediate", "value": [1, 2, 3] },
            "parallelism": 4
        })))
        .expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("split", &source)
            .expect("Split debug start should be supported");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["inputs"]["value"], json!([1, 2, 3]));
        assert_eq!(start["inputs"]["parallelism"], json!(4));

        // The Split result is recorded in the steps context before debug-end runs.
        let steps = manifest
            .split_output(0, &source, br#"[{"ok":true}]"#)
            .expect("Split steps context");
        let source = build_source(b"{}", b"{}", &steps).expect("source");

        let end = manifest
            .step_debug_end("split", &source)
            .expect("Split debug end should be supported");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["stepType"], json!("Split"));
        assert_eq!(end["outputs"]["outputs"], json!([{ "ok": true }]));
    }

    #[test]
    fn filter_debug_payloads_are_bounded() {
        // Regression: a Filter over a large collection used to store the entire
        // resolved array in the step-debug-start `inputs` (and the filtered
        // result in the end `outputs`) verbatim — multi-MB per event, GB-scale
        // across a loop body, which timed out the step-summaries query. Filter
        // (and the other non-Split arms) must be bounded like Split's inputs.
        let big: Vec<i64> = (0..1000).collect();
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "Filter",
            "filter",
            None,
            json!({
                "filters": [{
                    "id": 0,
                    "stepId": "filter",
                    "name": "Filter Items",
                    "stepType": "Filter",
                    "purpose": "filter.config",
                    "value": {
                        "value": { "valueType": "immediate", "value": big },
                        "condition": {
                            "type": "operation",
                            "op": "GT",
                            "arguments": [
                                { "valueType": "reference", "value": "item" },
                                { "valueType": "immediate", "value": 0 }
                            ]
                        }
                    }
                }]
            }),
        ))
        .expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("filter", &source)
            .expect("Filter debug start should be supported");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        // The 1000-item input array is replaced by a truncation stub, not stored.
        assert_eq!(start["inputs"]["_truncated"], json!(true));
        assert_eq!(start["inputs"]["_type"], json!("array"));
        assert_eq!(start["inputs"]["_length"], json!(1000));
        assert!(
            start["inputs"].to_string().len() < 1024,
            "bounded Filter inputs must be small, got {} bytes",
            start["inputs"].to_string().len()
        );

        // Output side: bound the heavy nested `outputs` while preserving the
        // envelope keys and the `_error` flag the status query reads.
        let big_output = json!({
            "stepId": "filter",
            "stepName": "Filter Items",
            "stepType": "Filter",
            "outputs": (0..1000).collect::<Vec<i64>>(),
        });
        let bounded = bound_debug_output(big_output);
        assert_eq!(bounded["stepType"], json!("Filter"));
        assert_eq!(bounded["outputs"]["_truncated"], json!(true));
        assert_eq!(bounded["outputs"]["_length"], json!(1000));
        // A small error envelope is left intact so `_error` survives for status.
        let err = json!({ "_error": true, "message": "boom" });
        assert_eq!(bound_debug_output(err.clone()), err);
    }

    #[test]
    fn compiled_condition_matches_interpreter() {
        // Differential oracle: the compiled condition must produce the SAME
        // Result (incl. Err message) as the JSON interpreter across every
        // operator + edge case. This guards the full-replacement cutover.
        fn parity(expr: Value, source: Value) {
            let interp = eval_condition_expression(&expr, &source);
            let compiled = compile_condition(&expr).eval(&source);
            assert_eq!(
                interp, compiled,
                "compiled/interpreted mismatch\n  expr={expr}\n  source={source}"
            );
        }
        let src = json!({
            "data": { "sku": "S5", "n": 5, "tags": ["a", "b"], "name": "widget", "flag": true },
            "item": { "sku": "S5", "n": 7, "s": "" },
            "variables": {},
            "steps": {}
        });
        let r = |p: &str| json!({ "valueType": "reference", "value": p });
        let i = |v: Value| json!({ "valueType": "immediate", "value": v });
        let op = |o: &str, a: Value| json!({ "type": "operation", "op": o, "arguments": a });

        // equality / inequality, with string<->number coercion + refs
        parity(op("EQ", json!([r("item.sku"), r("data.sku")])), src.clone());
        parity(
            op("EQ", json!([r("item.sku"), i(json!("S6"))])),
            src.clone(),
        );
        parity(op("NE", json!([r("item.n"), r("data.n")])), src.clone());
        parity(op("EQ", json!([r("item.n"), i(json!("7"))])), src.clone()); // coercion
        // comparisons (numeric + arity underflow)
        for o in ["GT", "GTE", "LT", "LTE"] {
            parity(op(o, json!([r("item.n"), r("data.n")])), src.clone());
            parity(op(o, json!([r("item.n")])), src.clone()); // <2 args -> false
        }
        // boolean combinators + short-circuit + nesting + missing NOT arg
        parity(
            op(
                "AND",
                json!([
                    op("EQ", json!([r("item.sku"), r("data.sku")])),
                    op("GT", json!([r("item.n"), i(json!(3))]))
                ]),
            ),
            src.clone(),
        );
        parity(
            op(
                "OR",
                json!([
                    op("EQ", json!([r("item.sku"), i(json!("x"))])),
                    i(json!(true))
                ]),
            ),
            src.clone(),
        );
        parity(
            op(
                "NOT",
                json!([op("EQ", json!([r("item.sku"), r("data.sku")]))]),
            ),
            src.clone(),
        );
        parity(op("NOT", json!([])), src.clone()); // missing arg -> true
        // string match
        parity(
            op("STARTS_WITH", json!([r("data.name"), i(json!("wid"))])),
            src.clone(),
        );
        parity(
            op("ENDS_WITH", json!([r("data.name"), i(json!("get"))])),
            src.clone(),
        );
        parity(
            op("STARTS_WITH", json!([r("data.n"), i(json!("5"))])),
            src.clone(),
        ); // non-string -> false
        // array match
        parity(
            op("CONTAINS", json!([r("data.tags"), i(json!("a"))])),
            src.clone(),
        );
        parity(
            op("IN", json!([i(json!("b")), r("data.tags")])),
            src.clone(),
        );
        parity(
            op("NOT_IN", json!([i(json!("z")), r("data.tags")])),
            src.clone(),
        );
        // length (as condition + nested as value)
        parity(
            op(
                "GT",
                json!([op("LENGTH", json!([r("data.tags")])), i(json!(1))]),
            ),
            src.clone(),
        );
        parity(op("LENGTH", json!([r("data.tags")])), src.clone());
        parity(op("LENGTH", json!([])), src.clone());
        // existence / emptiness (present + missing arg defaults)
        parity(op("IS_DEFINED", json!([r("data.sku")])), src.clone());
        parity(op("IS_DEFINED", json!([r("data.missing")])), src.clone());
        parity(op("IS_DEFINED", json!([])), src.clone());
        parity(op("IS_EMPTY", json!([r("item.s")])), src.clone());
        parity(op("IS_EMPTY", json!([r("data.tags")])), src.clone());
        parity(op("IS_EMPTY", json!([])), src.clone());
        parity(op("IS_NOT_EMPTY", json!([r("data.tags")])), src.clone());
        parity(op("IS_NOT_EMPTY", json!([])), src.clone());
        // truthiness of a non-operation value node
        parity(r("data.flag"), src.clone());
        parity(i(json!(0)), src.clone());
        parity(
            json!({ "type": "value", "valueType": "reference", "value": "data.name" }),
            src.clone(),
        );
        // error parity: query-only, unknown op, malformed
        parity(
            op("SIMILARITY_GTE", json!([r("item.sku"), i(json!(0.5))])),
            src.clone(),
        );
        parity(op("MATCH", json!([])), src.clone());
        parity(op("BOGUS_OP", json!([])), src.clone());
        parity(json!({ "type": "operation" }), src.clone()); // missing op
        parity(json!({ "op": "EQ" }), src.clone()); // missing arguments
        // reference defaults + type hints
        parity(
            json!({ "valueType": "reference", "value": "data.missing", "default": true }),
            src.clone(),
        );
        parity(
            json!({ "valueType": "reference", "value": "data.n", "type": "string" }),
            src.clone(),
        );
    }

    #[test]
    fn compiled_input_mapping_matches_interpreter() {
        // Differential oracle for mappings: compile_input_mapping().eval() must
        // equal apply_input_mapping() (incl. Err), across reference/immediate/
        // composite/template values, dotted keys, defaults, type hints, and the
        // not-an-object error.
        fn parity(mapping: Value, source: Value) {
            let interp = apply_input_mapping(&mapping, &source);
            let compiled = compile_input_mapping(&mapping).eval(&source);
            assert_eq!(
                interp, compiled,
                "compiled/interpreted mapping mismatch\n  mapping={mapping}\n  source={source}"
            );
        }
        let src = json!({
            "data": { "sku": "S5", "n": 5, "nested": { "x": 1 }, "list": [10, 20] },
            "variables": { "mode": "live" },
            "steps": { "prev": { "outputs": { "ok": true } } }
        });
        parity(
            json!({
                "sku": { "valueType": "reference", "value": "data.sku" },
                "count": { "valueType": "reference", "value": "data.n", "type": "string" },
                "missing": { "valueType": "reference", "value": "data.none", "default": "fallback" },
                "lit": { "valueType": "immediate", "value": 42 },
                "obj": { "valueType": "composite", "value": {
                    "a": { "valueType": "reference", "value": "data.nested.x" },
                    "b": { "valueType": "immediate", "value": [1, 2] }
                }},
                "arr": { "valueType": "composite", "value": [
                    { "valueType": "reference", "value": "data.list[0]" },
                    { "valueType": "immediate", "value": "x" }
                ]},
                "tmpl": { "valueType": "template", "value": "mode={{ variables.mode }}" },
                "nested.key": { "valueType": "reference", "value": "steps.prev.outputs.ok" }
            }),
            src.clone(),
        );
        // not-an-object error parity
        parity(json!("not an object"), src.clone());
        // empty mapping
        parity(json!({}), src);
    }

    #[test]
    fn filter_debug_end_reads_stored_output_without_recomputing() {
        // After execution the Filter output is persisted at steps.find and the
        // emitter rebuilds source before debug-end, so debug-end must return that
        // stored envelope rather than re-running apply_filter (the recompute that
        // doubled collection-heavy steps under track-events). Pre-populate
        // steps.find with a SENTINEL output apply_filter could never produce.
        let manifest = DirectJsonManifest::parse(&debug_manifest(
            "Filter",
            "find",
            None,
            json!({
                "filters": [{
                    "id": 0, "stepId": "find", "name": "find", "stepType": "Filter",
                    "purpose": "filter.config",
                    "value": {
                        "value": { "valueType": "immediate", "value": [{"sku":"A"},{"sku":"B"}] },
                        "condition": { "type": "operation", "op": "EQ", "arguments": [
                            { "valueType": "reference", "value": "item.sku" },
                            { "valueType": "immediate", "value": "A" }
                        ]}
                    }
                }]
            }),
        ))
        .expect("manifest");
        let steps = serde_json::to_vec(&json!({
            "find": { "stepId": "find", "stepName": "find", "stepType": "Filter",
                      "outputs": { "items": [{"sentinel": true}], "count": 99 } }
        }))
        .unwrap();
        let src_bytes = build_source(b"{}", b"{}", &steps).unwrap();
        let end = manifest
            .step_debug_end("find", &src_bytes)
            .expect("debug end");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(
            end["outputs"]["outputs"]["count"],
            json!(99),
            "debug-end must read the stored output, not recompute the filter"
        );
        assert_eq!(
            end["outputs"]["outputs"]["items"][0]["sentinel"],
            json!(true)
        );
    }

    #[test]
    #[ignore = "perf micro-benchmark; run with --ignored --nocapture"]
    fn filter_perf_breakdown() {
        use std::time::Instant;
        let cfg = json!({
            "value": { "valueType": "reference", "value": "data.items" },
            "condition": { "type": "operation", "op": "EQ", "arguments": [
                { "valueType": "reference", "value": "item.sku" },
                { "valueType": "reference", "value": "data.sku" }
            ]}
        });
        eprintln!(
            "\n{:>6} {:>10} {:>10} {:>10} {:>12} {:>10}",
            "M", "dbg_start", "filter", "dbg_end", "total/call", "per_item"
        );
        for m in [500usize, 1000, 2000, 4000, 8000] {
            let items: Vec<Value> = (0..m)
                .map(|i| json!({"sku": format!("S{i}"), "q": i}))
                .collect();
            let data = serde_json::to_vec(&json!({"items": items, "sku": "Sx"})).unwrap();
            let manifest = DirectJsonManifest::parse(&debug_manifest(
                "Filter",
                "find",
                None,
                json!({
                    "filters": [{ "id": 0, "stepId": "find", "name": "find", "stepType": "Filter",
                        "purpose": "filter.config", "value": cfg }]
                }),
            ))
            .unwrap();
            let src_bytes = build_source(&data, b"{}", b"{}").unwrap();
            let src: Value = serde_json::from_slice(&src_bytes).unwrap();
            let _ = apply_filter(&cfg, &src); // warm intern arena
            let t = Instant::now();
            manifest.step_debug_start("find", &src_bytes).unwrap();
            let ds = t.elapsed();
            let t = Instant::now();
            apply_filter(&cfg, &src).unwrap();
            let df = t.elapsed();
            let t = Instant::now();
            manifest.step_debug_end("find", &src_bytes).unwrap();
            let de = t.elapsed();
            let tot = ds + df + de;
            eprintln!(
                "{:>6} {:>10.2?} {:>10.2?} {:>10.2?} {:>12.2?} {:>8.1}us",
                m,
                ds,
                df,
                de,
                tot,
                tot.as_micros() as f64 / m as f64
            );
        }
    }

    #[test]
    fn while_debug_payloads_supported() {
        // Regression: a While step has the same debug-event gap as Split — its
        // `step_debug_start`/`step_debug_end` must not fall through to the
        // `other =>` arm (which would silently crash a track-events run).
        let manifest = DirectJsonManifest::parse(&while_manifest(
            json!({ "maxIterations": 5 }),
            json!({ "valueType": "immediate", "value": false }),
        ))
        .expect("manifest");
        let source = build_source(b"{}", b"{}", b"{}").expect("source");

        let start = manifest
            .step_debug_start("loop", &source)
            .expect("While debug start should be supported");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["inputs"]["maxIterations"], json!(5));

        let steps = manifest
            .while_output(0, &source, br#"{"index":2,"outputs":[{"ok":true}]}"#)
            .expect("While steps context");
        let source = build_source(b"{}", b"{}", &steps).expect("source");

        let end = manifest
            .step_debug_end("loop", &source)
            .expect("While debug end should be supported");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["outputs"]["stepType"], json!("While"));
        assert_eq!(end["outputs"]["outputs"]["iterations"], json!(2));
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
            .agent_debug_error(0, &source, b"Step agent failed")
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
    fn debug_events_carry_iteration_scope_from_source_variables() {
        // Regression: step-debug start/end (and the failed-agent debug-end) must
        // surface the per-iteration `_scope_id` / `_loop_indices` that
        // split/while set in the variables. They used to be hardcoded
        // null/[], so parallel Split iterations of the same step were
        // indistinguishable and the paired-summary query cross-produced them.
        let manifest = DirectJsonManifest::parse(&agent_manifest(json!({}))).expect("manifest");
        let variables =
            br#"{"_scope_id":"sc_split_2","_parent_scope_id":"sc_split","_loop_indices":[2]}"#;
        let source = build_source(b"{}", variables, b"{}").expect("source");

        let start = manifest.step_debug_start("agent", &source).expect("start");
        let start: Value = serde_json::from_slice(&start).expect("start json");
        assert_eq!(start["scope_id"], json!("sc_split_2"));
        assert_eq!(start["parent_scope_id"], json!("sc_split"));
        assert_eq!(start["loop_indices"], json!([2]));

        // The failed-agent debug-end goes through agent_debug_error, which now
        // takes the same source so its scope matches the start (otherwise a
        // failed agent inside an iteration can't be paired with its start).
        let end = manifest
            .agent_debug_error(0, &source, b"boom")
            .expect("agent debug error");
        let end: Value = serde_json::from_slice(&end).expect("end json");
        assert_eq!(end["scope_id"], json!("sc_split_2"));
        assert_eq!(end["loop_indices"], json!([2]));

        // A root-scope step (no iteration variables) stays null/[] as before.
        let root = build_source(b"{}", b"{}", b"{}").expect("root source");
        let root_start = manifest
            .step_debug_start("agent", &root)
            .expect("root start");
        let root_start: Value = serde_json::from_slice(&root_start).expect("root json");
        assert_eq!(root_start["scope_id"], Value::Null);
        assert_eq!(root_start["loop_indices"], json!([]));
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

#[cfg(test)]
mod invoke_error_and_delay_key_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invoke_error_fields_decomposes_structured_envelopes() {
        let envelope = json!({
            "code": "HTTP_TIMEOUT",
            "message": "upstream timed out",
            "category": "transient",
            "severity": "error",
            "retryable": true,
            "retryAfterMs": 1500,
            "attributes": {"host": "api.example.com"}
        });
        let fields = invoke_error_fields(envelope.to_string().as_bytes());
        assert_eq!(fields.code, "HTTP_TIMEOUT");
        assert_eq!(fields.message, "upstream timed out");
        assert_eq!(fields.category, "transient");
        assert_eq!(fields.severity, "error");
        assert!(fields.retryable);
        assert_eq!(fields.retry_after_ms, Some(1500));
        assert_eq!(
            fields.attributes.as_deref(),
            Some(r#"{"host":"api.example.com"}"#)
        );
    }

    #[test]
    fn invoke_error_fields_rides_raw_bytes_as_message() {
        for raw in [
            "plain failure text",
            "[1,2,3]",
            "\"just a json string\"",
            "{not json",
        ] {
            let fields = invoke_error_fields(raw.as_bytes());
            assert_eq!(fields.message, raw, "raw payload must ride message");
            assert_eq!(fields.code, "");
            assert!(!fields.retryable);
            assert_eq!(fields.retry_after_ms, None);
            assert_eq!(fields.attributes, None);
        }
        // An object without a message string surfaces the whole envelope.
        let fields = invoke_error_fields(br#"{"code":"X"}"#);
        assert_eq!(fields.code, "X");
        assert_eq!(fields.message, r#"{"code":"X"}"#);
    }

    #[test]
    fn delay_sleep_key_folds_loop_indices_and_keeps_top_level_bare() {
        let manifest_bytes = serde_json::to_vec(&json!({
            "graph": {
                "steps": [{
                    "id": "tick",
                    "stepType": "Delay",
                    "name": "Tick"
                }]
            }
        }))
        .expect("manifest json");
        let manifest = DirectJsonManifest::parse(&manifest_bytes).expect("manifest parses");

        // Top level: byte-identical to the legacy static key.
        let top = manifest
            .delay_sleep_key("tick", br#"{"data":{},"variables":{}}"#)
            .expect("top-level key");
        assert_eq!(top, "tick");

        // Inside a Split/While iteration: the indices fold in.
        let nested = manifest
            .delay_sleep_key(
                "tick",
                br#"{"data":{},"variables":{"_loop_indices":[2,0]}}"#,
            )
            .expect("nested key");
        assert_eq!(nested, "tick::2_0");

        // Unknown steps fail loudly.
        assert!(manifest.delay_sleep_key("nope", b"{}").is_err());
    }
}
