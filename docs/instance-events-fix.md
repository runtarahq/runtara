# Pending instance inputs: correctness fix

Status: implementation complete and locally verified on 2026-09-25; prepared for
PR review and not deployed. Re-centered at the user's request. This document
supersedes the expanded plan
preserved in [historical implementation notes](instance-events-implementation-notes.md).

## The bug and the completion boundary

An `external_input_requested` event describes a past request. It does not prove
that the wait is still open. A wait may have been answered, timed out, abandoned,
or cancelled without a matching debug completion event. Its root may also have
terminated. Reconstructing actionable inputs from unmatched events therefore
shows stale forms and can direct a response at a dead or different wait.

The fix is complete when every existing actionable consumer uses authoritative
request state, submissions arbitrate against closure and termination, and the
regressions below pass. Preserve historical events; do not fabricate successful
step completions or delete events to hide the bug. Optional debug telemetry must
not determine correctness.

This feature has not been used. No backward-compatible event fallback, dual
reads, legacy managed-submission adapter, or event backfill is required. Preserve
unrelated arbitrary custom signals and current authorization boundaries.

## Scope

Keep the durable request lifecycle, immutable acceptance receipts, necessary
runtime/compiler ownership and wake integration, authoritative discovery,
consumer migration, and their tests. These changes directly establish whether
an input is actionable and whether a response was accepted.

Channel migration remains required: channels cannot continue selecting the last
request event or writing raw signals after the compiler switches to managed
waits. Limit this to authoritative discovery, binding a response to the chosen
request, validated acceptance, and retaining a queued response until acceptance
or an explicit failure. Do not redesign provider intake or collectors to achieve
that migration.

Move durable provider inboxes, provider acknowledgement/deduplication redesign,
restartable multi-field collectors, startup handoff state machines, new channel
owner dashboards, generalized worker load/retention projects, and diagnostic
exactly-once delivery to [follow-ups](instance-events-follow-ups.md). They are
not release prerequisites for this fix. The implemented startup and replacement-launch draft has been extracted to
`codex/instance-input-delivery-followup`; its presence there is not a reason to
finish or ship it with this fix.

Existing session queue work can be reused where it directly preserves a pending
response and retry identity. It must not expand this fix into reliable launch
or provider delivery. No Control-agent implementation or MinIO replacement is
included.

## Required behavior

### Authoritative request lifecycle

- Register each logical wait durably, independently of `track_events`. Identity
  distinguishes loop iterations, embedded call sites, child invocations, and
  repeated AI wait-tool calls. Replayed registration preserves the original
  identity/deadline and never reopens an accepted or closed request.
- An actionable request belongs to the authorized tenant and an existing,
  non-terminal root, is open and unexpired, and has a valid logical invocation
  owner. A temporary physical runner lease change must not destroy a resumable
  logical wait.
- Timeout, abandonment, child cancellation, and root termination invalidate the
  appropriate unanswered requests. Child closure preserves live siblings.
  Handled errors cannot leave the old wait actionable while recovery continues.
- Missing instances remain not-found. Completed, failed, cancelled, and timed-out
  instances expose no actionable requests even with unmatched historical events.
- A registered unexpired wait may accept while explicitly paused, but acceptance
  cannot implicitly resume it. Sleeping/paused status alone creates no request.

### Atomic acceptance and retry

- Check current authorization before reading or returning any receipt. Require
  instance, request, stable caller operation identity, and payload; validate new
  responses against the wait's schema, not the startup schema.
- At persistence, arbitrate acceptance against request closure, root termination,
  and competing responses. If closure wins, reject without an acceptance or wake.
  If acceptance wins, retain that result even if the root subsequently terminates.
- A matching operation retry returns the original receipt before new-submission
  liveness/schema checks. Changing its request, canonical payload, or applicable
  report context conflicts. Canonical JSON sorts object keys while preserving
  array order, scalar types, and parsed number representations.
- Retain receipts for the retained instance's lifetime. Acceptance means the
  response was accepted, not that downstream workflow execution completed.
- Preserve the accepted bytes through consumption and replay. Raw writes cannot
  overwrite managed addresses; unmanaged custom signals retain their behavior.
- Persist acceptance and any required parked wake atomically. Cover both
  accept-before-park and park-before-accept. Never wake a terminal or explicitly
  paused instance through acceptance or stale scheduler work.

### Discovery and existing consumers

- Use one tenant-aware discovery contract for actions, pending-input APIs,
  reports, execution flags, MCP, web chat, and channels. Return request identity
  and the current wait schema. Lists are snapshots; acceptance rechecks state.
