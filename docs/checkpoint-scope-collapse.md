# Checkpoint scope collapse

A composite step (Split, While, EmbedWorkflow, a workflow-agent call, AiAgent)
checkpoints the result of every step inside it, and then its own result. Once
its result is recorded, replay reads that row first and never runs the step
again, so the rows inside it are dead weight until instance retention deletes
them. A Split over 10,000 items with one Agent in its body keeps 10,003 rows,
and stores every item result twice.

Scope collapse deletes those rows in the transaction that records the result:

- a crash before the commit leaves the inner rows, and replay resumes inside
  the step as it always has;
- after the commit, the result row answers every replay.

The host derives everything from the checkpoint key. The guest ABI (WIT) is
unchanged, and workflows already compiled with structured keys collapse too.

## The address

A version 2 key is `runtara:v2:` followed by the compact JSON tuple
`[kind, workflow, namespace, loop_path, parts]`, with an optional
`::attempt::N`, `::retry_sleep::N` or `::retry::N` suffix. Its position is
spread over several elements, and the kind comes first, so the rows inside a
step share no string prefix.

`runtara_core::persistence::checkpoint_scope::scope` rewrites the position as
one address, a compact JSON array of segments, outermost first:

| Segment | Meaning |
|---|---|
| `["wf", workflow]` | Opens each namespace level |
| `["Split", step, i]`, `["While", step, i]` | A loop iteration, as in the key |
| `["ns", kind, parts]` | A namespace frame (`child`, `tool-child`, `agent-aux`) |
| `["base", kind, parts]` | A result row (Agent, Split, EmbedWorkflow), with its key's kind and parts |
| `["own", kind, parts]` | Rows that result keeps for itself: attempts, retry records, retry sleeps, and a Split's or EmbedWorkflow's deadline rows |

Every row inside a step then shares a byte prefix of the address. The tags
repeat the result key's kind and parts, so they tell steps apart exactly as
well as the result keys do; an `onWait` branch may reuse a main-graph step id
at the same position. An Agent's deadline row cannot be matched to its result
key (one names the graph scope, the other the agent), so it carries only its
position and goes when an enclosing step collapses.

The store keeps the address in `checkpoints.scope` (`TEXT COLLATE "C"`,
migration 049), written by every insert, and indexes it with the instance id
(migration 050). Keys outside the canonical grammar, legacy keys and addresses
over 2 KB get NULL and are never collapsed.

Rows keep the address they were written with, and later closures match them,
so the format only grows. Rendering an existing kind differently would leave
its older rows unmatched: never collapsed, never collapsed wrongly.

## Closing a scope

`checkpoint_scope::closure(key)` returns what recording `key` makes obsolete:

- `owned`: the exact address of the step's own rows;
- `inner`: a prefix of every address inside the step, deleted as one index
  range scan.

`loop_closure` builds one for a Split's result (its iterations), and
`child_closure` for an EmbedWorkflow's or an Agent's (the child namespace of an
embedded workflow or a workflow-agent call). The inner ranges name the step by
its id alone. That is exactly as precise as the result keys: two Splits or
EmbedWorkflows with one id at one position already share a result key, and two
Agents with one id share their child namespace. Only an unsuffixed result key
has a closure, so a failed attempt never deletes work a retry will reuse. The
store applies the closure in the transaction that stores the result (a writer
that finds the key already stored writes and deletes nothing):
`checkpoints::record` on the root path, and the fenced `checkpoint` on the
isolated child path. The in-memory backend does the same.

Which kinds close their scope is a rule table in `checkpoint_scope`. A kind is
added only once replay is known never to read the rows its closure names after
the result exists. Composite steps join it one at a time, each with its own
end-to-end tests.

| Result kind | Closes | Why it is safe |
|---|---|---|
| `split` | every iteration, and the Split's attempt, retry and loop-deadline/complete rows | The result is saved only after every item succeeded. Replay looks it up before entering the loop, its deadline or its retries. A failed attempt is a suffixed row with no closure, so a retry keeps the items that already succeeded. |

`A → Split[N] → B` therefore keeps three rows, whatever N is. A Split inside
another Split's iteration collapses first, and the outer Split's range then
covers its result. A run parked or killed inside an iteration keeps that
Split's rows until the Split finishes. Not collapsed: a `durable: false` Split,
which saves no result, and rows written before migration 049.

## Recovery progress

Crash-loop recovery compares a progress marker across restarts. It used the
row count, which falls when rows are collapsed, so a healthy run killed both
before and after a collapse would look stuck.
`Persistence::checkpoint_progress` replaces it: the highest checkpoint row id
in Postgres, and stored plus collapsed rows in memory.
