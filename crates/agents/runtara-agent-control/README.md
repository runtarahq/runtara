# runtara-agent-control

The built-in `control` agent: a workflow starts child runs, reads runs of its
tenant, answers their signals, and pauses, resumes and cancels its children.
Waiting for children without holding a runner is the `WaitForInstances` step.
Headline use: parallel approvals.
It is available on every pricing tier, whatever the agent allowlist says.

## Capabilities

| Capability | Kind |
|---|---|
| `get`, `query`, `list-pending-signals` | Reads, tenant-wide |
| `start` | Admit a child run (`parentClosePolicy` required) |
| `send-signal` | Answer a WaitForSignal request (children, ancestors, `action.key` opt-ins) |
| `cancel`, `pause`, `resume` | Direct children only |

`start` and the mutations carry the `runtime:requires-run` tag: a test
invocation answers `CONTROL_REQUIRES_INSTANCE`. The full author reference
(authorization, capacity, statuses, limits, replay, validation codes,
lifecycle) is "As built" in
[docs/control-agent.md](../../docs/control-agent.md#as-built).

## How it runs

The copy composed into a workflow only forwards to
`runtara:control/executor`. The host's `ControlExecutor` runs the installed
bytes of this crate in a fresh store per call, where `runtara:control/api` is
real, and checks the workflow's control pin against the approved history
(`approved_builtin_artifacts`) on every call. Mutations run in a
compiler-emitted `runtara:workflow-operation` scope, so a replayed step never
applies twice.

## Building

```bash
./scripts/build-agent-components.sh
```

`build.rs` generates `wit/agent.wit` (package `runtara:agent-control@0.4.0`)
from `runtara-agent-wit/templates/control-agent.wit.in`; do not edit it. The
component imports `runtara:control/executor@0.1.0` and
`runtara:control/api@0.1.0`, and exports `capabilities` and
`runtara:control/execution@0.1.0`. Output:
`target/wasm32-wasip2/release/runtara_agent_control.wasm` and its
`.meta.json` sidecar.

## Tests

Unit tests in `src/lib.rs` pin metadata, tags and error codes. Host and
end-to-end coverage: `runtara-component-host/tests/control_agent.rs`,
`runtara-server/tests/control_component.rs`,
`runtara-environment/tests/wait_for_instances_runner_test.rs` (a control call
after a parked wake, across upgrades) and `e2e/test_control_agent.sh`.
