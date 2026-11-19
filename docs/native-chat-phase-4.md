# The native chat, finished: phase 4

Status: being built, a part at a time. What is built, and where it departs
from what is written here, is under [As built](#as-built) at the end. It
follows [native-chat.md](native-chat.md), whose phases 1 to 3 are on
`main`.

## What changes about phase 4

Phase 4 was "what adapters need". It is now **the native chat itself, made
whole**, before any adapter:

- sound in its architecture;
- good for an AI to use, as a client and as the workflow that answers;
- good for a person to use;
- good to build an adapter on;
- good to build a UI on, the console's or anyone's;
- streamed, with server-sent events, not polled.

What only a platform's webhook needs moves to phase 5, with the adapters:
a webhook's headers in the trigger's input, webhook checks declared by the
connection type, and a workflow's own answer to the synchronous trigger.
`conversation.send` stays here: it is what makes the conversation something
a workflow can build on, and an adapter is only one such workflow.

## What there is, measured against that

Read from the code on `main`, not from the earlier plan.

| | Today |
|---|---|
| A conversation by key, messages at any time, a bounded queue, once by id | built |
| Events numbered and kept, read on with `after` | built |
| `conversation.reply` from any step | built |
| A reply workflow started with each reply | built, proved by tests only |
| A stream with `Accept: text/event-stream`, resumed with `Last-Event-ID` | built, **and polled underneath** |

The stream is server-sent events to the client and two polls behind it:

1. The API asks the engine for new events **every 500 ms**, for each open
   stream (`STREAM_EVERY`, `api/handlers/conversations.rs`).
2. The engine's worker reads each active conversation's run **twice a
   second** for what it said (`Conversations::advance`).

So a reply reaches its reader up to a second after it is said, an idle
stream costs two queries a second, and nothing arrives as it is written:
the AI Agent step asks its model for a whole answer (`runtara-ai` has no
streamed completion, `runtara-http` reads a whole body).

What is missing for each of the five:

- **Architecture.** An event is `{kind: String, body: Value}`: its shape is
  in prose, not in a type or in the OpenAPI document. Nothing ties a reply
  to the message it answers. A reply said by a retried step is said twice.
  Events are never removed. `migrate-tenant` does not copy conversations.
- **An AI.** No MCP tool reaches a conversation. A reply is text and
  attachments, with no structured data beside it. A workflow cannot read
  what was said before, or send into another workflow's conversation.
- **A person.** No way to stop a run that is answering. No list of one's
  conversations, no title. No reading back from the newest. Nothing says
  which request a structured answer is for.
- **An adapter.** No `conversation.send`. The reply workflow is handed the
  reply and the conversation's key, and nothing the adapter kept about the
  chat on its platform, so it would have to look that up.
- **A UI.** The events' types cannot be generated. Only a caller with a
  tenant's credential can open a stream, so a page embedded for an end user
  has nothing to present.

## The design

### One log, typed

A conversation is one log of events, in order, and everything a client
shows comes from it. That holds; what it gains is a contract.

**Every event has the same envelope:**

```json
{"seq": 41, "kind": "reply", "turn": "m-7", "at": "…", "body": {…}}
```

- `seq` orders them and is the stream's event id, as now.
- `turn` is the id of the message that caused the event: the message that
  started the run, or the one that answered what it waited for. A client
  groups a transcript by it, and "send and wait" ends on it.
- `kind` and `body` are one tagged type in the code
  (`ConversationEventBody`), so the OpenAPI document says each body and the
  console's types are generated from it.

**The kinds, and whether they are kept:**

| Kind | Body | Kept |
|---|---|---|
| `message` | the message as sent: id, text, payload, sender, attachments, `inReplyTo` | yes |
| `run_started` | `instanceId` | yes |
| `reply` | `replyId`, `text`, `format`, `data`, `attachments`, `instanceId` | yes |
| `waiting_for_input` | `requestId`, `message`, `toolName`, `responseSchema`, `instanceId` | yes |
| `run_ended` | `instanceId`, `status`, `error` | yes |
| `message_failed` | `messageId`, `reason` | yes |
| `reply_route_failed` | `seq`, `replyWorkflowId`, `reason` | yes |
| `activity` | what a run is doing: a model call, a tool call, a step | yes, for a shorter time |
| `reply_delta` | `replyId`, `offset`, `text` | **no** |
| `caught_up` | `lastSeq` | **no** |

Changes from today:

- **`message` is its own event.** Today the message's text rides on
  `run_started` and `message_delivered`. One kind for "someone said this"
  makes the log the transcript without reading two kinds for it.
  `message_delivered` goes; `run_started` keeps only the run.
- **A reply has an id.** `conversation.reply` takes an optional `replyId`;
  left out, it is made from the step and how many replies the step has made
  in this run. The conversation keeps the ids a run has said and says each
  once. This closes "a retried step says it twice".
- **A reply may carry `data`**, any JSON, beside its text: what an AI client
  or an adapter acts on without parsing prose. `format` says what the text
  is, `markdown` (the default) or `plain`.
- **A message says what it answers.** `inReplyTo` names the `requestId` of
  the `waiting_for_input` it is for. With it, the message answers that
  request or fails as stale; without it, it answers what the run waits for
  now, as today.
- **Events that are not kept** are sent to whoever is listening and never
  stored: `reply_delta` (a piece of a reply as it is written) and
  `caught_up` (the stream has sent everything kept; what follows is live).
  They have no `seq`. A client that was not listening misses them and loses
  nothing: the `reply` that follows carries the whole text.

### Pushed, end to end

Nothing waits on a clock. Both polls go, and a slow one stays behind each
as a safety net.

```
run says something ──▶ runtime database ──NOTIFY──▶ engine: advance(conversation)
                                                        │ writes events, in one transaction
                                                        ▼
                                             server database ──NOTIFY──▶ engine: wakes held asks
                                                                              │
                                             client ◀──SSE── API ◀──held ask──┘
```

- **A run wakes its conversation.** Where the runtime records a run's
  custom event, its end, or a request it opens for input
  (`Persistence::insert_event` and its neighbours in `runtara-core`), it
  also says `pg_notify('runtara_run', <instance id>)`, in the same
  transaction. The engine listens on one connection, finds the conversation
  that run carries (an index in memory of the active ones), and advances
  it. The worker's pass over all of them stays, every five seconds in place
  of twice a second, for a notice that was missed.
- **A conversation wakes its readers.** `advance` says
  `pg_notify('runtara_conversation', <id>)` in the transaction that writes
  the events, so the notice arrives when they are readable and not before.
  The engine keeps, for each conversation someone is reading, a
  `tokio::sync::watch` of its last `seq`.
- **The API holds an ask, not a timer.** The operation
  `ConversationEvents` gains `wait`: with it, the engine answers at once if
  there is anything after `after`, and otherwise when the conversation is
  woken or `wait` runs out (25 seconds at most). It is the wire's ordinary
  request and answer, held, as `RunSyncForAnyone` already is. The API's
  stream is a loop of such asks, each answer written to the client as it
  comes. The wire is HTTP/2, so held asks share one connection.
- **Postgres, not Valkey, carries the notices.** It is always there, a
  notice is part of the transaction that made it true, and the API needs no
  new Valkey grant: the API listens to nothing, it asks the engine.

What this gives: a reply reaches its reader in tens of milliseconds, an
idle stream costs one held ask every 25 seconds, and the same code is right
for one engine or several on one database.

### The stream

Server-sent events are the way to read a conversation. The route is the one
there is, `GET …/conversations/{key}/events` with
`Accept: text/event-stream`.

- **Each event** is `id: <seq>`, `event: <kind>`, `data: <the envelope>`.
  Events that are not kept have no `id`, so a reconnect resumes from the
  last kept one.
- **It starts with the past.** From `after` or `Last-Event-ID` (or the
  start), it sends what is kept, then `caught_up`, then what comes. A UI
  shows its spinner until `caught_up`.
- **It says how to come back.** `retry: 2000` at its start. A comment line
  every 15 seconds keeps proxies from closing it. It ends after ten
  minutes; the client reconnects with `Last-Event-ID` and misses nothing.
- **It is bounded.** At most `RUNTARA_CONVERSATION_STREAMS` open at once
  for a tenant (64 unless set); one past it is `429`.
- **`fetch`, not `EventSource`.** A browser's `EventSource` cannot send an
  `Authorization` header. The console reads the stream with `fetch`, as it
  does now, and the documentation says so with an example.

**For a client that cannot hold a stream** (an MCP tool call, a workflow, a
script), the same route without the `Accept` header takes `wait=<seconds>`
and is answered as the held ask is: at once if there is something, else
when there is, else empty when the time is up. It is the same operation of
the engine, so it costs nothing more to offer. Streaming is what a UI uses;
this is what something that makes one request at a time uses.

**Sending and waiting, in one request.** `POST …/messages?wait=<seconds>`
sends the message and answers with the events of its turn, up to the first
`waiting_for_input` or `run_ended` for it, or as far as it got when the
time ran out (with `settled: false`, and the `seq` to read on from). This is
the whole of a chat for a simple client: one request, one answer.

### Replies as they are written

The log holds what was said. What is being written is shown and not kept.

- **A path for what is not kept.** The workflow API's `emit_events` records
  every event. It gains a second kind, transient, that the host hands on
  and does not record: `pg_notify('runtara_run_transient', …)` with the
  piece, at most a few kilobytes. The engine passes it to the
  conversation's readers as `reply_delta`. Pieces are gathered for 50 ms
  before they are sent, so a fast model is about twenty events a second.
- **`offset` makes a gap visible.** Each `reply_delta` says where its text
  begins in the reply. A client that connects in the middle sees an offset
  past what it has, shows nothing of that reply, and takes the whole of it
  from the `reply` event.
- **The AI Agent step streams its model's answer.** This is the larger
  part: a streamed completion in `runtara-ai` for each provider, a response
  body read as it arrives in `runtara-http` and through seal (which sends a
  streamed response on as one), and the step saying each piece as a
  transient event when its run carries a conversation. The final text is
  said with `conversation.reply`, under the same `replyId`.
- **A workflow may do it by hand**: `conversation.reply` with
  `partial: true` says a piece under a `replyId`; the call without it
  closes the reply.

This part is last in the order and may slip without holding up anything
else: every client is right without it, and shows whole replies.

### What a person needs

- **Stop.** `POST …/conversations/{key}/stop` stops the run that carries
  the conversation, as stopping an instance does, and says `run_ended` with
  `status: stopped`. Messages waiting their turn stay, unless `clearQueue`
  is set, which fails each with `message_failed`.
- **A list.** `GET /api/runtime/workflows/{id}/conversations`: newest
  activity first, a cursor, an optional key prefix. Each with its title,
  when it was last active, whether a run carries it, and the last thing
  said in it, shortened.
- **A title and what the client keeps.** `PUT` takes `title` and
  `metadata`, an object of the client's own, up to 4 KB. Neither means
  anything to the core.
- **Reading back.** The events route takes `before=<seq>` with
  `order=desc`, and `kinds=`, so a UI loads the last screen of `message`
  and `reply` events and reads further back as its user scrolls.
- **An end.** A conversation not touched for
  `RUNTARA_CONVERSATION_RETENTION_DAYS` (90 unless set, `0` for never) is
  removed with what was said in it. `activity` events are removed after
  seven days, since they are a run's working and not what was said. A
  conversation is removed with its workflow.

Not here: editing a sent message, asking for another answer to the same
message, more than one person in a conversation, uploading a file. An
attachment stays a name, a type and a URL; where files are kept is decided
with the adapters, which are the first to need it.

### What an AI needs

**As the client.** Four MCP tools, each an operation the engine already
has, under the permissions the routes have:

| Tool | |
|---|---|
| `conversation_send` | send a message and wait for the turn: the replies, and what the run now waits for, with its `responseSchema` |
| `conversation_read` | events after a `seq`, with `wait` |
| `conversation_list` | a workflow's conversations |
| `conversation_stop` | stop the run |

`conversation_send` opens the conversation if its key is new, so talking to
a workflow is one tool call. What it returns is the typed events, with each
reply's `data` beside its text.

**As the workflow.**

- **`conversation.history`**: the last messages and replies of the run's
  own conversation, newest last, up to a number. A workflow that answers in
  context needs what was said; today it is handed only the message.
- **The AI Agent step remembers by conversation.** Its memory is keyed by a
  `conversation_id` the author maps by hand. Where the run carries a
  conversation and the author mapped none, the step uses the
  conversation's id. A chat with an AI Agent then remembers with no
  configuration.
- **`conversation.send`**: below.

### What an adapter needs

An adapter is two workflows: one that takes a platform's message in, one
started with each reply. The conversation has to be enough for both with no
code of the server's.

- **`conversation.send`**, in the built-in agent beside `reply`: the target
  workflow, the key, the message (its id, text, payload, sender,
  attachments), and optionally the reply workflow and `metadata` to set as
  it opens the conversation. It is the engine's `send`, asked by a run for
  its own tenant. It answers what the route answers: the message's state,
  or that the queue is full.
- **The reply workflow is handed what the adapter kept.** Its input gains
  the conversation's `metadata` (the chat's id on the platform, a thread, a
  locale), so the workflow that sends a reply out looks nothing up:
  `data: {conversation: {id, key, workflowId, metadata}, event}`.
