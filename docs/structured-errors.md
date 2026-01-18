# Structured Errors in Runtara

This guide explains how to use structured error handling in Runtara workflows, including error classification, the Error step, and error routing.

## Overview

Runtara provides a structured error system that classifies errors into two categories, enabling intelligent retry behavior and workflow-level error handling.

### Error Categories

| Category | Description | Retry Behavior | Example |
|----------|-------------|----------------|---------|
| **Transient** | Temporary failures that are likely to succeed on retry | Auto-retry via `#[durable]` | Network timeout, rate limit, 5xx errors |
| **Permanent** | Failures that won't succeed on retry, but human intervention may help | No auto-retry; manual retry possible | 404 Not Found, validation errors, auth failures, business rule violations |

### Distinguishing Technical vs Business Errors

Within the **Permanent** category, you can distinguish between technical failures and business rule violations using:

- **`code`**: Use domain-specific codes like `CREDIT_LIMIT_EXCEEDED` or prefixes like `BUSINESS_*` for business errors, vs `VALIDATION_*` for technical errors
- **`severity`**: Use `warning` for expected business outcomes, `error` for technical failures

| Error Type | Category | Severity | Example Code |
|------------|----------|----------|--------------|
| Technical (validation) | Permanent | error | `VALIDATION_INVALID_EMAIL` |
| Technical (not found) | Permanent | error | `RESOURCE_NOT_FOUND` |
| Business (limit) | Permanent | warning | `CREDIT_LIMIT_EXCEEDED` |
| Business (availability) | Permanent | warning | `NO_AVAILABILITY` |

### Error Severity

Errors have a severity level for logging and alerting:

- `info` - Expected errors (informational)
- `warning` - Expected business outcomes (e.g., credit limit exceeded)
- `error` - Operation failed (default)
- `critical` - System-level failure

## How Errors Flow Through the System

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  Agent/Step     │────▶│  #[durable]     │────▶│  Workflow       │
│  Returns Error  │     │  Retry Logic    │     │  Error Routing  │
└─────────────────┘     └─────────────────┘     └─────────────────┘
        │                       │                       │
        │                       │                       │
   Transient?              Retries                 onError
   Permanent?              exhausted?              edges
                               │                       │
                               ▼                       ▼
                         Becomes              Route to handler
                         Permanent            based on category/code
```

1. **Agent returns structured error** with category, severity, and context
2. **`#[durable]` macro handles transient errors** with exponential backoff
3. **If retries exhausted**, transient becomes permanent (with original context preserved)
4. **Workflow routes error** to appropriate handler based on `onError` edges

## Using the Error Step

The Error step allows workflows to explicitly raise structured errors with full metadata.

### Basic Error Step

```json
{
  "stepType": "Error",
  "id": "validation_error",
  "code": "INVALID_ORDER",
  "message": "Order validation failed"
}
```

### Error Step with All Fields

```json
{
  "stepType": "Error",
  "id": "credit_limit_exceeded",
  "name": "Credit Limit Error",
  "category": "permanent",
  "code": "CREDIT_LIMIT_EXCEEDED",
  "message": "Order amount ${data.amount} exceeds credit limit of ${data.creditLimit}",
  "severity": "warning",
  "context": {
    "orderId": { "valueType": "reference", "value": "data.orderId" },
    "amount": { "valueType": "reference", "value": "data.amount" },
    "creditLimit": { "valueType": "reference", "value": "data.creditLimit" }
  }
}
```

> **Note:** Business errors use `"category": "permanent"` with `"severity": "warning"` to distinguish them from technical permanent errors.

### Field Reference

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `id` | Yes | - | Unique step identifier |
| `code` | Yes | - | Machine-readable error code (e.g., `CREDIT_LIMIT_EXCEEDED`) |
| `message` | Yes | - | Human-readable message (supports `${path}` interpolation) |
| `name` | No | - | Human-readable step name for UI display |
| `category` | No | `permanent` | Error category: `transient`, `permanent` |
| `severity` | No | `error` | Severity: `info`, `warning`, `error`, `critical` |
| `context` | No | - | Additional data to include with the error |

## Error Routing with onError Edges

Workflows can route errors to different handlers using `onError` edges with conditions. Conditions use the same expression format as the `Conditional` step, supporting operators like `EQ`, `AND`, `OR`, `STARTS_WITH`, etc.

### Basic onError Edge

```json
{
  "executionPlan": [
    { "fromStep": "call_api", "toStep": "process_result" },
    { "fromStep": "call_api", "toStep": "handle_error", "label": "onError" }
  ]
}
```

