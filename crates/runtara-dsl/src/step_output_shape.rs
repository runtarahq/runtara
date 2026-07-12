// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-step-type output shapes.
//!
//! Declares what each step type writes into `steps.<id>` in the runtime scope —
//! the piece the authoring schema historically omitted, which forced authors to
//! learn output shapes from failed runs (e.g. discovering that a Split's
//! `outputs` is the bare collected array, not an object with a `result` field).
//! Three consumers read this table:
//!
//! * the DSL authoring schema (`spec::dsl_schema`), which surfaces an
//!   `outputShape` alongside each step type's input `schema`,
//! * reference validation (`runtara-workflows`), which uses it to reject a
//!   mistyped output tail (e.g. `steps.split.outputs.result`) at preflight
//!   instead of letting it resolve to null at runtime, and
//! * the workflow editor's reference suggestions / step output panel, which
//!   consume the JSON form via the browser validation WASM.
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

/// A named field with a static JSON type, used for closed `outputs` objects and
/// for sibling fields under `steps.<id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeField {
    pub name: &'static str,
    /// JSON type of the field: `"string" | "number" | "integer" | "boolean" |
    /// "array" | "object" | "dynamic"` (dynamic = depends on runtime data or
    /// config, e.g. a While loop's last Finish outputs).
    pub ty: &'static str,
    /// One-line human summary for authoring surfaces (editor panels, MCP).
    pub description: &'static str,
    /// Step config key (camelCase, as authored) that must be truthy for the
    /// runtime to actually write this field (e.g. Split's failure siblings
    /// exist only with `config.dontStopOnFailed`). Referencing the field is
    /// always *valid*; suggestion surfaces should not offer it when the gate
    /// is off, or it resolves to null.
    pub gated_by: Option<&'static str>,
}

const fn field(name: &'static str, ty: &'static str, description: &'static str) -> ShapeField {
    ShapeField {
        name,
        ty,
        description,
        gated_by: None,
    }
}

const fn gated_field(
    name: &'static str,
    ty: &'static str,
    description: &'static str,
    gated_by: &'static str,
) -> ShapeField {
    ShapeField {
        name,
        ty,
        description,
        gated_by: Some(gated_by),
    }
}

/// Shape of the value at `steps.<id>.outputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputsShape {
    /// A homogeneous array — address elements by numeric index only. A named-key
    /// tail such as `.result` is never valid.
    Array,
    /// A fixed object with this closed set of top-level fields.
    Object(&'static [ShapeField]),
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
    pub siblings: &'static [ShapeField],
}

const SPLIT_SIBLINGS: &[ShapeField] = &[
    gated_field(
        "data",
        "object",
        "Per-outcome item buckets {success, error, aborted, unknown, skipped}; populated with config.dontStopOnFailed",
        "dontStopOnFailed",
    ),
    gated_field(
        "stats",
        "object",
        "Per-outcome counts plus total",
        "dontStopOnFailed",
    ),
    gated_field(
        "hasFailures",
        "boolean",
        "True when at least one iteration failed (with config.dontStopOnFailed)",
        "dontStopOnFailed",
    ),
];

const FILTER_FIELDS: &[ShapeField] = &[
    field("items", "array", "Input items that matched the condition"),
    field("count", "integer", "Number of items kept"),
];

const WHILE_FIELDS: &[ShapeField] = &[
    field("iterations", "integer", "Number of completed iterations"),
    field(
        "outputs",
        "dynamic",
        "The last iteration's Finish outputs (null when no iteration ran)",
    ),
];

const CONDITIONAL_FIELDS: &[ShapeField] =
    &[field("result", "boolean", "The evaluated branch decision")];

const SWITCH_SIBLINGS: &[ShapeField] = &[field(
    "route",
    "string",
    "Label of the matched case (routing switches)",
)];

const GROUP_BY_FIELDS: &[ShapeField] = &[
    field(
        "groups",
        "object",
        "Map of group key to the array of items in that group",
    ),
    field("counts", "object", "Map of group key to item count"),
    field("total_groups", "integer", "Number of distinct groups"),
];

