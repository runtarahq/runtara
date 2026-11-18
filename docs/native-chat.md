# A native chat, and channels built on it

Phases 1 and 2 are built: the platform channels and the Channel trigger
are removed, and the conversation is in the core, with its routes and the
`conversation` agent. Phase 3 waits for the console
([what is left](#what-is-left)). Phases 4 and 5, the adapters, are not
started.

## What is wanted

1. **A native chat**: one structure in Runtara for talking with a workflow,
   which the console's chat uses and which has nothing of any chat platform
   in it.
2. **Channels on top of it**: Telegram, Slack and the rest reach a workflow
   through that structure. Each is an integration, made of a workflow or an
   agent (a `.wasm`), and not code in the server that works around a
   platform.
3. **No quirks in the core**: the platform-specific channels and the
   Channel trigger type are removed from the server.

## What there is today

This was traced against the code before it was written.

### Two conversational paths, one of them full of platforms

**The platform channels** (`crates/runtara-server/src/channels/`, about
6,100 lines and 2,400 of tests, and `api/services/webhook_manager.rs`,
2,400 more). Four webhook routes, one for each of Telegram, Slack, Teams
and Mailgun; a verifier for each; a reply adapter for each; Telegram's
webhook registration and its sealed secret; a durable inbound table
(`channel_intake`); and a session loop that runs a conversation for up to
ten minutes, holding its state in process memory. This is the only code in
the engine that receives anonymous traffic for a conversation, and it
reaches the runtime directly, not through the engine's interface.

**The native surface**: the `chat` and `sessions` routes, a queue for each
session in Valkey (`session_queue/managed`), a delivery worker, and a
stream of server-sent events. It has no platform in it. It is also much
less than a chat:

- **A session is one run.** `POST /workflows/{id}/sessions` starts a run and
  names a session after it. Nothing native ever gives a session a second
  run. Only the platform channels do, by a call nothing else uses.
- **A message is accepted only while that run waits for input.** At any
  other moment `POST /sessions/{id}/events` answers `409
  INPUT_NOT_WAITING`. A message cannot start a conversation, or arrive
  while the workflow is thinking.
- **A session cannot be found by anything but its id**, a random UUID. No
  key says "the conversation with this person".
- **Replies are only pulled.** A client holds a stream open, which polls the
  run every half second and ends after ten minutes. Nothing pushes a reply
  anywhere.
- **What counts as a reply is inferred.** It is an AI agent step's output,
  read from debug events that exist only when the workflow tracks events,
  or one of four names in the run's final output. A workflow has no way to
  say "tell the user this" in the middle of a run.
- **Nobody is speaking.** The only identity is the API caller's. There is
  no end user and no attachment.

So the native chat is not something to move channels onto as it stands. It
is something to finish first.

### What an integration can and cannot do today

An integration built from a workflow and agents would receive a platform's
webhook on an HTTP trigger and reply with an agent. Agents that send
already exist for Slack, Teams and Mailgun. What stands in the way:

- **A workflow cannot talk to another workflow's conversation.** The
  `control` agent starts children of the calling run and answers a waiting
  run whose id it already knows. A run started by a webhook has no way to
  find the conversation its message belongs to.
- **A webhook's headers are not given to the workflow** an HTTP trigger
  starts, and a platform's signature is in them.
- **Webhooks are checked for Mailgun only**, by code written for Mailgun.
- **The synchronous trigger wraps what the workflow answers**, and some
  platforms need a body of their own back at once (Slack's handshake).
- **There is no Telegram agent.**
- **Teams replies need a signed target** that only the channels code makes.

## The design

### The conversation, in the core

One new thing: a **conversation**. It is a workflow, a key, and what has
been said.

- **It is found by its key.** `(tenant, workflow, key)` names one
  conversation, durably. The key is the client's: the console uses one for
  each chat a user opens; an integration uses the platform's own
  (`telegram:<chat id>`). The same key always reaches the same
  conversation. It is a row in the server database, not a map in memory.
- **A message can be sent at any time.** If no run is active the
  conversation starts one with the message. If the run is waiting for
  input the message answers it. If the run is busy the message waits its
  turn, in order. When a run ends, the next message starts the next run.
  This replaces "a session is one run".
- **A message is sent once.** It carries the sender's own id for it; the
  same id again is the same message. The queue that exists already does
  this for seven days, and is kept.
- **It says who is speaking.** A message carries a sender (an id and a
  name, as the client gives them) and attachments (a name, a type, and
  where to fetch each). The core passes them to the workflow and makes
  nothing of them.
- **A workflow says what it tells the user.** A built-in agent,
  `conversation`, with a capability `reply`, called from any step of a
  run: text, and attachments. What the user is told is then what the
  workflow said, not what was guessed from its debug events. The final
  output of a run is still a reply, so a workflow that only answers at its
  end needs no change.
- **Replies are pushed as well as pulled.** A conversation may have a
  **reply route**: a workflow to start with each reply (and with each
  "waiting for input"). Without one, replies are read from the stream, as
  the console reads them. With one, the core starts that workflow, through
  the outbox, like any other run nobody asked for.

The conversation's own loop (start a run, deliver a message, collect
replies) is the generic half of what the channels' session loop does
today. It moves behind the engine's interface, is kept durably, and loses
everything that names a platform.

### The native chat

The console's chat is the first client, over the API:

| | |
|---|---|
| `PUT /api/runtime/workflows/{id}/conversations/{key}` | open or find a conversation; optionally set its reply route |
| `POST …/conversations/{key}/messages` | send a message: its id, text or payload, sender, attachments |
| `GET …/conversations/{key}/events` | the stream: replies, waiting for input, what a run is doing |
| `GET …/conversations/{key}` | its state: the active run, what it waits for, what is queued |

The existing `chat` and `sessions` routes become these, and are removed:
they are the same thing with less in it.

### Channels on top of it

An integration is two workflows and the agents they call. Nothing of it is
in the server.

(Returned to after the core is settled: phases 4 and 5.)

- **In.** An HTTP trigger receives the platform's webhook. Its workflow
  turns the platform's message into a conversation message and sends it,
  with the `conversation` agent's `send`: the target workflow, the key, the
  message's id, the sender, the text, the attachments. It sets the reply
  route to the integration's other workflow.
- **Out.** The core starts that workflow with each reply. It calls the
  platform's agent to send it.

For that the core gains four things, each generic:

1. **`conversation.send`**, for a workflow to send a message into another
   workflow's conversation. Beside `reply`, in the same built-in agent.
2. **A webhook's headers in the trigger's input**, so a workflow can read
   them.
3. **Webhook checks declared, not written.** A connection type says how its
   webhooks are signed: which field holds the secret, what is signed (a
   template over the headers and the body), where the signature is, and
   how old it may be. Intake checks every webhook that way, with seal,
   before a run is queued. This covers a shared-secret header (Telegram)
   and a signature over the body (Slack, Mailgun), and replaces the code
   written for Mailgun. A platform that signs some other way (Teams, with a
   token checked against published keys) is checked by its own agent, in
   the integration's workflow.
4. **A workflow's own answer to a synchronous trigger**: a status, headers
   and a body, for a platform that needs one back at once.

And two things each integration needs of its own platform, which are its
agent's to do: registering its webhook (Telegram), and making the target a
reply is sent to (Teams).

### What is removed

Done, as phase 1. One thing differs from the list below: sealing and
checking a webhook's secret token stays in `runtara-connections`. It is
seal's own primitive, exercised by the tests of seal as a service, and the
declared webhook checks of phase 4 need it for a platform that sends a
shared secret.

- `crates/runtara-server/src/channels/`, whole.
- `api/services/webhook_manager.rs`, whole.
- The Channel trigger type, its validation, its hooks in the trigger
  handlers, its webhook URL and secret handling.
- The engine's `Channels` part and its `reconcile_channel_webhooks`
  operation, the channel intake worker, the startup pass that seals
  webhook secrets.
- `channel_intake`, by a new migration. Triggers of the Channel type are
  deleted by the same migration: nothing reads them afterwards.
- In `runtara-connections`: looking up a connection by id across tenants
  for a channel, and sealing and checking a Telegram webhook secret.
- The buffer functions of `session_queue.rs` that only channels use.
- Their tests, e2e scripts and documents.

Kept, because agents or the conversation use them: the `slack_bot`,
`teams_bot`, `mailgun` and `telegram_bot` connection types, checking a
signature with seal, the managed queue and its delivery worker, and what
verifies the target of a Teams reply.

### As built

`api/services/conversations.rs` is the whole of it, behind the engine's
interface (`Engine::open_conversation` and four more, each an operation of
the wire). Three tables of the server database, which the API role is
granted nothing on: `conversations`, `conversation_messages`,
`conversation_events`.

- **One loop, safe to repeat** (`Conversations::advance`), run when a
  message is sent and by the engine's `conversations` worker, under the
  conversation's row lock. It reads what the active run said, says what it
  waits for, hands it the next message, starts the next run, and starts the
  reply workflow. A run's id is made from the conversation's and the
  message's, so starting it again starts the same run.
- **A message** is `{id, text?, payload?, sender?, attachments?}`. It
  starts a run with `data: {message, payload, messageId, sender,
  attachments, conversation: {id, key, workflowId}}`. Answering a waiting
  run, `payload` is the answer as it is, and `text` alone is `{"message":
  text}`. Its state is `queued`, `started`, `delivered` or `failed`; the
  answer to sending it says which, where that is known at once.
- **Events** are numbered and kept: `run_started`, `reply`,
  `waiting_for_input`, `message_delivered`, `message_failed`, `run_ended`,
  `reply_route_failed`, and `activity`, which is what the old chat stream
  showed of a run (model and tool calls), for a client that shows it. A
  reply is what the run said with `conversation.reply`. A run that said
  nothing answers with what it returned: `answer.response`, or `answer`,
  `response` or `message` as a string. A run that replied is not also
  answered for from its output.
- **The reply workflow** is started once for each `reply` and each
  `waiting_for_input`, in order, with `data: {conversation, event: {seq,
  kind, body}}`. One that cannot be started now is tried again; one that
  never can be is said so (`reply_route_failed`) and the reply is still
  read.
- **The bound** is 32 messages waiting, or
  `RUNTARA_CONVERSATION_QUEUE_BOUND`. One past it is `429
  CONVERSATION_QUEUE_FULL`.
- **Deleting** a conversation deletes what was said in it. A run that
  carries it is left to end.

What differs from the plan above:

- **The queue is rows of the conversation**, not the Valkey queue of the
  old sessions: a message waits until its turn, however long, and "once"
  holds for as long as the conversation does. The managed queue and its
  delivery worker are still what the old `sessions` routes use.
- **A reply said by a step that is retried may be said twice.** A step
  that ran is not run again on resume, so this is a step that failed after
  saying it.
- **A conversation is read twice a second while it has something to do**
  (a run, a message waiting, a reply not yet routed), by one pass of the
  worker over all of them. Enough for now, and the first thing to change
  if conversations are many: the run's own events could wake it.

Tested by `tests/conversations.rs` (against the database, with runs stood
in for), `e2e/test_conversations.sh` (a server, real workflows and the
agent), and in `e2e/test_two_roles.sh` and `e2e/test_served_tenants.sh`
(through an API in another process, and under row-level security).

### What is left

Phase 3 is the console's: it moves from `POST /workflows/{id}/chat` and the
`sessions` routes to the conversation routes, and stops offering the
Channel trigger type, in runtara-frontend. The console builds against a
release's `openapi.json`, so that follows a release that has these routes.
The `chat` and `sessions` routes are kept until then and are removed
here once the console has moved, with the managed session queue and its
delivery worker if nothing else uses them by then.

## What this does to the engine split

- **The engine receives no anonymous traffic for conversations.** A
  platform's webhook is an ordinary trigger's, which intake already
  receives. "Move chat channels to intake" (splitting-the-engine, Phase 7,
  preparation 2) is no longer needed: there is nothing to move.
- **What checks a webhook is the connection type's declaration**, kept
  where intake can read it without the connection's row. This replaces the
  copy on each trigger that Phase 7 wrote, and its thirty-second pass.
- **OAuth redirects** are then the only anonymous traffic the engine
  receives.

## Phases

The architecture first, then the adapters (decision 2).

**Now: the core.**

1. **Remove the platform channels and the Channel trigger** (decision 1). A
   deletion, with one migration. It goes first and stands alone. From then
   on no chat platform reaches a workflow, until an adapter is built.
2. **The conversation in the core**: the row and its key, a message at any
   time held in a bounded queue, the `conversation` agent's `reply`, the
   reply route, and the four routes. The delivery queue and worker are
   kept. Tests: a conversation across several runs; a message while the
   run is busy; the queue at its bound; the same message twice; a reply
   from the middle of a run; a reply route started for each reply; two
   tenants' conversations with the same key.
3. **The native chat on it**: the console moves to the conversation
   routes (in runtara-frontend), and the `chat` and `sessions` routes are
   removed.

**Later: what adapters need, and the adapters.** Not started until the
core above is settled.

4. `conversation.send`, headers in the trigger's input, declared webhook
   checks in intake, a workflow's own answer to the synchronous trigger.
5. The adapters: each two workflows and its platform's agent.

Deferring the adapters has one cost to keep in mind: the conversation is
designed for them and proved without one. The reply route in particular is
exercised only by tests until an adapter uses it. Phase 2's tests stand in
for an adapter with a workflow that does what one would: sends a message
into a conversation, and is started with each reply.

## Decisions

1. **Remove first.** The platform channels and the Channel trigger go
   before anything is built in their place.
2. **No integrations yet.** The architecture is fixed first; adapters, and
   where they live, are returned to afterwards.
3. **A reply route starts a workflow**, and nothing else for now.
4. **A conversation lives until it is deleted.** What its queue holds is
   kept seven days, as now.
5. **A busy conversation queues messages**, in order, up to a bound; past
   the bound a message is refused, and the sender is told so.

## Size

Phase 1 is a day or two, mostly deletion and its tests. Phase 2 is the
work: about the size of Phase 2 of separating-concerns' larger groups, a
week. Phase 3 is small here and is the console's elsewhere. Phases 4 and
5, when they are returned to, are a few days each.
