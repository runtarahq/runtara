// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-step-type output shapes.
//!
//! Declares what each step type writes into `steps.<id>` in the runtime scope —
//! the piece the authoring schema historically omitted, which forced authors to
//! learn output shapes from failed runs (e.g. discovering that a Split's
//! `outputs` is the bare collected array, not an object with a `result` field).
//! Two consumers read this table:
//!
//! * the DSL authoring schema (`spec::dsl_schema`), which now surfaces an
//!   `outputShape` alongside each step type's input `schema`, and
//! * reference validation (`runtara-workflows`), which uses it to reject a
//!   mistyped output tail (e.g. `steps.split.outputs.result`) at preflight
//!   instead of letting it resolve to null at runtime.
//!
//! GROUND TRUTH is the stdlib emitter (`runtara-workflow-stdlib::direct_json`):
//! `step_output_envelope` / `split_result` / `split_dont_stop_result` /
//! `while_output` / `filter` / `value_switch` / `group_by`. Keep this table in
//! sync with those helpers — the `output_shape_covers_all_step_types` test
//! guards completeness against the registered step types.
//!
//! Deliberately NOT gated behind `json-schema`: the WASM validator (which can
//! build `runtara-dsl` with `default-features = false`) needs the preflight
//! lookup, and the data here is plain `&'static str` with no `schemars`
//! dependency.

use serde_json::{Value, json};

/// Shape of the value at `steps.<id>.outputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputsShape {
    /// A homogeneous array — address elements by numeric index only. A named-key
    /// tail such as `.result` is never valid.
    Array,
    /// A fixed object with this closed set of top-level fields.
    Object(&'static [&'static str]),
    /// Shape depends on runtime data or config (agent responses, Finish outputs,
    /// GroupBy/Switch results). Not statically checkable — never flagged.
    Dynamic,
}

/// Static description of what a step writes under `steps.<id>`.
#[derive(Debug, Clone, Copy)]
pub struct StepOutputShape {
    /// One-line human summary for the authoring schema.
    pub summary: &'static str,
    /// Shape at `steps.<id>.outputs`.
    pub outputs: OutputsShape,
    /// Sibling fields the step also writes under `steps.<id>` beyond `outputs`
    /// (e.g. Split's `data` / `stats` / `hasFailures`, Switch's `route`). Always
    /// permitted references even when their presence is config-gated, so the
    /// preflight check never flags them.
    pub siblings: &'static [&'static str],
}

