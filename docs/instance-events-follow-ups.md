# Channel input reliability: follow-up spec

Gaps that remained after the managed input-request fix (#267, `decf5c86`).
Sections 1 and 2 are implemented; section 3 is open. Each section states the
defect, the required behaviour, and what proves it.
Paths are relative to `crates/runtara-server/src/`.

## 1. Durable provider intake — implemented

Inbound messages were acknowledged before anything durable was written, and a
Valkey dedup key then blocked the provider's redelivery.

Now (`channels/intake.rs`, migration `20260926000000_channel_intake.sql`):

- Each handler resolves the route, inserts the message into `channel_intake`
  keyed on `(tenant, connection, identity)`, and only then returns 200. A
  storage failure returns 503 so the provider retries. A duplicate identity is
  acknowledged and dropped. Teams stores before its ack; session handoff still
  runs in the background.
- Identity comes from the native handlers (Slack `event_id`, Telegram
  `update_id`, Teams activity `id`, Mailgun `Message-Id`), falling back to a
  payload hash. Moving extraction into channel agents is tracked in
  [trusted-improvements.md](trusted-improvements.md).
- A row stays `pending` until the session launches an execution, buffers or
  refuses the reply, or consumes it for field collection. Rows that can never
  succeed (no route, invalid input) become `failed`.
- `ChannelRouter::run_intake_worker` runs for the life of the process. At
  startup it dispatches rows an earlier process left pending. Every 30s it
  claims pending rows whose `next_attempt_at` has passed and dispatches them
  again, so a transient launch failure is retried without a restart. A new row
  gets a 2-minute handoff grace; each claim then backs off 30s doubling to an
  hour, and a row claimed 12 times is failed.
- Handled (`processed`/`failed`) rows are deleted 7 days after their last
  update, in batches from the same worker. That exceeds the providers'
  redelivery windows (Telegram's is the longest, about a day). A redelivery
  after deletion still maps to the same instance id and is deduplicated by the
  execution engine. Pending rows are never purged.
- An actor that exits with messages still in its channel routes them again.
- The Valkey dedup key is removed.

Proven by `channels/intake_tests.rs`, run for all four providers through the
HTTP routes: storage failure → 5xx then a successful retry; acknowledged
message whose launch is lost → launched once after a restart; transient launch
failure → left alone during the grace, then launched once by the sweep;
redelivery → one intake row and one execution; a reply left by an ended
session → handed to its own request once, never launched. Retention and attempt
exhaustion are tested against the store.

- A reply records the request it was bound to (session, instance, request,
  payload) before it is buffered, and is buffered under its intake id. Its row
  stays pending until the reply reaches the managed queue, whose delivery
  worker needs no session. If the session ends first, the sweep hands the reply
  to that request under the same id (deduplicated if the handoff already
  happened), and never launches a run from it. A reply whose request closed is
  marked `undelivered`; one to a structured request is `interrupted`, since
  field collection lived in the ended session.

### Remaining

- The 7-day retention and provider retry assumptions are not yet checked
  against each provider's official webhook documentation.

## 2. Startup launch identity — implemented

- Every channel launch uses the intake id as its instance id, including
  messages that start a run from an idle session. The intake id is derived from
  the provider identity, so even a redelivery after its row is removed maps to
  the same instance and is deduplicated by the execution engine.
- An idle-session message stays pending until its run is queued, so a crash
  between taking it from the startup buffer and queueing launches it after
  restart.
- The session-map check and insert are atomic (`DashMap::entry`): concurrent
  first messages from one sender share one session.

Still true, and must stay so: a startup message is never delivered as a
response to the new execution's first wait. Startup and reply buffers are
separate, and managed delivery is bound to a specific request.

- The workflow and its current version are fixed when the message is
  accepted (`channel_intake.workflow_id`, `workflow_version`). Every launch
  from the row, including retries, restarts, and idle-session launches, uses
  that version even if a newer one is published first.

Prior art: unverified draft on local branch
`codex/instance-input-delivery-followup` (`509e2600`). Do not merge it; its
replacement-launch behaviour conflicts with "a stale managed reply never
launches a new execution".

## 3. Restartable structured collectors

### Defect

- Field progress (`collected`), validation retry counts and processed replies
  exist only in the session actor (`channels/collector.rs:16`, `:41`). The
  `prompted` set is in memory too (`channels/session/inputs.rs:32`).
- Buffered replies are acknowledged when read (`channels/collector.rs:86`),
  before progress is persisted. After a restart, collection restarts at the
  first field and already-given answers are lost.
- If the process dies after collection completes but before the final response
  is enqueued, the payload is lost. After the enqueue, delivery is already
  idempotent: a stable operation id, and core returns the stored receipt.

### Requirements

- Persist per-request collection state: the target request and schema,
  collected fields, validation attempts, processed reply ids, issued prompts,
  and the final response operation id.
- Acknowledge a reply only after its effect on the collection state is
  persisted.
- Report "consumed for collection" separately from "accepted as the workflow
  response".
- Cancelling a collection does not submit a response.

### Acceptance

- Restart between fields resumes at the next unanswered field without
  re-prompting answered ones.
- Restart after the final field and before enqueue → exactly one response is
  submitted.
- Replaying a processed reply id has no effect.

## Verified

- **Reply before request registration.** A reply that arrives before its
  execution has registered and prompted an input request is refused with a
  "nothing is waiting" notice. It is never buffered for, or bound to, a later
  request, and it cannot launch a second execution (it arrives on the reply
  path, not the startup path). Covered by
  `a_reply_needs_a_prompted_open_request_when_it_arrives`
  (`channels/session/input_tests.rs`), for both "not registered" and
  "registered but not yet prompted".