- Fail discovery visibly on backend errors. Do not report an empty list or
  `has_pending_input=false` when the state is unknown. Suspended waits remain
  discoverable; event retention and pagination cannot determine openness.
- Page/count authoritative requests correctly, including workflows with more
  executions than the first execution-list page.
- Auto-select only one eligible request. Multiple requests require selection or
  an already explicit target; never select the newest historical event. Once a
  response is bound, retries keep that target and operation identity.
- Stale/conflicting submissions refresh or clear the action without overwriting
  a response, consuming another wait, or implicitly starting another execution.
  A failed queued-response delivery must retain the message or record an explicit
  failure; changing target is not an automatic retry policy.
- Retain a client submission's payload and identity after an uncertain result.
  Receipt retry must still work if discovery has removed the accepted action.
  Report replay preserves its original effective payload while enforcing current
  access. The current mounted-page retry guarantee does not promise browser
  reload persistence.
- History may display request events. Historical display must not create an
  actionable form, collect an answer, or submit a response on its own.

## Current-state audit

Inspected and tested on 2026-09-25. The checkpoints below record the commands,
results and limitations; historical notes retain earlier runs.

| Area | Observed implementation | Remaining evidence/work |
| --- | --- | --- |
| Core and stores | Managed request/receipt types, memory/PostgreSQL implementations, migration 030 and lifecycle arbitration. | 199 Core/store tests passed, including managed conformance and database races. |
| Runtime/compiler | Managed imports, ownership, closure and compiled fixtures. Exit cleanup retains its physical generation through storage failure. | Production scoped/managed/wake tests and 396 compiled cases passed. The library fixture migration and component checks passed as recorded below. |
| API/report/MCP | Managed discovery/submission and contextual report receipt replay. | Real REST/MCP, report authorization, pagination, unavailable discovery and receipt replay regressions passed. |
| Frontend | Authoritative chat polling and mounted execution/report retry owners. | 1,476 tests, lint and production build passed; runtime client regenerated. |
| Channels | Periodic managed discovery drives plain and structured input independently of debug history. Responses retain their original execution, request, operation and payload through queue handoff. | Closure, ambiguity, discovery failure and uncertain delivery cases passed. Provider/collector redesign stays deferred. |
| Extra delivery work | Startup modes and replacement-launch orchestration extracted to the local follow-up branch/worktree. | Follow-up worktree is clean; its draft remains unverified and outside the completion gate. |

Concrete source evidence:

- `crates/runtara-server/src/channels/session.rs` and `session/inputs.rs`: the
  actor polls managed requests independently of historical events. The event
  dispatcher only renders output. Neither the latest-event selector nor raw
  response writes remain. Collection uses the current schema and checks closure;
  failure or `/cancel` leaves the request unanswered.
- `crates/runtara-workflows/tests/direct_wasm_execute.rs`: `PersistingRuntimeHost`
  now registers, polls and closes real in-memory managed requests. Its composed
  and embedded tests submit validated responses and reject raw polling.
  `CheckpointingRuntimeHost` and both capturing fixture bindings now use managed
  requests too. Timeout tests assert persistence-owned expiry. Deadline
  checkpoints remain valid replay inputs; they do not establish whether a request
  is open. The optional composed HTTP runtime still traps before input IO; see the
  latest checkpoint for the control result and verification limit.
- The active `api/services/session_queue/delivery.rs` no longer queues executions.
  Every response is bound to its request when it is retained (see the post-review
  fixes below); delivery submits only that binding and replays its receipt before
  current-state checks. An unbound legacy message is blocked for resolution.

### Scope inventory after extraction

| Retained change | Why the original fix needs it |
| --- | --- |
| Managed request records, migration, store arbitration | Events cannot establish whether a wait is still actionable; acceptance must race closure atomically. |
| Runtime/WIT/compiler registration, ownership, closure, required wakes | Tracking-off and child waits need real lifecycle state and must still resume correctly. |
| Retained runner-exit intent and forward environment migration | A failed terminal cleanup must remain recoverable instead of reopening an abandoned request through ordinary orphan recovery. |
| API/report/MCP/frontend migration and receipt retries | Existing consumers must agree on actionable state and handle stale or uncertain submissions. |
| Response envelopes, lease/bind/ack, delivery status and worker | A failed managed response cannot be popped and lost or rebound to a newer request. |

