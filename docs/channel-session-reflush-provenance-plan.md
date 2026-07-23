# Channel session re-flush fix — owning-session provenance guard

Status: **planned** (not started). Supersedes the "surface the `Deduplicated`
flag through the engine" idea, which the research below shows is the wrong seam.

## The bug

An inbound channel message (Teams / Slack / Telegram / Mailgun) drives
`channels/session.rs::session_loop`, which calls `ExecutionEngine::queue()` with
a **deterministic** `instance_id` (`UUIDv5(channel:org:activity_id)`). `queue()`
publishes one `TriggerEvent` to the Valkey trigger stream and returns
`QueuedExecution` **immediately** — before any instance starts. The real start
happens later, in a different task: `trigger_worker → execute_detached →
RuntimeClient::start_instance`, which returns `deduplicated: bool`. When the
deterministic id was already accepted, the worker gets
`DetachedExecution::Deduplicated` — and that fact **dead-ends**: it is ACK'd to
the stream and logged, never persisted, never emitted, never returned to the
`queue()` caller (`workers/trigger_worker.rs:~398`).

So a duplicate `session_loop` — holding only the pre-publish `QueuedExecution` —
re-polls the already-run instance from `event_offset = 0` and `flush_events()`
**re-dispatches every past bot reply** to the user (`channels/session.rs:~536`
resets the offset each instance-loop pass; `~546` flushes from 0).

Two existing layers *mostly* prevent reaching this, but neither informs the
session:

1. `reserve_activity` Valkey `SET NX` (TTL now 4h) drops duplicates **before** a
   session is created — the primary guard.
2. The deterministic `instance_id` is the engine backstop (Environment dedups
   the start).

**Residual window:** the Layer-1 Valkey key is lost/evicted **and** a fresh
session spawns for the same `activity_id` — the deterministic-id backstop dedups
at the engine while the fresh session polls and re-flushes.

## Why not "surface the `Deduplicated` flag through the engine"

`queue()` is fire-and-forget across a Valkey stream; the dedup outcome is only
known **later**, in another task. To return it from `queue()` you would have to
either (a) block `queue()` on the downstream start — reintroducing the ack-fast
latency the webhook pipeline just removed and forking a second `execute_detached`
call site — or (b) build an async back-channel keyed by `instance_id` that the
session polls, which is *eventual* (fails open to a duplicate under worker
backlog) and touches the shared worker signature. Both change a boundary used by
**7 other `queue()` callers** (chat, sessions, http events, cron, replay,
reports, MCP) for a bug that lives in exactly **one** consumer.

