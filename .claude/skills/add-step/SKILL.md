---
name: add-step
description: Use when adding a new DSL step type (Conditional, Split, Filter, While, Log, etc.) to the workflow language. Steps are the building blocks of workflow graphs and appear in the Step Picker UI. Distinct from agents and capabilities — a step is workflow-control machinery, not external logic.
---

# Add a new DSL step

A **step** is a node type in the workflow graph (e.g. `Agent`, `Conditional`, `Split`, `Switch`, `While`, `Log`). Existing step structs live in `crates/runtara-dsl/src/schema_types.rs`, registration in [crates/runtara-dsl/src/step_registration.rs](../../../crates/runtara-dsl/src/step_registration.rs).

## Steps

### 1. Define the struct

In `crates/runtara-dsl/src/schema_types.rs`, add a `pub struct MyStep { ... }`. Derive `schemars::JsonSchema`, `serde::Serialize`, `serde::Deserialize`, and any field-level attributes that drive form rendering.

Look at neighbouring step structs in the same file for the exact derive set and field-attribute style — match the existing pattern.

### 2. Re-export from the crate root

Add `MyStep` to the public re-exports so `step_registration.rs` can import it. Check the top of [step_registration.rs](../../../crates/runtara-dsl/src/step_registration.rs:19) — it imports steps from `crate::{...}`.

### 3. Add a schema generator function

In [step_registration.rs](../../../crates/runtara-dsl/src/step_registration.rs:28), add:

```rust
fn schema_my_step() -> schemars::schema::RootSchema {
    schemars::schema_for!(MyStep)
}
```

### 4. Add the `StepTypeMeta` static

```rust
static MY_STEP_META: StepTypeMeta = StepTypeMeta {
    id: "MyStep",                          // PascalCase, matches the struct name
    display_name: "My Step",                // shown in the Step Picker
    description: "What this step does",
    category: "control" | "execution" | "utility",
    schema_fn: schema_my_step,
};
```

### 5. Register with inventory

Add at the bottom of the `native` module:

```rust
inventory::submit! { &MY_STEP_META }
```

### 6. Wire up execution

The DSL crate only describes the step. Execution lives in the runtime — find where existing steps are dispatched (search for `StepKind::` or pattern-match on the step enum in `crates/runtara-workflows` / `runtara-environment`) and add a branch for `MyStep`.

### 7. Frontend pickup

The Step Picker reads from the auto-generated API client — no manual frontend registration. After backend changes:

1. Run `regen-frontend-api`. The new step appears in [StepPickerModal.tsx](../../../crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/NodeForm/StepPickerModal.tsx) automatically.
2. If the step needs a custom config form (beyond what the auto-generated schema renders), add a section in [NodeForm/index.tsx](../../../crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/NodeForm/index.tsx).

### 8. Verify

Run `e2e-verify` with a workflow that uses the new step. Compile + register + execute and assert the step's observable behavior — unit tests on the schema do not prove the step actually runs.

## Files touched

- `crates/runtara-dsl/src/schema_types.rs` — add struct
- `crates/runtara-dsl/src/lib.rs` (or wherever the re-export lives) — export struct
- `crates/runtara-dsl/src/step_registration.rs` — schema fn + meta + `inventory::submit!`
- Runtime dispatch in `crates/runtara-workflows` or `crates/runtara-environment` — execution branch
- (optional) Frontend custom form
