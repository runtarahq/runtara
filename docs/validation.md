# Workflow Validation Improvements

This document outlines planned validation and diagnostic improvements for `runtara-workflows` to enhance developer experience without changing any public contracts.

## Overview

Current state: The crate only validates connection data leakage (security). Many workflow errors are only caught at runtime or during rustc compilation, leading to poor error messages.

Goal: Catch errors early with clear, actionable messages.

---

## Phase 1: Graph Structure Validation

Add validation for workflow graph integrity.

### 1.1 Entry Point Validation
- [ ] Verify `entry_point` exists in `steps` HashMap
- [ ] Error: `Entry point 'start' not found in steps. Available steps: fetch, transform, finish`

### 1.2 Unreachable Steps Detection
- [ ] Build reachability set from `entry_point` using `execution_plan` edges
- [ ] Warn for each step not in reachability set
- [ ] Warning: `Step 'unused_transform' is unreachable from entry point`

### 1.3 Dangling Steps Detection
- [ ] Non-Finish steps must have outgoing edges in `execution_plan`
- [ ] Error: `Step 'process' has no outgoing edges (not a Finish step)`

### 1.4 Empty Workflow Detection
- [ ] Warn if `steps` is empty
- [ ] Warning: `Workflow has no steps`

**Files to modify:** `crates/runtara-workflows/src/validation.rs`

**New error variants:**
```rust
pub enum ValidationError {
    // ... existing variants ...
    EntryPointNotFound { entry_point: String, available_steps: Vec<String> },
    UnreachableStep { step_id: String },
    DanglingStep { step_id: String },
    EmptyWorkflow,
}
```

---

## Phase 2: Reference Validation

Validate that data references point to valid locations.

### 2.1 Step Reference Validation
- [ ] Parse `steps.X.outputs` references in all `input_mapping` fields
- [ ] Verify referenced step ID exists in `steps`
- [ ] Error: `Step 'transform' references 'steps.fetch_data.outputs' but step 'fetch_data' not found`

### 2.2 Reference Path Syntax
- [ ] Validate reference paths are well-formed
- [ ] Check for empty segments (e.g., `steps..outputs`)
- [ ] Error: `Invalid reference path 'steps..outputs' - empty path segment`

### 2.3 Self-Reference Warning
- [ ] Warn if a step references its own outputs (except in While loops)
- [ ] Warning: `Step 'process' references its own outputs - this may cause issues`

**Implementation approach:**
- Extract all `MappingValue::Reference` from step input mappings
- Use existing `extract_step_id_from_reference()` helper
- Build set of valid step IDs for lookup

---

## Phase 3: Agent/Capability Validation

Validate agent and capability usage against the registry.

### 3.1 Agent Existence Check
- [ ] Use `runtara_dsl::agent_meta::find_agent_module()` to verify `agent_id`
- [ ] Error: `Step 'call_api' uses unknown agent 'httpp'. Did you mean 'http'?`
- [ ] Include available agents in error message

### 3.2 Capability Existence Check
- [ ] Use `runtara_dsl::agent_meta::get_capability_inputs()` to verify capability
- [ ] Error: `Agent 'http' has no capability 'post'. Available: http-request`

### 3.3 Required Input Validation
- [ ] Get capability inputs via `get_capability_inputs(agent_id, capability_id)`
- [ ] Check that all `required: true` fields are present in `input_mapping`
- [ ] Error: `Step 'fetch': capability 'http-request' requires 'url' but it's not provided`

### 3.4 Unknown Input Field Warning
- [ ] Warn about input mapping keys that don't match any capability input
- [ ] Warning: `Step 'fetch': input 'urll' is not a known field for 'http-request'. Did you mean 'url'?`

**Files to modify:** `crates/runtara-workflows/src/validation.rs`

**Dependencies:** `runtara_dsl::agent_meta`

---

## Phase 4: Configuration Warnings

Add warnings for potentially problematic configurations.

### 4.1 Retry Configuration
- [ ] Warn if `max_retries > 50`
- [ ] Warn if `retry_delay > 3600000` (1 hour in ms)
- [ ] Warning: `Step 'fetch' has max_retries=200. High retry counts may cause long execution times.`

### 4.2 Split Configuration
- [ ] Warn if `parallelism > 100`
- [ ] Warning: `Split step 'process_items' has parallelism=500. Consider reducing for resource efficiency.`

### 4.3 While Loop Configuration
- [ ] Warn if `max_iterations > 10000`
- [ ] Warning: `While step 'poll' has max_iterations=100000. This may indicate an infinite loop risk.`

### 4.4 Timeout Configuration
- [ ] Warn if `timeout > 3600000` (1 hour)
- [ ] Warning: `Step 'long_process' has timeout of 2 hours. Consider breaking into smaller steps.`

**Implementation:** Add `ValidationWarning` enum separate from `ValidationError`.

---

## Phase 5: Connection Validation

Validate connection step configurations.

### 5.1 Integration ID Validation
- [ ] Use `runtara_dsl::agent_meta::find_connection_type()` to verify `integration_id`
- [ ] Error: `Connection step 'auth' uses unknown integration 'bearerr'. Available: bearer, api_key, basic_auth, sftp`