### Category-Based Error Routing

Route different error categories to different handlers using conditions:

```json
{
  "executionPlan": [
    { "fromStep": "call_api", "toStep": "process_result" },
    {
      "fromStep": "call_api",
      "toStep": "retry_handler",
      "label": "onError",
      "condition": {
        "type": "operation",
        "op": "EQ",
        "arguments": [
          { "valueType": "reference", "value": "__error.category" },
          { "valueType": "immediate", "value": "transient" }
        ]
      },
      "priority": 10
    },
    {
      "fromStep": "call_api",
      "toStep": "permanent_error_handler",
      "label": "onError",
      "condition": {
        "type": "operation",
        "op": "EQ",
        "arguments": [
          { "valueType": "reference", "value": "__error.category" },
          { "valueType": "immediate", "value": "permanent" }
        ]
      },
      "priority": 5
    }
  ]
}
```

### Routing Business vs Technical Permanent Errors

Use conditions with `STARTS_WITH` or combine conditions with `AND` to distinguish business errors from technical permanent errors:

```json
{
  "executionPlan": [
    { "fromStep": "process_order", "toStep": "complete" },
    {
      "fromStep": "process_order",
      "toStep": "business_error_handler",
      "label": "onError",
      "condition": {
        "type": "operation",
        "op": "AND",
        "arguments": [
          {
            "type": "operation",
            "op": "STARTS_WITH",
            "arguments": [
              { "valueType": "reference", "value": "__error.code" },
              { "valueType": "immediate", "value": "CREDIT_" }
            ]
          },
          {
            "type": "operation",
            "op": "EQ",
            "arguments": [
              { "valueType": "reference", "value": "__error.severity" },
              { "valueType": "immediate", "value": "warning" }
            ]
          }
        ]
      },
      "priority": 10
    },
    {
      "fromStep": "process_order",
      "toStep": "technical_error_handler",
      "label": "onError",
      "condition": {
        "type": "operation",
        "op": "EQ",
        "arguments": [
          { "valueType": "reference", "value": "__error.category" },
          { "valueType": "immediate", "value": "permanent" }
        ]
      },
      "priority": 5
    }
  ]
}
```

### Condition Operators

Conditions use the same operators as `Conditional` steps:

| Operator | Description | Example |
|----------|-------------|---------|
| `EQ` | Equality check | `__error.category == "transient"` |
| `NE` | Not equal | `__error.category != "transient"` |
| `STARTS_WITH` | String prefix match | `__error.code` starts with `"CREDIT_"` |
| `ENDS_WITH` | String suffix match | `__error.code` ends with `"_TIMEOUT"` |
| `CONTAINS` | String/array contains | `__error.code` contains `"LIMIT"` |
| `AND` | Logical AND | Both conditions must match |
| `OR` | Logical OR | Either condition matches |
| `NOT` | Logical NOT | Negate condition |
| `GT`, `GTE`, `LT`, `LTE` | Numeric comparisons | For comparing numeric attributes |
| `IN`, `NOT_IN` | Value in array | Check if value is in a list |

### Available Context in Conditions

For `onError` edges, the `__error` context variable contains:

| Field | Description |
|-------|-------------|
| `__error.code` | Machine-readable error code (e.g., `HTTP_NOT_FOUND`) |
| `__error.message` | Human-readable error message |
| `__error.category` | Error category: `transient` or `permanent` |
| `__error.severity` | Severity: `info`, `warning`, `error`, `critical` |
| `__error.attributes.*` | Additional error attributes (e.g., `status_code`) |

Additionally, all standard context is available:
- `data.*` - Input data
- `steps.<stepId>.outputs.*` - Previous step outputs
- `variables.*` - Workflow variables

### Priority

When multiple `onError` edges could match, `priority` determines which is used (higher = checked first, default = 0). If no `onError` edge matches (either no condition is satisfied or no onError edge exists), the workflow fails with the error.

## Accessing Error Context

In error handlers, access the error context via the `__error` variable:

```json
{
  "stepType": "Log",
  "id": "log_error",
  "message": "Error occurred",
  "data": {
    "errorCode": { "valueType": "reference", "value": "__error.code" },
    "errorMessage": { "valueType": "reference", "value": "__error.message" },
    "errorCategory": { "valueType": "reference", "value": "__error.category" },
    "statusCode": { "valueType": "reference", "value": "__error.attributes.status_code" }
  }
}
```

## HTTP Agent Error Classification

The HTTP agent automatically classifies errors based on status codes:

| Status Code | Category | Error Code |
|-------------|----------|------------|
| 408 Request Timeout | Transient | `HTTP_TIMEOUT` |
| 429 Too Many Requests | Transient | `HTTP_RATE_LIMITED` |
| 500 Internal Server Error | Transient | `HTTP_INTERNAL_ERROR` |
| 502 Bad Gateway | Transient | `HTTP_BAD_GATEWAY` |
| 503 Service Unavailable | Transient | `HTTP_SERVICE_UNAVAILABLE` |
| 504 Gateway Timeout | Transient | `HTTP_GATEWAY_TIMEOUT` |
| 400 Bad Request | Permanent | `HTTP_BAD_REQUEST` |
| 401 Unauthorized | Permanent | `HTTP_UNAUTHORIZED` |
| 403 Forbidden | Permanent | `HTTP_FORBIDDEN` |
| 404 Not Found | Permanent | `HTTP_NOT_FOUND` |
| Other 4xx | Permanent | `HTTP_ERROR` |
| Other 5xx | Transient | `HTTP_ERROR` |

Network failures (connection refused, DNS errors) are classified as **Transient** with code `NETWORK_ERROR`.

## Example Workflows

### 1. API Call with Error Handling

```json
{
  "name": "API Call with Error Handling",
  "steps": {
    "call_api": {
      "stepType": "Agent",
      "id": "call_api",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "method": { "valueType": "immediate", "value": "GET" },
        "url": { "valueType": "reference", "value": "data.apiUrl" },
        "failOnError": { "valueType": "immediate", "value": true }
      }
    },
    "process_result": {
      "stepType": "Log",
      "id": "process_result",
      "message": "API call succeeded"
    },
    "handle_permanent_error": {
      "stepType": "Error",
      "id": "handle_permanent_error",
      "category": "permanent",
      "code": "API_PERMANENT_ERROR",
      "message": "API returned permanent error",
      "context": {
        "originalError": { "valueType": "reference", "value": "__error.message" },
        "statusCode": { "valueType": "reference", "value": "__error.attributes.status_code" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish"
    }
  },
  "entryPoint": "call_api",
  "executionPlan": [
    { "fromStep": "call_api", "toStep": "process_result" },
    { "fromStep": "process_result", "toStep": "finish" },
    {
      "fromStep": "call_api",
      "toStep": "handle_permanent_error",
      "label": "onError",
      "condition": {
        "type": "operation",
        "op": "EQ",
        "arguments": [
          { "valueType": "reference", "value": "__error.category" },
          { "valueType": "immediate", "value": "permanent" }
        ]
      }
    }
  ]
}
```

### 2. Business Error with Workflow Retry

```json
{
  "name": "Booking with Availability Check",
  "steps": {
    "check_availability": {
      "stepType": "Agent",
      "id": "check_availability",
      "agentId": "booking",
      "capabilityId": "checkAvailability"
    },
    "no_availability": {
      "stepType": "Error",
      "id": "no_availability",
      "category": "permanent",
      "code": "NO_AVAILABILITY",
      "message": "No spots available for requested date",
      "severity": "warning",
      "context": {
        "requestedDate": { "valueType": "reference", "value": "data.date" },
        "retryAfterHours": { "valueType": "immediate", "value": 24 }
      }
    },
    "proceed_booking": {
      "stepType": "Agent",
      "id": "proceed_booking",
      "agentId": "booking",
      "capabilityId": "createBooking"
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish"
    }
  },
  "entryPoint": "check_availability",
  "executionPlan": [
    { "fromStep": "check_availability", "toStep": "proceed_booking" },
    { "fromStep": "proceed_booking", "toStep": "finish" },
    {
      "fromStep": "check_availability",
      "toStep": "no_availability",
      "label": "onError",
      "condition": {
        "type": "operation",
        "op": "AND",
        "arguments": [
          {
            "type": "operation",
            "op": "STARTS_WITH",
            "arguments": [
              { "valueType": "reference", "value": "__error.code" },
              { "valueType": "immediate", "value": "NO_" }
            ]
          },
          {
            "type": "operation",
            "op": "EQ",
            "arguments": [
              { "valueType": "reference", "value": "__error.severity" },
              { "valueType": "immediate", "value": "warning" }
            ]
          }
        ]
      }
    }
  ]
}
```

> **Note:** This example uses `STARTS_WITH` on the error code and checks the severity with `EQ` to route business errors (permanent + warning) to the appropriate handler.

### 3. Retry Exhausted to Permanent Error

