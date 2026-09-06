# A1 + A4 — retiring the wire shapes inside the process

`runtara-environment` is a library that runs in the server's own process. Its
read handlers still answered in the shape the retired management HTTP protocol
used: instants as epoch milliseconds, bodies as base64 strings, enums as
strings. `runtara-server` immediately converted every one of those back. Nothing
serialized the intermediate form.

This plan removed the round trip and, once it was gone, the handlers whose only
remaining job was performing it.

## 1. What was there

Measured on `024c4c5c`, not estimated.

### The single consumer

`runtara_environment::handlers` exported eight `#[derive(Serialize)]` response
types: `ImageSummary`, `InstanceStatusResponse`, `InstanceSummary`,
`CheckpointSummary`, `EventSummary`, `StepSummary`, `ScopeInfo`,
`MetricsBucket`. Grepping the workspace for each name outside `handlers.rs`
returned exactly one non-test consumer:
`crates/runtara-server/src/environment_client.rs`. Every method there mapped the
handler type field-for-field into `crate::runtime_types::*` and dropped the
original. `serde_json` was never applied to any of them.

### The conversions

| Round trip | Down (environment) | Up (server) |
| --- | --- | --- |
| `DateTime<Utc>` → `i64` → `DateTime<Utc>` | 13 `timestamp_millis()` calls | 13 `ms_to_datetime` / `opt_ms_to_datetime` calls |
| `Vec<u8>` → base64 `String` → `Option<Value>` | 3 `STANDARD.encode` | 3 `decode_base64_json` |
| `InstanceStatus` → `String` → `InstanceStatus` | passed through from `db` | `instance_status_from_string` |
| `PairedRecordStatus` → `String` → `StepStatus` | `match` to `"running"`/… | `step_status_from_string` |
| `EventType` → `&str` → `EventType` | filter built from string | `parse_event_type` |
| `&[u8]` → `String` → `Vec<u8>` | `p.as_bytes().to_vec()` | `String::from_utf8_lossy` |

Deliberately left alone: `duration_ms`, `avg/min/max_duration_ms` are genuine
durations, and `launched_at_ms` / `settled_at_ms` arrive from `runtara-core` as
`i64` and pass through untouched at both layers. The five `timestamp_millis()`
calls in `runner/embedded.rs` and `runtime_host.rs` face the WASM guest, which
really does speak milliseconds.

## 2. Two defects the round trip was hiding

### 2.1 `signal_shutdown` had never worked — confirmed

`EnvironmentClient::send_signal` mapped `SignalType::Shutdown` to the string
`"shutdown"`. `handlers::handle_send_signal` matched only `"cancel"` and
`"pause"`, so it returned `UnknownSignalType`, which the client turned into
`EnvironmentError::InvalidInput`.

Verified against `InMemoryPersistence` before the fix:

```
PROBE signal=cancel    outcome=Delivered
PROBE signal=pause     outcome=Delivered
PROBE signal=shutdown  outcome=UnknownSignalType { signal_type: "shutdown" }
```

`shutdown.rs::drain_executions` calls `RuntimeClient::signal_shutdown` for every
running synchronous execution so the guest checkpoints and exits cleanly. That
call always failed, the error was swallowed as a `warn!`, and the drain then
waited out the grace period. The `cancel_flag` still tripped, so executions did
unwind — but the durable `shutdown` signal that lets the SDK checkpoint first
was never written.

The rest of the crate understood the signal: `runtime_host.rs:519` decodes
`"shutdown"`, and `runtime.rs:632` writes `SignalType::Shutdown` straight
through `Persistence`, bypassing the handler. The handler's `match` was the only
place that disagreed, and a stringly-typed signal argument is what let it drift.

### 2.2 A non-JSON body read as no body

`decode_base64_json` ended in `serde_json::from_slice(&bytes).ok()`. A workflow
output that was valid base64 but not valid JSON became `None` — the same answer
the caller gets for an instance that produced no output at all.

### Three more, smaller

- `ms_to_datetime` ended `.unwrap_or_else(Utc::now)`: an unrepresentable instant
  became the current time, a fabricated value rather than an error.
- `String::from_utf8_lossy` on signal payloads substitutes U+FFFD for invalid
  UTF-8. Lossless in practice only because every caller passes
  `serde_json::to_vec`.
- `timestamp_millis()` truncated Postgres microsecond precision.

## 3. The shape moved to

No third type set. The handlers already held richer values than they returned —
`db::InstanceFull` carries `DateTime<Utc>`, `Option<Vec<u8>>` and `i32`. The
change was to stop downgrading them.