Moved: `StartupIntent`, `DeliveryMode`, startup handoff states/transitions,
`delivery/startup.rs`, replacement `LaunchIntent` preparation/publication,
launch-only route snapshots, and their existing launch recovery tests. The
follow-up branch keeps the exact pre-extraction source with its shared
prerequisites; this is preservation of unfinished work, not release approval.
Provider inboxes and restartable collectors were plans, not implemented code,
and stay in the follow-up document. Unrelated local build configuration and
personal documents remain untouched and are excluded from the follow-up branch.

### Extraction verification (2026-09-25)

Deferred implementation is saved on `codex/instance-input-delivery-followup`:
`71b32eae` preserves the unfinished prerequisites and `509e2600` isolates the
restored delivery draft and branch handoff notes. The follow-up worktree is
`/private/tmp/runtara-input-delivery-followup`; its top commit is the review unit
for later implementation. No remote push occurred. The active branch/index were
not committed or changed by the extraction.

Fresh checks on the narrowed active code:

- `cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests --lib api::services::session_queue::managed::tests -- --test-threads=1`:
  13 passed with isolated Valkey.
- `cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests --test session_delivery --test managed_actions -- --test-threads=1`:
  5 passed with isolated server/runtime PostgreSQL (pgvector for server) and Valkey.
  Coverage includes terminal targets without replacement launch, ambiguity,
  delayed initial registration, and lost acknowledgement before/after completion.
