# Brief: StartScenario Input Validation

## Problem Statement

When a parent scenario calls a child scenario via `StartScenario`, the child only receives inputs that are explicitly mapped in the parent's `inputMapping`. If required inputs are missing, the child scenario fails at runtime with cryptic errors like:

```
Invalid input for update-instance: invalid type: null, expected a string
```

This error occurs deep in the execution (e.g., step 15 of 31), making it difficult to diagnose. The root cause is that the parent's `StartScenario` step didn't map all required inputs.

### Why This Only Affects StartScenario

- **Direct execution**: Child receives inputs directly from API → all required fields present
- **Via StartScenario**: Child only receives what parent maps → missing fields become `null`

From `start_scenario.rs`:
```rust
// All mapped inputs become child's data (myParam1 -> data.myParam1)
// Child variables are always isolated - never inherited from parent
let child_scenario_inputs = ScenarioInputs {
    data: Arc::new(child_inputs),  // Only contains mapped values!
    ...
};
```

## Proposed Solution

### 1. Compile-Time Validation (Preferred)

**Location**: `runtara-workflows/src/codegen/validation.rs` or new `runtara-workflows/src/validation/start_scenario.rs`

**When**: During scenario compilation, before code generation

**Implementation**:

```rust
/// Validate that StartScenario steps provide all required inputs for the child scenario.
pub fn validate_start_scenario_inputs(
    step: &StartScenarioStep,
    child_graph: &ExecutionGraph,
) -> Result<(), ValidationError> {
    // 1. Get child's input schema (required fields)
    let required_inputs: Vec<&str> = child_graph
        .input_schema
        .iter()
        .filter(|(_, field)| field.required.unwrap_or(true))
        .map(|(name, _)| name.as_str())
        .collect();

    // 2. Get mapped inputs from parent's inputMapping
    let mapped_inputs: HashSet<&str> = step
        .input_mapping
        .as_ref()
        .map(|m| m.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    // 3. Find missing required inputs
    let missing: Vec<&str> = required_inputs
        .iter()
        .filter(|name| !mapped_inputs.contains(*name))
        .copied()
        .collect();

    if !missing.is_empty() {
        return Err(ValidationError::MissingRequiredInputs {
            step_id: step.id.clone(),
            child_scenario_id: step.child_scenario_id.clone(),
            missing_inputs: missing.iter().map(|s| s.to_string()).collect(),
        });
    }

    Ok(())
}
```

**Error Message**:
```
Compilation error in step 'sync-products':
  StartScenario calls child 'product-sync-handler' but is missing required inputs:
    - schema_name (String): The name of the object model schema
    - instance_id (String): The ID of the instance to update

  Add these to the step's inputMapping or mark them as optional in the child scenario's inputSchema.
```

**Limitations**:
- Only works when child's `inputSchema` is defined
- Cannot validate dynamic references (e.g., `data.items[*].id` in a Split)
- Cannot validate type mismatches for reference values

### 2. Runtime Validation (Fallback)

**Location**: Generated code in `start_scenario.rs` emitter

**When**: At StartScenario execution, before calling child function

**Implementation** (in generated code):

```rust
// In the durable function, before executing child:
async fn #durable_fn_name(...) -> Result<serde_json::Value, String> {
    // Runtime validation of child inputs against schema
    let validation_result = validate_child_inputs(
        &child_inputs,
        CHILD_INPUT_SCHEMA,  // Embedded at compile time
        child_scenario_id,
        step_id,
    );

    if let Err(missing) = validation_result {
        let structured_error = serde_json::json!({
            "stepId": step_id,
            "stepName": step_name,
            "stepType": "StartScenario",
            "code": "CHILD_MISSING_REQUIRED_INPUTS",
            "message": format!(
                "StartScenario step '{}' is missing required inputs for child scenario '{}': {}",
                step_id, child_scenario_id, missing.join(", ")
            ),
            "category": "permanent",
            "severity": "error",
            "childScenarioId": child_scenario_id,
            "missingInputs": missing,
        });
        return Err(serde_json::to_string(&structured_error).unwrap());
    }

    // ... execute child scenario
}
```