When transient errors exhaust retries, capture the original context:

```json
{
  "name": "Unreliable API with Retry Exhausted Handling",
  "steps": {
    "unreliable_call": {
      "stepType": "Agent",
      "id": "unreliable_call",
      "agentId": "http",
      "capabilityId": "request",
      "inputMapping": {
        "url": { "valueType": "reference", "value": "data.apiUrl" },
        "failOnError": { "valueType": "immediate", "value": true }
      }
    },
    "handle_retries_exhausted": {
      "stepType": "Error",
      "id": "handle_retries_exhausted",
      "category": "permanent",
      "code": "RETRIES_EXHAUSTED",
      "message": "All retry attempts failed",
      "severity": "error",
      "context": {
        "originalCategory": { "valueType": "immediate", "value": "transient" },
        "originalError": { "valueType": "reference", "value": "__error.message" },
        "retryCount": { "valueType": "reference", "value": "__error.attributes.retry_count" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish"
    }
  },
  "entryPoint": "unreliable_call",
  "executionPlan": [
    { "fromStep": "unreliable_call", "toStep": "finish" },
    { "fromStep": "unreliable_call", "toStep": "handle_retries_exhausted", "label": "onError" }
  ]
}
```

## Best Practices

### 1. Choose the Right Category and Severity

- **Transient**: Use for infrastructure issues that typically resolve themselves (network, rate limits, 5xx)
- **Permanent + error severity**: Use for technical errors that require human intervention (validation, auth, missing resources)
- **Permanent + warning severity**: Use for business rule violations that may resolve with time or business process changes (credit limits, availability)

### 2. Provide Meaningful Context

Always include relevant context in errors to help with debugging and recovery:

```json
{
  "context": {
    "orderId": { "valueType": "reference", "value": "data.orderId" },
    "customerId": { "valueType": "reference", "value": "data.customerId" },
    "attemptedAction": { "valueType": "immediate", "value": "create_order" }
  }
}
```

### 3. Use Error Codes Consistently

Define a consistent error code scheme across your workflows:

- `VALIDATION_*` - Input validation errors
- `AUTH_*` - Authentication/authorization errors
- `RESOURCE_*` - Resource-related errors (not found, already exists)
- `LIMIT_*` - Limit/quota errors
- `EXTERNAL_*` - External service errors

### 4. Consider Human-in-the-Loop Recovery

Permanent errors can be recovered from by human intervention:

1. Human fixes the underlying issue (updates config, creates missing resource)
2. Human restarts the workflow instance
3. Workflow resumes from the last checkpoint

Design your workflows to support this pattern by:
- Saving meaningful checkpoints before potentially failing steps
- Including enough context in errors to understand what went wrong
- Structuring steps so they can be safely retried after a fix

### 5. Business Errors and Scheduling

For business errors (permanent category with warning severity) that may resolve with time (e.g., "no availability"), consider:

1. Recording the error with `retryAfterHours` or similar context
2. Having an external scheduler restart the workflow after the suggested delay
3. Using the workflow's durable sleep for short delays within the workflow

### 6. Backwards Compatibility

For backwards compatibility, legacy `"category": "business"` values in JSON are automatically mapped to `"permanent"` when parsed. New workflows should use `"category": "permanent"` with `"severity": "warning"` for business errors.

## Troubleshooting

### Error Not Being Caught by onError Edge

1. Check that `failOnError: true` is set on the agent step
2. Verify the `condition` matches the actual error fields (`__error.category`, `__error.code`, etc.)
3. Check that the condition syntax is correct (uses `type: "operation"` and `op` fields)
4. For business errors, verify the condition checks both `__error.code` pattern and `__error.severity`
5. Check `priority` values if multiple onError edges exist

### Wrong Error Category

If HTTP errors are being classified incorrectly:
1. Check the actual HTTP status code in `__error.attributes.status_code`
2. Review the [HTTP Agent Error Classification](#http-agent-error-classification) table
3. For custom agents, ensure they return properly structured errors with `category` and `severity`

### Missing Error Context

If `__error.attributes` is empty:
1. Verify the agent returns structured errors (JSON serialized)
2. Check that the error includes an `attributes` field
3. For HTTP errors, ensure `failOnError: true` is set

### Condition Not Matching

If your condition doesn't match as expected:
1. Use the `Log` step to debug available error fields before routing
2. Verify string comparisons use exact case (`transient` not `Transient`)
3. Check that nested `AND`/`OR` conditions have correct structure
4. Remember: if no condition matches, the workflow fails with the error
