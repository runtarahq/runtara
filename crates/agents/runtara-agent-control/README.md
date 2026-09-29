# runtara-agent-control

The built-in `control` agent: a workflow starts child runs, reads runs of its
tenant, answers their signals, and pauses, resumes and cancels its children.
Waiting for children without holding a runner is the `WaitForInstances` step.
Headline use: parallel approvals.
It is available on every pricing tier, whatever the agent allowlist says.

## Capabilities

| Capability | Kind |
|---|---|
| `get`, `get-state`, `query`, `list-pending-signals` | Reads, tenant-wide |
| `start` | Admit a child run (`parentClosePolicy` required) |
| `send-signal` | Answer a WaitForSignal request (children, ancestors, `action.key` opt-ins) |
| `cancel`, `pause`, `resume` | Direct children only |

`start` and the mutations carry the `runtime:requires-run` tag: a test
invocation answers `CONTROL_REQUIRES_INSTANCE`. The full author reference
(authorization, capacity, statuses, limits, replay, validation codes,
lifecycle) is "As built" in
[docs/control-agent.md](../../docs/control-agent.md#as-built).

## How it runs

An ordinary composed agent: each capability makes one `runtara:control/api`
call, which the host makes real in the run's own store (and `denied` anywhere
else), bounded by the step's deadline and 90 s. The compiler grants
`runtara:control` to this agent alone. Mutations run in a compiler-emitted
`runtara:workflow/operation` scope, so a replayed step never applies twice.

## Building

```bash
./scripts/build-agent-components.sh
```

The `agent_component!` macro generates its `runtara:agent-control@1.0.0`
package from `runtara_wit::agent_package`; there is no WIT file to edit. The
component imports `runtara:control/api@1.0.0` and exports `capabilities`.
Output:
`target/wasm32-wasip2/release/runtara_agent_control.wasm` and its
`.meta.json` sidecar.

## Tests

Unit tests in `src/lib.rs` pin metadata, tags and error codes. Host and
end-to-end coverage: `runtara-component-host/tests/control_agent.rs`,
`runtara-server/tests/control_component.rs`,
`runtara-environment/tests/wait_for_instances_runner_test.rs` (a control call
after a parked wake) and `e2e/test_control_agent.sh`.
