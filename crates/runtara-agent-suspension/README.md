# runtara-agent-suspension

Typed agent suspension: the Rust side of `runtara:agent/suspension` (the WIT
lives in `runtara-wit`) and the vocabulary guest agents, the compiler and the host share. A capability
declared `suspends` may answer an invocation with `suspended { wakes, state }`
instead of a result; the host keeps `state`, parks the calling workflow
without holding a runner, and re-invokes the capability with that state when
any wake fires. No built-in agent suspends: it is the extension point for
long-polling agents. A run that waits on other runs uses the
`WaitForInstances` step instead, and an agent suspension may not name an
instance wait (`AGENT_INVALID_SUSPENSION`).

## Contract

- [`wit/runtara-agent-suspension.wit`](wit/runtara-agent-suspension.wit):
  `types` (`wake`: `at(epoch-ms)` or `instances(wait-id)`; `suspension`;
  `outcome`), `context` (`continuation()` for ordinary suspending agents) and
  the `suspending-agent` world. Released WIT is never edited, only versioned.
- A suspending agent exports `runtara:agent-<id>/suspendable@0.4.0`
  (`SUSPENDABLE_INTERFACE_WIT`) beside `capabilities`, whose plain `invoke`
  refuses a suspending capability.
- `MAX_CONTINUATION_BYTES` = 64 KiB of state per step operation and attempt;
  at most `MAX_WAKES` (16) wakes; instance wait ids of 1-64 bytes.
  `validate_suspension` checks the same caps the host enforces.

## Error codes

| Code | When |
|---|---|
| `AGENT_INVALID_SUSPENSION` | The host refused a suspension (no wakes, too many, oversized state, or an instance wake the operation did not register); the step fails instead of parking. |
| `AGENT_CONTINUATION_REJECTED` | A capability refused the state it was handed (for example a version it does not read). The failed step discards its state, so a retry starts afresh. |
| `SUSPENSION_UNSUPPORTED` | A suspending capability was invoked through a path that cannot park (plain `invoke`, a test invocation). |
| `AGENT_UNEXPECTED_SUSPEND` | A suspension came back through a path that cannot park (a non-suspending capability, plain `invoke`). |

A suspension within one second of the step timeout fails with
`AGENT_TIMEOUT` instead of parking.

## Authoring

With `runtara-agent-macro`, mark the capability `suspends = true`, take a
`&SuspendContext` and return `Suspendable<Output>`, and list it under
`suspending = [...]` in `agent_component!` (see
`runtara-agent-macro/tests/ui/pass/ordinary_suspending_agent.rs`). The
catalog then reports `suspends: true`, and validation requires the step to be
durable (E028) with a timeout above zero (E029); see
`runtara_dsl::step_context_rules` for where such steps may sit.

## Users

- `runtara-agent-macro` generates the `suspendable` export and continuation
  glue.
- `runtara-workflows` emits suspension handling only for steps whose
  capability suspends.
- `runtara-component-host` keeps continuations (`instance_agent_continuations`,
  deleted with the run), attaches instance waits and parks the run.