The load-bearing realization: **the session doesn't need the engine to tell it.**
`InstanceInfo.input` (`runtara-management-sdk` `types.rs:197`, "Input data
provided when starting the instance") is decoded on every `get_instance_status`
(`client.rs:548`) and `session_loop` already calls `get_instance_info` every
~300ms poll tick (`session.rs:~544`). Each session embeds its own
`session_id = Uuid::new_v4()` as `data.sessionId` into the workflow start inputs
(`session.rs:~453`). Environment's **atomic start-or-attach** lets exactly one
`start_instance` win and **permanently persists that winner's input** — so a
session that landed on a foreign-owned instance can read the winner's
`data.sessionId`, see it is not its own, and suppress instead of re-flushing.

## Chosen approach — D: owning-session provenance guard

A session dispatches/flushes for an instance **only when the polled instance's
`input.data.sessionId` equals its own `session_id`.** A session that finds a
*foreign* owner (the redelivery case) suppresses all dispatch.

Properties:

- **Marker is an identity, not a timestamp** — no clock-skew hazard (rejects the
  `created_after` variant), no grace-poll tail (rejects the eventual-key
  variant). It resolves the tight concurrent double-delivery race that a
  pre-check cannot: the loser of a simultaneous double-delivery attaches to the
  winner's instance and reads a mismatched owner.
- **Zero engine blast radius.** `queue()`, `QueuedExecution`, `DetachedExecution`,
  the `trigger_worker` Deduplicated arm, `runtime_client`'s start path, and the
  SDK are all **untouched**. The only new dependency is a read of
  `InstanceInfo.input`, already populated on every poll. The other 7 `queue()`
  callers are unaffected.
- **Zero added latency** — reuses the existing poll tick. The ack-fast webhook
  win is untouched.
- **Fail-open** on absent/malformed `data.sessionId` (matches the existing
  `reserve_activity_dedup` stance) — safe against dropping a real first reply.

### Rejected alternatives (from the design panel)

| Approach | Verdict | Why not |
|---|---|---|
| **A** — async nonce-scoped verdict key | 72 | Eventual: fails open to a duplicate under worker backlog; adds ~1s first-reply latency; touches worker signature. |
| **B** — session-side existence pre-check | 62 | Explicitly does **not** close the residual window (the sub-second race between the first delivery's `queue()` and its `start_instance` INSERT still sees `NotFound`). Good as a defense-in-depth early-exit, not as the gate. |
| **C** — inline synchronous `start_now` | 74 | Full correctness, but **highest** engine risk: a second `execute_detached` call site, a `queue()` prelude refactor, bypasses the trigger stream's at-least-once start durability. D achieves the same closure for a fraction of the risk. |

## Mandatory corrections from adversarial review

The panel's original Slice 3 (graft B's **attach-forward** onto the foreign
in-flight branch) was found **unsound** and is **dropped**:

> In a concurrent double-delivery under `per_message` (a fresh session per
> delivery; the in-memory routing guard at `session.rs:~307` is skipped for
> `per_message`), the *loser* attaches-forward and dispatches all future events
> while the *winning owner is also dispatching them* → the user gets every reply
> from that point on **twice**. Taking over an in-flight foreign instance is only
> safe with a real owner-liveness signal, which D does not have.

Therefore:

1. **Foreign ALWAYS suppresses** — both terminal and non-terminal. On
   foreign + non-terminal, dispatch nothing and keep polling until the instance
   goes terminal, then fall through to idle/exit. No attach-forward.
2. **Decide ownership before ANY dispatch.** Guard the running/streaming dispatch
   block (`session.rs:~570-650`), not just the terminal flush. If `input` is
   absent on a non-terminal first poll, **wait another tick** rather than
   dispatching from offset 0 (transient `input == None` must not leak the running
   branch).
3. **Reset `owns_instance` at the top of each instance-loop iteration** (beside
   `event_offset` at `session.rs:~536`) so an idle-requeued instance
   re-derives ownership. The idle re-queue uses `instance_id: None` (fresh v4)
   but reuses the **same** `data.sessionId` (`session.rs:~714`), so it classifies
   as owned — correct, provided the cache is reset.

### The owner-died-before-flush decision (must be explicit)

Suppressing a **foreign + terminal** instance assumes the owner already flushed.
That is **false** when the owning session died before flushing: it hit
`max_duration` (600s) or panicked before the worker registered the instance, or
the server restarted after the instance completed durably but before the flush;
and note **ownership follows the start winner, not the `queue()` caller** — env
stamps whichever trigger event registered first. In those corners a later
redelivery (Layer-1 key lost) lands on a terminal instance whose owner never
delivered replies, sees a foreign `sessionId`, suppresses, and **the user gets
nothing** — where today it would flush them once. D converts a potential
*duplicate* into a potential *total silence* for that corner.

This needs a decision, not a silent ship. Two options:

- **(v1, recommended) Accept + document + regression-test.** The corner requires
  Layer-1 loss **and** owner death — strictly rarer than the duplicate storm it
  fixes. Ship the guard, document the limitation, and add a regression test that
  pins the intended behavior.
- **(hardening, optional Slice 5) Flush-claim lease.** Make the terminal flush a
  single-flighted claim: any session (owner or redelivery) must win
  `SET NX channel_flush_claim:{instance_id}` (TTL ≈ session `max_duration`)
  before flushing terminal events; the loser suppresses. This delivers replies a
  dead owner never sent (no silence) **and** stays dup-free for the live-owner
  case. Residual: a dead-owner-*mid-stream* death still risks a partial duplicate
  if the recovery flushes from offset 0 — narrower still, and acceptable.

Recommendation: ship v1 (accept + document) with the regression test; file the
flush-claim lease as a fast-follow if the silence corner is unacceptable for a
given deployment.

## The two dispatch sites and the guard

`classify_ownership(input: Option<&Value>, session_id: &str) -> bool` (pure,
near `is_simple_schema`): returns **true** (own/dispatch) when
`input.data.sessionId` is `None` **or** `== session_id`; **false**
(foreign/suppress) only when a *differing* string `sessionId` is present.
Fail-open on absent/malformed input.

In `session_loop`:

- `let mut owns_instance: Option<bool> = None;` beside `event_offset`
  (`session.rs:~536`), reset per instance-loop iteration.
- On the first `Ok(info)` whose `info.input` is present, set
  `owns_instance = Some(classify_ownership(info.input.as_ref(), &session_id))`.
  Never decide on pre-registration `NotFound` ticks (naturally safe: no events
  are listable before the row exists).
- **Terminal flush** (`~546-556`): dispatch only when `owns_instance != Some(false)`.
  Foreign → set `instance_done = true`, skip `flush_events` **and** skip the
  "Sorry, something went wrong" Failed text; fall through to idle so
  `per_sender`/`per_trigger` stay alive for genuinely new turns (`per_message`
  exits).
- **Running/streaming dispatch** (`~570-650`): dispatch only when
  `owns_instance == Some(true)`. Foreign or undecided (`input` not yet present)
  → suppress and keep polling.
- Leave `flush_events`' signature unchanged — gate at the two call sites.

## Blast radius

Confined to `crates/runtara-server/src/channels/session.rs`. Verified untouched:
`queue()`/`QueuedExecution` (`execution_engine.rs:204`), `DetachedExecution`, the
`trigger_worker` Deduplicated arm (`trigger_worker.rs:~398`), `runtime_client`'s
start path, the management SDK, and the other `queue()` callers (`chat.rs`,
`sessions.rs`, the idle re-queue, `workflows.rs`, replay, `reports.rs` — which
keeps its own `SET NX` idempotency). No new field, no return-type change, no
back-channel, no new worker signature.

The engine-level architectural gap — the dedup verdict dead-ending at
`trigger_worker.rs:~398` so any *future* consumer reusing a deterministic id must
re-implement its own guard — is **not** closed by this plan. File it as separate
tech-debt.

## Implementation slices

1. **`classify_ownership` pure helper + exhaustive unit tests.** equal→own,
   different→foreign, absent/`None`/malformed→fail-open-own, `sessionId`
   present-but-empty→own, non-envelope input→own. No I/O; lands independently.
2. **`owns_instance` cache + OWNED and FOREIGN branches** (foreign always
   suppresses, per correction #1); guard the terminal flush and running dispatch
   behind the ownership decision; reset per iteration (#3); wait-a-tick on
   undecided (#2). Unit flush-gating tests with a counting `Channel` double + a
   stub `RuntimeClient` returning a fixed `input.data.sessionId`: matching id →
   sends == 1; different id → sends == 0 (incl. no Failed text). **This slice
   alone closes the dominant terminal-redelivery window.**
3. **e2e `test_channel_reflush_provenance.sh`** on an isolated server (own Valkey,
   relocated env ports, never touch :7001), driving the **webhook/session** path
   the existing `test_trigger_replay_idempotency.sh` never exercises:
   - **residual-window**: deliver an activity, wait terminal, capture reply count,
     `DEL channel_activity_dedup:{tenant:conn:A}`, redeliver the same
     `activity_id` → assert exactly **one** `instances` row **and zero**
     additional replies.
   - **owner-silent-while-alive**: two near-simultaneous deliveries of the same
     fresh `activity_id` (Layer-1 bypassed), both sessions alive → assert exactly
     **one** reply set and the **owner never drops** (this is the assertion that
     would have caught the dropped attach-forward bug).
   - **owner-died-before-flush**: start, expire/kill the owning session before
     flush, mark terminal, redeliver with Layer-1 key deleted → assert the
     **decided** behavior (silence in v1; delivery if the flush-claim lease
     ships).
   - **regression**: single delivery still flushes all N once; `WaitForSignal`
     still fires; idle-phase re-queue with the same `session_id` still dispatches.
   Keep `test_trigger_replay_idempotency.sh` green.
4. **OPTIONAL / fast-follow (non-blocking):**
   - Flush-claim lease (the owner-died hardening above), if v1's documented
     silence corner is unacceptable.
   - `RuntimeClient::try_get_instance_info` (`Ok(Some)`/`Ok(None)` on
     `InstanceNotFound`/`Err`) + factor the `UUIDv5` derivation into a shared
     `channel_activity_instance_id(...)` helper + a `handle_message` up-front
     pre-check to skip spawning a duplicate session actor for `per_message`
     terminal redeliveries. Defense-in-depth on top of the authoritative in-loop
     guard; **not** the gate (it leaves the tight race open).

Slices 1–2 are the minimum that fixes the reported bug; 3 is the proof; 4 is
optional hardening.

## Verified code anchors

- `execution_engine.rs:476` `queue()`; `:581` returns `QueuedExecution`; `:204`
  struct (no dedup field); `:212` `DetachedExecution`; `:~1042` dedup decision;
  `:843/865` Started-only product-event + terminal-watcher gates.
- `runtime_client.rs:67-73` `StartInstanceOutcome{deduplicated}`; `:338`
  populated; `:656` `get_instance_info` (returns SDK `InstanceInfo` verbatim).
- `runtara-management-sdk` `types.rs:197` `InstanceInfo.input: Option<Value>`;
  `client.rs:548` base64-decodes it on `get_instance_status`.
- `trigger_worker.rs:~398` `ProcessResult::Deduplicated` = ACK + log only (verdict
  dead-end).
- `channels/session.rs:~443` `session_id`; `~453` `data.sessionId`; `~472`
  deterministic `instance_id`; `~497` instance from `queue()`; `~536`
  `event_offset` reset; `~544` poll `get_instance_info`; `~546` terminal flush;
  `~570-650` running dispatch; `~714/724` idle re-queue (`instance_id: None`,
  same `session_id`); `~166` `DEDUP_TTL_SECS = 4*3600`; `~300/307` `per_message`
  spawn + skipped routing guard; `~519` `max_duration` 600s.
- Environment atomic start-or-attach persists the winner's input (per SDK
  read-back chain above); `e2e/test_trigger_replay_idempotency.sh` operates only
  at the trigger-stream/Environment layer — no webhook/session coverage today.

_(Line numbers are anchors from a multi-agent read of the tree at plan time;
re-locate before editing.)_
