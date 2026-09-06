# Cooperative cancellation implementation record

Status: initial standards proof, 2026-09-06. Governing contract:
[cooperative cancellation plan](selective-isolation-plan.md). This record does not
supersede the plan's full scope. Production defaults are unchanged.

## P0: current ABI inventory

Inspected the existing source and the dependencies resolved by the current build:

- Wasmtime 46.0.1 has Component Model async enabled in the host dependency. The
  proof uses the existing engine builder, without optional async extensions,
  custom task imports or isolated execution.
- `runtara:agent/capabilities.invoke` and workflow lifecycle `invoke` are
  async-typed. The HTTP agent uses `wit_bindgen::generate! { async: false }`;
  the workflow lifecycle explicitly documents its synchronous lift.
- `runtara-http/src/host_io.rs` similarly synchronously lowers the async-typed
  request import. Its concurrent host implementation permits overlap, but that
  does not make the blocked guest call acknowledge cancellation.
- The resolved guest binding generator is wit-bindgen 0.58. Its standard async
  support receives cancellation event 6 and destroys the guest future, invoking
  destructors and standard subtask cancellation. Switching export typing alone
  does not select that implementation.
- Generated parallel drains already use waitable sets and poll lifecycle signals,
  but an operation must wake the drain before polling can observe a newly arrived
  signal. Signal readiness/timed polling must participate in the wait itself.
- `RuntimeHost::is_cancelled` acknowledges the existing root Cancel when observed;
  the new path must distinguish that from cleanup/terminal publication as the
  plan requires. No new signal transport or acknowledgement changes are made yet.

No feasibility result below is a production performance baseline. The historical
measurements remain linked from the plan; fresh paired measurements are pending.

## P1: standard composed-component proof

Added the automatically discovered integration target
[`cooperative_cancellation.rs`](../crates/runtara-component-host/tests/cooperative_cancellation.rs)
with two handwritten WAT fixtures. It runs one standard composed component in one
Store, with parent workflow and two agent instances composed as peers. It uses
standard async calls, waitable sets, `subtask.cancel`, `subtask.drop` and
`task.cancel`. There is no package catalog, task resource, launcher, invocation
ledger or per-invocation Store.

The signal import is a test readiness gate, not the production lifecycle signal
interface. The I/O import is a test transport, not a built HTTP agent. These
boundaries are deliberate and remain outstanding for full P1 completion.

| Test | Evidence |
|---|---|
| `standard_cancellation_preserves_composed_sibling_and_instance_state` | Both operations start before the signal; parent WASM requests cancellation; native I/O future destruction precedes guest acknowledgement, which precedes parent continuation. Sibling finishes and the same cancelled agent instance is invoked again with its global call counter preserved. |
| `callee_can_return_a_value_instead_of_acknowledging_cancellation` | Callee cleans up but returns normally; parent receives the standard returned state and checks the result instead of assuming a cancellation outcome. |
| `standard_cancellation_closes_pending_http_headers` | Controlled loopback endpoint withholds headers; standard cancellation drops the pending request and the endpoint observes connection closure. |
| `standard_cancellation_closes_pending_http_body` | Endpoint sends headers and an incomplete body; cancellation closes the connection before the response finishes. |
| `async_typing_with_synchronous_bindings_does_not_acknowledge_cancellation` | Same parent/control path with the existing agent ABI shape remains unresolved until the test watchdog. A trap or invalid fixture does not count as the expected result. |

The positive tests also require reusing the original agent instance successfully;
throwing away that instance would fail the global-state assertion. Trace assertions
check the actual order of I/O destruction, guest acknowledgement and parent
continuation. The server never supplies a full response to make a cancellation
case pass. Test server tasks are owned and cleaned up on errors.

Two ABI constraints were established while constructing the proof:

1. Standard composition connects peer components. A core caller in an ancestor
   component cannot use an adapter to reenter its nested descendant; the first
   fixture arrangement correctly trapped. The final fixture follows the normal
   sibling-component composition shape.
2. Before synchronous subtask cancellation, remove the subtask from its waitable
   set with `waitable.join(handle, 0)`. Otherwise the pinned runtime traps because
   a waitable cannot be used synchronously while it belongs to a set. Wait for
   resolution before dropping the subtask and its set.

## Verification and remaining implementation

All five integration tests passed against Wasmtime 46.0.1, including the
controlled HTTP header/body cases and the synchronous-binding negative case.
`cargo fmt --all -- --check`, `git diff --check`, and
`cargo clippy -p runtara-component-host --all-targets -- -D warnings` passed. The test command was
`cargo test -p runtara-component-host --test cooperative_cancellation`, with
`RUSTC_WRAPPER=`, `SQLX_OFFLINE=true`, and the existing isolated target directory.
No built Agent, DSL, database or server E2E qualification was claimed or run in
this proof; guest production sources and WIT have not changed.

Next implementation: expose a cancellation-capable async HTTP transport/agent
binding using the existing request interface and preserve request shaping,
connection proxy behavior and error coercion. Prove the built agent in standard
composition, then wire emitted workflow waits to existing lifecycle signals.
Keep the synchronous/reference path until differential behavior is qualified.

P1 remains incomplete until the built HTTP agent and emitted DSL pass; P0 still
needs the fresh baseline and experimental-artifact usage inventory. The complete
construct parity, persistence/lifecycle races, timeout integration, emergency
abort qualification, size/timing comparison, Linux capacity and local-server E2E
gates remain pending. No production code or default was changed by this proof.