### 5.2 Unused Connection Warning
- [ ] Track which Connection steps are referenced by secure agents
- [ ] Warn about Connection steps whose outputs are never used
- [ ] Warning: `Connection step 'unused_auth' is defined but never referenced`

---

## Phase 6: Child Scenario Validation

Validate StartScenario step configurations.

### 6.1 Version Format Validation
- [ ] Validate `version` is "latest", "current", or a positive integer string
- [ ] Error: `StartScenario step 'run_child' has invalid version 'v2'. Use 'latest', 'current', or a number like '2'.`

### 6.2 Circular Dependency Surface
- [ ] Existing circular dependency detection exists in code generation
- [ ] Surface this check earlier in validation phase
- [ ] Error: `Circular dependency detected: workflow-a -> workflow-b -> workflow-a`

---

## Phase 7: Better Error Messages

Improve error output quality.

### 7.1 Structured Error Codes
- [ ] Add error codes to all validation errors (e.g., `E001`, `W001`)
- [ ] Format: `[E003] Step 'fetch' references unknown step 'nonexistent'`
- [ ] Create error code documentation

### 7.2 Error Aggregation
- [ ] Collect all validation errors before returning (already done)
- [ ] Group errors by category (graph, reference, agent, config)
- [ ] Sort by severity (errors before warnings)

### 7.3 Rustc Error Parsing
- [ ] Parse rustc stderr for common error patterns
- [ ] Provide user-friendly suggestions:
  - Missing target: `"Run: rustup target add x86_64-unknown-linux-musl"`
  - Missing library: `"Run: cargo build -p runtara-workflow-stdlib --release"`
  - Syntax error: Extract line number from generated code

**Files to modify:** `crates/runtara-workflows/src/compile.rs`

### 7.4 Code Generation Panic Context
- [ ] Wrap code generation with step context
- [ ] On panic, include: step ID, step type, and relevant config
- [ ] Error: `Code generation failed at step 'transform' (Agent): invalid input mapping structure`

---

## Phase 8: Diagnostic Output

Add optional diagnostic features for debugging.

### 8.1 Source Code Emission
- [ ] Add `--emit-source <path>` CLI flag
- [ ] Save generated `main.rs` to specified path
- [ ] Helps debug code generation issues

**Files to modify:** `crates/runtara-workflows/src/bin/compile.rs`

### 8.2 Workflow Analysis Report
- [ ] Add `--analyze` CLI flag
- [ ] Output workflow statistics:
  ```
  Workflow Analysis:
    Steps: 12 total
      - Agent: 5 (http: 3, transform: 2)
      - Conditional: 3
      - Split: 2
      - While: 1
      - Finish: 1
    Child scenarios: 2
    Connections: 1 (bearer)
    Has side effects: yes
    Max nesting depth: 4
  ```

### 8.3 Verbose Compilation Output
- [ ] Add `--verbose` CLI flag
- [ ] Show compilation progress:
  ```
  [1/5] Validating workflow... OK (0 errors, 2 warnings)
  [2/5] Generating Rust code... OK (12 steps, 4.2 KB)
  [3/5] Compiling with rustc... OK (2.1s)
  [4/5] Calculating checksum... OK
  [5/5] Done (2.3s total)
  ```

---

## Implementation Order

### Sprint 1: Core Validation (High Impact, Low Effort)
1. Entry point validation (1.1)
2. Step reference validation (2.1)
3. Agent existence check (3.1)
4. Capability existence check (3.2)

### Sprint 2: Extended Validation
5. Unreachable steps detection (1.2)
6. Required input validation (3.3)
7. Integration ID validation (5.1)
8. Reference path syntax (2.2)

### Sprint 3: Warnings & Diagnostics
9. Configuration warnings (4.1-4.4)
10. Unknown input field warning (3.4)
11. Unused connection warning (5.2)
12. Structured error codes (7.1)

### Sprint 4: Developer Tools
13. Source code emission (8.1)
14. Rustc error parsing (7.3)
15. Workflow analysis report (8.2)
16. Verbose output (8.3)

---

## Validation Function Signature

Current:
```rust
pub fn validate_workflow(graph: &ExecutionGraph) -> Vec<ValidationError>;
```

Proposed:
```rust
pub struct ValidationResult {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

pub fn validate_workflow(graph: &ExecutionGraph) -> ValidationResult;
```

This separates hard errors (compilation should fail) from soft warnings (compilation proceeds but issues are reported).

---

## Testing Strategy

Each validation should have tests for:
1. **Positive case**: Valid workflow passes
2. **Negative case**: Invalid workflow produces expected error
3. **Edge cases**: Empty values, special characters in IDs, etc.

Example test structure:
```rust
#[test]
fn test_entry_point_not_found() {
    let graph = create_graph_with_missing_entry_point();
    let result = validate_workflow(&graph);
    assert!(result.errors.iter().any(|e| matches!(e, 
        ValidationError::EntryPointNotFound { .. }
    )));
}
```

---

## Success Metrics

- Reduce runtime errors by catching issues at compile time
- Reduce "contact support" errors by providing actionable messages
- Reduce time-to-fix by including suggestions in error messages
