# runtara-sdk-macros

Proc-macro crate providing the `#[resilient]` attribute for runtara-sdk.

## What it is

A procedural macro crate exporting a single attribute: `#[resilient]`. Applied to a synchronous `fn` whose first argument is an idempotency key and whose return type is `Result<T, E>`, it expands into retry-aware SDK calls. When `durable = true` (default), it also checks for a checkpoint, returns the cached value on resume, and saves the successful result. Configured via `durable`, `max_retries`, `strategy = ExponentialBackoff`, `delay`, and `rate_limit_budget`, the generated code performs retries with exponential backoff, treats errors with `category = "permanent"` as terminal, and honors `retryAfterMs` on rate-limited errors via durable sleep (or `std::thread::sleep` when `durable = false`). It also reacts to pause, cancel, and shutdown signals surfaced by the SDK after each checkpoint. Consumed through `runtara-sdk`, which re-exports the macro — you should not depend on this crate directly.

## Using it standalone

This crate is consumed via `runtara-sdk`'s re-export. Add `runtara-sdk` to your `Cargo.toml` and import the macro from there:

```rust
use runtara_sdk::resilient;

#[resilient(max_retries = 3, strategy = ExponentialBackoff, delay = 1000)]
pub fn submit_order(key: &str, order: &Order) -> Result<OrderResult, OrderError> {
    external_service.submit(order)
}
```

The durable-path expansion emits calls to `runtara_sdk::sdk()`, `acknowledge_cancellation`, and `acknowledge_shutdown`, so it only compiles when `runtara-sdk` is in scope. Depending on this crate directly offers no benefit and will not compile without those SDK symbols.

## Inside Runtara

- Consumed exclusively by `runtara-sdk`, which re-exports `resilient` (`pub use runtara_sdk_macros::resilient;`).
- Depends on `syn` (with `full` + `extra-traits`), `quote`, and `proc-macro2`.
- Durable path expands into calls against the global SDK handle (`get_checkpoint`, `checkpoint`, `sleep`, `record_retry_attempt`) plus `acknowledge_cancellation` / `acknowledge_shutdown`. Non-durable path (`durable = false`) emits only the retry loop with `std::thread::sleep` and no SDK calls.
- Integration point: user agent code written for the runtara runtime wraps its step functions in `#[resilient]` to get retry budgets and (optionally) checkpoint caching, cooperative pause/cancel/shutdown exits.
- Enforces constraints at compile time: function must be non-async, first arg must be a simple identifier (the idempotency key), and the return type must be `Result<T, E>`.
- Runs in: proc-macro at compile time.

## License

AGPL-3.0-or-later.
