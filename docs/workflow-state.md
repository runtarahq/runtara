# Queryable workflow state

Status: proposal, 2026-09-28; decisions recorded below. Nothing here is implemented yet.

## Problem

A running workflow is durable, but from the outside it is a black box until
it finishes. Nothing can read what a run knows while it runs or waits:

- checkpoints are opaque bytes per step, meant for replay, not for readers;
- `variables` are constants;
- a run label is set once, when the run is registered;
- a waiting request's `context` is readable only while the run waits at that
  `WaitForSignal` (the workflow actions API);
- execution listings return run metadata only (status, times, label), not what
  the run holds.

So authors copy state into object-model tables to make it readable: a second
source of truth, wired by hand in every workflow.

Workflows are durable, so a workflow can itself be the storage. What is
missing is one general mechanism: **a run exposes typed state that others can
query.** Operational UI is the first reader: progress, approvals and in-place edits
all become "read state, act by sending to the run", with nothing
case-specific in the platform.

## Goals

- A workflow declares its **state** as a typed contract.
- The workflow computes its state **with its own logic** and writes it with
  explicit steps.
- State is **durable**: readers only see state the run will not take back.
- Readers get **one run's state, or lists of runs filtered and sorted by
  state**, without waking the run.
- Workflows read state through ordinary read-only steps; the API exposes the
  same reads for operational UI.

## Non-goals

- Computing state **on read**, by running code inside the workflow
  (query handlers). See [Why state is stored](#why-state-is-stored-not-computed-on-read).
- Case-specific steps (progress, decisions, audit logs). Those are state the
  author chooses to keep.
- Replacing the object model for analytics over large data.

## What exists today

| Piece | Where | Relevance |
|---|---|---|
| Checkpoints: first write wins per `(instance, checkpoint_id)`; a replay gets the saved bytes back | `crates/runtara-core/src/instance_handlers/checkpoint.rs` | State rides on them |
| Checkpoint save and run pointer update are two separate writes | same, `save_checkpoint` then `update_instance_checkpoint` | Needs one transaction with state |
| Workflow `durable` (default `true`; `false` compiles with no checkpoint reads or writes) and step `durable` | `crates/runtara-dsl/src/schema_types.rs` | Decides whether state is kept |
| Custom signals stored per checkpoint | `put_custom_signal` / `get_custom_signal` in `crates/runtara-core/src/persistence/mod.rs` | Basis for messages to a running workflow |
| Workflow actions API: open input requests of a run | `crates/runtara-server/src/api/services/pending_inputs.rs`, `workflow_runtime.rs` | Answers stay here; state reads sit beside them |

## Concepts

### State schema

A workflow declares `stateSchema` next to `inputSchema` and `outputSchema`,
in the same field format:

```json
"stateSchema": {
  "orderId":   { "type": "string" },
  "stage":     { "type": "string", "enum": ["waiting", "approved", "rejected"] },
  "decidedAt": { "type": "string", "format": "date-time" }
}
```

State is the run's public contract, separate from its internals: refactoring
steps does not break readers, and intermediate outputs or secrets are not
exposed unless the author maps them into state.

### Updating state: the `state` agent

State changes only through explicit steps of a built-in `state` agent:

- **`state:set`** merges the given fields into the run's state (shallow: each
  given field replaces its value; a field set to `null` is cleared; arrays are
  replaced). Fields not given are kept.
- **`state:get`** returns the run's current state, for read-modify-write inside
  the workflow (a loop that adds to a counter or a list).

A new run's state is **empty**.

```json
"record": {
  "stepType": "Agent", "agentId": "state", "capabilityId": "set",
  "inputMapping": {
    "stage":     { "valueType": "template",
                   "value": "{% if steps.approve.outputs.approved %}approved{% else %}rejected{% endif %}" },
    "decidedAt": { "valueType": "reference", "value": "steps.now.outputs" }
  }
}
```

Why an agent rather than a mapping on every step:

- every write is a visible step on the canvas, not a side effect of another
  step type;
- the agent appears in the catalog with a form like any other;
- "where does state change" is a search for `state:set`.