**Helper function** (in `runtara-workflow-stdlib`):

```rust
/// Validate that all required inputs are present and non-null.
pub fn validate_child_inputs(
    inputs: &serde_json::Value,
    schema: &[(&str, bool)],  // (field_name, required)
    child_scenario_id: &str,
    step_id: &str,
) -> Result<(), Vec<String>> {
    let obj = inputs.as_object();
    let mut missing = Vec::new();

    for (field, required) in schema {
        if *required {
            let is_missing = match obj {
                Some(map) => {
                    map.get(*field)
                        .map(|v| v.is_null())
                        .unwrap_or(true)
                }
                None => true,
            };

            if is_missing {
                missing.push(field.to_string());
            }
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}
```

### 3. Type Validation (Enhancement)

For additional safety, validate types when possible:

```rust
pub fn validate_input_types(
    inputs: &serde_json::Value,
    schema: &HashMap<String, SchemaField>,
) -> Result<(), Vec<TypeMismatch>> {
    let mut mismatches = Vec::new();
    let obj = inputs.as_object().unwrap_or(&serde_json::Map::new());

    for (field_name, field_schema) in schema {
        if let Some(value) = obj.get(field_name) {
            if !value.is_null() {
                let expected_type = &field_schema.field_type;
                let actual_type = json_type_name(value);

                if !types_compatible(expected_type, actual_type) {
                    mismatches.push(TypeMismatch {
                        field: field_name.clone(),
                        expected: expected_type.clone(),
                        actual: actual_type.to_string(),
                    });
                }
            }
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches)
    }
}
```

## Implementation Plan

### Phase 1: Compile-Time Validation (High Priority)
1. Add `ValidationError::MissingRequiredInputs` variant to codegen errors
2. Implement `validate_start_scenario_inputs()` in validation module
3. Call validation during `emit()` for StartScenario steps
4. Add tests for missing input detection

### Phase 2: Runtime Validation (Medium Priority)
1. Add `validate_child_inputs()` helper to `runtara-workflow-stdlib`
2. Modify StartScenario emitter to embed schema and validation call
3. Add structured error code `CHILD_MISSING_REQUIRED_INPUTS`
4. Add tests for runtime validation

### Phase 3: Enhanced Validation (Lower Priority)
1. Type checking for mapped values
2. Warning for unmapped optional inputs
3. Validation of nested/array access paths

## Test Cases

```rust
#[test]
fn test_compile_error_on_missing_required_input() {
    // Parent maps only 'name', but child requires 'name' AND 'id'
    let parent_step = StartScenarioStep {
        id: "call-child".to_string(),
        child_scenario_id: "my-child".to_string(),
        input_mapping: Some(hashmap! {
            "name".to_string() => immediate_value("test")
        }),
        ..default()
    };

    let child_schema = hashmap! {
        "name".to_string() => SchemaField { required: Some(true), .. },
        "id".to_string() => SchemaField { required: Some(true), .. },
    };

    let result = validate_start_scenario_inputs(&parent_step, &child_graph);
    assert!(matches!(result, Err(ValidationError::MissingRequiredInputs { .. })));
}

#[test]
fn test_runtime_validation_catches_null_required() {
    let inputs = json!({ "name": "test", "id": null });
    let schema = &[("name", true), ("id", true)];

    let result = validate_child_inputs(&inputs, schema, "child", "step");
    assert_eq!(result, Err(vec!["id".to_string()]));
}
```

## Migration Notes

- Existing scenarios with missing mappings will fail to compile (breaking change)
- Add `--skip-input-validation` flag for gradual migration
- Provide clear error messages with fix suggestions

## Related Files

- `runtara-workflows/src/codegen/ast/steps/start_scenario.rs` - Emitter
- `runtara-workflows/src/codegen/validation.rs` - Validation (new)
- `runtara-workflow-stdlib/src/runtime/validation.rs` - Runtime helpers (new)
- `runtara-dsl/src/schema_types.rs` - SchemaField definition