- Instants as `DateTime<Utc>`.
- Bodies as `Option<Vec<u8>>`. Environment stores opaque bytes and has no
  business asserting they are JSON; the server decides that.
- Statuses as the enums that already exist, converted with
  `runtara_store_postgres::encoding::status_from_str`.
- Signal payloads as `&[u8]` end to end.

`runtime_types` is unchanged. It is the server's own vocabulary; merging the two
crates' vocabularies would trade a cheap conversion for a coupling.

## 4. Phases

**Phase 0 — the shutdown signal.** Decode with the storage layer's parser so the
accepted set is by construction the set the column can hold.

**Phase 1 — instants.** 13 fields, 13 calls each way, all deleted. No test in
either crate asserted on a `*_ms` field, so the phase was safe but needed new
assertions rather than adapted ones.

**Phase 2 — bodies.** Base64 gone; the JSON parse stays on the server but is no
longer silent. Signal payloads take `&[u8]`.

**Phase 3 — enums.** The one behavioural question was settled rather than left
open: `instance_status` is declared in `001_initial_schema.sql` with exactly six
labels and no migration has ever `ALTER TYPE`d it, and `'sleeping'` is a
`termination_reason` label from `008_termination_tracking.sql` — a different
column. So the reader's `"sleeping"` arm and its `_ => Unknown` catch-all were
both unreachable, and both were deleted rather than carried across.

**Phase 4 — the layer left over.** See section 7 for how this landed, which
differs from what was first sketched.

## 5. Verification

Per phase: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
--features $GATE_FEATURES` (scoped `-p` hides required-features targets),
workspace unit tests, and the feature-gated integration suites against isolated
databases with `--test-threads=1` (the launch queue claims globally, so parallel
tests fight over the same rows).

## 6. Sequencing

Phase 0 was small, independent, and fixed a live defect. Phases 1–3 were
mechanical and compiler-guided. Phase 4 could not be scoped honestly until the
mapping was gone, because only then is it visible which layers do nothing.

## 7. What landed

Eight commits on top of `024c4c5c`:

| Commit | Phase |
| --- | --- |
| Accept the shutdown signal the drain path has always sent | 0 |
| Report instants as instants, not epoch milliseconds | 1 |
| Carry bodies as bytes instead of base64 strings | 2 |
| Stop deriving Serialize on types nothing serializes | 2b |
| Report statuses as the enums they already are | 3 |
| Let the image registry answer image reads | 4 |
| Let the instance repository answer instance reads | 4 |
| Read the status fixture through the repository it now belongs to | 4 |

Two things went differently from the plan.

**The `Serialize` derives had to go, and that was the load-bearing step.**
Phase 3 could not compile while the response structs still derived `Serialize`,
because core's status enums do not. Removing the eight derives and their
forty-six `skip_serializing_if` attributes compiled the whole workspace
untouched — which is the proof this plan asserted but could not demonstrate:
the JSON shape had no reader. It is its own commit because it stands on its own.

**Phase 4 stopped short of moving the checkpoint and event reads.** The first
sketch treated the identity mapping in `EnvironmentClient` as the thing to
delete. Most of those mappings are a crate boundary, not a redundant layer:
`handlers::ScopeInfo` and `runtime_types::ScopeInfo` are two crates' versions of
one idea, and one has to be built from the other. What was genuinely redundant
was a *handler* that added nothing over the owner of its table — true of the
image reads and the instance reads, both of which moved to `ImageRegistry` and
`InstanceRepository`. The checkpoint and event handlers each run a paginated
read plus a degraded count, which is assembly rather than pass-through, so they
stayed, as did `handle_list_step_summaries` (step vocabulary),
`handle_get_scope_ancestors` (ancestry reconstruction) and
`handle_get_tenant_metrics` (bucket validation).

Moving the instance reads also let `InstanceStatusResponse` lose its `found`
flag and the `not_found()` constructor that filled twenty fields with `None`.
Absence was modelled that way so HTTP could answer it with a 200; in-process it
is an `Option`, and the only caller was already turning it back into an error.

### Still open

`EnvironmentClient` constructs `db::ListInstancesOptions` directly, so
`handlers` is not the crate's front door and `db` is not private. Settling that
changes the crate's public surface and wants its own review.

### Fixed along the way

- `signal_shutdown` never worked (section 2.1), confirmed by a failing test
  before the fix.
- Instance timestamps were truncated from Postgres microseconds to milliseconds.
- A non-JSON body was indistinguishable from no body; it is now logged.
- Signal payloads no longer pass through `String::from_utf8_lossy`.
- Two unreachable arms in the status reader are gone, and `base64` is no longer
  a dependency of `runtara-environment`.
