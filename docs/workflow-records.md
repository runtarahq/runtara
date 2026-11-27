# Workflow records: a workflow's runs as its store

A workflow is already a store. Each run label names a record, and the run's state
(what its `SetState` steps write) is the record's data. A lead, an order, a keyword's
sync cursor and a ticket are each one label of one workflow. A new run of the label is
a new event in the record's life, and the label's latest run holds the record as it
stands now.

Runtara gains no new concept for this. What it gained are the things that make the
pattern safe and usable:

- **reading** a workflow's records, from people and AI tools and from workflows;
- **one run per label at a time**, so a read and the write after it cannot interleave
  with another run's;
- **state carried forward** from a label's latest run to its next;
- **retention** that keeps each label's latest run.

The design took a while to settle, and the choices below are the result. The earlier
designs live in git history: SQL over runs through DataFusion, then a separate
"entity workflow" concept. The committed migrations of this work still name that
earlier brief, `docs/workflows-as-sql-poc.md`.

## What a workflow declares

Four settings on the workflow's own graph (DSL 3.9.0). Each is ignored on a nested
graph, with warning W084, and the engine reads each from the definition it approved,
as it reads `executionTimeoutSeconds`.

| Setting | Values | What it does |
| --- | --- | --- |
| `labelConcurrency` | `allow` (default), `queue`, `refuse`, `once` | What a start does when its run label is held by another run of the workflow. A run holds its label from its start until it ends, suspended time included. `queue` waits, in the order started; `refuse` is turned away; `once` is turned away too after a run with the label has completed (a failed or cancelled one does not use it up). |
| `labelRequired` | `true` / `false` | Refuse a start without a run label. |
| `carryStateForward` | `true` / `false` | Start each run with its label's latest state: copied at admission or, for a run waiting for its label, when it is let in. |
| `keepLatestPerLabel` | `true` / `false` | Keep each label's latest run past the retention that deletes finished runs, until a newer run of the label takes its place. |

A typical record workflow sets `labelConcurrency: queue` and `carryStateForward: true`.
Each run then reads the record with `GetState`, changes it with `SetState`, and the
next run of the label starts from what this one left. Events for one record apply in
order, and none is lost across a wait.

Labels are not unique, and that is deliberate. Many runs per label is the normal case:
recurring syncs, new events on one record, retries. Two at once is what
`queue`/`refuse` solve; the same event twice is what the trigger's idempotency key
solves; exactly once is what `once` solves.

## Reading records

### The query

A query over one workflow's runs (`runtara_environment::run_query::RunQuery`). It takes
no SQL.

- **`latestPerLabel`:** one row per label, its latest run (the current records); off,
  every run (history).
- **`condition`:** the object model's shape, `{op, arguments}`, with `AND`, `OR`,
  `NOT`, `EQ`, `NE`, `GT`, `GTE`, `LT`, `LTE`, `IN`, `NOT_IN`, `CONTAINS`,
  `STARTS_WITH`, `ENDS_WITH`, `IS_DEFINED`, `IS_EMPTY` and `IS_NOT_EMPTY`.
- **Paths:**
  - run columns: `runId`, `label`, `status`, `version`, `createdAt`, `startedAt`,
    `finishedAt`, `updatedAt`;
  - state, as deep as 16 segments: `state.customer.country`, `state.items.0.sku`;
  - in a condition, `state.items[*].sku` holds if any element matches.
- **`unnest`:** an array in state becomes one row per element, read as `item.<field>`.
- **`select`, `groupBy`, `aggregates`:** `count`, `count_distinct`, `min`, `max`,
  `sum`, `avg`.
- **`orderBy`, `limit` (at most 1,000), `offset`, `count`.**

Comparisons compare values of the operand's JSON type only, so a string `"12"` never
equals a number 12, and a missing field matches nothing but `IS_DEFINED false` and
`IS_EMPTY`.

### Where it is asked

| Who | How |
| --- | --- |
| People, scripts | `POST /api/runtime/runs/query` (`{workflow: id or slug, query}`), `GET /api/runtime/runs/state` (workflows and their state schemas) |
| AI tools | MCP `query_runs` and `describe_run_state` |
| Workflows | the `control` agent: `get-latest-run` (`{workflow, label}` → `found` and the run, with its state), `query-workflow-runs` (the whole query; rows as objects) |

Each needs `InvocationHistoryRead`, as listing executions does. A workflow reads any
workflow of its own tenant, read-only. The two `control` capabilities run only as a step
of a workflow run (`runtime:requires-run`): called any other way, such as from
`test_capability`, they answer `CONTROL_REQUIRES_INSTANCE`, so permission to run
workflows is not a way around `InvocationHistoryRead`. A step's cross-workflow read is
logged with the run that made it.

