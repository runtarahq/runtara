# Implementation Plan: Add Execution Branching to Switch Step

## Overview

Enhance the Switch step to support multi-way execution branching via `execution_plan` edges, similar to how Conditional uses `"true"`/`"false"` labels. When case edges are defined, Switch routes execution to different steps based on matched cases. When no case edges exist, Switch behaves as today (data-only output).

## Key Design Decisions

### Edge Label Convention
- **Case edges**: `case:<case_id>` (e.g., `case:us`, `case:eu`)
- **Default edge**: `default`
- This mirrors Conditional's `true`/`false` pattern while supporting N-way branching

### Backward Compatibility
- If no `case:*` edges exist in `execution_plan`, Switch produces output only (current behavior)
- `case_id` is optional on cases - cases without IDs participate in matching but cannot route execution
- **Fall-through behavior**: Cases with `case_id` but no corresponding edge fall through to the next sequential step (graceful degradation)

## Files to Modify

### 1. [schema_types.rs](../crates/runtara-dsl/src/schema_types.rs) (lines 950-966)

Add `case_id` field to `SwitchCase`:

```rust
pub struct SwitchCase {
    /// Optional identifier for routing execution via execution_plan edges.
    /// Use with `case:<case_id>` labels in execution_plan to branch to different steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,

    pub match_type: SwitchMatchType,
    pub match_value: serde_json::Value,
    pub output: serde_json::Value,
}
```

### 2. [switch.rs](../crates/runtara-workflows/src/codegen/ast/steps/switch.rs)

Major refactor to support branching. The emit function needs to:

1. **Accept `ExecutionGraph`** - Change signature to `emit(step: &SwitchStep, ctx: &mut EmitContext, graph: &ExecutionGraph)`

2. **Detect branching mode** - Check if any `case:*` or `default` edges exist for this step

3. **Branch code generation** (when edges exist):
   - Find target steps for each case via `find_next_step_for_label(step_id, "case:<case_id>", ...)`
   - Find default target via `find_next_step_for_label(step_id, "default", ...)`
   - Implement multi-way merge point detection (extend diamond pattern from Conditional)
   - Generate match expression that routes to appropriate branch code

4. **Preserve current behavior** - When no case edges, emit output-only code (current implementation)

### 3. [mod.rs](../crates/runtara-workflows/src/codegen/ast/steps/mod.rs) (lines 36, 67)

Update Switch dispatch to pass graph:
```rust
Step::Switch(s) => switch::emit(s, ctx, graph),  // Add graph parameter
```

### 4. [validation.rs](../crates/runtara-workflows/src/validation.rs)

Add new validation rules:

1. **Unique case IDs** - Within a Switch step, all `case_id` values must be unique
2. **Edge-case alignment** - If `case:X` edge exists, a case with `case_id: "X"` must exist (error if edge references non-existent case)
3. **Valid default** - If `default` edge exists but step has no `default` config, warn
4. **Orphaned edges** - Warn if case has `case_id` but no corresponding edge (falls through, but may be unintentional)

### 5. [step_registration.rs](../crates/runtara-dsl/src/step_registration.rs) (lines 79-85)

Update description to reflect branching capability:
```rust
description: "Multi-way branch based on value matching. Supports execution routing via case edges.",
```

## Implementation Steps

### Step 1: DSL Schema Change
- Add `case_id: Option<String>` to `SwitchCase` in schema_types.rs
- Update serde attributes for proper JSON serialization (`camelCase`)

### Step 2: Branching Detection & Edge Finding
- Add helper function `has_switch_branching_edges(step_id, execution_plan) -> bool`
- Add helper function `find_switch_case_edges(step_id, execution_plan) -> HashMap<String, String>` (case_id -> target_step)

### Step 3: Multi-way Merge Point Detection
- Extend `find_merge_point` pattern to handle N branches instead of 2
- Create `find_multi_merge_point(branch_starts: Vec<String>, graph) -> Option<String>`

### Step 4: Switch Code Generation Refactor
- Restructure `emit()` to handle both modes:
  - **Output-only mode**: Current implementation (no graph needed)
  - **Branching mode**: Generate match arms with branch code for each case

### Step 5: Validation Rules
- Add validation for case_id uniqueness
- Add validation for edge-case correspondence
- Add warnings for incomplete edge coverage

### Step 6: Tests
- Unit tests for new DSL types (case_id serialization)
- Codegen tests for Switch with branching edges
- Codegen tests for mixed mode (some cases with IDs, some without)
- Integration tests with diamond patterns

## Example Workflow

```json
{
  "steps": {
    "route-order": {
      "stepType": "Switch",
      "id": "route-order",
      "config": {
        "value": {"valueType": "reference", "value": "data.country"},
        "cases": [
          {"caseId": "us", "matchType": "EQ", "match": "US", "output": {"zone": "NA"}},
          {"caseId": "eu", "matchType": "IN", "match": ["DE", "FR", "IT"], "output": {"zone": "EU"}}
        ],
        "default": {"zone": "Other"}
      }
    },
    "handle-us-order": { "stepType": "Agent", "..." : "..." },
    "handle-eu-order": { "stepType": "Agent", "..." : "..." },
    "handle-other-order": { "stepType": "Agent", "..." : "..." }
  },
  "executionPlan": [
    {"fromStep": "route-order", "toStep": "handle-us-order", "label": "case:us"},
    {"fromStep": "route-order", "toStep": "handle-eu-order", "label": "case:eu"},
    {"fromStep": "route-order", "toStep": "handle-other-order", "label": "default"}
  ]
}
```

## Generated Code Pattern (Branching Mode)

```rust
// Evaluate switch value and cases...
let matched_case_id: Option<&str> = /* find matching case_id */;
let output = /* matched output or default */;

// Store output
steps_context.insert(step_id, ...);

// Branch based on matched case
// Note: Cases without edges fall through to sequential step (empty arm)
match matched_case_id {
    Some("us") => {
        // Emit handle-us-order step code (has edge)
    }
    Some("eu") => {
        // Emit handle-eu-order step code (has edge)
    }
    _ => {
        // Emit handle-other-order step code (default edge)
        // If no default edge, this is empty (falls through)
    }
}

// Execute common suffix after merge point (if diamond pattern)
// Or continue to next sequential step if no branching occurred
```

## Verification

1. **Build**: `cargo build -p runtara-workflows -p runtara-dsl`
2. **Tests**: `cargo test -p runtara-workflows -p runtara-dsl`
3. **Lint**: `cargo clippy -p runtara-workflows -p runtara-dsl`
4. **Manual test**: Create a test workflow JSON with Switch branching and verify compilation
