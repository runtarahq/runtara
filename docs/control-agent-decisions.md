# Control agent: decisions

Decided 2026-09-26. Everything not listed here is an engineering default,
recorded in the implementation plan,
[control-agent-implementation-plan.md](control-agent-implementation-plan.md).

| # | Decision | Decided |
|---|---|---|
| D1 | Can a workflow answer another process's pending approval? | Only its own children, its ancestors, and requests that opt in with `action.key`. |
| D2 | Host verification that only the approved control agent can call control | Revised 2026-09-29: only the canonical control agent from the primary components dir may import `runtara:control/*`, enforced by the compile-time import allowlist; the host verifies nothing further, and `runtara:control/api` is real only in a workflow store with a run identity. Was: check the composed bytes at load and on each call. See [control-simplification.md](control-simplification.md). |
| D3 | Running children when the parent ends | The author chooses `cancel` or `leave_running`, and the editor preselects `cancel`. Cancel fires on any parent ending, with a 5 s grace. |
| D4 | Pausing a waiting run | It pauses immediately, in control and in the public API. This needs a release note. |
| D5 | Pricing and capacity | Control is available on every tier. Children count against the tenant concurrency limit, which counts only starting or running runs: a parked run gives its slot back. At the limit, `start` returns a retryable capacity error. Control-started children may hold at most `max(1, floor(0.8 × limit))` slots, so outside triggers keep headroom. |
| D6 | Runaway starts | Lineage depth is capped at admission (16). |
| D7 | `start` of a missing or broken workflow | Fails immediately with `not-found` or `not-runnable`. "Not compiled yet" is accepted. |
| D8 | Parent in the public API and history UI | Yes: a `parentInstanceId` field and filter. |

## Ops answers

- **No query outside the repository searches `instances.run_label`.** The label migration therefore drops the unused trigram index `idx_instances_run_label_search` instead of rebuilding it. The exact-match btree stays.
- **The largest `instances` table is assumed to hold thousands to tens of thousands of rows.** The widening lock is under a second, so a plain transactional migration is used.
- **Whether any deployment sets `RUNTARA_DIRECT_RUNTIME_BINDING=composed` is still unanswered.** Until it is confirmed, slice 1 removes `RUNTARA_HTTP_URL` from guests, but it does not fail closed on the composed binding.