### Where a run's label comes from

A label is fixed when the run is created, by whoever starts it, and a run never changes
it: admission, carrying state forward and the record all depend on it before the first
step runs.

| Starter | Label |
| --- | --- |
| The execute API, MCP (`execute_workflow`, `execute_workflow_wait`, `execute_workflow_sync`) | `runLabel`, from a caller who authenticated |
| A workflow's step (`control:start`) | `runLabel`, mapped from the workflow's data |
| A webhook (`/events/http/…`, `/events/http-sync/…`), cron, other triggers | none: a public endpoint refuses `runLabel` with 400 |

A webhook takes no label because anyone may send to it, and a label names a record: a
sender who chose one could use up a key (`once`), hold it (`refuse`), queue behind it, or
replace its data. A webhook that feeds records starts a small workflow instead, which checks
what it received (the signature, that the order exists) and starts the record workflow
with `control:start` and a label derived from what it checked. The workflow starting it has
no label, so it appears in history, not as a record. A workflow with `labelRequired`
cannot be started by a trigger directly; its deliveries are skipped.

### Seeing what a run waits for, or why it did not start

- A run waiting for its label reads `executionPhase: "waiting_for_label"`, with
  `waitingFor: {label, holder, ahead}`, in an execution's detail and in lists.
- A start refused for its label reads `failed` with termination type `label_refused`,
  and an error naming the run that holds the label. `POST /api/runtime/workflows/{id}/execute`
  queues the start and reports the refusal on the execution (`label_refused`); a missing
  required label (`labelRequired`) is rejected with 400 at admission once the version is
  compiled; a trigger delivery is skipped.

## How it works

### Each label's latest run: `run_keys`

`run_keys` holds one row per tenant, workflow and label: the latest run, a copy of its
state, and its status and times. It is kept by triggers, on a run's image binding, its
state writes, its label and its status. "Latest" is the run whose state was written
last, then the newest. A label's row outlives the run's own rows when they are pruned.

`latestPerLabel` queries read `run_keys` alone, and a GIN index on its state serves
equality on any path, nested or in arrays.

### Queries

The engine answers in Postgres, where the runs are (`Engine::query_runs`, wire
operation `QueryRuns`). The statement is the module's own:

- **Bound inputs.** Every path and value is a bound parameter.
- **Index use.** Equality (`EQ`, `IN`, or `CONTAINS` of a non-string) also tests
  containment, `state @> {…}`, so the GIN index finds candidates.
- **Type guards.** Comparisons cast under `CASE`, because Postgres does not promise
  to evaluate `AND` left to right.
- **Bounds.** Each query runs under a 30-second `statement_timeout`. Rows are read as a
  stream and stop at 8 MB (`hasMore` says there is more); a single row over 8 MB is
  refused rather than returned, so a large record is read by selecting fewer fields.
- **Limits on the query itself** (each refused with 400 past it):

  | What | Most |
  | --- | --- |
  | Rows a page (`limit`) / offset | 1,000 (default 100) / 100,000 |
  | Operators in a condition / nesting depth | 256 / 16 |
  | Path segments / bytes a segment | 16 / 256 |
  | Values in one `IN` or `NOT_IN` / across a query | 1,000 / 4,000 |
  | `select` / `groupBy` / aggregates / `orderBy` | 64 / 16 / 32 / 8 |

  A selected path or output name may appear once, and an aggregate's `as` must be a
  plain name. An `IN` of up to 32 values also uses the GIN index (one containment test
  each); a longer one is answered without it.
- **Errors.** The API answers 400 for an invalid query, 404 for an unknown workflow,
  429 when busy, 504 past the time limit, 503 when the engine cannot be reached, and 500
  with a fixed message otherwise: the cause is logged, never returned. A step gets the
  same as `CONTROL_INVALID`, `CONTROL_CAPACITY_RATE_LIMITED`, `CONTROL_TIMEOUT` and
  `CONTROL_UNAVAILABLE`.

Queries take turns, so they cannot crowd out the engine's own work on the same
database. By default one per tenant and two in all, with a wait of up to 10 s before a
query is refused as busy (429 from the API; `CONTROL_CAPACITY_RATE_LIMITED`, which is
retryable, from a step). The settings are `RUNTARA_RUN_QUERY_PER_TENANT`,
`RUNTARA_RUN_QUERY_TOTAL` and `RUNTARA_RUN_QUERY_WAIT_MS`. A step's `get-latest-run` is
a lookup by key and takes no turn.