- **It chooses what it is started for.** `replyEvents` on the conversation:
  `reply` and `waiting_for_input` unless set; `run_ended` and
  `message_failed` may be added, for a platform that shows "something went
  wrong".
- **The contract is written down**: started once for each such event, in
  order, at least once (an event's `seq` tells a repeat), retried while it
  cannot be started, and `reply_route_failed` when it never can.
- **Proved by a stand-in.** An e2e test with two workflows that do what an
  adapter does, against a fake platform: a message in through an HTTP
  trigger and `conversation.send`, replies out through the reply workflow
  to the fake's endpoint, in order, each once.

### What a UI needs, beyond the console

- **The types are generated.** The events and their bodies are in
  `openapi.json`, which the console already builds against.
- **A credential for one conversation.** Today a stream needs a tenant's
  API key or a signed-in user, so a chat embedded in someone's page for
  their end user has nothing safe to hold. A **conversation token** is made
  by a caller who may run the workflow
  (`POST …/conversations/{key}/token`): short-lived, signed by the engine,
  good for sending to and reading that one conversation and nothing else.
  The page presents it as a bearer credential. The engine verifies it as it
  verifies every credential, and the routes that take it are the
  conversation's own four. Its sender is fixed when it is made, so a page
  cannot speak as someone else.
- **Cross-origin as the API is.** The routes are under the API's CORS
  settings; the stream sets no cookie and needs none.

## What it changes in the code

- **`api/services/conversations.rs`**: the typed event, `turn`, reply ids,
  `inReplyTo`, `metadata`, `title`, `replyEvents`, listing, reading back,
  stop, retention, the notices, and the watch the held ask waits on.
- **`engine.rs`, `engine/wire.rs`, `engine/remote.rs`**:
  `ConversationEvents` gains `wait` and the reading-back fields;
  `SendMessage` gains `wait`; new `ListConversations`, `StopConversation`,
  `ConversationToken`. Each names the permissions of the route that asks
  it.
- **`api/handlers/conversations.rs`**: the stream as a loop of held asks;
  the new routes.
- **`runtara-core`**: the notice beside a run's custom event, its end and a
  request for input; the transient event's path.
- **`crates/agents/runtara-agent-conversation`**: `send`, `history`,
  `replyId`, `data`, `format`, `partial`.
- **`mcp/tools`**: `conversations.rs`, and its entries in
  `tool_permissions.rs`.
- **`auth`**: the conversation token, verified by the engine
  (`engine/listener.rs`), as a credential that names one conversation.
- **`migrate_tenant.rs`**: the three tables.
- **A forward migration** of the server database: `title`, `metadata`,
  `reply_events`, `last_active_at` on `conversations` with an index to list
  by; `turn` on `conversation_events`; the replies a run has said.
- **`runtara-ai`, `runtara-http`, the AI Agent step**: streamed
  completions, last.
- **`scripts/roles/api-database-role.sql`**: nothing. The API is still
  granted nothing on these tables.
- **runtara-frontend**: generated event types, `caught_up`, turns, stop, the
  list and titles, reading back, and `reply_delta`.

The events change shape (`message` in place of the text on `run_started`
and `message_delivered`). Nothing but the console reads them, and the
console and the server move together, as they did in phase 3: no old shape
is kept beside the new one.

## The order

Each is a pull request with its tests, and each leaves `main` working.

| | What | Tests that say it is done | Size |
|---|---|---|---|
| 4.1 | **The contract.** Typed events in OpenAPI, `message`, `turn`, reply ids said once, `data` and `format`, `inReplyTo`. | A retried step's reply is said once. A structured answer with a stale `inReplyTo` fails and is not delivered to another request. The OpenAPI document names every kind's body. | 3 days |
| 4.2 | **Pushed.** The two notices, the held ask, the stream rewritten, `caught_up`, heartbeat and `retry`, the bound on streams, `wait` on reading and on sending. | A reply reaches an open stream within 200 ms of being said, in one process and across the API and engine roles. With notices turned off the slow pass still delivers. An idle stream makes no query between heartbeats. A reconnect with `Last-Event-ID` misses nothing and repeats nothing. | 4 days |
| 4.3 | **For a person and a UI.** Stop, the list, title and metadata, reading back, retention, removal with the workflow, `migrate-tenant`. | Stopping ends the run and keeps the queue. A list is in order of activity and pages. An old conversation is removed and a recent one is not. A moved tenant keeps its conversations. | 3 days |
| 4.4 | **For an AI.** The four MCP tools, `conversation.history`, memory by conversation. | An MCP client holds a three-turn conversation, one of them a structured answer. An AI Agent workflow recalls the first turn in the third with no memory key mapped. | 2 days |
| 4.5 | **For an adapter.** `conversation.send`, `metadata` to the reply workflow, `replyEvents`, the stand-in adapter. | The stand-in adapter's e2e: in order, each reply once, across a busy conversation and a full queue. | 3 days |
| 4.6 | **A credential for one conversation.** | A token reads and sends to its conversation and is refused every other route, another conversation, and after it expires. The engine refuses one the API vouches for and did not sign. | 3 days |
| 4.7 | **Replies as they are written.** The transient path, then streamed completions provider by provider. | A streamed reply's pieces arrive in order and add up to the `reply`. A client that connects in the middle shows the whole reply once. Nothing transient is in any table. | 1 to 2 weeks |

The console follows 4.1 to 4.3 in runtara-frontend, against the release
that carries them, and 4.7 when it lands.

4.1 and 4.2 are the foundation and go first, in that order. 4.3, 4.4 and
4.5 are independent of each other. 4.6 can wait for the first embedded
chat. 4.7 can land after the adapters are begun.

**Then phase 5**: a webhook's headers in the trigger's input, declared
webhook checks in intake, a workflow's own answer to the synchronous
trigger, and the first adapter.

## Decisions to make

Each with what this plan assumes.

1. **Notices over Postgres or Valkey.** Assumed: Postgres. It is always
   present, a notice is part of the transaction, and the API needs no new
   grant. Valkey would be faster to fan out at a volume this is far from.
2. **A request that waits, beside the stream.** Assumed: yes, `wait` on
   reading and sending, for clients that make one request at a time. The
   stream stays the way a UI reads.
3. **Whether replies as they are written are part of phase 4.** Assumed:
   yes, last, and allowed to follow the adapters. The contract in 4.1
   leaves room for them either way.
4. **The conversation token.** Assumed: built in 4.6. If no chat will be
   embedded outside the console soon, it waits, and nothing else depends on
   it.
5. **How long a conversation is kept.** Assumed: 90 days from its last
   activity, `activity` events 7 days, both settable.
6. **Reply text is Markdown unless it says otherwise.** Assumed: yes. An
   adapter turns it into what its platform takes.
7. **What stop does to the queue.** Assumed: keeps it. The next message
   starts the next run.
8. **Files.** Assumed: an attachment stays a URL through phase 4, and where
   files are kept is decided with the first adapter.

## What is not known yet

- **Whether a streamed model response passes through the host to a guest
  as it arrives.** seal sends a streamed response on; `runtara-http` and the
  host's side of a guest's request read whole bodies today. 4.7 starts by
  finding out what that takes.
- **How many notices a busy runtime makes.** A notice for every custom
  event of every run is cheap one at a time. If it shows in a profile, only
  runs that carry a conversation are noticed for, which needs the runtime
  to be told which those are.
- **Whether `turn` is right for a run that answers long after its
  message**, for example one that sleeps and replies the next day. It is
  still the message that started the run; a UI may want to show such a
  reply as new. Settled in 4.1 with the console.

## As built

Each part as it landed, and where it departs from the plan above. The plan
is left as it was written, so the two can be compared.

### 4.1 The contract

Built: the typed event (`ConversationEvent`, `ConversationEventKind` and a
type for each body, all in the API's document), `turn` on every event, the
`message` event, a reply's `replyId`, `data` and `format`, and a message's
`inReplyTo`. A forward migration adds `turn` to the events and to the
conversation, and an index that makes a reply's id unique in its run.

Departures:

- **A reply's id is the author's, or it has none.** The plan had one made
  from the step and how many replies the step had said. The `conversation`
  agent is a component called by the step and is told nothing of it: not
  which step, not which call. So a reply with a `replyId` is said once in
  its run, and one without is said each time, as before. The AI Agent step
  gives its own in 4.7.
- **`run_started` and `message_delivered` still carry the message's text
  and sender**, beside the new `message` event. The plan dropped both at
  once, on the reasoning that only the console reads them and moves with
  the server. The console that is released reads them, and a server
  release must not break its chat before a console release that reads
  `message` exists. They are marked as going, and go when that console is
  the one the release runs.
- **`message_delivered` stays.** The plan removed it. Without it nothing
  says that a queued message was taken as an answer and not as the start
  of a run, or which request it answered.
- **A `message` is logged when it is sent, not when its turn comes.** The
  plan did not say. A message sent while a run is busy is therefore in the
  log before the busy run's later replies, each still marked with its own
  turn: the log is in the order things were said, and `turn` is what groups
  them.
- **The envelope's time is `createdAt`**, as every other time in the API
  is, not `at`.
- **A message with `inReplyTo` and no run to answer fails.** The plan said
  only "answers that request or fails as stale". With no run, the request
  it names is gone, so it fails and starts none: an answer is not a first
  message.

### 4.2 Pushed

Built: a run says when it has something new, and its conversation is read
then; a reader's ask is held by the engine until the conversation says
something; the stream is a loop of such asks, with `retry`, a comment
while nothing is said, `caught_up`, and a bound on a tenant's open streams;
`wait` on reading without a stream; `wait` on sending, answered with the
message's turn. Measured on a running server: a reply reaches an open
stream 8 to 17 ms after the run says it (before, up to a second, and five
when a notice was missed).

Departures:

- **A conversation wakes its readers in memory, not through Postgres.** The
  plan had a second notice, `runtara_conversation`, on the server
  database. The engine is one process (`docs/roles.md`), and both the
  conversation's loop and the asks that wait on it run in it, so a
  `tokio::sync::Notify` for each conversation someone is waiting on is
  enough, is told after the commit, and needs no connection. If the engine
  is ever several processes on one database, this is where a notice goes.
- **The run's notice is one channel on the runtime database** (`runtara_run`,
  `runtara-store-postgres`' `notices`), said from the three places a run's
  event or state is written: an event of the runtime's own, an event a
  running guest leaves, and a change of state. The plan named one place;
  the guest's is another statement, and missing it was found by the
  latency test, which had two of three replies arrive by the slow pass.
- **Every run is told of, not only those that carry a conversation.** The
  runtime does not know which do. The engine drops a notice for a run no
  conversation follows, from an index it keeps in memory. Recorded under
  "What is not known yet" as the first thing to narrow if it shows.
- **The bound on open streams is each API process's own.** A tenant holds
  64 streams open in one API process, not across them: the API keeps no
  count that is shared.
- **`caught_up` is `{"kind": "caught_up", "body": {"lastSeq": n}}`**: the
  envelope without a `seq`, a `turn` or a time, since it is not kept.
- **Sending waits two minutes at most** (`wait` up to 120), reading
  twenty-five seconds. The plan gave only the second.
- **Two tests in the plan are covered another way.** "An idle stream makes
  no query between heartbeats" is not measured: what is tested is that a
  reader that waits is answered when something is said and otherwise when
  its time is up (`tests/conversations.rs`), which is all a stream does
  between events. "With notices turned off the slow pass still delivers"
  is the loop as it was before, still run every five seconds; the latency
  test showed it deliver when a notice was missing.

### 4.3 For a person and a UI

Built: `POST …/stop`, the list of a workflow's conversations, a title and
`metadata`, reading back (`before`, `order=desc`, `kinds`), retention, and
conversations in `migrate-tenant`. A forward migration adds `title` and
`metadata`, and two indexes.

Departures:

- **No `last_active_at`.** The plan added a column to list by.
  `updated_at` already is when a conversation was last active: it moves
  when something is said in it. The list is ordered by it, with an index.
- **A conversation goes with its workflow within the hour, not at once.**
  The plan said "removed with its workflow". The API deletes a workflow by
  marking it in a table of its own, and is granted nothing on a
  conversation's. So what is past keeping is removed by the conversation
  worker's sweep, hourly, and that sweep also removes the conversations of
  a workflow that is marked deleted. One that a run still carries waits
  for the run to end.
- **A run's working has its own setting.** `activity` events are kept
  `RUNTARA_CONVERSATION_ACTIVITY_DAYS` (7 unless set, `0` for ever), beside
  `RUNTARA_CONVERSATION_RETENTION_DAYS`. The plan fixed it at seven.
- **`kinds` holds for a stream too.** The plan named it for reading back. A
  UI that shows only what was said asks for `kinds=message,reply,…` on its
  stream and is sent none of a run's working.
- **Stopping a conversation that no run carries is not an error.** It
  answers that nothing was stopped (`instanceId` absent). The plan did not
  say; a client that stops twice should not have to tell the cases apart.
- **`migrate-tenant` gives a message's place in its queue anew**, in the
  same order, as it does every serial column, since the target's own
  sequence has gone its own way.
