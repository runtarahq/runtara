# Queryable workflow state

Status: approved 2026-09-29; being implemented. The `stateSchema` declaration
shipped in #270 (DSL 3.4.0).

Revises `docs/workflow-state.md` on `claude/worktree-wasm-reports-state-5dc188`.
Builds on the control agent (runtarahq/runtara#270, merged) and depends on
`docs/wit-refactor.md` (all runtara WIT in `runtara-wit` at `1.0.0`).

## Problem

A running workflow is durable, but from the outside it is a black box until it
finishes. Nothing can read what a run knows while it runs or waits:

- checkpoints are opaque bytes per step, meant for replay, not for readers;
- variables are constants;
- a run label is set once, when the run is registered;
- a waiting request's context is readable only while the run waits at that
  WaitForSignal (`control:list-pending-signals`);
- `control:get` and `control:query` return run metadata (status, times, label,
  parent), not what the run holds.

So authors copy state into object-model tables to make it readable: a second
source of truth, wired by hand in every workflow.

Workflows are durable, so a workflow can itself be the storage. What is missing
is one general mechanism: a run exposes typed state that other workflows can
query. Reports are the first reader: progress, approvals and in-place edits all
become "read state, act by sending to the run", with nothing case-specific in
the platform.

## Goals

- A workflow declares its state as a typed contract.
- The workflow computes its state with its own logic and writes it with
  explicit steps.
- State is durable: readers only see state the run will not take back.
- Readers get one run's state, or lists of runs filtered by state, without
  waking the run. Lists return runs, not their state.
- Workflows read other runs' state through the control agent; the API exposes
  the same reads.

## Non-goals

- Computing state on read by running code inside the workflow (query
  handlers). See [Why state is stored](#why-state-is-stored-not-computed-on-read).
- Case-specific steps (progress, decisions, audit logs). Those are state the
  author chooses to keep.
- Replacing the object model for analytics over large data.

## Model

- **Own state is part of the run.** A workflow reads and writes its own state
  with two step types, `SetState` and `GetState`, like Finish, Delay or
  WaitForSignal. They are not agents: an agent step invokes a capability, and
  the state belongs to the run itself.
- **Other runs are reached through control.** The control agent already
  interacts with other runs by id. It gains `get-state` for one run and a state
  filter on `query`.

## What exists today

| Piece | Where | Relevance |
|---|---|---|
| `stateSchema` declaration: graph field, validation (E008 rows, W081 for `required`/`default`/`visibleWhen`), API, MCP `get_state_schema`/`set_state_schema`, Settings › State editor, `currency` format hint | shipped in #270 | The contract `SetState` writes against |
| Replay from the start; checkpoints are a result cache keyed by call site, not by input | `runtara-environment/src/recovery.rs:14-17`; `direct_wasm/compile/agent.rs:192-211` | Why `GetState` must be cached |
| v2 checkpoint key: `[kind, workflow, namespace frames, loop path, [...ids]]`, one per loop iteration and embedding | `runtara-workflow-stdlib/src/direct_json.rs:3937, 4003-4080` | Identity of a state write |
| Compiler-emitted runtime calls: the guest passes a key, the host takes the run from its own store | `runtara:workflow/runtime@1.0.0` `get-checkpoint` / `checkpoint`; `runtara:workflow/waits@1.0.0` (WaitForInstances) | The pattern for the state calls |
| One-transaction first-write-wins: `accept_input`, the fenced checkpoint (`ON CONFLICT DO NOTHING`) | `runtara-store-postgres/src/inputs.rs:443-502`, `invocations.rs:141-181` | The pattern for the state write |
| Graph schemas in the compiled manifest | `DirectGraphManifest.input_schema/output_schema`, `direct_wasm/manifest.rs:181-184` | Where `stateSchema` reaches the runtime |
| Runtime schema checks in the stdlib | `validate_split_schema`, `direct_json.rs:4899` | Precedent for checking state on write |
| `control:get` / `control:query`, tenant-wide reads; `query` via `push_instance_filters` or the children CTE | `runtara-server/src/api/services/control.rs`; `runtara-environment/src/db.rs:159-223`, `control_reads.rs` | Where `get-state` and the state filter go |
| Custom signals: one row per `(instance, checkpoint_id)`, last write wins, refused when a managed request exists | `runtara-store-postgres/src/backend.rs:388-428` | Not a basis for messages |
| Park and wake: `instance_input_parks`, `sleep_until` stamp, reconciler, `wake_reason` CHECK | core migrations 030, 040 | Reusable for a mailbox |

Things an earlier draft assumed that do not exist:

- **Finish is not checked against `outputSchema`.** `outputSchema` is never
  validated (`runtara-workflows/src/validation.rs` checks only E117 and E118).
  Typing state writes is new work.
- **Control receipts are not written with their effect in one transaction.**
  They are intent-first, three statements
  (`runtara-core/src/persistence/control_receipts.rs:1-19`).
- **The ordinary checkpoint save is an upsert,** not first-write-wins
  (`dialect/postgres.rs:95-100`).
- **The 64 KiB continuation cap is a constant,** not configurable.
- **Nothing compacts checkpoints of a live run.** Retention and pinned-child
  pruning only touch terminal runs.

## Concepts

### State schema (shipped)

A workflow declares `stateSchema` on its graph, next to `inputSchema` and
`outputSchema`, in the same `SchemaField` format
(`runtara-dsl/src/schema_types.rs`):

```json
"stateSchema": {
  "order":    { "type": "string", "label": "Order" },
  "customer": { "type": "string", "label": "Customer" },
  "amount":   { "type": "number", "label": "Amount", "format": "currency" },
  "stage":    { "type": "string", "label": "Stage",
                "enum": ["received", "credit_check", "approval", "fulfilment", "delivered"] },
  "dueAt":    { "type": "string", "format": "datetime", "label": "Due" }
}
```

State is the run's public contract, separate from its internals: refactoring
steps does not break readers, and intermediate outputs or secrets are not
exposed unless the author maps them into state. State starts empty, so
`required`, `default` and `visibleWhen` have no effect (W081).

Only top-level fields are filterable. `object` and `array` fields are stored
and returned but not filtered.

### Steps

**`SetState`** merges the given fields into the run's state. The merge is
shallow: each given field replaces its value, a field set to `null` is cleared,
and arrays are replaced. Fields not given are kept. It has no outputs.

```json
"record": {
  "stepType": "SetState",
  "values": {
    "stage":     { "valueType": "template",
                   "value": "{% if steps.approve.outputs.approved %}approved{% else %}rejected{% endif %}" },
    "decidedAt": { "valueType": "reference", "value": "steps.now.outputs" }
  }
}
```

**`GetState`** returns the run's state as of that point in the run, for
read-modify-write (a loop that adds to a counter or a list). Its output is the
state object, `steps.<id>.outputs.<field>`, with an output shape taken from
`stateSchema`.

A new run's state is empty.

Why steps and not mappings on other step types:

- every write is a visible node on the canvas, not a side effect of another
  step;
- "where does state change" is a search for `SetState`;
- a `SetState` step's values can reference any step's output, so the workflow
  computes its state with its full logic.

**How they run.** The compiler emits calls to a new `state` interface of the
`runtara:workflow` package, which only compiled workflow logic imports, the way
it emits `checkpoint` and the WaitForInstances `runtara:workflow/waits` calls:

```wit
package runtara:workflow@1.0.0;

interface state {
    /// Merge a JSON patch into the calling run's state, once per key.
    set: func(key: string, patch: list<u8>) -> result<_, string>;
    /// The calling run's current state.
    get: func() -> result<list<u8>, string>;
}
```

- The compiled code passes the step's state key, a v2 key of kind `state`
  (`["state", workflow, namespace frames, loop path, [step_id]]`). The host
  hashes it into the write's `operation_id`.
- The host takes the tenant and the run from its own store. Split bodies run
  in the root store.
- A workflow can only reach its own run's state; the key only decides whether a
  replayed write is a no-op.
- The stdlib checks the patch against `stateSchema` from the manifest and
  canonicalises it before calling `set`.

### Rules

E133 and W081 are taken by WaitForInstances literals and ineffective
state-schema settings.

- **Own run only.** `SetState` and `GetState` act on the calling run. Other runs
  are read with `control:get-state` and `control:query`.
- **Declared.** A state step in a workflow without `stateSchema` is an error
  (E134).
- **Typed.** A `SetState` field must exist in `stateSchema` (E135). Values are
  checked twice:
  - at validation, for immediate values, reusing `check_type_compatibility`
    (E023) and the enum check (E024);
  - at run time, by the stdlib against the manifest's `stateSchema`. A value
    that does not match fails the step.
- **No step-level durability.** State steps have no `durable` field; they
  follow the workflow's durability.
- **Only the outer run publishes.** A workflow running as an inline-embedded
  child (EmbedWorkflow) or as a published workflow-agent runs inside its
  parent's instance, so it must not write the parent's state. Its state steps
  still work, on *local* state: scoped to that invocation (its call site and
  the parent's loop iteration), validated against the child's own
  `stateSchema`, and never seen by readers. The same holds for a non-durable
  workflow. So a child's own read-modify-write logic behaves the same whether
  it runs embedded or on its own. A parent that embeds a child with state
  steps gets a warning (W083).

  Local state lives in guest memory and never reaches the host, which
  refuses a state key with namespace frames. It is made replay-safe with
  checkpoints: a local SetState checkpoints the state it produced and a local
  GetState what it read, and a replay restores them. Replay also skips
  regions whose result is checkpointed (a Split result, a completed While),
  so their SetState steps never run again; a Split or While whose body writes
  local state therefore checkpoints a snapshot of the local state beside its
  result and restores it when replay takes the result from the checkpoint.
  An embedded child's result needs no snapshot: the parent never reads the
  child's local state.

### Durability

State is exactly as durable as the step that writes it.

**The write.** The host applies a `SetState` in one transaction, following
`accept_input` and the fenced checkpoint:

1. Check that the run is not terminal and belongs to the tenant.
2. Insert `(instance_id, operation_id, patch)` into the write log with
   `ON CONFLICT DO NOTHING`.
3. Only if the insert happened, merge the patch into the run's state and set
   `state_updated_at`.

A replayed `SetState` finds its row and changes nothing, even when a crash fell
between the write and the step's checkpoint, and even when its value came from
a non-durable step. Replaying old steps never writes an older state over a
newer one.

**The read.** `GetState` returns the state at that point in the run, not the
latest state. A replay runs from the start and reaches every `GetState` again;
if it read live state, a replayed read-modify-write loop would see writes from
later steps and take a different path. So `GetState` checkpoints its result
under its key, and a replay gets back what the first execution read.

**Non-durable workflows** (`durable: false`). State is local: the steps work,
but readers see no state for the workflow's runs. Validation warns the author
(W082). The platform does not make the workflow or its steps durable on its
own.

**Referencing state in mappings.** There is no `state.*` reference root. A
reference resolves when a step's inputs are built, and that read is not
checkpointed: a replay could see writes the first execution made later and
take another path. The sound form is a checkpointed read, which is what
`GetState` is; reference its output (`steps.<id>.outputs.<field>`). A
`state.*` root could later be sugar the compiler lowers into the same
checkpointed read before the referencing step.

**Parallel branches.** `SetState` runs in parallel branches like any step. Each
write merges its fields, so branches writing different fields keep both. For
the same field, the last write to commit wins; between concurrent branches that
order is not defined.

### Storage

Core migrations in `crates/runtara-store-postgres/migrations/postgresql/`,
starting at the next free number (`043` when this was written):

- `instance_state`: `instance_id` (primary key, FK to `instances` with
  `ON DELETE CASCADE`), `state` JSONB, `state_updated_at`. A separate table, so
  state writes do not add updates to the `instances` row.
- `instance_state_writes`: `(instance_id, operation_id)` primary key, FK with
  `ON DELETE CASCADE`, and the patch. `operation_id` is the 64-character hash
  of the state key, like `instance_control_receipts.operation_id`.
- A size cap on the state document of 64 KiB, a constant enforced in code and
  by a CHECK, like agent continuations. A `SetState` that would exceed it fails
  the step.
- Values are stored in canonical form: date-times in UTC with fixed precision,
  numbers as JSON numbers. This is what makes filters compare correctly.

Persistence follows the optional sub-trait pattern
(`runtara-core/src/persistence/mod.rs`): `Persistence::run_state()` returns
`Option<&dyn RunState>`, default `None`, with a memory implementation and
conformance tests run by both backends.

Retention:

- Deleting a run deletes both tables by cascade. The memory backend's
  `delete_instances_batch` needs the same.
- Pruning pinned children keeps `instance_state`, like the run's outcome, and
  deletes `instance_state_writes`, since a terminal run never replays. That
  adds one statement to `PRUNE_STATEMENTS`
  (`runtara-store-postgres/src/ops_common/ops/retention.rs`) and the memory
  backend's `prune_instance`.

Indexes: a GIN index (`jsonb_path_ops`) on `instance_state.state`. There are no
JSONB indexes over run data today. At thousands to tens of thousands of runs
per tenant this covers equality filters; range filters run over the set
already narrowed by workflow and status.

### Reading other runs

Reads come from stored state. They never wake, resume or replay a run, so they
work the same for running, waiting and finished runs.

These extend `runtara:control@1.0.0`. It is unreleased, so it is extended in
place; once a release ships it, additions follow the `runtara-wit` versioning
rule (a new function is a minor bump, a changed record a major one):

- **`get-state(instance-id)`** returns one run's status, version, state and
  `state-updated-at-ms`. It is the only read that returns state.
- **`query`** gains a state filter. It still returns run summaries only, never
  state, and its sort stays by time.
  - Without a parent filter it goes through `push_instance_filters`, shared with
    the executions API, which gains JSONB predicates.
  - With a parent filter it goes through the children CTE; only launched
    children have state, so admitted-not-launched and outcome-only children
    never match a state filter.

```json
{ "agentId": "control", "capabilityId": "query",
  "inputMapping": {
    "workflowId": { "valueType": "immediate", "value": "wf-approval" },
    "state":      { "valueType": "immediate",
                    "value": [{ "field": "stage", "op": "eq", "value": "approval" }] },
    "pageSize":   { "valueType": "immediate", "value": 50 } } }
```

Filters are a fixed set of operators on one top-level state field each,
combined with AND: `eq`, `ne`, `in`, `lt`, `lte`, `gt`, `gte`, `exists`.
Values are literal JSON scalars; there is no `now`. Time-dependent questions
such as "overdue" pass the current time as a value (`dueAt lt <now>`): a
workflow maps it from a step, a UI computes it. There are no free-form
expressions. A filter on a field that a run's state does not have does not
match that run; it is not an error.

The executions API follows the same split: the single-run endpoint
(`GET /api/runtime/workflows/instances/{instance_id}`) returns `state`, and the
list accepts a state filter but returns no state. The filter is a `POST` query
endpoint taking the same filter JSON, since it does not fit in query
parameters.

**Visibility.** Any workflow in the tenant can read any workflow's state, as
control reads already do. State reaches report viewers through workflows, so
authors keep in state only what may be shown.

**Versions.** Runs of different workflow versions can carry different state
shapes; `get-state` returns the run's version.

## Messages to a running workflow (later, separate design)

Reading state needs nothing else. Writing to an entity (edits, commands) needs
a way to send a message to a run at any time, not only while it waits at a
specific WaitForSignal.

Custom signals are not the base: they are stored per `(instance,
checkpoint_id)`, last write wins, so the sender must name the exact wait and a
second message replaces the first. Managed inputs are refused before their
request is registered; the S0.1 prototype (`e2e/test_control_signals_only.sh`)
showed that an answer to a wait that has not opened yet is refused.

Messages need a mailbox per run:

- an append-only queue per run, each message with an increasing sequence
  number and an idempotency key from the sender;
- a receive step that takes the next unconsumed message and records the
  sequence number it consumed in its checkpoint, so a replay takes the same
  message;
- a receive step with an empty mailbox parks. It reuses the park and wake
  plumbing (`instance_input_parks`, the `sleep_until` stamp, the reconciler)
  with a new wake reason, which means replacing the `wake_reason` CHECK. Wait
  targets are not reusable: they point at another run's completion, not at a
  message.

Who may send to whom follows control decision D1.

## Why state is stored, not computed on read

Letting the workflow compute its state is intended; computing it when a reader
asks is not, because of how runs execute:

- **Waiting runs are unloaded.** Suspended runs are not in memory. Answering
  would mean relaunching the run and replaying it from its checkpoints.
- **Replay is not read-only.** Durable steps return saved results, but
  non-durable steps run again; a read could repeat an email or an API call.
- **Lists would be impractical.** Listing 100 entities would replay 100 runs on
  every read, with no way to filter.
- **Running runs cannot be interrupted.** A run inside a step answers only at
  its next wait point.
- **Consistency.** In-memory values can include work that is not durable yet,
  so a reader could see state that disappears after a restart.

When only the workflow can produce an up-to-date answer, the reader sends it a
message; the workflow computes, updates its state and replies.

## Use cases

**Approvals.** The approval workflow keeps `{ order, stage, dueAt }`, written
by `SetState` steps when a request is created and when it is decided. A queue
lists runs with `control:query` where `stage eq "approval"`; its history lists
the others. Whether decisions are also logged elsewhere is the author's choice.

**Progress.** A fulfilment workflow runs `SetState` with `stage` at each
milestone. A workflow behind a report lists the runs and reads each run's stage
with `control:get-state`; refreshing re-reads state.

**Entity records with edits.** A long-lived workflow holds a record in its
state. An edit is a message; the workflow validates it, updates its state and
replies. Requires messages and a decision on long-lived runs.

## Decisions

Settled on 2026-09-28:

| Topic | Decision |
|---|---|
| How state is declared | `stateSchema` on the graph, same `SchemaField` format as input/output (shipped in #270) |
| How own state is written | `SetState` / `GetState` step types, not agent steps; no mappings on other step types |
| How other runs are read | The control agent: `get-state`, and a state filter on `query` |
| Lists | Filter by state, never return it |
| Initial state | Empty |
| Merge | Shallow; `null` clears a field; arrays are replaced |
| Parallel branches | Shallow merge; for the same field the last write wins |
| Non-durable workflows | Local state, W082 warning; nothing made durable automatically |
| Retention | State lives and is deleted with its run |
| Filtering | Every top-level scalar state field is filterable; values are literals (no `now`) |
| Visibility | Any workflow in the tenant can read any workflow's state |
| Schema evolution | Readers see each run's version |
| Derived fields | Dropped; time-based questions are filters with a caller-supplied time |
| Who acted | Not needed |

Approved on 2026-09-29:

| Topic | Proposal |
|---|---|
| Runtime interface | New `state` interface in `runtara:workflow@1.0.0` (`set(key, patch)`, `get(key)`) |
| Write identity | Hash of a v2 `state` key, keyed in `instance_state_writes` |
| Write unit | One transaction: write-log insert, then merge only if inserted |
| `GetState` on replay | Checkpoints its result; state steps have no `durable` field |
| Typing | E134/E135 plus E023/E024 at validation; the stdlib checks and canonicalises against the manifest's `stateSchema` at run time |
| Embedded children and workflow-agents | Local state per invocation in guest memory, checkpointed (Split/While snapshots); W083 on embed |
| Storage | `instance_state` and `instance_state_writes` tables; 64 KiB constant cap; canonical values |
| Pruning | Keeps `instance_state`, deletes `instance_state_writes` |
| `state.*` references | Not now; GetState plus `steps.<id>.outputs.<field>` |
| Filtering scope | Top-level fields only; fixed operators, AND only; a missing field does not match |
| Control changes | `get-state` and the `query` state filter in `runtara:control@1.0.0`, in place |
| Executions API | Single-run endpoint returns state; the list filters by it through a `POST` query endpoint |
| Indexing | GIN (`jsonb_path_ops`) on `instance_state.state` |
| Messages | A per-run mailbox with sequence numbers, not custom signals; designed separately |

## Open questions

- **Long-lived entities.** Nothing compacts a live run's checkpoints. Replay
  walks every iteration and makes one checkpoint lookup per durable step per
  iteration, and While defaults to 10 iterations. Continue-as-new or
  compaction must be decided before messages ship, since entities with edits
  depend on both.
- **Messages.** The mailbox design above: limits, ordering across senders, and
  how a reply reaches the sender.

## Phases

1. **Write path.**
   - ~~DSL: `stateSchema` on `ExecutionGraph`~~ (shipped in #270, DSL 3.4.0).
   - The `SetState` and `GetState` step variants; a `DSL_VERSION` bump and
     changelog entry; OpenAPI and the generated client.
   - Every exhaustive match over step types: validation, step context rules,
     the direct emitter's plan and compile modules, the stdlib's three debug
     builders, step summaries and the timeline, the canvas nodes and their
     forms, and the MCP authoring schema and graph-mutation tools.
   - Validation: E134, E135, W082 and W083; `stateSchema` threaded into Split
     and While bodies like the root `inputSchema` (`DataScope`).
   - `stateSchema` in the manifest (a `DIRECT_WORKFLOW_MANIFEST_VERSION` bump);
     stdlib check and canonicalisation; the `state` key kind.
   - `runtara:workflow/state` and its host implementation.
   - Core: the next migration, the `RunState` sub-trait with Postgres and
     memory backends and conformance tests, the one-transaction write, the cap,
     and retention and pruning.
2. **Read path.**
   - `control:get-state` and the state filter on `control:query`, in the WIT,
     the control agent and `NativeControl`, for both query paths.
   - `state` on the single-run executions endpoint and the `POST` query
     endpoint for the filtered list.
   - An end-to-end test: a workflow lists approvals by state with
     `control:query` and reads each run's state with `control:get-state`.
3. **Messages and long-lived entities** (separate design).

Each phase is verified end to end: compile, run, restart mid-run, and check
that readers see only committed state before and after replay. Phase 1 also
replays a read-modify-write loop from its start and checks that it takes the
same path, and crashes between a `SetState`'s write and its checkpoint.