### Labels held one run at a time

The engine passes the workflow's settings to Environment's start as reserved markers,
as it passes `single_instance`. `claim_initial` decides, after it inserts the run and
before it binds the image, under an advisory lock per tenant, workflow and label
(`runtara_label_lock`):

- The **holder** is a run of the workflow with the label that is pending, running or
  suspended, found by the run's own label (`instances.run_label`).
- **`refuse` and `once`** roll the start back, so nothing is left behind. `once` also
  looks at `run_keys`, so it holds after runs are pruned.
- **`queue`** stores the launch as `waiting`: a launch state that holds no slot (it
  releases its admission reservation, like a suspended run) and has no deadline.
- **A descendant** that would wait for any ancestor's label is refused instead.

When a labelled run ends, the trigger `label_released` takes the same lock and queues
the oldest waiting run if no other run holds the label. Both sides take the lock, so a
start that decides to wait and a holder that ends at that moment cannot miss each
other. When nobody waits (the usual case) the trigger stops as soon as it holds the
lock, using a partial index of waiting launches. A run cancelled while it waits leaves
the queue.

### State carried forward

`carryStateForward` copies the label's latest state (from `run_keys`) into the new
run's state:

- at admission, for a run that starts at once;
- in `label_released`, for a run let in from the queue, so it sees what its
  predecessor ended with.

`GetState` reads it; `SetState` merges onto it.

### Retention

The engine records `keepLatestPerLabel` per workflow with each start
(`workflow_retention`), writing only when it changes. When the cleanup worker reads a
page of finished runs past retention, it leaves out any run that is its label's latest
(`run_keys`) in a workflow that keeps it.

### Tenancy

- **Row-level security.** Every table here has the standard tenant policy, and every
  path runs in its tenant's scope.
- **The split roles.** The API resolves the workflow by id or slug from the server
  database, which its role reads, and asks the engine; the engine checks the caller
  it was handed. A workflow's control calls are tenant-scoped by the run's authority.
  So a run of one tenant naming another tenant's workflow finds none.

## Measured

Postgres 16 in Docker on a 16-core Mac; harnesses in
`crates/runtara-environment/examples/`.

**Queries at 1M runs** (800k labels, order-shaped nested state; `run_query_scale.rs`;
median of five; every listing also counted all matches):

| Query | Time |
| --- | ---: |
| One label (latest run, or its history) | 1.6–2.1 ms |
| Equality on nested paths, or inside arrays (GIN) | 17–190 ms |
| Ranges, `CONTAINS`, `STARTS_WITH`, missing fields | 0.33–0.81 s |
| Group by, unnest top-10 SKUs, page 101 | 0.5–0.87 s |

At 100k runs the scans and groups take 30–130 ms. A path that gets hot can be given
an expression index without changing the API.

**What `run_keys` costs the write path** (8,000 runs, 16 workers, five state writes
each through the store's `apply_state`; `run_keys_write_cost.rs`):

- With 300-byte state, throughput moved by less than the noise and each step took
  0.1–0.5 ms longer.
- With 16 KB incompressible state, throughput fell by about a fifth, each state write
  took about 1 ms longer, WAL grew by half, and the state is stored twice.
- The cost is the copy of the state, not the trigger.

**Queries against the engine's own work** (8 workers running runs nonstop, query
clients looping the heaviest 1M-run queries, one 32-connection pool;
`run_query_load.rs`):

| Query clients | Limit | Engine runs/s | State write p99 |
| ---: | --- | ---: | ---: |
| 0 | — | 343–378 | 7–8 ms |
| 20 | none | 56 | 208 ms |
| 20 | 4 at once | 264 | 12 ms |
| 20 | 2 at once | 322 | 9.3 ms |

The default (two at once) leaves the engine 93–95% of its throughput and its normal
p99. Every client here read one tenant's data, which stayed cached; many large tenants
would also compete for cache and disk.

**What labels cost** (8,000 labelled runs, 16 workers; `label_release_cost.rs`):

- `label_released` adds 0.3–0.6 ms (p50) to a labelled run's end.
- `queue` adds 0.5–1 ms to a start.
- Throughput is within the noise.

**Metrics.** Turns are measured: `runtara.run_query.running`, `.waiting`,
`.refused`, `.wait.duration` (by `outcome`) and `.limit` (by `scope`), with
`tenant_id` when `RUNTARA_METRICS_TENANT_LABEL` is on. They are there to tune the
limits per deployment.

