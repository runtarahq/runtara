# Independent step cancellation: executable proof

Follow-up: [production research, measurements, and tradeoffs](isolated-step-production-research.md).
It adds four executable assumption checks and a manual real-agent benchmark.

## Result and architectural boundary

The experiment demonstrates that a parent WASM component can kill a
non-cooperative child step and continue its own recovery and sibling execution,
**provided each step has a separate Wasmtime Store**. It does not establish that
one task can be forcibly removed from the existing shared Store.

The workflow remains a self-contained WASM artifact: the parent contains its
child component binaries in data segments. Its WASM instructions decide which
children to start, which user command cancels a child, which outcome starts
recovery, and when the sibling may finish. The host has no graph, branch rules,
retry policy, component registry, or workflow-specific scheduling decisions.

The required architectural change is a generic host execution interface for
disposable Stores. This is a test-only proof of that interface, **not a DSL
feature or a change to production execution**. Existing workflows still use
their current composition and E128 rules. If “self-contained” also requires all
components to execute in one Store with no new execution imports, this solution
does not satisfy that additional constraint.

## Run the proof

From the repository root, with the pinned Rust toolchain:

```sh
RUSTC_WRAPPER= cargo test -p runtara-component-host \
  --features isolated-step-poc --lib isolated_step_poc -- --nocapture
```

No database, credentials, external services, prebuilt agent bundle, or special
WASM toolchain is needed for this command. HTTP tests use ephemeral loopback
servers. WAT fixtures are assembled and compiled in memory using the existing
test dependencies and the production `build_engine` configuration.

The feature includes the experiment only under `cfg(test)`. Enabling it in a
normal library build does not expose new imports or alter production code.

Source: [executor and tests](../crates/runtara-component-host/src/isolated_step_poc.rs),
[parent WASM](../crates/runtara-component-host/tests/fixtures/isolated_step_poc/parent.wat),
[CPU victim](../crates/runtara-component-host/tests/fixtures/isolated_step_poc/cpu.wat),
[HTTP victim](../crates/runtara-component-host/tests/fixtures/isolated_step_poc/http.wat).

## Execution contract

The miniature proof ABI is deliberately separate from the production Agent WIT:

| Host primitive | Mechanism; policy remains in WASM |
|---|---|
| `spawn(component bytes, input) → handle` | Instantiate and execute the supplied component in a new Store. |
| `cancel(handle)` | Set only that execution's cancellation flag, wake its pending call, and advance the shared engine epoch. |
| `join(handle) → tagged outcome` | Wait until the execution and its Store have been destroyed; return success, cancellation, or trap. |
| `send(handle, message)` | Deliver a message to that child. |
| `receive-command() → message` | Wait for external input; the parent decides how to interpret it. |

Children export `run(u32) → u32`. The join outcome uses a separate high-word
status tag so arbitrary guest return values cannot masquerade as cancellation.
The parent preserves a memory sentinel; the sibling preserves its own input in
linear memory while waiting for the parent to release it.

The host owns resource lifetime, just as it already owns memory, I/O, and whole
workflow execution limits. The parent owns the orchestration:

```text
parent WASM: spawn victim + sibling → await user command → cancel victim
             → join cancellation → run recovery → release/join sibling → finish

host:        separate Store per invocation; generic cancellation and teardown
```

For a CPU loop, the victim's epoch callback traps at a WASM interruption point.
Other Stores share the engine but check their own flags and keep running. For
pending I/O, cancelling the outer call and dropping that child Store disposes of
its host futures. This is **not** merely dropping a `call_concurrent` result future
while leaving its task alive in a shared Store.

## What the tests establish

