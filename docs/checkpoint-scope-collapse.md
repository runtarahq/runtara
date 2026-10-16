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

`loop_closure` builds one for a Split's or While's result (its iterations), and
`child_closure` for an EmbedWorkflow's or an Agent's (the child namespace of an
embedded workflow or a workflow-agent call). The inner ranges name the step by
its id alone. For a loop that is exactly as precise as its result key: two
loops of one kind with one id at one position already share it. A child namespace is
looser: every step that can call a child (EmbedWorkflow, Agent) shares one
namespace with any other such step that has its id at its position, whatever
its kind. Step ids are unique within a graph, so this happens only when an
`onWait` graph, which keeps its wait step's position, reuses an id of the
graph around it (the workflow editor never does). Deleting a shared namespace
would hurt when the other step has no result to replay from: it is
`durable: false`, it failed and its `onError` handled the failure, or it is
still in progress. An `onWait` graph runs again on every replay of its wait,
so its child steps would run again.

So a child closure carries its step (`ScopeClosure::child_step`), and a run
records which of its steps may share a namespace (`SharedChildSteps`): every
step id that occurs inside an `onWait` graph anywhere in its workflow, its
embedded children and the workflow-agents it calls. A child namespace whose
step is listed is kept; the step's own attempt and retry rows still go, since
their tag carries the whole result key. Environment reads the list when it
starts the run, from the manifests the compiler writes into every
workflow-logic component (a composed artifact keeps each workflow-agent's,
an isolated package holds them as artifacts), and stores it in
`instances.shared_child_steps` (migration 051). When the list is unknown (no
manifest, one that does not parse, a called workflow-agent whose manifest is
missing, or a run started before migration 051) the column is NULL and the
run keeps every child namespace. A loop's iterations are not affected: once a
Split's result exists, replay never enters the loop again, an `onWait` graph
inside an iteration included.

Only an unsuffixed result key
has a closure, so a failed attempt never deletes work a retry will reuse. The
store applies the closure in the transaction that stores the result (a writer
that finds the key already stored writes and deletes nothing):
`checkpoints::record` on the root path, and the fenced `checkpoint` on the
isolated child path. In Postgres the delete rides in the statement that
points the instance at its new result (a data-modifying `WITH`), so a closure
costs no extra round trip; that matters because every durable Agent result
has one and most delete nothing. The in-memory backend does the same.

Which kinds close their scope is a rule table in `checkpoint_scope`. A kind is
added only once replay is known never to read the rows its closure names after
the result exists. Composite steps join it one at a time, each with its own
end-to-end tests.

| Result kind | Closes | Why it is safe |
|---|---|---|
| `split` | every iteration, and the Split's attempt, retry and loop-deadline/complete rows | The result is saved only once the loop has finished: every item succeeded, or with `dontStopOnFailed` every item ran and the result records the failures. Replay looks it up before entering the loop, its deadline or its retries. A failed attempt is a suffixed row with no closure, so a retry keeps the items that already succeeded. |
| `while` | every iteration, and the While's loop-deadline/complete rows | The result, `{iterations, outputs}`, is saved only once the loop has exited (its condition false or its maximum reached), in a durable graph; a failure, a timeout, an `onError` route or a suspension inside it saves none. Replay looks it up before the first condition and the loop's deadline, so the body never runs again. A body that writes local state (inside a child) carries a snapshot of it in the result, as a Split's does. Older workflows, compiled before While saved a result, replay every iteration as they always have. |
| `embed_workflow` | the child's rows, nested children included, and the Embed's attempt, retry, retry-sleep and deadline rows | The result is saved only once the child has finished: it succeeded, or, for an Embed called as an AiAgent tool, its failure became the tool result. A child that parks exits the run first, and an enclosing deadline that unwinds skips the save. Replay looks the result up before mapping the child's input, its deadline or its retries. |
| `agent` | a workflow-agent's child rows, and the Agent's attempt, retry and retry-sleep rows | The result is saved only when the call succeeded; a failure goes to `onError` or fails the run. Replay looks it up before the Agent's deadline, its retries, its continuation and the call. The key does not say whether the agent is a workflow, so every durable Agent that retried also drops its retry rows. An AiAgent's tool and memory calls run under their own frames (`tool-child`, `agent-aux`) and are not touched; neither is the Agent's deadline row, which carries only its position. |

`A → Split[N] → B` therefore keeps three rows, whatever N is, and so do
`A → While[N] → B`, `A → EmbedWorkflow → B` and `A → workflow-agent → B`,
whatever the loop or the child does. A resume after a finished While no
longer replays its iterations.
A composite step inside another one collapses first, and the outer step's
range then covers its result: a Split inside an Embed's child, an Embed inside
a Split's iteration or inside another Embed. A run parked or killed inside a
step keeps that step's rows until the step finishes, and a child that runs as
an isolated invocation writes through the fenced path into the same instance,
so its caller's result collapses it the same way. Not collapsed: a
`durable: false` step, which saves no result, and rows written before
migration 049.

## State and retry rows

A workflow running embedded or as a published workflow-agent keeps its state
locally. Where it can replay (it is durable and imports the runtime), it
checkpoints that state: each SetState saves the record it produced
(`state-local`: its patch and the whole local state, which is capped at
64 KiB, so a record can approach twice that), each GetState what it read
(`state-read-local`). Those rows carry the child's namespace, like every other
row the child writes, so the result of the EmbedWorkflow or Agent call that
runs the child closes them with the rest, under the rules above: a namespace
another step may share, or one in a run whose list is unknown, is kept, and a
row whose address is over 2 KB is never collapsed.

Two kinds of state rows have no closing result:

- a workflow-agent called as an AiAgent tool writes them under the tool
  call's frame, and the tool call records no result of its own (AiAgent does
  not close its scope);
- a non-durable top-level workflow whose Split or While body is marked
  `durable: true` checkpoints local state in that body, under a loop that
  saves no result.

A durable top-level run writes no such rows: it publishes its state to the
host (`instance_state`, one row per instance, and `instance_state_writes`, its
patch log), and only its GetState reads are checkpointed.

Superseded state rows are not replaced while the child runs. Replay runs the
child from its start, and a SetState whose row was gone would compute its
state again from inputs a `durable: false` step may produce differently. So a
running child keeps one row per SetState execution (a While loop in it adds
rows on every iteration until its own result collapses them), and so does a
child whose caller saves no result, until retention.

A step's `::retry::N` row records an attempt's error message for audit. Only
the generic checkpoint listing returns it; nothing reads it as retry history,
so it goes with the step's other attempt rows once the step's result is
recorded.

## Recovery progress

Crash-loop recovery compares a progress marker across restarts. It used the
row count, which falls when rows are collapsed, so a healthy run killed both
before and after a collapse would look stuck.
`Persistence::checkpoint_progress` replaces it: the highest checkpoint row id
in Postgres, and stored plus collapsed rows in memory.
