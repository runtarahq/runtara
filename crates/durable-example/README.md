# Runtara SDK Examples

This crate contains example applications showcasing various `runtara-sdk` use-cases for building durable workflows.

## Examples

| Example | Description |
|---------|-------------|
| `basic_example` | Fundamental SDK lifecycle: init, connect, register, progress, complete |
| `checkpoint_example` | Checkpointing for durability and crash recovery |
| `sleep_example` | Durable sleep pattern for long-running tasks |
| `signal_example` | Handling cancel, pause, and resume signals |
| `error_example` | Error handling patterns: retry, timeout, partial failure |
| `http_example` | Real HTTP requests with checkpoints (fetches runtara.com) |

## Running Examples

All examples can run in **standalone mode** (without runtara-core) for demonstration purposes.

### Run with Cargo

```bash
# Basic workflow example
cargo run -p durable-example --bin basic_example

# Checkpointing example
cargo run -p durable-example --bin checkpoint_example

# Durable sleep example
cargo run -p durable-example --bin sleep_example

# Signal handling example
cargo run -p durable-example --bin signal_example

# Error handling example
cargo run -p durable-example --bin error_example

# HTTP requests example
cargo run -p durable-example --bin http_example
```

### Enable Debug Logging

```bash
RUST_LOG=debug cargo run -p durable-example --bin basic_example
```

## Example Descriptions

### basic_example

Demonstrates the fundamental runtara-sdk lifecycle:

1. Create SDK with `RuntaraSdk::localhost()`
2. Connect to runtara-core
3. Register the instance
4. Process work in steps with heartbeat reporting
5. Send `completed` event with output

### checkpoint_example

Shows how to use checkpoints for durability:

- Serialize state with serde after each operation
- Use `sdk.checkpoint(id, state)` which handles both save and resume:
  - Returns `None` if checkpoint is saved (fresh execution)
  - Returns `Some(existing_state)` if checkpoint exists (resume case)
- Resume from where you left off after crashes

### sleep_example

Demonstrates durable sleep for workflows that need to wait:

- Short sleeps (< 30s): Handled in-process
- Long sleeps (>= 30s): Instance exits, runtara-core wakes it later
- State preserved across sleep/wake cycles

### signal_example

Shows how to handle external signals:

- `check_cancelled()` - Simple cancellation check in loops
- `poll_signal()` - Manual polling for cancel/pause/resume
- `acknowledge_signal()` - Confirm signal handling
- Graceful shutdown on cancellation
- Pause/resume with checkpoint

### error_example

Demonstrates error handling patterns:

- Retry with exponential backoff
- Unrecoverable error handling
- Timeout with `tokio::time::timeout`
- Partial failure with continue

### http_example

Real-world example fetching URLs from runtara.com:

- Pre-fetch checkpoints (before HTTP request)
- Post-fetch checkpoints (after success)
- Progress reporting during operations
- Resumable from any checkpoint

## Standalone Mode

When runtara-core is not running, examples will:

1. Detect connection failure
2. Print workflow steps that would execute
3. Demonstrate the API usage patterns

This allows learning the SDK without running the full infrastructure.

## With Runtara-Core

To run with actual durable execution:

1. Start runtara-core:
   ```bash
   cargo run -p runtara-core
   ```

2. Run any example:
   ```bash
   cargo run -p durable-example --bin basic_example
   ```

The example will connect to runtara-core and execute with full durability guarantees.