| Test | Evidence |
|---|---|
| `guest_cancels_cpu_bound_step_and_preserves_parent_and_live_sibling` | Victim loops forever with no await or cancellation check; parent receives cancellation, runs recovery, preserves memory, and joins the still-live sibling. |
| `guest_cancels_infinite_component_initialization_and_continues` | Cancellation also covers guest initialization before the exported entry point is reached. |
| `guest_cancels_http_waiting_for_headers_and_continues` | A real request using production host-io is accepted by a server that never responds; user cancellation closes the client connection and parent execution continues. |
| `guest_cancels_http_stalled_body_and_continues` | Same proof with a server that sends headers and a partial body but never completes it. |
| `no_cancel_command_leaves_busy_step_and_parent_running` | Several engine ticks do not accidentally end the victim; sending the command to that same parent then succeeds. |
| `duplicate_cancel_is_safe_and_does_not_cancel_the_sibling` | Repeated requests do not change the outcome or destroy sibling execution. |
| `cancellation_after_completion_preserves_the_result` | Late cancellation preserves the already published successful result. |
| `child_trap_is_contained_and_parent_runs_recovery` | A child `unreachable` trap is returned to the parent rather than killing it. |
| `parent_trap_cancels_and_reaps_its_live_children` | Parent teardown cancels its live children before test-harness cleanup; no recovery is invented by the host. |
| `arbitrary_guest_return_value_cannot_forge_a_cancelled_outcome` | A returned `u32::MAX` remains a success payload, not a cancellation tag. |

Successful cancellation assertions require this order:
`cancel requested → victim Store dropped → join returns → recovery starts`.
The sibling Store must outlive the victim and return its normal result. The CPU
and HTTP cancellation timing assertions allow five seconds for loaded CI; this
is a test bound, not a production latency SLA. HTTP requests have 120-second
deadlines, so their success cannot be explained by the transport timeout. A
separate 15-second test failsafe is an error, never a successful cancellation.

## Limits and next implementation work

- This proves interruption of arbitrary **guest WASM computation** under
  Wasmtime's execution guarantees, including non-cooperative loops. It does not
  kill arbitrary blocking native host code, cancel JIT compilation, recover from
  a process crash, or roll back remote side effects.
- Child guest memories are limited to 1 MiB each, with bounded instance/memory/
  table counts. Package sizes are bounded for the fixture layout. Aggregate
  admission limits, production task quotas, caching and performance are not
  established by these tests.
- Production `Agent`/`EmbedWorkflow` lowering, JSON/WIT argument marshalling,
  tenant-scoped handles, connection capabilities, and nested task trees would
  need integration. The embedded-child proof uses a component boundary; it does
  not make an already inlined `EmbedWorkflow` independently disposable.
- Durable replay needs stable invocation IDs, persisted cancellation outcomes,
  result publication fencing, and defined completion/cancellation races. The
  PoC proves live execution isolation, not that durability contract.
- The existing UI Stop/cancel signal path is unchanged. The tests inject a user
  command into the parent, which explicitly chooses a victim. No production
  cancellation API has been rewired.

## Original proof verification

Verified locally on 2026-09-06 in `codex/wasm-emitter-audit`, based on merge
commit `0a04277c` (upstream `main` at `6f7db0c4`), using pinned Rust 1.97.0 and
Wasmtime 46.0.1:

- `cargo test -p runtara-component-host --features isolated-step-poc,component-integration-tests`:
  **55 passed, 0 failed, 0 ignored**, including all **10 PoC cases** and the
  existing dispatcher/component integration tests.
- `cargo test -p runtara-workflows --features direct-wasm-integration-tests`:
  **565 library, 221 composed execution, and 54 native integration tests passed**,
  including all **77 emitter audit cases**. One pre-existing doctest is ignored.
- `cargo clippy -p runtara-component-host --all-targets --features isolated-step-poc,component-integration-tests -- -D warnings`:
  passed.
- Workspace formatting, diff whitespace checks, documented test names, and local
  documentation links: passed.

The component CI test command now enables `isolated-step-poc`, so the proof runs
with the existing component integration gate. That remote CI run has not been
performed for these local changes. No production guest source or WIT changed;
existing workflow regressions used the previously rebuilt worktree-local agent
bundle. Database/service E2E, production rollout, durable cancellation/replay,
aggregate resource limits and performance benchmarks were not tested in this
original proof run. Follow-up research and its verification are recorded in the
linked production research document.
