# Start-time run labels

Status: implemented.

## Purpose

Make `runLabel` an optional, stable reference supplied when an execution starts.
For example, an order-processing execution started with `runLabel: "order-123"`
must be discoverable by workflow ID and order ID while queued, running, waiting,
or finished.

This supports reports that query workflow instances without querying business
objects directly. It also supports execution lookup outside reports.

## Previous behavior

- The top-level Finish step accepts a `runLabel` mapping. The value is resolved
  and stored on successful completion, so it cannot identify a pending approval
  before that execution finishes.
- The public execute request has no `runLabel` parameter.
- The general execution-list API already supports workflow ID, statuses,
  creation/completion date ranges, and exact label matching. SQL filtering is
  applied before pagination.
- The report workflow-runtime provider uses a narrower execution-list path and
  filters fetched rows afterward. It can miss matching executions outside the
  fetched page.
- Label validation currently trims spaces, restricts the character set, and
  silently truncates values to 250 characters. Invalid dynamic Finish labels
  are ignored. These display-oriented rules are unsuitable for exact external
  references.

Relevant code: [Finish schema](../crates/runtara-dsl/src/schema_types.rs),
[label normalization](../crates/runtara-dsl/src/run_label.rs),
[execute request](../crates/runtara-server/src/api/dto/workflows.rs), and
[execution query contract](../crates/runtara-server/src/api/dto/executions.rs).

## Contract

Accept `runLabel` as execution metadata alongside inputs, rather than as a
workflow output or DSL expression:

```json
{
  "inputs": { "orderId": "order-123" },
  "runLabel": "order-123"
}
```

- Persist the label atomically with execution creation, before work can start.
- Keep it immutable through retries, suspension, resume, success, failure,
  timeout, and cancellation. Completion must not clear or replace it.
- Preserve accepted strings exactly. Do not truncate, case-fold, or silently
  discard invalid values. Validate starts and exact-match queries consistently.
- Keep labels optional and non-unique. Multiple executions may concern the same
  order; queries return all matches unless further filtered.
- Scope lookup to the authenticated tenant. Support combined workflow ID,
  label, status, and date filters with deterministic pagination and matching
  total counts.
- Keep label correlation separate from instance identity and idempotency. The
  same label does not deduplicate independent starts. Replayed starts must not
  mutate an existing instance's label.
- Retain the existing display fallback to workflow name when no label exists.

A label identifies the business context, not the current stage. Execution
statuses such as `suspended` are distinct from business stages such as
`awaiting_finance_approval`. General business-stage or checkpoint-state querying
is outside this change.

## Implementation scope

1. Thread the optional label through public start requests, relevant MCP/SDK
   entry points, queue/trigger transport, Environment registration, and Core
   persistence. Audit other execution-start paths for consistent behavior.
2. Preserve labels during every terminal and non-terminal state transition;
   do not merely add the start field while retaining completion-time overwrite
   behavior.
3. Retire Finish-step label assignment across the DSL, validation, compiler,
   runtime completion interfaces, and frontend editor. Keep existing storage,
   display, and query support. Use a targeted migration rather than reverting
   unrelated execution-query improvements.
4. Reuse the existing filtered instance-query machinery for workflow-backed
   reports. Apply filters before pagination instead of filtering a capped first
   page. Review indexes for tenant/workflow/label lookup and status/date queries.
5. Regenerate affected schemas and API clients through their existing tooling.
   Add forward SQL migrations if needed; do not edit committed migrations.

Inline embedded workflows share their parent's execution and cannot assign a
separate instance label. Independently started executions can receive their own
label through the start contract.

## Contract decisions

- Accept 1–250 printable ASCII bytes, including `_` and ordinary spaces, with
  at least one non-space character. Preserve the entire accepted string exactly.
  Omission/null means no label; empty, control, non-ASCII, and oversized strings
  are rejected on both starts and exact lookup.
- No backward compatibility is required for this unused feature (confirmed by
  the user). Remove `Finish.runLabel` and label-aware completion APIs entirely;
  old Finish fields are rejected and components must be rebuilt.
- Keep existing persisted labels as historical metadata. Do not fabricate labels
  for unlabeled executions or infer them from inputs.
- An idempotent start replay with a different label returns a conflict and never
  mutates the original execution or its label. Duplicate labels on independent
  starts remain valid.
- JSON execute requests use `runLabel`; MCP start tools use `run_label`. Raw HTTP
  webhook and synchronous endpoints accept `?runLabel=...`, preserving the raw
  request body as workflow input. Automatic chat/session/cron/report/replay
  starts remain unlabeled unless their caller explicitly supplies metadata.
- The label is stored in the source outbox before handoff, then atomically with
  the Core instance and Environment launch. Execution queries expose it once
  that instance is registered, including while the durable launch is queued.

## Acceptance criteria

- A labeled execution is discoverable before completion, including while waiting
  for a signal, and retains its label through all lifecycle transitions.
- Duplicate labels return distinct instances; tenant isolation and existing
  idempotency behavior remain intact.
- Exact lookup preserves valid identifiers and rejects invalid/oversized values
  without truncation or accidental collisions.
- Given 1,000 executions with 10 matching unfinished runs outside the first page,
  filtering finds all 10 with correct counts and pagination.
- Workflow ID, label, statuses, and date boundaries compose correctly.
- Unlabeled starts work; Finish label assignment is rejected and rebuilt
  components use the plain completion API.
- Focused lifecycle, query, and contract-rejection tests cover these behaviors; run
  affected crate checks, component checks if interfaces change, and frontend/API
  generation checks as applicable.

## Local regression tests

- `e2e/test_start_run_labels.py` runs against a local server with compiled
  components. It exercises waiting/resume/completion/cancellation, duplicate
  labels, idempotent conflicts, exact filters and inclusive date boundaries,
  synchronous starts, invalid labels, and rejection of Finish assignment.
- `e2e/test_run_label_reports.py` additionally needs `psql` and an explicitly
  selected isolated runtime database through `PGHOST`, `PGPORT`, `PGUSER`, and
  `PGDATABASE`. It seeds 1,000 instances and verifies all 10 older matching runs
  across report pages, including SQL-filtered and residual-condition paths.
- Both use `RUNTARA_API_URL` (default `http://127.0.0.1:17760/api/runtime`) and
  create unique fixtures without deleting existing data.