/// Look up the output shape for a PascalCase step type id (matches
/// `agent_meta::StepTypeMeta::id` and `Step` variant names). Returns `None` for
/// an unknown step type.
pub fn step_output_shape(step_type: &str) -> Option<StepOutputShape> {
    let shape = match step_type {
        "Split" => StepOutputShape {
            summary: "`outputs` is the array of successful per-item subgraph outputs (the collected results). With `config.dontStopOnFailed` the step also exposes `data.{success,error,aborted,unknown,skipped}`, `stats.{...,total}`, and `hasFailures`.",
            outputs: OutputsShape::Array,
            siblings: SPLIT_SIBLINGS,
        },
        "Filter" => StepOutputShape {
            summary: "`outputs` is `{items: <filtered array>, count: <number kept>}` — the input array narrowed to the items that matched the condition, plus their count.",
            outputs: OutputsShape::Object(FILTER_FIELDS),
            siblings: &[],
        },
        "While" => StepOutputShape {
            summary: "`outputs` is `{iterations: <count>, outputs: <last iteration's output>}`.",
            outputs: OutputsShape::Object(WHILE_FIELDS),
            siblings: &[],
        },
        "Conditional" => StepOutputShape {
            summary: "`outputs` is `{result: <bool>}` — the evaluated branch decision (materialized under track-events; used for edge routing otherwise).",
            outputs: OutputsShape::Object(CONDITIONAL_FIELDS),
            siblings: &[],
        },
        "Switch" => StepOutputShape {
            summary: "`outputs` is the matched case's output value (shape depends on the case). Routing switches also expose `route`, the matched case label.",
            outputs: OutputsShape::Dynamic,
            siblings: SWITCH_SIBLINGS,
        },
        "GroupBy" => StepOutputShape {
            summary: "`outputs` is `{groups: {<groupKey>: [items...]}, counts: {<groupKey>: <n>}, total_groups: <n>}`.",
            outputs: OutputsShape::Object(GROUP_BY_FIELDS),
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

fn shape_field_json(f: &ShapeField) -> Value {
    let mut v = json!({
        "name": f.name,
        "type": f.ty,
        "description": f.description,
    });
    if let Some(gate) = f.gated_by {
        v["gatedBy"] = json!(gate);
    }
    v
}

/// Render a step type's output shape as JSON for the authoring schema (the
/// `outputShape` key on each step type). Returns `null` for a step type with no
/// declared shape (e.g. the virtual `Start` step).
///
/// Object fields and sibling fields are emitted as `{name, type, description}`
/// objects so authoring surfaces can show per-field types, not just names.
pub fn output_shape_json(step_type: &str) -> Value {
    let Some(shape) = step_output_shape(step_type) else {
        return Value::Null;
    };
    let outputs = match shape.outputs {
        OutputsShape::Array => json!({
            "kind": "array",
            "note": "address elements by numeric index (e.g. `steps.<id>.outputs.0`); a named-key tail like `.result` is invalid and rejected at preflight",
        }),
        OutputsShape::Object(fields) => json!({
            "kind": "object",
            "fields": fields.iter().map(shape_field_json).collect::<Vec<_>>(),
        }),
        OutputsShape::Dynamic => json!({
            "kind": "dynamic",
            "note": "shape depends on runtime data or config and is not statically validated",
        }),
    };
    json!({
        "summary": shape.summary,
        "reference": "steps.<id>.outputs",
        "outputs": outputs,
        "siblingFields": shape.siblings.iter().map(shape_field_json).collect::<Vec<_>>(),
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

    fn field_names(fields: &[ShapeField]) -> Vec<&'static str> {
        fields.iter().map(|f| f.name).collect()
    }

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
        let OutputsShape::Object(filter_fields) = step_output_shape("Filter").unwrap().outputs
        else {
            panic!("Filter outputs must be a closed object");
        };
        assert_eq!(field_names(filter_fields), vec!["items", "count"]);
        let OutputsShape::Object(while_fields) = step_output_shape("While").unwrap().outputs else {
            panic!("While outputs must be a closed object");
        };
        assert_eq!(field_names(while_fields), vec!["iterations", "outputs"]);
        let OutputsShape::Object(group_fields) = step_output_shape("GroupBy").unwrap().outputs
        else {
            panic!("GroupBy outputs must be a closed object");
        };
        assert_eq!(
            field_names(group_fields),
            vec!["groups", "counts", "total_groups"]
        );
        let OutputsShape::Object(cond_fields) = step_output_shape("Conditional").unwrap().outputs
        else {
            panic!("Conditional outputs must be a closed object");
        };
        assert_eq!(field_names(cond_fields), vec!["result"]);
        // Split's failure buckets are siblings, not under `outputs`.
        assert!(
            step_output_shape("Split")
                .unwrap()
                .siblings
                .iter()
                .any(|s| s.name == "data")
        );
    }

    /// Field types feed editor type badges; pin the ones the runtime guarantees.
    #[test]
    fn field_types_are_declared() {
        let OutputsShape::Object(filter_fields) = step_output_shape("Filter").unwrap().outputs
        else {
            unreachable!();
        };
        assert_eq!(
            filter_fields
                .iter()
                .map(|f| (f.name, f.ty))
                .collect::<Vec<_>>(),
            vec![("items", "array"), ("count", "integer")]
        );
        let OutputsShape::Object(while_fields) = step_output_shape("While").unwrap().outputs else {
            unreachable!();
        };
        assert_eq!(while_fields[0].ty, "integer");
        let OutputsShape::Object(cond_fields) = step_output_shape("Conditional").unwrap().outputs
        else {
            unreachable!();
        };
        assert_eq!(cond_fields[0].ty, "boolean");
        let switch = step_output_shape("Switch").unwrap();
        assert_eq!(switch.siblings[0].name, "route");
        assert_eq!(switch.siblings[0].ty, "string");
        let split = step_output_shape("Split").unwrap();
        assert_eq!(
            split
                .siblings
                .iter()
                .map(|s| (s.name, s.ty))
                .collect::<Vec<_>>(),
            vec![
                ("data", "object"),
                ("stats", "object"),
                ("hasFailures", "boolean")
            ]
        );
        // Every declared type must be from the closed vocabulary.
        const TYPES: &[&str] = &[
            "string", "number", "integer", "boolean", "array", "object", "dynamic",
        ];
        for step_type in ALL_STEP_TYPES {
            let shape = step_output_shape(step_type).unwrap();
            if let OutputsShape::Object(fields) = shape.outputs {
                for f in fields {
                    assert!(
                        TYPES.contains(&f.ty),
                        "{step_type}.outputs.{}: bad type {}",
                        f.name,
                        f.ty
                    );
                }
            }
            for s in shape.siblings {
                assert!(
                    TYPES.contains(&s.ty),
                    "{step_type}.{}: bad type {}",
                    s.name,
                    s.ty
                );
            }
        }
    }

    /// The JSON form must carry {name, type, description} per field so the
    /// editor can render badges without re-parsing prose.
    #[test]
    fn json_emits_typed_fields() {
        let v = output_shape_json("Filter");
        let fields = v["outputs"]["fields"].as_array().unwrap();
        assert_eq!(fields[0]["name"], "items");
        assert_eq!(fields[0]["type"], "array");
        assert!(fields[0]["description"].as_str().unwrap().len() > 5);

        let split = output_shape_json("Split");
        let siblings = split["siblingFields"].as_array().unwrap();
        assert_eq!(siblings[2]["name"], "hasFailures");
        assert_eq!(siblings[2]["type"], "boolean");
        // The runtime writes the failure siblings only with dontStopOnFailed;
        // suggestion surfaces read this gate so they don't offer paths that
        // resolve to null under the default config.
        assert_eq!(siblings[2]["gatedBy"], "dontStopOnFailed");
        assert_eq!(split["outputs"]["kind"], "array");

        // Ungated fields carry no gatedBy key at all.
        let filter = output_shape_json("Filter");
        assert!(filter["outputs"]["fields"][0].get("gatedBy").is_none());
    }
}
