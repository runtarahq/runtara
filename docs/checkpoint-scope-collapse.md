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
| `["base", family, step]` | A step's result row (Agent, Split, EmbedWorkflow) |
| `["own", family, step]` | Rows a step keeps for itself: attempts, retry records, retry sleeps, deadlines |

Every row inside a step then shares a byte prefix of the address. The store
keeps it in `checkpoints.scope` (migration 049: `TEXT COLLATE "C"`, indexed
with the instance id), written by every insert. Keys outside the canonical
grammar, legacy keys and addresses over 2 KB get NULL and are never collapsed.

## Closing a scope

`checkpoint_scope::closure(key)` returns what recording `key` makes obsolete:

- `owned`: the exact address of the step's own rows;
- `inner`: a prefix of every address inside the step, deleted as one index
  range scan.

Only an unsuffixed result key has a closure, so a failed attempt never deletes
work a retry will reuse. The store applies the closure in the transaction that
records the result: `checkpoints::record` on the root path, and the fenced
`checkpoint` on the isolated child path. The in-memory backend does the same.

Which kinds close their scope is a rule table in `checkpoint_scope`. A kind is
added only once replay is known never to read the rows its closure names after
the result exists. The table starts empty; composite steps join it one at a
time, each with its own end-to-end tests.

## Recovery progress

Crash-loop recovery compares a progress marker across restarts. It used the
row count, which falls when rows are collapsed, so a healthy run killed both
before and after a collapse would look stuck.
`Persistence::checkpoint_progress` replaces it: the highest checkpoint row id
in Postgres, and stored plus collapsed rows in memory.
