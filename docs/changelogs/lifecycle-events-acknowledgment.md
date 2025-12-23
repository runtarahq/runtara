# All Instance Events Now Wait for Server Acknowledgment

**Version:** 1.0.25
**Type:** Protocol Enhancement
**Impact:** Improved reliability for all workflow events

## Summary

All instance events (`heartbeat`, `custom`, `completed`, `failed`, `suspended`) now use request-response semantics instead of fire-and-forget. The SDK waits for the server to acknowledge that each event has been persisted before returning.

## Problem

Previously, events like `heartbeat` and `custom` (including debug events like `step_debug_start` and `step_debug_end`) used fire-and-forget semantics. This caused issues:

1. Debug events sent just before `completed()` could be lost due to race conditions
2. Custom telemetry events might not be persisted if the process exits quickly
3. The `step_debug_end` event for the Finish step was frequently missing

## Solution

All instance events now use request-response semantics:

| Event Type | Behavior | Use Case |
|------------|----------|----------|
| `heartbeat` | Request-response | Activity tracking |
| `custom` | Request-response | Debug events, telemetry |
| `completed`, `failed`, `suspended` | Request-response | Lifecycle state changes |

When calling any event method (`sdk.heartbeat()`, `sdk.send_custom_event()`, `sdk.completed()`, etc.), the SDK now:

1. Sends the event to the server
2. Waits for an `InstanceEventResponse` acknowledgment
3. Only returns after the server confirms persistence

## What This Means for Users

### No Code Changes Required

This is a transparent protocol improvement. Your existing workflows will automatically benefit from the enhanced reliability without any code modifications.

### Slightly Longer Event Processing

All event calls now include a round-trip to the server. In practice, this adds only a few milliseconds per event but ensures data integrity.

### Guaranteed Event Persistence

All events are now guaranteed to be persisted before the SDK method returns. This ensures consistent audit trails, complete debug information, and no lost telemetry.

## Technical Details

### Protocol Changes

New message type added to `instance.proto`:

```protobuf
message InstanceEventResponse {
  bool success = 1;
  optional string error = 2;
}
```

### Server Behavior

The server now returns `InstanceEventResponse` for all instance events (heartbeat, custom, completed, failed, suspended).

### Error Handling

If the server fails to persist any event, the SDK will receive an error response and propagate it to the caller, allowing for proper error handling and retry logic.

## Migration

No migration is required. The change is backward compatible at the application level. Workflows compiled against older SDK versions will continue to work, though they won't benefit from the acknowledgment guarantee until recompiled.
