# Debug Events Guide

This guide explains how to enable debug mode for workflows and retrieve debug events for troubleshooting and monitoring.

## Enabling Debug Mode

When compiling a workflow, set `debug_mode: true` in the compilation options:

```rust
let options = CompileOptions {
    debug_mode: true,
    // ... other options
};
let result = sdk.compile_workflow(source, options).await?;
```

When debug mode is enabled, the workflow runtime automatically emits detailed events at each step execution.

## Debug Event Types

With debug mode enabled, workflows emit the following events:

| Event Type | Subtype | Description |
|------------|---------|-------------|
| `custom` | `step_debug_start` | Emitted when a step begins execution |
| `custom` | `step_debug_end` | Emitted when a step completes |
| `custom` | `workflow_log` | Log messages from within the workflow |

### Event Payloads

**step_debug_start**
```json
{
  "step_name": "fetch_order",
  "step_type": "activity",
  "inputs": { "order_id": "12345" },
  "timestamp": "2025-01-15T10:30:00Z"
}
```

**step_debug_end**
```json
{
  "step_name": "fetch_order",
  "step_type": "activity",
  "duration_ms": 150,
  "result": "success",
  "output": { "status": "shipped" },
  "timestamp": "2025-01-15T10:30:00.150Z"
}
```

## Fetching Debug Events

Use the Management SDK to retrieve events for a workflow instance:

```rust
use runtara_management_sdk::{ManagementSdk, ListEventsOptions};

let sdk = ManagementSdk::connect("127.0.0.1:8002").await?;

// Fetch all events for an instance
let result = sdk.list_events("instance-id", None).await?;
println!("Total events: {}", result.total_count);
for event in result.events {
    println!("{}: {} - {}", event.created_at, event.event_type, event.subtype.unwrap_or_default());
}
```

### Filtering Events

Use `ListEventsOptions` to filter and paginate results:

```rust
// Filter by event subtype (e.g., only step starts)
let options = ListEventsOptions {
    subtype: Some("step_debug_start".to_string()),
    limit: Some(100),
    ..Default::default()
};
let result = sdk.list_events("instance-id", Some(options)).await?;

// Filter by time range
let options = ListEventsOptions {
    created_after: Some(Utc::now() - Duration::hours(1)),
    created_before: Some(Utc::now()),
    ..Default::default()
};

// Full-text search in event payloads
let options = ListEventsOptions {
    payload_contains: Some("fetch_order".to_string()),
    ..Default::default()
};
```

### Pagination

For instances with many events, use pagination:

```rust
let mut offset = 0;
let limit = 50;

loop {
    let options = ListEventsOptions {
        limit: Some(limit),
        offset: Some(offset),
        ..Default::default()
    };
    
    let result = sdk.list_events("instance-id", Some(options)).await?;
    
    for event in &result.events {
        process_event(event);
    }
    
    offset += result.events.len() as i64;
    if offset >= result.total_count {
        break;
    }
}
```

## ListEventsOptions Reference

| Field | Type | Description |
|-------|------|-------------|
| `limit` | `Option<i64>` | Maximum number of events to return |
| `offset` | `Option<i64>` | Number of events to skip (for pagination) |
| `event_type` | `Option<String>` | Filter by event type (e.g., `"custom"`) |
| `subtype` | `Option<String>` | Filter by subtype (e.g., `"step_debug_start"`) |
| `created_after` | `Option<DateTime<Utc>>` | Only events after this timestamp |
| `created_before` | `Option<DateTime<Utc>>` | Only events before this timestamp |
| `payload_contains` | `Option<String>` | Full-text search in event payload JSON |

## EventSummary Fields

Each returned event contains:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `i64` | Unique event identifier |
| `instance_id` | `String` | The workflow instance ID |
| `event_type` | `String` | Event type (e.g., `"custom"`) |
| `subtype` | `Option<String>` | Event subtype (e.g., `"step_debug_start"`) |
| `checkpoint_id` | `Option<String>` | Associated checkpoint if any |
| `payload` | `Option<Vec<u8>>` | JSON payload as bytes |
| `created_at` | `DateTime<Utc>` | When the event was recorded |

## Best Practices

1. **Use debug mode only when needed** - Debug events add overhead; disable in production unless actively troubleshooting.

2. **Filter early** - Use specific filters to reduce data transfer and processing time.

3. **Paginate large result sets** - Don't fetch all events at once for long-running workflows.

4. **Search payloads efficiently** - The `payload_contains` filter performs full-text search; use specific terms for better performance.

5. **Correlate with checkpoints** - Use `checkpoint_id` to correlate debug events with specific workflow checkpoints.