The workflow computes its state with its full logic: a `state:set` step's
inputs can reference any step's output.

Rules:

- **Own run only.** `set` and `get` act on the calling run. Other runs are
  read with [`query-runs`](#reading).
- **Typed.** A `set` step's fields must exist in `stateSchema`, and their types
  are checked, the way a Finish is checked against `outputSchema`. A `state`
  step in a workflow without `stateSchema` is an error.
- **Implementation note.** A host-backed capability: the host supplies the
  calling run and the step's checkpoint key, so authors never pass them.

### Durability

State is exactly as durable as the checkpoint of the step that writes it.

- **Durable workflows.** A `state:set` write is stored **first-write-wins under
  the calling step's checkpoint key**, in the same transaction that updates the
  run's current state. A replaying `set` step finds its write already stored
  and changes nothing, even when a crash fell between the write and the step's
  checkpoint, and even when the value came from a non-durable step. Replaying
  old steps never writes an older state over a newer one.
- **Non-durable workflows** (`durable: false`). State is ignored:
  `state:set` does nothing, `state:get` returns an empty state, and reads
  return no state for the workflow's runs. Validation warns the author
  (**W081**, *state is ignored in a non-durable workflow*). The platform does
  not make the workflow or its steps durable on its own.
- **A `state` step marked `durable: false`** in a durable workflow has no
  meaning: validation warns and the flag has no effect.
- **Parallel branches.** Each `set` merges its fields, so branches writing
  different fields keep both; for the same field, the write that commits last
  wins.

### Storage

In runtara-core, which owns durable execution state (forward migration):

- the run record gains `state` (JSONB, empty for a new run) and
  `state_updated_at`;
- each `set` write is kept under its step's checkpoint key, which makes it
  first-write-wins on replay;
- a size cap on the state document (for example 64 KiB, configurable), so
  state stays a summary, not a store for large collections;
- state is kept and deleted **together with its run**, under the workflow's
  retention.

### Reading

Reads come from the stored state. They never wake, resume or replay a run, so
they work the same for running, waiting and finished runs.

- **One run:** its status, workflow version and state.
- **Many runs of a workflow:** filtered by status and by **any state field**,
  sorted by a state field or time, paged. Filters can compare with the current
  time (`dueAt < now`), which covers time-dependent questions such as
  "overdue" without the workflow updating anything.

Exposed as read-only capabilities for workflows and as API endpoints:

```json
{ "agentId": "runs", "capabilityId": "query-runs",
  "inputMapping": {
    "workflowId": { "valueType": "immediate", "value": "wf-approval" },
    "where":      { "valueType": "immediate", "value": { "stage": "waiting" } },
    "orderBy":    { "valueType": "immediate", "value": [{ "field": "state.decidedAt", "direction": "desc" }] },
    "limit":      { "valueType": "immediate", "value": 50 } } }
```

returning items like
`{ instanceId, status, version, createdAt, stateUpdatedAt, state: { … } }`.
A `get-run-state` capability reads one run.

- **Visibility:** any workflow in the tenant can read any workflow's state.
  State reaches operational UI viewers too, so authors keep in state only what
  may be shown.
- **Versions:** runs of different workflow versions can carry different state
  shapes; each item carries its run's `version`.

### Messages to a running workflow (later, separate mechanism)

Reading state needs nothing else. Writing to an entity (edits, commands) needs
a way to **send a message to a run at any time**, not only while it waits at a
specific `WaitForSignal` with a request ID, as answers work today. This will
be designed separately; core's custom signals are the likely base.

## Why state is stored, not computed on read

Letting the workflow compute its state is intended; computing it **when a
reader asks** is not, because of how runs execute:

1. **Waiting runs are unloaded.** Suspended runs are not in memory (store-freeing
   suspend). Answering would mean relaunching the run and replaying it from
   its checkpoints.
2. **Replay is not read-only.** Durable steps return saved results, but
   non-durable steps run again; a read could repeat an email or an API call.
3. **Lists would be impractical.** Listing 100 entities would replay 100 runs
   on every read, with no way to filter or sort.
4. **Running runs cannot be interrupted.** A run inside a step answers only at
   its next wait point.
5. **Consistency.** In-memory values can include work that is not durable yet,
   so a reader could see state that disappears after a restart.

When only the workflow can produce an up-to-date answer, the reader sends it a
message; the workflow computes, updates its state and replies.

## Use cases

**Approvals.** The approval workflow keeps `{ orderId, stage, decidedAt }`,
written by `state:set` steps when a request is created and when it is decided.
The queue view lists runs where `stage = "waiting"`; its history lists the
others. Whether decisions are also
logged elsewhere is the author's choice.

**Progress.** A fulfilment workflow runs `state:set` with `stage` (and, if it
wants, `percent`) at each milestone. A view lists runs with their stage;
refreshing re-reads state.

**Entity records with edits.** A long-lived workflow holds a record in its
state. An edit is a message; the workflow validates it, updates its state and
replies. Requires [messages](#messages-to-a-running-workflow-later-separate-mechanism).

## Decisions

Settled on 2026-09-28:

| Topic | Decision |
|---|---|
| How state is written | Explicit `state:set` / `state:get` agent steps, no mappings on other step types |
| Initial state | Empty |
| Merge | Shallow; `null` clears a field; arrays are replaced |
| Parallel branches | Merge; for the same field the last committed write wins |
| Non-durable workflows | State ignored, W081 warning; nothing made durable automatically |
| Retention | State lives and is deleted with its run |
| Filtering | Every state field is filterable; filters can compare with `now` |
| Visibility | Any workflow in the tenant can read any workflow's state |
| Schema evolution | Readers see each run's `version` |
| Derived fields | Dropped; time-based questions are filters with `now` |
| Who acted | Not needed |

## Open questions

1. **Messages to a running workflow:** a separate mechanism, designed later.
2. **Long-lived entities:** checkpoints accumulate on every loop turn, and
   replay walks all of them. Continue-as-new or checkpoint compaction, decided
   later.
3. **Filtering implementation:** a general JSONB index, or indexes created as
   filters are used; to be settled with measurements.

## Phases

1. **Write path.**
   - DSL: `stateSchema`; validation of `state` steps against it; W081.
   - The `state` agent (`set`, `get`), host-backed.
   - Core: the state columns, first-write-wins writes keyed by the step's
     checkpoint, one transaction with the run's current state, and the size
     cap.
2. **Read path.** The `get-run-state` and `query-runs` capabilities, with
   filters (including `now`), sorting and paging; the API endpoints; an
   operational view built on them in an end-to-end test.
3. **Messages and long-lived entities** (separate design).

Each phase is verified end to end: compile, run, restart mid-run, and check
that readers see only committed state before and after replay.

## Lessons from the reports prototype

A reports prototype (workflows returning a rendered document, built on a
branch and dropped on 2026-09-28, together with the older block-DSL reports)
was the first attempt at operational UI. What carries over:

- **Operational cases are the target.** The cases that mattered were work
  queues with decisions, requests started from a list (restock), run
  monitoring, one-entity pages, progress, and in-place edits. Analyst tooling
  (pivots, ad-hoc slicing, large interactive tables), maps, PDF distribution
  and broad locale support were ruled out.
- **Copying state out was the main friction.** Every queue or monitor needed
  the workflow to write its decisions to an object-model table, because
  runtime reads returned only run metadata. This proposal removes that step.
- **Acting is talking to a workflow.** Approvals answer a waiting request;
  edits and commands are messages to a run, never direct writes to a table the
  run owns.
- **Interaction details that worked:** an action is bound on the server to the
  exact request, so a stale page cannot answer the wrong item; answering with
  one button disables its alternatives; a pending action shows a spinner, not
  status text; the view refreshes when the run accepts or completes the
  action; a lost response is safe to retry with the same operation ID.
- **What users missed:** who is viewing (to show each manager only their
  team), bulk selection for one action, readable statuses, dates and IDs
  instead of raw values, an optional refresh interval for monitors, and a link
  from a run to its execution page.