- `cargo clippy -p runtara-server --features db-integration-tests,valkey-integration-tests --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, and `git diff --check`: passed.
- `npm run generate-api-runtime-offline` with the pinned Node version: passed and
  produced byte-identical runtime TypeScript, resolving the draft status mismatch
  by removing its deferred fields rather than expanding the client contract.

The follow-up branch's pre-commit workspace formatting/Clippy checks also passed;
its runtime behavior is still an unverified draft. Hash comparison confirmed all
131 paths outside the extraction's eight source/test paths and two active docs
were unchanged. Full lifecycle/compiled/frontend release verification has not
been rerun for this extraction. Channel migration continued below; this
checkpoint does not complete the overall fix.

### Structured channel collection checkpoint (2026-09-25, superseded below)

The structured-input path now verifies that exactly one eligible managed request
matches the event's diagnostic address. A closed/terminal, ambiguous or different
request cannot start collection. Prompt/schema metadata come from the request,
not the historical event. The collector checks eligibility before prompting,
after each field, before completion and periodically while waiting without a new
reply. Failure and `/cancel` leave the workflow request unanswered.

`enqueue_targeted` atomically retains a response with its selected instance and
request before a worker can claim it. Collected replies use this path, retaining
one operation and payload across an uncertain enqueue. Queue redelivery cannot
choose a different request. Field progress remains actor-local; this adds no
provider inbox, startup handoff or restartable collector.

Fresh verification: five `channels::session::input_tests` and all 15
`api::services::session_queue::managed::tests` passed with isolated Valkey and
in-memory runtime persistence, using the server's database/Valkey integration
feature gates. Channel cases cover completed/failed/cancelled roots, abandoned
waits, ambiguity, another open wait, authoritative schema, cancellation, closure
between fields and closure while idle. Queue cases cover atomic initial binding,
matching receipt replay, changed-target conflicts and refusing to repurpose an
unbound operation. Server check and Clippy across all targets with both feature
gates passed. No public DTO changed.

This was the partial migration checkpoint. The following work replaces its
event-triggered collection and completes the plain-text path. Full release
verification remains outstanding.

### Channel discovery and response handoff checkpoint (2026-09-25)

Plain and structured waits now use periodic authoritative discovery, including
when there are no debug events. Zero requests, one request, ambiguity and backend
failure are separate outcomes. Historical request events cannot prompt or consume
an answer. Startup input is never reused to answer the first wait.

The channel buffer records a unique message identity and its original execution.
Before handoff, it atomically binds the selected request in the source record.
Only successful managed enqueue allows a conditional source acknowledgement.
An uncertain handoff retries that exact target and operation before consulting
new discovery, including after the destination queue's completed record expires.
The retained runtime receipt remains the authority for acceptance.

Terminal execution cleanup retains active replies for explicit resolution;
they cannot become a fresh startup message. Outstanding managed replies prevent
an idle actor from silently launching another execution. The remaining destructive
`take_startup_event` is limited to fresh idle/startup messages. Reliable startup
handoff remains deferred on the separate branch.

Collectors consume early buffered fields, check closure before/after reads and
while idle, and retain the final response with one operation across uncertain
enqueue. Field progress remains in memory. Provider acknowledgement/deduplication
and actor restart recovery are unchanged; this does not promise durable provider
intake or restartable collection.

Source audit: remaining raw `send_custom_signal` methods are the unmanaged
runtime/environment facade, not channel response call sites. The event matcher
in `pending_inputs.rs` is historical diagnostics only; chat history conversion
still renders diagnostic events. Actionable channel selection uses neither.

Fresh verification with isolated Valkey and separate runtime/server PostgreSQL
(pgvector for server):

- Server library suite with `db-integration-tests,valkey-integration-tests`:
  1,188 passed, including 11 channel regressions and 15 managed queue regressions.
- `managed_actions` and `session_delivery` with those feature gates: five passed.
- Server Clippy with both feature gates and all targets: passed after correcting
  one collapsible conditional. Workspace formatting and whitespace checks passed.

The channel cases include direct closed/terminal discovery, tracking-off requests,
early buffered fields, ambiguity, backend failure, stale targets after handoff
failure, conditional acknowledgement and original receipt replay after destination
retention cleanup. No public DTO changed. These checks do not complete the release
matrix. The next work is compiled test-host migration and lifecycle recovery after
mandatory closure failure, followed by final consumer and CI checks.

### Compiled composed-wait checkpoint (2026-09-25)

`PersistingRuntimeHost` delegates managed registration, poll and close to the real
in-memory persistence implementation through `tests/support/managed_inputs.rs`.
Scripted replies are validated and accepted only after a request exists. Raw
polling returns an error, so the tests cannot accidentally pass via the old path.
The two-site embedded/composed cases also inspect the retained managed records.
Pause/resume submits before resuming and verifies unchanged request identity,
specification and registration timestamp after acceptance.

Fresh checks:

- `RUSTC_WRAPPER= RUNTARA_NO_INSTALL_TOOLS=1 scripts/build-agent-components.sh`:
  passed, rebuilding/staging 26 agents and both shared workflow components.
- `cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute per_site_signal_ids -- --test-threads=1`:
  two passed with event tracking disabled.
- Same target with `pause_ -- --test-threads=1`: five passed, including composed
  and nested waits plus sibling checkpoint/cancellation regressions. Three local
  HTTP fixtures initially could not bind under the sandbox; the rerun with local
  networking permission passed all five.
- Same target with `scoped_signal_wait_survives_drain_and_resume -- --exact`:
  one passed. This case preloads a deadline checkpoint; it does not by itself prove
  production runner recovery after a failed mandatory close.
- `cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings`,
  workspace formatting and whitespace checks: passed.

This host models shared root ownership for composed calls; it does not replace
production independently cancellable child-owner conformance. The next checkpoint migrates the remaining fixtures and adds repeated AI waits.
Production recovery, child/wake race coverage and the final CI matrix remain
separate evidence requirements.

### Managed fixture and recovery checkpoint (2026-09-25)

The virtual-clock host and native/HTTP capturing fixtures now register, poll and
close through real in-memory managed persistence. They reject raw signal polling.
A response script answers each new logical request once; replay keeps the original
store and supplies no new response. The two-run wait tests compare complete
request/receipt snapshots before and after replay, instead of re-injecting raw
signals into fresh stores. These hosts still model root ownership, not independent
child authority or all root terminal callbacks.

New/updated compiled coverage proves:

- Guest time ahead of the deadline cannot expire a request while persistence
  still considers it open, and accepted input is still consumable.
- Guest time behind persistence cannot keep an expired request open; late
  acceptance fails. Handled `WAIT_TIMEOUT` leaves the original request closed.
- Repeated AI calls to the same wait tool register distinct requests with tracking
  disabled, validate and retain separate responses, and feed both into the model
  conversation.
- Existing loop, embedded/composed-site, `onWait`, suspend and budget fixtures use
  managed acceptance. Their artificial Unix-epoch clocks were moved into the
  future where they register real deadlines; virtual budget arithmetic is retained.

Fresh checks (pinned Rust, previously rebuilt shared components):

- `cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wait -- --test-threads=1`:
  26 passed. The first migration run exposed one epoch-based expired fixture;
  its clock was corrected before the passing run.
- Same target with
  `wasm_emitter_audit::audit_05_signal_wakes_respect_the_enclosing_budget -- --exact`:
  one passed. An earlier unqualified exact filter ran zero tests and is not evidence.
- Same target with `direct_wasm_execute_wait_timeout_routes_to_on_error -- --exact`:
  one passed after adding the authoritative closed-state assertion.
- Workflow Clippy with `direct-wasm-integration-tests` and all targets: passed.
  Workspace formatting and whitespace checks passed.

The optional `RUNTARA_DIRECT_RUNTIME_BINDING=composed` check failed during SDK
connection with `cannot block a synchronous task before returning`, before any
managed input request. The no-wait
`direct_wasm_execute_finish_passthrough_reports_completion` control failed at the
same boundary. This proves the failure is not specific to managed wait handling;
it does not prove that optional runtime binding works. The HTTP fixture source is
migrated, but compiled HTTP registration/replay remains unverified. The production
embedded runner requires lifecycle-invoke for generated workflows; determine this
optional binding's release applicability from current code/CI before changing
unrelated runtime machinery. Do not count these failed checks as passing coverage.

PostgreSQL recovery evidence improved as well:
`cargo test -p runtara-environment --features db-integration-tests --lib failed_input_abandonment_is_closed_by_production_exit_monitor -- --test-threads=1`
passed against an isolated database. The test injects a child abandonment-write
failure, verifies fenced IO cannot continue or publish success, restores storage,
finishes child disposal and invokes the actual physical-exit monitor with an
exited mock runner. The monitor, not a test status update, marks the root crashed,
closes its request and removes it from discovery.
Environment Clippy with `db-integration-tests` and all targets also passed.

At this checkpoint, storage was restored before the monitor ran. The subsequent
work below covers failure of the monitor's terminal write as well.

### Retained exit cleanup and final verification (2026-09-25)

The physical-exit monitor now retains an exact-handle exit intent before attempting
Core cleanup. Failed terminal writes leave that intent and registration available
for retry. Existing restart recovery consumes an observed crash/timeout as terminal
cleanup, rather than treating it as an unobserved Environment loss and resuming the
old wait. The new forward environment migration
`20260925000000_observed_runner_exit.sql` adds the intent column. It does not change
provider delivery or startup handoff.

Launch and registration locks fence cleanup against replacements; a new physical
handle clears the old intent even when it reuses a durable launch ID. Existing
terminal or explicitly suspended outcomes are preserved. A graceful drain retains
its existing recovery semantics. An outage before the first intent is persisted
still depends on the live monitor retrying; persistence cannot record an observation
while unavailable.

Fresh checks with disposable PostgreSQL and, where needed, Valkey:

- Three exit-intent regressions passed: automatic retry after failed request
  closure, restart recovery without reopening the wait, and replacement-handle
  protection during retry.
- Existing `container_registry_test`, `handlers_test`, `heartbeat_monitor_test`
  and `launch_queue_test` with `db-integration-tests`: 95 passed.
  The subsequently added
  `observed_failure_survives_owner_expiry_without_reopening_its_wait` passed too:
  a peer leaves a live owner alone, then applies its retained failure after the
  launch lease expires without scheduling the old wait for recovery.
- Environment library plus `scoped_runner_test`, `cooperative_stop_test`,
  `managed_inputs` and `wake_scheduler_test`, with
  `scoped-workflow-integration-tests`: 267 library and 36 integration tests passed.
  This includes the production failed-abandonment monitor, compiled tracking-off
  waits, nested child suspend/replay, child cancellation and wake races.
- Core and PostgreSQL suites with `runtara-core/test-support` and
  `runtara-store-postgres/db-integration-tests`: 199 tests passed. One pre-existing
  documentation example is ignored. Managed conformance covers receipt replay,
  tenant/owner isolation, deadlines, raw writes, acceptance/closure races and both
  park orderings.
- Server `managed_actions`, `report_input_retries` and `session_delivery`, with
  database and Valkey gates: 10 passed. This includes pagination, stale targets,
  changed report defaults, revoked access and original receipt replay.
  After removing the six unused event-inference tests, the server library passed
  all 1,182 tests. The extended `managed_actions` case also passed through actual
  MCP tools and REST handlers: matching discovery/schema, unavailable discovery
  remaining an error, receipt replay after completion and foreign-tenant rejection.
- Frontend with pinned Node 22.12.0: 1,476 tests passed; lint passed with 33 warnings;
  the production build, including regenerated WASM validation, passed.
- Runtime TypeScript regenerated using `generate-api-runtime-offline`. The chat
  API description now directs clients to authoritative actions and stable response
  identities rather than historical wait events.

The optional composed HTTP binding is a separate diagnostic limitation: test
selection in `direct_wasm_execute.rs` defaults to native host imports and lifecycle
invoke. CI runs that default; `launch_dispatcher.rs` derives the generated-image
lifecycle requirement, and `runner/embedded.rs` rejects generated images without
that entrypoint. The optional HTTP binding's pre-input trap therefore does not
expand this fix into redesigning the legacy runtime. Its compiled HTTP path remains
unverified, rather than counted as supported-path evidence.

Workspace Clippy with the complete `GATE_FEATURES` list from CI passed, as did
server Clippy after the final MCP test additions. The complete
`direct_wasm_execute` target with `direct-wasm-integration-tests` passed 396 tests;
three manual release measurement/benchmark tests remain ignored.

The component-host library passed 143 tests. Its first cooperative run passed 88 but
hit the one-second HTTP fixture deadline in
`real_agent::async_http_preserves_coercion_host_context_and_error_response` while
the compiled suite ran concurrently. After that suite finished, the exact test
passed in isolation in 0.75 seconds without code changes. The manual capacity soak
remains ignored. The complete cooperative suite subsequently passed all 89 tests
without code changes, and `isolated_capability` / `isolated_package` passed three
tests. The initial suite failure is retained in this record.

The broad library run exposed one additional unmigrated fixture:
`compile/agent_deadline_tests.rs::Host` rejected managed imports, causing eight
nested-wait/AI replay cases to fail while 715 workflow-library tests passed.
This host now uses the same real managed persistence fixture as the integration
battery. Its raw polling rejects compiled waits; nested replay submits through
managed acceptance and compares retained records. The timed child case waits for
persistence expiry while the guest clock stays before the deadline.

Final library verification:

- `cargo test -p runtara-dsl -p runtara-sdk -p runtara-workflow-stdlib -p runtara-workflow-runtime -p runtara-workflows --features runtara-workflows/direct-wasm-integration-tests --lib -- --test-threads=2`:
  DSL 259, SDK 74, runtime 9 and stdlib 243 passed; one stdlib microbenchmark was
  ignored. The workflow portion passed 715 and exposed the eight fixture failures
  described above.
- After migrating that host, workflow `--lib nested_suspend::` passed eight tests;
  `--lib ai_response_preserves_agent_and_signal_tool_decisions_across_resume` and
  `--lib nested_ai_wait_resume_preserves_both_decisions_and_original_signal` each
  passed one. All eight earlier failures are covered by these reruns. The other
  715 cases were not rerun after this test-only host change.
- Workflow Clippy with `direct-wasm-integration-tests` and all targets passed
  after the migration. Formatting and whitespace checks passed on the final diff.

No remote CI run or deployment is claimed. The optional legacy composed HTTP
binding remains unverified as explained above. Manual benchmarks/capacity soak
are excluded; startup/provider reliability remains on the follow-up track. These
limitations do not leave an actionable event-based consumer or an unresolved
failure in the supported managed-input path.

### Post-review fixes (2026-09-25)

A review of this change found the stale-target class of bug surviving in
response delivery, plus robustness gaps. Fixed:

- **Responses bind when retained, never at delivery.** Channel replies are bound
  on arrival to the single open request whose prompt was sent, and buffered per
  execution (apart from idle startup messages, so neither blocks the other).
  A reply whose request closes before handoff is reported undelivered and
  dropped, never re-aimed at a newer request; so are replies with no prompted
  open request, or several open. Buffered replies of an execution an actor
  leaves are settled on exit, and a retained response that became stale is
  failed and reported while idle instead of blocking the session.
  `POST /sessions/{id}/events` binds at submit time: to `requestId` when given,
  else to the only open request, else `409 INPUT_NOT_WAITING`/`INPUT_AMBIGUOUS`.
  Delivery never selects a target; an unbound message is blocked (`no_target`).
- **Persistence owns deadlines.** Hosts rebase the guest's host-clock deadline
  onto persistence time at registration (`persistence_deadline_ms`), and a
  timed signal park wakes at the stored deadline. Host/database skew can no
  longer pre-expire, shorten or lengthen a wait, or cause a re-park loop.
- **Replay registration compares identity only** (full signal id and logical
  owner); the first registration's metadata and deadline win. Migration 031
  stores the spec as exact text: JSONB renumbered floats (`1e16`) and rejected
  NUL escapes, which failed replay or registration on PostgreSQL only.
- **Discovery takes no row locks.** Reads use a repeatable-read, read-only
  snapshot. Workflow-wide discovery considers only live instances holding an
  open request, not every historical run.
- Accept's final update is guarded by `state='open'`. `FenceRejected` delivery
  failures block as stale instead of retrying forever. The delivery worker caps
  deliveries rather than visited sessions per tick, and a drained session's
  route expires after an idle week.
- The run history hides inputs once a run is finished; web chat drops every
  retained retry of a request that closed unanswered and treats a refused
  session message as final.

Waits suspended before this deployment are not backfilled: the feature is
unused, and deriving requests from historical events would reintroduce the bug.
Deploy with rebuilt workflow images as described in the release notes.

### Remaining event and signal uses

- `api/handlers/chat.rs`, frontend `queries/chat.ts`, `useChatStream.ts` and
  `ChatBubble/index.tsx` render historical request messages. They do not select an
  input or submit a response. The channel dispatcher ignores `WaitingForInput`.
- `ExecutionTimeline/index.tsx` derives actionable cards from managed discovery;
  event-derived step status remains presentation only.
- The unused `open_input_events` matcher and its legacy inference tests were
  removed. There are no production callers and no compatibility fallback.
- `runtime_client.rs`, `environment_client.rs`, SDK/runtime raw-signal imports and
  scoped wrappers retain arbitrary unmanaged signals. Managed addresses are
  protected by persistence; managed waits use separate registration/poll/close.
- `api/services/session_queue.rs` uses source peek/bind/conditional acknowledgement.
  Channel replies retain their execution/request/operation and are not destructively
  popped before managed acceptance. Only an unbound initial message may enter the
  existing explicit startup path.

### Supported ownership paths

| Path | Authority and invalidation | Evidence |
| --- | --- | --- |
| Generated root wait | Trusted root lease; required abandonment or terminal cleanup closes unanswered requests. | Compiled tracking-off/replay and mandatory-close failure tests in `managed_inputs`; production monitor and retained-exit tests. |
| Embedded/composed call sites | Compiler-qualified logical addresses under the enclosing owner; replay retains the original request. | Per-site, repeated-loop and pause/replay cases in `direct_wasm_execute`. |
| Published workflow child | Native runtime ownership retained through root park and physical lease replacement. | `a_published_child_wait_accepts_wakes_and_replays_without_debug_events` and the nested wake-scheduler test. |
| Independently cancellable child | Exact invocation fence; child cancellation/settlement invalidates its requests and descendants, preserving siblings. | `nested_child_input_survives_supervised_suspend_and_lease_replay`, `cancelling_a_supervised_child_closes_its_open_input`, and memory/PostgreSQL descendant conformance. |
| Repeated AI wait tool | Each logical call has a separate deterministic request address under its enclosing owner. | Compiled repeated-tool test with tracking disabled and separate accepted answers. |

Scoped children without durable invocation authority explicitly reject managed
registration/poll/close in `runtime_host/scoped.rs`. Runtime hosts without managed
input support return errors through the trait defaults; they cannot silently fall
back to raw signals. Historical composed HTTP fixtures are subject to the separate
verification limitation above.

## Focused implementation sequence

### 1. Separate the scope before adding behavior

Classify the existing diff by the required behavior above. Preserve all current
work. Keep lifecycle, authoritative consumers, stable receipt retry, and the
minimum response-retention dependencies. Extract deferred-only startup/provider/
collector changes into a separate reviewable patch or worktree before preparing
the fix for merge. Do not blindly revert intertwined session changes or edit
committed migrations. No commits, pushes, or history rewriting are implied.

**Exit:** the inventory above and the follow-up branch separate the source.
Verify the retained server/runtime/client contracts and focused regressions
before treating extraction as complete.

### 2. Close the channel correctness gap

Use `api/services/pending_inputs.rs` and the existing managed submission service
from `channels/session.rs`. Poll authoritative requests independently of debug
messages, including with tracking disabled. Historical output streaming remains
presentation only. Distinguish no request, one eligible request, multiple
requests, and discovery failure explicitly.

Bind a plain or collected response to the selected request and one stable
operation. Keep its payload unchanged after uncertainty and replay through
acceptance. Preserve the response until receipt or explicit rejection/failure;
reuse existing managed queue primitives where needed. A queue pop followed by a
failed send is not sufficient. Never substitute a newer request on retry.

Keep the current collector interaction model for this fix. Before presenting a
prompt and before submission, use authoritative state; cancel/failure does not
submit `{}`. Final submission still arbitrates races. Do not add restartable
field state, provider-wide intake deduplication, or new startup delivery modes.
Check the existing startup-message behavior only as needed to ensure it cannot
be misrouted as an unintended managed response.

**Exit:** channel regressions prove terminal/stale history cannot prompt or
receive input, tracking-off waits can be answered, ambiguous waits do not select
arbitrarily, and failed delivery preserves target/payload/operation. Test the
collector's cancellation, stale target, and uncertain final submission.

### 3. Finish lifecycle proof and compiled regressions

Map supported root, embedded, published-child, independently cancellable child,
and AI-tool wait paths to their registration owner and close/cancel behavior.
Fix only gaps that can leave an invalid request actionable or prevent a valid
wait from completing. Unsupported paths must reject managed waits explicitly.

Migrate affected test hosts to managed registration, polling and closure. Run
real compiled waits with tracking disabled, repeated identities, timeout with
`onError`, abandonment, sibling cancellation, and replay. Inject closure storage
failure, restore storage, and exercise the actual production recovery path to
prove eventual invalidation without test-only forced termination. Keep necessary
wake/lease race tests; do not redesign unrelated scheduler behavior.

**Exit:** the relevant memory/PostgreSQL invariants and supported compiled paths
pass. Missing or duplicate diagnostics have no effect on authoritative state;
exactly-once diagnostic storage is not required.

### 4. Verify consumers and generated contracts together

Run the API, report, MCP, chat, execution-form, and channel regressions against
the same request lifecycle. Test current authorization before receipt replay,
including after completion and changed report defaults; unknown discovery;
requests beyond page boundaries; stale forms; and lost acknowledgements after
an action disappears. Regenerate runtime TypeScript only for the retained public
contract. Preserve arbitrary custom signals and historical event rendering.

Audit every remaining event matcher, waiting-event consumer, raw signal write,
and destructive response pop. Classify retained uses as historical display or
unmanaged signaling with their file and reason. Any remaining actionable legacy
path is a blocker, not deferred reliability work.

### 5. Run the release checks for the retained change

Use the pinned Rust and frontend Node toolchains. Start with focused tests, then
run the CI feature matrix for all retained changes; don't count an integration
target that Cargo skipped because its feature was absent.

- Core/store: input and invocation conformance, including PostgreSQL races,
  rollback, deadline boundaries, immutable replay, and tenant isolation.
- Environment/compiler/host: rebuild components with
  `scripts/build-agent-components.sh`; run `managed_inputs` and affected scoped
  tests with `scoped-workflow-integration-tests`, and compiled workflow tests
  with `direct-wasm-integration-tests`. Run affected component-host integration
  tests with their manifest/CI features and component setup.
- Server: `managed_actions`, `report_input_retries`, and retained response-queue
  tests with their database/Valkey gates. Server migrations need an isolated
  database with pgvector; runtime persistence needs its own isolated database.
- Frontend: tests, lint, and production build; regenerate the runtime API through
  `generate-api-runtime-offline` when required.
- Finish with `cargo fmt --all -- --check`, relevant feature-gated Clippy from
  `.github/workflows/ci.yml`, generated diff review, and whitespace checks.

Record actual commands/results and service prerequisites. Previous green results
are useful history, not release approval for the current dirty worktree. New
failures introduced by retained code remain blockers even if their area is broad.

Apply additive migrations and deploy matching host/runtime/compiler/server/client
artifacts together, rebuilding affected workflow images. Verify the unused-feature
assumption before activation. No legacy event backfill or compatibility bridge is
planned. Once managed requests exist, rollback must preserve their correctness;
reverting to an old raw writer is not a safe automatic rollback.

## Acceptance checklist

- [x] Terminal roots and closed/expired/abandoned/cancelled waits expose no actions,
  regardless of unmatched historical events.
- [x] Valid suspended waits remain discoverable with the right schema and identity,
  including tracking-off, repeated, embedded, published-child, and AI-tool paths.
- [x] Acceptance, closure, and termination races have one valid winner; a second
  response or raw write cannot replace accepted data.
- [x] Authorized identical retries recover the original receipt after completion
  or acknowledgement loss, without another response or wake.
- [x] Required wake/replay works in either park ordering without reviving terminal
  roots or implicitly resuming explicit pause.
- [x] All existing actionable consumers, including channels, share these semantics;
  unknown discovery is visible and stale history cannot drive a submission.
- [x] Queued responses are not silently lost or rebound on failed delivery; invalid
  collection/cancellation does not manufacture a response.
- [x] Retained contracts and generated clients agree; deferred-only work is separated
  and the applicable tests/lint/build gates pass on the final change.

This checklist is the completion gate. The archived I1–I7/P1–P6 roadmap and the
follow-up reliability projects do not extend it. The behavioral checks and local
verification gate are complete with the evidence and limitations above. Review
and deployment are subsequent actions; they do not reopen deferred delivery work.