## Tested

**Exactness:** 108 queries over runs with nested, mixed-type and missing state answer
as the same query computed independently in Rust (`tests/run_query_test.rs`).

**Labels, carry-forward and retention** (`tests/label_concurrency_test.rs`,
`tests/db_cleanup_worker_test.rs`):

- all four modes;
- a suspended holder;
- cancelling while waiting;
- `once` against completed, failed and pruned runs;
- a child of the holder;
- holders whose launches carry no label;
- carry-forward at admission and at promotion;
- retention keeping each label's latest, then letting it go;
- races: four concurrent starts admit one, and a start racing its holder's end never
  waits on a gone holder.

**Row-level security:** a query reads its own scope's tenant only.

**End to end** (`e2e/test_served_tenants.sh` section 12), with the server as a plain
database role and again with API and engine as separate processes (`TWO_ROLES`):

- carry-forward over two runs of one label, read back by slug;
- a queued start waiting for a suspended holder, and naming it;
- a refused start (`label_refused`);
- a workflow's steps reading another workflow's records;
- the second tenant reading none of the primary's records: 404 by slug and by id,
  and its run naming the primary's workflow finds none.

**Validation WASM:** the console's validator accepts the four settings, warns W084 on
a nested graph, and refuses an unknown value.

**AI tools over MCP.** Claude Code and Codex, each in a fresh session with only this
server's MCP tools, answered nine questions about three seeded workflows (1,600 leads,
12 search keywords, 300 support tickets; 2,900 runs), one question a session:

| Client | Correct | Tool calls | Tool errors |
| --- | --- | --- | --- |
| Claude Code | 8 of 9 | 29 | 0 |
| Codex | 8 of 9 | 19 | 0 |

- **What they had to find on their own:** that a label is the record and its latest run
  the current one (`latestPerLabel`), nested and array paths (`state.tags[*]`), grouping
  with aggregates, and a label's history across versions.
- **Every query was valid.** Each began with `describe_run_state`, then one to five
  `query_runs`.
- **The miss was the question, not the query.** "Which urgent tickets with a breached
  SLA are still open?": the tickets' state has no open or closed field. Both counted a
  ticket as open while its latest run is pending, running or suspended (4 tickets), and
  both pointed out the one ticket whose latest run failed, which the answer key counted
  as open (5). A state field for the ticket's own status would have settled it.

## Why this way

- **No SQL.**
  - **What SQL showed:** a SQL layer over runs (DataFusion, planned on the API's side)
    worked, and AI tools wrote correct SQL for it: Claude and Grok, 9/9 each.
  - **Its cost:** SQL is a promise of a database (writes, joins, a dialect) runtara
    does not mean to make. It needed translating runs into tables, it added a large
    engine, and it opened spill files on the API host.
  - **The choice:** records are read through one structured query that workflows, the
    API and MCP share. SQL can still be built over it later, if it is needed.
- **No entity concept.** A separate "entity workflow" mixed keyed data and execution in
  one idea, and each real use case pulled the halves apart. A workflow, its run labels
  and its run state already are the pattern.
- **Settings on the definition.** What makes a workflow runnable is what the engine
  approved, so the engine reads these settings from the definition it approved.
  Nothing is taken on the API's word.
- **A suspended run holds its label.** A write lost across a wait is the case worth
  preventing. Restate's virtual objects keep their key through an await for the same
  reason.

## Open

- **The console** (runtara-frontend):
  - show `waitingFor` and `label_refused`;
  - edit the four settings;
  - list a workflow's records.
- **User documentation** of the pattern.
- **`_run_label`.** A run cannot name its own label, so a lookup of its own record
  needs the label passed in as input. A `_run_label` runtime variable would remove
  that. With `carryStateForward`, the common case does not need it.
- **Cross-workflow reads.** Any run may read any workflow of its tenant, where the
  control agent used to read only the calling run and its children. Deliberate: only
  from a run, under `InvocationHistoryRead`, and logged. Worth a reviewer's look.
- **Large states** are copied into `run_keys`. If that cost matters, mark each label's
  latest run on `instance_state` and index only those rows. That costs a join per label
  in scans, which needs its own measurement.
- **Paging** uses `offset`, so deep pages get slower (page 101 at 1M runs took 0.5 s).
  A cursor would keep them flat.
- **Many large tenants** have not been measured together.
- **Migrations.** The `run_keys` and `label_concurrency` migrations fill in rows for
  runs already there. That is harmless on the fresh databases this ships to, and would
  need batching before an upgrade of a large one.