/// Look up the output shape for a PascalCase step type id (matches
/// `agent_meta::StepTypeMeta::id` and `Step` variant names). Returns `None` for
/// an unknown step type.
pub fn step_output_shape(step_type: &str) -> Option<StepOutputShape> {
    let shape = match step_type {
        "Split" => StepOutputShape {
            summary: "`outputs` is the array of successful per-item subgraph outputs (the collected results). With `config.dontStopOnFailed` the step also exposes `data.{success,error,aborted,unknown,skipped}`, `stats.{...,total}`, and `hasFailures`.",
            outputs: OutputsShape::Array,
            siblings: &["data", "stats", "hasFailures"],
        },
        "Filter" => StepOutputShape {
            summary: "`outputs` is `{items: <filtered array>, count: <number kept>}` — the input array narrowed to the items that matched the condition, plus their count.",
            outputs: OutputsShape::Object(&["items", "count"]),
            siblings: &[],
        },
        "While" => StepOutputShape {
            summary: "`outputs` is `{iterations: <count>, outputs: <last iteration's output>}`.",
            outputs: OutputsShape::Object(&["iterations", "outputs"]),
            siblings: &[],
        },
        "Conditional" => StepOutputShape {
            summary: "`outputs` is `{result: <bool>}` — the evaluated branch decision (materialized under track-events; used for edge routing otherwise).",
            outputs: OutputsShape::Object(&["result"]),
            siblings: &[],
        },
        "Switch" => StepOutputShape {
            summary: "`outputs` is the matched case's output value (shape depends on the case). Routing switches also expose `route`, the matched case label.",
            outputs: OutputsShape::Dynamic,
            siblings: &["route"],
        },
        "GroupBy" => StepOutputShape {
            summary: "`outputs` is `{groups: {<groupKey>: [items...]}, counts: {<groupKey>: <n>}, total_groups: <n>}`.",
            outputs: OutputsShape::Object(&["groups", "counts", "total_groups"]),
            siblings: &[],
        },
        "Agent" => StepOutputShape {
            summary: "`outputs` is the agent capability's response payload — consult the capability's output schema (get_capability / get_output_schema).",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "AiAgent" => StepOutputShape {
            summary: "`outputs` is the AI agent's result (text or structured response; the tool-call trace appears in step events). Shape depends on the configured response format.",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "EmbedWorkflow" => StepOutputShape {
            summary: "`outputs` is the child workflow's Finish outputs — consult the child's output schema.",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "WaitForSignal" => StepOutputShape {
            summary: "`outputs` is the payload delivered when the awaited signal arrives (shape defined by the signal schema).",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "Finish" => StepOutputShape {
            summary: "Terminal step: defines the workflow's own outputs. It is not referenced via `steps.<id>`.",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "Log" => StepOutputShape {
            summary: "Diagnostic only: emits a log/debug event and writes no referenceable `outputs`.",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "Delay" => StepOutputShape {
            summary: "Pauses execution for a fixed duration; writes no referenceable `outputs`.",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        "Error" => StepOutputShape {
            summary: "Terminates the workflow with a structured error; writes no referenceable `outputs`.",
            outputs: OutputsShape::Dynamic,
            siblings: &[],
        },
        _ => return None,
    };
    Some(shape)
}

/// Render a step type's output shape as JSON for the authoring schema (the
/// `outputShape` key on each step type). Returns `null` for a step type with no
/// declared shape (e.g. the virtual `Start` step).
pub fn output_shape_json(step_type: &str) -> Value {
    let Some(shape) = step_output_shape(step_type) else {
        return Value::Null;
    };
    let outputs = match shape.outputs {
        OutputsShape::Array => json!({
            "kind": "array",
            "note": "address elements by numeric index (e.g. `steps.<id>.outputs.0`); a named-key tail like `.result` is invalid and rejected at preflight",
        }),
        OutputsShape::Object(fields) => json!({ "kind": "object", "fields": fields }),
        OutputsShape::Dynamic => json!({
            "kind": "dynamic",
            "note": "shape depends on runtime data or config and is not statically validated",
        }),
    };
    json!({
        "summary": shape.summary,
        "reference": "steps.<id>.outputs",
        "outputs": outputs,
        "siblingFields": shape.siblings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registered step type must declare an output shape, so the authoring
    /// schema never silently omits one (the gap the reporter hit). This list
    /// mirrors `step_registration::STEP_TYPES`; a new step type added there must
    /// be added here too, which this test forces.
    const ALL_STEP_TYPES: &[&str] = &[
        "Finish",
        "Agent",
        "Conditional",
        "Split",
        "Switch",
        "EmbedWorkflow",
        "While",
        "Log",
        "Error",
        "Filter",
        "GroupBy",
        "WaitForSignal",
        "AiAgent",
        "Delay",
    ];

    #[test]
    fn output_shape_covers_all_step_types() {
        for step_type in ALL_STEP_TYPES {
            assert!(
                step_output_shape(step_type).is_some(),
                "missing output shape declaration for step type '{step_type}'"
            );
            assert!(
                !output_shape_json(step_type).is_null(),
                "output_shape_json returned null for '{step_type}'"
            );
        }
        assert!(step_output_shape("NotAStep").is_none());
        assert!(output_shape_json("NotAStep").is_null());
    }

    /// Guards the invariants the preflight check depends on, verified against the
    /// stdlib emitters (`direct_json`): Split emits the bare collected ARRAY at
    /// `outputs`; Filter/While/GroupBy/Conditional emit closed OBJECTS. Getting
    /// these wrong makes the preflight reject valid references, so they are
    /// pinned here.
    #[test]
    fn control_flow_output_invariants() {
        // Split default accumulator is a bare array (direct_json `split_result`
        // → `step_output_envelope(.., results, ..)` with `results` a `Vec`).
        assert_eq!(
            step_output_shape("Split").unwrap().outputs,
            OutputsShape::Array
        );
        // `apply_filter_compiled` returns `{items, count}` — an object, NOT a
        // bare array. `steps.filter.outputs.items` must stay valid.
        assert_eq!(
            step_output_shape("Filter").unwrap().outputs,
            OutputsShape::Object(&["items", "count"])
        );
        assert_eq!(
            step_output_shape("While").unwrap().outputs,
            OutputsShape::Object(&["iterations", "outputs"])
        );
        assert_eq!(
            step_output_shape("GroupBy").unwrap().outputs,
            OutputsShape::Object(&["groups", "counts", "total_groups"])
        );
        assert_eq!(
            step_output_shape("Conditional").unwrap().outputs,
            OutputsShape::Object(&["result"])
        );
        // Split's failure buckets are siblings, not under `outputs`.
        assert!(
            step_output_shape("Split")
                .unwrap()
                .siblings
                .contains(&"data")
        );
    }
}
