# runtara-control-contract

The numbers and names every side of the control service agrees on:
`runtara-agent-control` (the guest), `runtara-component-host` (the executor)
and `runtara-server` (the `NativeControl` service). The ABI itself is the WIT
in `runtara-workflow-wit/wit/control` (`runtara:control@0.1.0`); this crate is
serde-only so all three can depend on it.

## What it fixes

| Item | Value |
|---|---|
| Control call input / outcome / response | 1 MiB / 4 MiB / 4 MiB |
| One control call | 90 s (`CONTROL_TIMEOUT`) |
| `get` inlined output / error | 1 MiB / 64 KiB |
| `wait` targets, inlined output / error per child, total | 1000; 256 KiB / 16 KiB; 3 MiB |
| Page size | 1-100 |
| Lineage depth (decision D6) | 16 |
| Run label | 1024 bytes (equals `runtara_dsl::run_label::MAX_RUN_LABEL_LENGTH`) |
| `cancel` grace | 0-3600000 ms, default 5000 |
| Parent-close grace (decision D3) | 5000 ms |

- `control_share(limit)` = `max(1, floor(0.8 × limit))`, the slots
  control-started children may hold (decision D5); `capacity_satisfiable`
  is false for a limit of at most 1.
- `capacity` splits on its retry hint: `CONTROL_CAPACITY_RATE_LIMITED`
  (retryable, 3-8 s hint from `capacity_retry_after_ms`) or
  `CONTROL_CAPACITY_UNSATISFIABLE` (permanent).
- `ErrorCode` mirrors the WIT `error-code` case for case;
  `agent_code` gives the agent-facing `CONTROL_<CODE>` and `agent_error` the
  `#[capability]` error envelope. `all_agent_codes` lists every code a control
  step can fail with.
- `REQUIRES_RUN_TAG` (`runtime:requires-run`) marks capabilities that only run
  inside a workflow run; a test invocation answers
  `CONTROL_REQUIRES_INSTANCE`.
- `ParentClosePolicy` (`cancel` | `leave_running`) and `start_input_schema`,
  where the policy is required and has no default.
- `CONTROL_CONTINUATION_V1`, the version of the control agent's `wait`
  continuation.

The author-facing reference built from these values is `controlAgent` in the
MCP workflow authoring schema and "As built" in
[docs/control-agent.md](../../docs/control-agent.md#as-built); a server test
fails when they drift.
