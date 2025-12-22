# Step Types Improvements

Proposed improvements to the runtara-workflows DSL (v3.0.0).

## Execution Plan Improvements

### 1. Error Handling via `onError` Edge Label

Instead of a dedicated TryCatch step, error handling is expressed through optional `onError` transitions in the execution plan. Each step (except Start, Finish, Conditional) can have an `onError` edge leading to an error handling step.

#### Example: Order Processing with Error Handling

```
Submit Order --> Send Email ---------------------------------> Finish
      |                                                          ^
      |                                                          |
      +-- onError --> Send Error Email --> Notify Stockout ------+
```

```json
{
  "steps": {
    "submit_order": {
      "stepType": "Agent",
      "id": "submit_order",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {...}
    },
    "send_email": {
      "stepType": "Agent",
      "id": "send_email",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {...}
    },
    "send_error_email": {
      "stepType": "Agent",
      "id": "send_error_email",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "errorMessage": { "valueType": "reference", "value": "error.message" },
        "failedStep": { "valueType": "reference", "value": "error.stepId" }
      }
    },
    "notify_stockout": {
      "stepType": "Agent",
      "id": "notify_stockout",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {...}
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {...}
    }
  },
  "entryPoint": "submit_order",
  "executionPlan": [
    { "fromStep": "submit_order", "toStep": "send_email" },
    { "fromStep": "submit_order", "toStep": "send_error_email", "label": "onError" },
    { "fromStep": "send_email", "toStep": "finish" },
    { "fromStep": "send_error_email", "toStep": "notify_stockout" },
    { "fromStep": "notify_stockout", "toStep": "finish" }
  ]
}
```

#### Execution Plan Edge Labels

| Label | Description |
|-------|-------------|
| (none) | Normal transition on success |
| `onError` | Transition when step fails (after retries exhausted) |
| `true`/`false` | Conditional step branches (existing) |

#### Error Context

When `onError` transition is taken, the following context is available:

- `error.message` - Error message string
- `error.stepId` - ID of the step that failed
- `error.code` - Error code (if available)

#### Behavior

- If a step fails and has no `onError` edge, the workflow fails
- If a step fails and has an `onError` edge, execution continues to the error handler
- Agent-level retries are exhausted before `onError` transition is taken
- Error handlers can lead to Finish or continue normal workflow

---

## New Step Types

### 2. While Step

Conditional looping - repeat until condition is false.

```json
{
  "stepType": "While",
  "id": "poll_status",
  "condition": {
    "type": "operation",
    "op": "NE",
    "arguments": [
      { "valueType": "reference", "value": "loop.outputs.status" },
      { "valueType": "immediate", "value": "completed" }
    ]
  },
  "subgraph": {
    "steps": {...},
    "entryPoint": "check_status"
  },
  "config": {
    "maxIterations": 10
  }
}
```

**Config options:**
- `maxIterations` - Maximum loop iterations (default: 10)

**Internal behavior:**
- Each iteration produces a heartbeat to maintain instance liveness

**Context in subgraph:**
- `loop.index` - Current iteration (0-based)
- `loop.outputs` - Outputs from previous iteration (null on first)

**Use cases:**
- Poll API until job completes
- Retry with custom logic until success
- Process paginated results

---

### 3. Log Step

Emit custom debug events during workflow execution.

```json
{
  "stepType": "Log",
  "id": "log_progress",
  "level": "info",
  "message": "Processing order",
  "context": {
    "orderId": { "valueType": "reference", "value": "data.orderId" },
    "itemCount": { "valueType": "reference", "value": "data.items.length" }
  }
}
```

**Levels:** `debug`, `info`, `warn`, `error`

**Behavior:**
- Emits event to `instance_events` table
- No effect on workflow outputs
- Useful for debugging and observability

---

## Step Configuration Improvements

### Per-Step Timeout

Add `timeout` field to all step types (currently only execution-level timeout exists).

```json
{
  "stepType": "Agent",
  "id": "slow_api",
  "agentId": "http",
  "capabilityId": "request",
  "timeout": 30000,
  "inputMapping": {...}
}
```

**Behavior:**
- Step fails if execution exceeds timeout
- Distinct from retry behavior
- Applies to: Agent, Split, StartScenario, While

---

## Implementation Status

| Improvement | Priority | Schema | Code Gen |
|-------------|----------|--------|----------|
| `onError` edge label | High | Done | Done |
| While step | Medium | Done | Done |
| Log step | Low | Done | Done |
| Per-step timeout | Medium | Done | Pending |

### Implementation Notes

#### Per-Step Timeout
The `timeout` field is added to Agent, Split, StartScenario, and While steps.
Runtime enforcement will wrap step execution with tokio::time::timeout.
