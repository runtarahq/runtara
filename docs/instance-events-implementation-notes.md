# Historical instance-input implementation notes

Archived on 2026-09-25 when the user requested re-centering on the original
stale pending-input bug. This file preserves the previous checkpoint and expanded
plan verbatim below. **Its phase lists, completion claims, and release gates are
historical, not the active scope.** Previous test results are not verification of
the current worktree.

The authoritative scope and completion checklist are now in
[instance-events-fix.md](instance-events-fix.md). Deferred reliability work is in
[instance-events-follow-ups.md](instance-events-follow-ups.md). Do not resume the
old I1–I7 or P1–P6 roadmap merely because it appears in this archive.

---

# Pending instance inputs: correctness fixes

Status: prerequisite implementation brief and proposed implementation plan for
the Control agent and further workflow-backed report work. Implementation is
in progress; the phases below remain the completion checklist.

### Implementation checkpoint

Implemented so far: core managed-request types and validated acceptance,
in-memory/PostgreSQL persistence with forward migration 030, receipt replay,
root and descendant-invocation closure, raw-write protection, shared schema validation,
native runtime and internal SDK/HTTP operations, and standalone/AI wait compiler
registration and polling. Runtime-client forwarding methods are available for
the consumer migration. The new runtime/stdlib WASM components have been rebuilt.

Managed parking now persists the current signal set and rechecks accepted
responses atomically. Acceptance schedules a matching parked wake in the same
transaction as the receipt. Per-park scheduling state preserves timer/launch
claims, and bounded reconciliation repairs pending wake intents. Scheduler
retries compare their original lease; raw-signal wake notifications check
suspension under the lifecycle lock. Pause invalidates wake enqueue/start,
preparation-failure cleanup, and queue expiry without failing the paused root.
Explicit resume can authorize a queued wake that a preceding pause invalidated.

Nested admission now records immutable host-supplied parent paths. Cancelling or
settling an ancestor atomically invalidates descendant attempts and closes their
unanswered requests, preserving siblings and accepted receipts. Admission rejects
ancestry changes and inactive parents. Scoped factories can bind a trusted parent
fence; supervised suspension preserves the logical attempt, while supervised
child cancellation records a cancellation tombstone. A PostgreSQL terminal-status
trigger covers environment-owned SQL transitions as well as core lifecycle calls,
with request closure and wake cleanup rolled back if the root transaction fails.

Task supervision now distinguishes explicit child cancellation from resumable
owner teardown. Automatic Store destruction stops child execution immediately
and defers settlement until the owner supplies its disposition. Nested cleanup
inherits that disposition; local cancellation remains terminal in either race
order. Native root pause, breakpoint, and restartable shutdown preserve child
attempts and requests, while terminal teardown closes them. Shutdown retains its
existing recovery wake; accepting a response does not create a wake for an
explicit pause or breakpoint.

The production scoped runner now claims a bounded root invocation lease using
the unique physical runner handle and attaches it to the root host and child
factory. Claiming refuses a competing active owner; failed/uncertain claims and
setup/start-confirmation failures clean up only their exact proposed lease.
Normal execution releases ownership after supervised teardown. Every transition
out of `running` also revokes execution authority atomically in both stores,
including environment recovery SQL, while preserving resumable logical attempts.
Stale cleanup cannot revoke a replacement lease.

Verified at this checkpoint: 95 core tests, 35 relocated schema-validation
tests, all 30 PostgreSQL conformance tests, 53 wait-related compiler tests, two
native runtime input tests, 18 launch-queue tests, 14 wake-scheduler tests, five
workflow-launch-lease tests, and one real compiled-workflow
registration/park/acceptance/wake-claim/replay test with tracking disabled.
Two additional task-supervisor tests cover nested factory admission with parent
authority across suspension/lease replacement and local child cancellation.
The component-host library suite passed 141 tests, followed by an additional
Store-drop regression. All 35 scoped runtime tests passed against isolated
PostgreSQL, including native pause/shutdown/breakpoint replay and local cancellation
before/during resumable cleanup. The three managed-input integration tests were
rerun successfully after the supervision change. Component-host/environment
Clippy with the scoped integration feature gates and workspace formatting passed.
PostgreSQL checks include rollback of failed acceptance/park wake writes and
bounded recovery using a fresh backend. Database integration checks used isolated
test containers; the wake-scheduler test harness now supports that fallback.
The server, SDK (including embedded backend), workflow runtime, compiler and
environment passed compilation checks. Core/store/environment Clippy and Rust
formatting passed; the complete feature-gated CI matrix is still required.
After production lease wiring, the 95 core tests, all 30 PostgreSQL conformance
tests, all 35 scoped runtime tests, and eight compiled scoped-runner tests passed.
New checks cover start-confirmation rejection, fresh replay epochs, physical
owner identity, competing claims, and stale cleanup. A compiled published-child
test now verifies managed registration/acceptance, actual scheduler claim,
response replay, and receipt retry after completion with debug tracking disabled.
The isolated orphan-recovery test also verifies that revocation preserves an
open child request and permits its response during the gap before relaunch.

Closed-state compiler handling now exits both timed and untimed waits with a
structured error preserving the closure reason. Expiry remains `WAIT_TIMEOUT`;
abandonment and invocation cancellation have distinct codes. Post-registration
runtime failures attempt abandonment before unwinding. Seven managed-input
integration tests passed, including real compiled recovery into a second wait,
untimed closure without recovery, zero-deadline expiry, and an injected polling
failure with root terminal cleanup disabled. The shared closure formatter test
passed and the workflow runtime/stdlib components were rebuilt. Closure failure
backstops across all child paths and compiled AI coverage remain required.

Batched actionable-input existence is implemented in memory and PostgreSQL,
sharing eligibility with paged discovery. PostgreSQL authorizes and locks the
requested roots in one ordered query. Execution DTO enrichment and all three
execution-engine callers now use the tenant-scoped batch and propagate errors.
Memory batch conformance, the PostgreSQL managed-input conformance suite, server
compilation, and two server regressions passed. The server tests cover suspended
waits, acceptance clearing a flag, foreign-tenant rejection, and a closed fixture
database pool failing without partially mutating flags. Other discovery and
submission consumers still require migration.

After these changes, all 12 memory managed-input conformance tests and 53
wait-related compiler tests passed. Clippy passed for core, PostgreSQL storage,
workflow compiler/stdlib, environment, and server across all targets, including
the PostgreSQL and scoped-workflow integration feature gates. Workspace
formatting and diff whitespace checks passed. This is focused verification;
the remaining consumer/queue tests and full Phase 7 matrix are still outstanding.

Workflow action discovery now reads managed requests, maps immutable metadata,
and pages requests after resolving the complete tenant/workflow instance set.
Per-instance action routes authorize the retained instance without requiring an
open-input flag. Direct action submission requires `requestId`, `operationId`,
and `payload`, uses atomic acceptance, and returns a payload-free receipt with
stable error codes. Ownership checks now explicitly verify the instance tenant
as well as its workflow image prefix. Matching receipt retries work after
completion. Report action discovery pages beyond its former first 100 results;
report submission checks retained metadata against the block's scope and filter
instead of requiring membership in the current open-action list. The MCP
action-response tool carries the same request/operation identities.

Report action forms now preserve operation IDs across uncertain retries of an
equivalent JSON payload, allocate a new operation for an edited response, and
refresh/clear stale conflicts. Direct and report action routes, receipt/error
schemas, and required request fields are registered in OpenAPI; runtime
TypeScript was regenerated with the offline script. These changes do not yet
migrate the separate pending-input/signal APIs, chat forms, or queue consumers.

Verified for this migration: the new PostgreSQL `managed_actions` integration
test covers 210 requests across 105 executions, literal workflow-ID filtering,
request page counts, foreign-tenant denial, invalid schema, competing replies,
receipt replay after completion, operation conflicts, and sanitized service
unavailability. All 232 frontend report/submission tests and the frontend build
passed using Node 22.12.0. Frontend lint passed with existing repository warnings;
the changed frontend files passed focused lint without warnings. Server,
environment, and report-DSL Clippy passed across all targets with the server
database integration gate. Report-specific retained-scope/implicit-payload retry
regressions remain necessary; the direct-action regression does not prove those
paths or the still-unmigrated consumers.

Pending-input and public signal handlers now use the same managed discovery and
acceptance contracts. Session metadata is resolved under the caller's tenant,
and backend failures are sanitized. Signal MCP tools require request/operation
IDs. Runtime TypeScript was regenerated with pending-input/session/signal paths;
execution and chat forms now submit the managed contract. Unused bounded event
fetching and `has_open_inputs` helpers have been removed; the pure historical
identity matcher remains.

Chat restores pending inputs from authoritative discovery instead of historical
waiting events. Polling operates independently of the SSE stream, rejects stale
responses after session/execution changes, and exposes discovery failures.
Multiple requests require explicit selection. Uncertain chat acknowledgements
retain the original form and operation for retry, even after discovery excludes
an accepted request. Accepted/stale targets clear their form without carrying
its text into the next request. Execution timeline forms now remain visible when
there are no debug events or the event timeline is still loading.

Verified for these changes: the PostgreSQL/Valkey managed-action regression
covers session/instance agreement without events, tenant denial, invalid signal
payload/operation identity, malformed JSON, and signal receipt replay after
completion. The MCP parameter contract and six historical matcher tests also
passed. The
workflow/report frontend suite passed 881 tests, followed by 21 focused chat,
HTTP contract, and timeline tests including six new form/timeline regressions.
A final 24-test chat discovery/store run passed after session-reset handling.
The frontend production build passed with Node 22.12.0; lint passed with 33
existing warnings, none in the changed files. Server Clippy passed across all
targets with database and Valkey integration features. Remaining work includes
queue/channel delivery, report effective-payload retry context, retaining
uncertain operation state when other action forms disappear during refresh, and
the runtime/full-matrix gates. Client retry state is currently held in memory;
no browser-reload retry guarantee is claimed.

The managed queue foundation now lives in
`api/services/session_queue/managed.rs` and its embedded Lua script. Enqueue
atomically deduplicates message/operation identities and canonical payloads.
Claims retain the FIFO head under a backend-timed lease; bind, renew, retry,
block, acknowledge, and explicit failure check the current token/state. Targets
cannot change after binding. Unresolved envelopes have no TTL; accepted/failed
envelopes and deduplication identities are retained for at least seven days,
then removed by bounded cleanup. User JSON stays encoded through Lua so numeric
values are not rounded by its JSON codec. Corrupt state produces a typed error.

A shared delivery routine discovers zero/one/many managed requests, persists the
target before acceptance, and acknowledges only a validated receipt. Bound
redelivery skips discovery and replays against its original instance/request.
Transient errors back off; ambiguity, invalid payloads, and stale targets block
the FIFO head for explicit resolution. Persistent owner records support cursor
scanning after restart without an attached SSE stream. Session metadata updates
are now atomic and remove the old one-hour TTL.

All 12 managed-queue tests passed against isolated Valkey, including competing
workers, expired-token rejection, immutable binding, schema/ambiguity handling,
corrupt envelopes, corruption-tolerant restart scanning, metadata retention,
large JSON numbers, and bounded completed-message cleanup. These are protocol
and shared-delivery checks; they do not prove production adapter integration.
The final PostgreSQL/Valkey `managed_actions` regression passed, as did server
Clippy across all targets with database/Valkey features, workspace formatting,
and diff whitespace checks. The disposable PostgreSQL harness now waits for its
final TCP listener rather than the temporary initialization socket.

Session enqueue, persisted launch intents, and `session_delivery_worker` are
now wired into the server; the SSE loop observes execution events and routes.
The browser sends stable message/operation IDs and validates enqueue identity.
An uncertain enqueue retries before considering a newly discovered input, and
has a separate retry control so a changing form cannot erase that message.
Enqueue success permits further queued messages without waiting for SSE.

Session delivery list/detail/resolution routes now expose payload-free status.
They authorize tenant/session ownership, and selection additionally authorizes
the workflow instance and retained input request. Atomic resolution checks the
blocked state; bound targets remain immutable. Chat scans statuses independently
of SSE, deduplicates pages, displays unavailable/blocked/failed outcomes, and
offers explicit selection, original-target retry, or failure. Public scan cursors
are strings to preserve all 64 bits through JavaScript; a page is not an exact
total or a complete history. OpenAPI and runtime TypeScript include these routes.

Authoritative chat discovery can advance the displayed session execution without
an SSE event. Request selections include their instance identity so identical
compiled wait IDs in consecutive executions cannot collide. Uncertain managed
responses retain their original payload and operation separately from current
forms, with an explicit original-response retry after edits or route changes.
This retry context remains in memory; browser-reload receipt retry is not claimed.

Verified for this session integration: all 13 isolated Valkey queue tests,
including a new launch-intent/reclaim/route-publication test, and the extended
PostgreSQL/Valkey `managed_actions` test passed. The latter exercises enqueue
replay/conflict, tenant denial, payload-free status, competing resolvers,
resolution while leased, and explicit failure. It also starts the production
delivery worker with a fresh connection and no SSE subscriber to recover a
committed receipt after root completion and a lost queue acknowledgement.
This checkpoint proves bound receipt recovery; replacement-launch coverage is
recorded separately below.

The workflow/report frontend suite passed 895 tests before the final retry
snapshot addition; the final focused run passed all 44 chat/store/HTTP tests.
Coverage includes cursor preservation, status pagination/resolution, old-session
responses, and original response retries after edits and execution changes.
The final frontend production build passed on Node 22.12.0. Full lint passed
with repository warnings; the changed chat files passed focused lint with zero
warnings after fixing the new cleanup warning. Server Clippy passed across all
targets with database/Valkey features, and formatting/whitespace checks passed.
The complete runtime/component/release matrix remains outstanding.

Replacement delivery now observes durable source state rather than treating any
retained outbox identity as a progressing execution. An expired, cancelled, or
terminal source blocks its message with `launch_rejected`, including after route
publication. A subsequent message on that rejected, unregistered route also gets
a visible blocked outcome. Initial session launches still awaiting registration
defer without creating another execution. Missing runtime instances now retain
their typed error through `RuntimeClient`, separating that outcome from storage
failure. Bound receipt retries continue to bypass mutable routing/source state.

The new `tests/session_delivery.rs` target is gated by both database and Valkey
integration features. All six tests passed against separate isolated server and
runtime PostgreSQL databases plus Valkey. They cover fresh-engine recovery before
enqueue, after source commit, and after route publication; two engine enqueues
of the same launch intent; replay after workflow deletion; pinned version/input
snapshots; failed source lookup; source expiry; conflicting source identity;
initial registration delay; and a replacement root terminating before a wait.
The tests proceed to managed acceptance and verify that another wait does not
receive a duplicate. Runtime instance/wait registration is supplied by the test
fixture; this does not claim to execute the compiled component or the full
trigger-worker/Environment handoff in that test.

All 15 managed-queue tests and the existing `managed_actions` integration test
also passed after this change. New queue checks reject corrupt launch metadata
without removing the envelope and require the original route version, inputs,
and routing policy at publication. The worker now shares one five-second budget
across discovery and its delivery batch, retaining unprocessed scopes for later
ticks. Server Clippy with both integration feature gates passed. Load/fairness
testing and idle routing-metadata retention remain separate outstanding gates.

Channel intake still uses the old queue and separate provider reservations.
Finish compiled launch/handoff coverage, atomic provider enqueue, channel delivery
and owner status, worker load/retention verification, and the remaining runtime and
report gates before treating production delivery as complete. Remove the old
`push_event`/`pop_event` paths with their remaining channel callers; that queue
still has its one-hour message TTL until then.

Mandatory abandonment failure now aborts the current WASM execution. The native
`close-input` import traps when the runtime cannot confirm closure; compiled
waits also trap on a returned close error from a composed SDK runtime. This
prevents `onError` or an enclosing embedded call from continuing after uncertain
closure. Successful closure still preserves the original workflow error, and
accepted/closed request states remain ordinary successful close outcomes.

The compiled native regression covers timed and untimed waits with a recovery
wait, proving that a failed close cannot open that recovery request. An emitted
core-WASM test covers success/error responses across CLI, lifecycle-invoke, and
workflow-agent exports without relying on the native import's trap. A PostgreSQL
fault test proves closure rollback, sticky child IO failure, rejected success,
lease revocation, and closure by a subsequent root terminal transition. The
fixture invokes that recovery transition explicitly; automatic production
recovery after storage restoration remains an integration gate. Child admission
tests now require explicit cancellation to retain its cancellation record even
when admission's acknowledgement was lost, and reject reopening that child.
Unfenced child-host tests also reject managed registration/poll/closure without
creating a root-owned request or implicitly acquiring an invocation lease.

Verification for this abandonment change: the component build script rebuilt
all 26 agent components and both shared workflow components; 142 component-host
library tests, 602 workflow/compiler library tests, 79 environment runtime-host
tests against isolated PostgreSQL, and all eight compiled managed-input tests
passed. The latter suite includes existing response replay, cancellation,
resumable nested ownership, expiry, and successful abandonment regressions.
These are focused gates, not the complete Phase 7 integration matrix.
Component-host/workflow/environment Clippy passed across all targets with the
scoped-workflow integration feature enabled; workspace formatting and diff
whitespace checks passed. No frontend code changed in this checkpoint.

Report acceptance now retains a trusted canonical caller context alongside the
effective accepted payload. Memory and PostgreSQL compare that context during
early receipt lookup and locked acceptance; direct submissions cannot replay a
report operation using only the effective payload. The report service checks
current report/block/target scope before replay, then evaluates implicit fields
only for a new operation. Renaming a report or changing implicit defaults does
not change an already accepted response. Internal acceptance context is excluded
from public receipts.

The new `report_input_retries` integration target verifies changed defaults and
slug after completion, principal/report/block/filter/payload conflicts, direct
submission isolation, invalid new responses, revoked instance/workflow/filter
scope, deleted reports, and authorization database failure. Its HTTP case uses
the real authentication/authorization middleware, report handler, PostgreSQL
API-key validation, and Valkey membership/revocation reads. Removed membership,
revoked token identities, and revoked API-key rows prevent receipt replay.
OIDC signature verification is stubbed in this fixture; the test establishes
current authorization, not external identity-provider validation. Viewer access
remains valid under the existing report-consumption permission policy.

Verification for report retries: all five report integration tests, the existing
managed-actions integration test, 13 memory input conformance tests, and all
three PostgreSQL `managed_input` tests passed against isolated PostgreSQL and
Valkey where required. The PostgreSQL cases include contextual replay races and
rollback of acceptance context with receipt/wake state. Core/store/server Clippy
passed across all targets with their test-support, database, and server Valkey
feature gates. This completes I1's focused report verification. Public DTOs did
not change for report context, so
runtime TypeScript regeneration was not required for this change.

Execution and report forms now share a mounted page submission owner. Each
intent retains its original instance, request, operation, payload, and report
ID/block/filter context independently of actionable discovery. Retry controls
survive an empty action refresh and action-block or timeline-tab unmounts.
Changed responses or report filters create separate operations while uncertain
originals remain available. Confirmed acceptance hides only the matching target,
including its instance identity; identical request hashes in different instances
remain independent. Invalid or missing receipt bodies remain uncertain.

The page owner resets on tenant or principal changes and ignores late completion
callbacks from the previous owner. Report view/edit modes use the resolved report
ID, and retries show the original submitted payload and filters. Query refresh
failure cannot turn confirmed acceptance into an uncertain submission. Browser
reload and navigation that unmounts the entire page still end this in-memory
guarantee; no response payload is written to browser storage.

Verification for I2: the full frontend suite passed 1,476 tests across 140 files.
After the final context-module split and presentation changes, all 24 focused
store, timeline, report action, and report page tests passed again. Coverage
includes lost acknowledgement followed by action removal, block/tab remount,
changed filters, instance/report switching, concurrent operations and delayed
responses, duplicate request hashes, malformed receipts, and tenant/principal
changes. The production build (including browser validation WASM regeneration)
passed with the pinned Node version. Frontend lint passed with zero errors and
33 existing warnings; diff whitespace checks passed. I1 and I2 have focused
completion evidence, while I3–I7 and the coordinated Phase 7 matrix remain open.

This is not deployable as a completed fix yet. Remaining work includes completing
the child-admission matrix, production closure-failure recovery coverage,
diagnostic replay deduplication,
migration of remaining
discovery/submission consumers and generated contracts, production queue integration,
child-owned wake and broader fault/race coverage, and the full
verification/rollout gates below. Existing callers still use event
reconstruction and raw submission, so the consumer migration must accompany the
new compiled wait contract. Existing integration test hosts also need migration
to the managed input contract. The original scope and acceptance criteria remain
unchanged.

In particular, production scoped admission currently fences compiler-proven
durable Agent calls. The parent-aware nested factory path is verified through
factory/supervisor tests; complete the inventory of ordinary embedded, published,
nested, and non-durable/unknown child paths before treating all ownership work as
finished. The published-child integration test uses the composed runtime path,
not an isolated child with an independently cancellable invocation fence.

## Problem

An `external_input_requested` event records that an execution requested input.
It does not, by itself, establish that the request is still actionable.

Cancellation, failure, or timeout can terminate an instance without emitting the
matching step-completion event. Event matching alone then leaves a stale request
apparently open. A response can also have been accepted before the workflow
resumes and emits completion, leaving a window for duplicate submissions.
An individual wait may also time out or be abandoned while its parent instance
continues through recovery or another branch.

Instance lifecycle, request lifecycle, and accepted-response state must take
precedence over an unmatched historical request event. Preserve the history;
correctness must not depend on deleting request events or fabricating successful
step completions.

## Current implementation and gaps

- [pending_inputs.rs](../crates/runtara-server/src/api/services/pending_inputs.rs)
  now exposes authoritative discovery for action consumers. Its remaining event
  helper matches request events against `step_debug_end` events. Standalone waits use
  signal identity; AI tool requests use the synthetic tool invocation identity.
  This helper does not establish instance liveness.
- The matcher assumes completion telemetry that is not always emitted.
  [debug.rs](../crates/runtara-workflows/src/direct_wasm/compile/debug.rs)
  suppresses step debug events when `track_events=false` and for published
  workflow agents. The current AI wait-tool emitter calls the shared
  [wait event builder](../crates/runtara-workflow-stdlib/src/direct_json.rs),
  which does not include the AI identity fields expected by the matcher's
  synthetic-step branch. Audit actual compiled producers as well as consumers;
  tests using hand-built events alone do not establish this contract.
- [wait.rs](../crates/runtara-workflows/src/direct_wasm/compile/wait.rs)
  can route `WAIT_TIMEOUT` through `onError` while the instance continues.
  Instance liveness alone cannot establish that the original wait is still open.
- [workflow_runtime.rs](../crates/runtara-server/src/api/services/workflow_runtime.rs)
  now builds action DTOs from managed requests and submits through atomic
  acceptance. Tenant-aware discovery handles terminal eligibility at storage;
  submission authorizes ownership independently of current openness before
  receipt replay. Pending-input and public signal handlers share this managed
  contract; remaining low-level raw-signal paths still require the final audit.
- The per-instance actions endpoint in
  [step_events.rs](../crates/runtara-server/src/api/handlers/step_events.rs) and
  report provider paths in
  [workflow_runtime.rs](../crates/runtara-server/src/api/services/reports/providers/workflow_runtime.rs)
  now delegate actionable state to managed discovery. Keep route authorization
  separate from current openness so matching receipts remain retryable.
- `enrich_pending_input` in
  [runtara_dto.rs](../crates/runtara-server/src/workers/runtara_dto.rs)
  now uses authoritative batched discovery, including suspended waits, and
  propagates failures through its three execution-engine callers. Remaining
  channel consumers must migrate to agree with these flags.
- The chat session pending-input endpoint in
  [sessions.rs](../crates/runtara-server/src/api/handlers/sessions.rs) and the
  per-instance pending-input/signal handlers now use managed-service discovery
  and acceptance. Their frontend and MCP callers submit request and operation
  IDs. Session queued delivery is partially wired; channel delivery still needs
  migration to the shared queue protocol.
- The [channel session module](../crates/runtara-server/src/channels/session.rs) still has a
  `find_pending_signal_id` helper that selects the latest request event without
  checking whether that particular request remains open.
- The former bounded actionable event-fetch helper and session latest-event
  selector have been removed. The channel selector still needs migration; incomplete
  diagnostic history cannot establish an authoritative target.
- Raw custom-signal persistence in
  [backend.rs](../crates/runtara-store-postgres/src/backend.rs)
  now rejects registered managed addresses under the lifecycle lock. Preserve
  that boundary when migrating callers: arbitrary unmanaged signals retain
  their existing behavior, while managed waits consume immutable accepted data.

## Required behavior

### One shared definition of a pending input

An actionable input requires an existing, authorized, non-terminal instance and
a valid request for a specific invocation that has not been resolved or already
answered. A label or step ID alone is not sufficient identity across loops,
embedded call sites, or repeated AI tool calls.

- Terminal instances return no actionable pending inputs, regardless of retained
  request events. Missing instances remain a not-found result.
- A suspended instance is not necessarily awaiting input: it may be sleeping,
  explicitly paused, or stopped at a breakpoint. Do not infer requests from
  status alone or require all active waits to have the same status.
- Define whether an explicitly paused instance may accept a response for a
  previously open wait. Signal delivery must not bypass an explicit pause.
- Return the wait's response schema, not the workflow's startup input schema.
- Discovery errors or incomplete history must not be presented as a confirmed
  open request or a confirmed empty result. Surface an error/unknown state.

Keep the low-level event matcher reusable, but expose a shared lifecycle-aware
discovery service to callers. Apply the same semantics to action APIs, reports,
chat sessions, channels, pending-input flags, and the proposed Control agent.

### Authoritative request lifecycle

Register and resolve requests through an authoritative lifecycle independent of
optional debug telemetry. Request identity, invocation ownership, response schema,
resolution, and acceptance must survive restart and replay for durable workflows.
Debug events may describe this state, but absence of a `step_debug_end` cannot
establish that a request is still open. Paging cannot recover events that were
never emitted.

Registration must be idempotent for the same logical wait across replay. Distinct
loop iterations, embedded call sites, and AI tool invocations must not share a
request identity. Re-emitting a request must not reopen an answered or closed
request. Verify the identities emitted by actual compiled workflows, including
published workflow agents, rather than relying only on synthetic event fixtures.

Close or invalidate the owning request when its wait times out, is abandoned, or
its invocation is cancelled, even if the root instance remains active. Acceptance
must arbitrate atomically against these request transitions as well as instance
termination. Define how deadlines and invocation invalidation are represented so
a delayed completion event cannot leave an expired request actionable.

This requires durable control state, but does not prescribe a new table. A
dedicated record, suitable existing state, or mandatory lifecycle events with an
authoritative projection may satisfy it. Optional or independently retained debug
events alone cannot.

### Atomic response acceptance

Checking the instance and then independently writing a signal is insufficient.
The authoritative acceptance operation must coordinate with instance termination,
request closure, and competing submissions at the persistence boundary.

- Authorize the caller for the target tenant/request, then look up an existing
  receipt for the operation before applying current liveness, openness, or schema
  checks to a new submission. Receipt lookup must not disclose another tenant's
  result. Validate request identity and payload schema for new acceptance.
- If instance termination or request closure wins before a new acceptance,
  reject the response as no longer active, without storing it as an accepted
  response or scheduling a wake.
- If response acceptance wins first, it may return success even if the instance
  terminates afterward. Success means accepted, not guaranteed continuation or
  completion of downstream work.
- Record acceptance durably so a second response cannot overwrite the first
  while the workflow has not yet emitted its completion event.
- An idempotent retry returns the original acceptance outcome. A conflicting
  response receives an explicit conflict. An accepted response whose
  acknowledgement was lost still replays successfully after completion or
  cancellation; it must not be rejected by the current-state check or trigger
  another wake. Define the
  operation identity, payload-equivalence rules, receipt retention, and conflict
  behavior before implementation. Queue redelivery must reuse a stable operation
  identity rather than allocate a new one on each attempt.
- Wake only according to lifecycle rules; never revive a terminal instance or
  implicitly resume an explicitly paused one. Handle crash recovery between
  durable acceptance and wake scheduling.

Reuse existing persistence primitives where sufficient; otherwise add a narrow
conditional acceptance operation or durable request/receipt record. Do not
require a particular storage design before auditing the current signal contract.

Validated pending-input submission and arbitrary custom-signal delivery are
different contracts. Audit raw signal paths, but do not globally impose
human-input schema/open-request checks on every custom signal. Pending-input
responders must consistently use the validated acceptance path.

Define the boundary at managed request addresses: raw delivery must not overwrite
an accepted response or change what the wait or its replay consumes. Either guard
those addresses against conflicting raw writes or make the accepted response an
immutable source used by the consumer. Preserve arbitrary-signal behavior for
unmanaged addresses, and define how a preexisting raw value at a managed address
is handled before a new response is accepted. Cover concurrent raw and validated
writes, not only two calls to the validated endpoint.

### Complete discovery and safe client behavior

- Avoid silently truncated event reconstruction. When reconstructing from events,
  use complete mandatory lifecycle history or its authoritative projection,
  including request closure and response acceptance. Page through all relevant
  history used by the low-level matcher. Account for retention; missing history
  is not proof that a request is open or absent, and paging debug telemetry alone
  does not satisfy the request-lifecycle contract.
- Include `enrich_pending_input` in the shared-service migration. Suspended waits
  must remain discoverable. Define an explicit unknown/error representation for
  pending-input flags, or fail the enclosing request; do not serialize discovery
  failure as `has_pending_input=false`.
- Workflow-wide discovery must not scan only the first execution page and imply
  that its results cover every pending request. Define pagination/count semantics
  for actions separately from pagination of instances.
- Replace latest-event shortcuts in chat/channel delivery with selection of a
  verified open request. If several requests are eligible, require an explicit
  target or a documented routing rule rather than silently choosing the newest.
- Pending-input lists are snapshots. Revalidate on submission; an already-rendered
  form can become stale. Clients should handle the conflict by refreshing or
  clearing the form, without starting another execution or retrying indefinitely.
- Preserve queued chat messages until delivery is durably accepted or apply the
  existing explicit recovery policy. A stale request must not silently consume
  a user's message.

## Verification and acceptance criteria

- Completed, failed, cancelled, and timed-out instances expose no pending inputs,
  even with an unmatched request event and no step-completion event.
- Active waits remain discoverable with the correct response schema. Sleeping
  or paused status alone does not create a request.
- Repeated waits, embedded invocation sites, and repeated AI tool calls resolve
  only their own requests; preserve existing identity-matching tests.
- Run compiled-workflow tests with `track_events=false`, published child agents,
  and repeated AI wait-tool calls. Verify actual request/resolution identities,
  and that replayed registration never duplicates or reopens a request.
- Deterministic race tests cover termination-before-acceptance and
  acceptance-before-termination, with no terminal-instance wake.
- Cover wait timeout followed by `onError` recovery, abandoned waits, and child
  invocation cancellation while the root continues. Race late responses against
  each request closure and verify that only one outcome wins.
- Concurrent replies cannot overwrite one another. Retries after an uncertain
  acknowledgement recover the original result, including across restarts and
  after instance completion/cancellation, without another delivery or wake.
- Raw-signal writes cannot replace an accepted response before consumption or
  replay. Test both sequential and concurrent raw/validated submissions, while
  preserving the arbitrary-signal contract at unmanaged addresses.
- An accepted response disappears from actionable discovery before step-end
  events arrive, or is explicitly exposed as answered rather than open.
- Requests beyond current event/instance page limits are handled correctly.
  Fetch failures and incomplete retained history do not create false open inputs.
- Reports, instance APIs, chat sessions, channels, and future Control calls agree
  on request state. Stale UI submissions return a defined conflict.
- Pending-input flags expose suspended waits and propagate discovery failures as
  error/unknown rather than a false empty result.
- Tests cover queued-message delivery failure and explicit-pause behavior.

Start with shared service and persistence conformance tests, then isolated
PostgreSQL integration tests and relevant API/chat/report tests. Add forward
migrations if needed and regenerate contracts through existing tooling. Report
which checks were actually run.

## Detailed implementation plan

### Design decisions

Use a dedicated durable input-request record in core persistence. The current
custom-signal mailbox has neither request metadata nor conditional acceptance;
optional events cannot supply missing lifecycle facts. Keep the existing event
matcher for historical display only. All actionable discovery will read the new
records through one service.

The plan describes the final contract. Some interfaces already exist in the
worktree, as recorded in the checkpoint above; new interfaces below remain
proposals until implemented. Complete storage and runtime support before
switching callers. This is a coordinated change across the server, runtime,
compiler, and generated clients. Since this feature has not been used, backward
compatibility is not required: do not add dual reads, legacy submission adapters,
or a compatibility path that treats old unmatched debug events as actionable
requests. Preserve unrelated arbitrary custom-signal behavior.

| Decision | Planned behavior |
| --- | --- |
| Request identity | Tenant + instance + the full deterministic wait signal identity; labels and `action_key` are display/routing metadata only. |
| Ownership | Store the logical invocation identity and its current execution fence separately. Restart/lease changes do not create a new logical request. |
| State | `open`, `accepted`, or `closed`, with a closure reason. Acceptance is final even if the instance subsequently fails. Consumption is separate optional metadata. |
| Deadline | Persist one absolute deadline at first registration; replay cannot extend it. Acceptance and discovery use persistence-side time. |
| Explicit pause | An already registered, unexpired wait can accept a response while paused, but acceptance cannot resume the instance. Explicit resume remains necessary. |
| Idempotency | Require a caller-generated `operation_id`, scoped to tenant + instance. Bind it to one request and one canonical JSON payload. |
| Payload equivalence | Recursively sort object keys; preserve array order, scalar types, and the parsed JSON number representation. Ignore transport whitespace. Compare canonical bytes, not only a hash. |
| Receipt lifetime | Retain the accepted payload and receipt for the retained instance's lifetime, independently of event retention. Deleting the instance ends the replay guarantee. |
| Raw delivery | Reject raw writes at registered managed addresses, including before acceptance. Unmanaged addresses retain their existing custom-signal contract. |
| Discovery failure | Return a service error. Keep `has_pending_input` boolean by failing the enclosing response when its value cannot be established. |
| Ambiguous chat routing | Auto-select only when exactly one eligible request exists. Otherwise retain the message and require a target. |

### Execution order and remaining work

Use the existing implementation as the starting point. The checkpoint records
previous verification; it does not mean an entire phase is complete. Finish the
following work packages in order, adding the indicated regression tests with
each package. All packages are required before release.

| Package | Existing foundation | Work still required | Completion evidence |
| --- | --- | --- | --- |
| A. Ownership and closure | Core types, both stores, receipts, trusted parent links, descendant invalidation, lifecycle SQL trigger, resumable teardown, production scoped leases | Complete child-admission matrix, abandonment, broader compiled child coverage | Shared conformance tests plus compiled child cancellation with a live sibling |
| B. Park and wake | Persisted park identities, atomic scheduling, bounded recovery, conditional claim retries, pause arbitration through launch cleanup | Child-owned wake integration and the remaining fault/race matrix | Both acceptance/park orderings and crash/pause races on memory and PostgreSQL |
| C. Compiled lifecycle | WIT/native/SDK registration and polling; closure errors and abandonment traps; compiled recovery tests | Production closure-failure recovery, test-host migration, diagnostic deduplication, published/AI replay coverage | Real compiled workflows covering every lifecycle case in Phase 7 |
| D. Discovery and submission | Batched execution flags; managed action/report/pending-input/signal APIs; signal MCP tools; request pagination; contextual report replay with database/HTTP authorization tests | Channel consumers and remaining cross-surface regression coverage | Cross-surface agreement, exact counts, failure propagation, receipt retries |
| E. Queue delivery | Atomic managed envelopes, FIFO leases/binding/ack/reclaim, restart discovery; session intake, launch/source recovery tests, worker receipt recovery, status/resolution API | Compiled handoff coverage, channel migration, provider deduplication, channel owner status, worker load/retention verification | Two-worker and crash tests plus production-adapter integration against isolated Valkey/PostgreSQL |
| F. Contracts and delivery | Internal SDK operations; managed API/MCP/frontend contracts; session status/resolution UI; authoritative chat polling; mounted execution/report owners retaining uncertain submissions through refresh/remount | Final cross-surface audit and coordinated lint/test matrix | Generated diff reviewed and combined acceptance suite passes |

Package C depends on A and B for lifecycle guarantees. D can use the existing
store interfaces while those packages are developed, but must not be enabled
until A–C pass. E depends on D's submission contract. Final frontend integration
and release verification depend on D and E. These are review boundaries, not
separately deployable feature increments.

### Implementation work items and completion evidence

Use this ledger to execute the remaining work. The detailed behavior in the
sections below remains authoritative; these items identify the next edits and
the evidence needed to close them. An existing implementation or an older test
result does not close an item. I1 and I2's focused checks have passed as recorded
in the checkpoint; I3–I7 remain open. All checks must still pass against the final
coordinated changes. Paths are relative to the repository
root; proposed new test files are explicitly identified.

#### I1. Finish report receipt replay verification (D)

The worktree contains `InputAcceptanceContext`, contextual receipt matching in
both stores, and replay before implicit payload evaluation in
`crates/runtara-server/src/api/services/reports.rs`. The following checks now
have focused coverage in `report_input_retries` and shared conformance tests;
retain them as regression gates for subsequent work.

- Add `crates/runtara-server/tests/report_input_retries.rs` (new), registered in
  the server manifest with `required-features = ["db-integration-tests"]`.
  Use isolated runtime and server databases, applying each owning crate's
  migrations; the server database must support its pgvector migrations.
- Accept a report response, change implicit defaults and the report slug,
  complete the instance, and repeat the original operation through the report
  service. Assert the identical receipt and unchanged effective payload.
- Cover changed principal, report/block, filters, caller payload, request, and
  tenant. A direct submission must not replay a contextual report operation.
  Assert public receipts contain no internal context or accepted payload.
- Revoke report/block target access and remove the report before retrying.
  Exercise role/API-key permission revocation through the HTTP authorization
  layer as well; service-only tests cannot establish middleware enforcement.
- Run `contextual_receipt_replay` through memory and PostgreSQL conformance,
  including concurrent contexts and rollback of context, receipt, and wake.
  Confirm a rejected new payload leaves no acceptance fields behind.

**Done when:** service and HTTP tests demonstrate current authorization before
replay, and both stores preserve the original effective payload atomically.

#### I2. Retain execution and report submission intents (F; depends on I1)

Implemented under the server frontend in `workflows/utils/retained-inputs.ts`,
`workflows/components/ManagedInputSubmissions.tsx`, its context hook, and the
execution/report page owners. `ActionsBlock.tsx` uses the page owner rather than
a local operation tracker. The following requirements remain regression gates.

- Store an immutable submission snapshot keyed by tenant, instance, request,
  and operation. For reports retain report ID, block, caller filters, and caller
  payload; a retry must not pick up newly selected filters or current form data.
- Model submitting, uncertain, accepted, and definitively rejected outcomes.
  Discovery removing an action cannot turn an uncertain operation into success.
  Provide retry controls independent of the currently actionable list.
- Keep uncertain old operations available when a user intentionally creates a
  new response. Allocate a new operation for edits; never overwrite the old
  snapshot or silently retarget it to another instance.
- Extend `input-submission.test.ts`, execution timeline tests, and report page
  tests with lost acknowledgements, removed actions, block remounts, delayed
  responses, and identical request hashes in different instances. Verify tenant
  changes cannot display or retry another tenant's retained intent.

**Done when:** the mounted page can recover the original receipt after its
action disappears. Reload persistence remains outside this baseline guarantee.

#### I3. Close compiled ownership and recovery gaps (A–C)

Audit `RuntimeHost` implementations, `runtime_host/scoped/`, scoped runner
factories, and `crates/runtara-workflows/tests/direct_wasm_execute.rs`. Record
the owner/fence/settlement matrix required by completion step 1 below.

- Migrate raw-only test hosts to managed registration, response polling, and
  closure so real compiled tests exercise the production lifecycle contract.
- Add compiled repeated AI-tool, loop, embedded, and independently cancellable
  child cases. Verify cancellation closes only the owning subtree while a
  sibling remains actionable; replay cannot reopen a cancelled invocation.
- Extend `crates/runtara-environment/tests/managed_inputs.rs` and scoped runner
  tests with a storage failure during abandonment, followed by restoration and
  the actual production recovery worker. Do not substitute an explicit test
  call to root termination for automatic recovery evidence.
- Test child-owned park/wake with acceptance before and after parking, stale
  leases, restart, and explicit pause. Build and run with event tracking off.
- Define diagnostic deduplication by logical request/event identity and prove
  that missing or replayed diagnostics cannot alter authoritative input state.

**Done when:** every admitted child path has a durable owner or an explicit
unsupported result, and actual compiled recovery cannot leave an orphaned wait.

#### I4. Make provider intake durable (D–E)

Change `crates/runtara-server/src/channels/session.rs` and all four adapters:
`teams_webhook.rs`, `slack_webhook.rs`, `webhook.rs`, and `mailgun_webhook.rs`.
Reuse `api/services/session_queue/managed.rs` and `managed.lua` for intake.

- Implement the provider identity/acknowledgement table described in step 5.
  Persist an authenticated routing snapshot and stable normalized message in
  one enqueue/deduplication operation before acknowledging intake.
- Make `per_message` session identity deterministic across redelivery. Treat
  reused provider IDs with changed semantic content as conflicts.
- Move dispatch to the shared delivery worker. In-memory notifications may
  accelerate delivery but cannot be its only trigger.
- Test real HTTP adapters with enqueue failure, provider retry, duplicate
  acknowledgement, and reconstructed process-local router state.

**Done when:** provider acknowledgement proves durable intake and redelivery
cannot create a second envelope or execution. Verify provider retry semantics
against official documentation during implementation.

#### I5. Persist collectors and channel outcomes (E; depends on I4)

Replace receiver-only state in `channels/collector.rs` with durable collection
state and atomic transitions in the managed queue. Extend shared delivery,
status/resolution DTOs, and exhaustive state matches together.

- Persist target, schema, accumulated fields, processed message IDs, and the
  final operation/payload. Apply each reply once before advancing the queue.
- Distinguish startup handoff, consumed-for-collection, and accepted-response
  outcomes. Startup delivery must never also answer the first managed wait.
- Resume collection after process restart. Cover invalid repeated replies,
  skip, `/cancel`, target closure, and loss of the final acceptance response.
- Expose blocked/failed collector and startup states to the channel owner with
  current authorization and conditional resolution operations.
- Remove `reserve_activity`/`release_activity`, legacy `push_event`/`pop_event`,
  event-based target selection, and raw managed writes after all callers move.

**Done when:** recovery succeeds without retaining the original actor, and no
acknowledged channel message depends on a destructive queue pop.

#### I6. Prove background handoff and bounded recovery (E; depends on I3–I5)

Extend `crates/runtara-server/tests/session_delivery.rs` and tests for
`workers/session_delivery_worker.rs`, `workers/execution_outbox.rs`, and the
shared delivery service.

- Launch an actual compiled workflow, observe its managed wait, bind and accept
  a queued response, lose acknowledgement, restart delivery, and recover the
  same receipt. Assert the next wait receives no duplicate response.
- Exercise both startup and response modes across uncertain launch/source
  acknowledgement, absent SSE subscribers, and replacement session routes.
- Test more scopes than one scan page and worker batch; include a slow scope,
  blocked FIFO head, expired leases, corrupt metadata, and shutdown mid-cycle.
  Prove progress for unrelated scopes and bounded work per cycle.
- Test completed-envelope pruning while unresolved messages and routing remain
  retained. Record actual completed-message and provider deduplication windows,
  plus the deployed Valkey persistence configuration.

**Done when:** two-worker crash tests prove durable acceptance/replay and worker
load tests establish recovery without a connected browser or channel actor.

#### I7. Complete the contract audit and release gate (F; depends on I1–I6)

Search production callers of event reconstruction, raw signal submission,
legacy queue helpers, and action submission without operation IDs. Classify
each retained use as historical display or unmanaged signaling, with its file
and reason; remove managed-input fallbacks.

- Regenerate runtime TypeScript when public DTOs change, rebuild components,
  and run the Phase 7 commands with the feature gates declared in manifests/CI.
- Record each gate as command, prerequisites, result, and remaining failure.
  Run server tests with isolated PostgreSQL/pgvector and Valkey; run frontend
  tests, lint, and build using its pinned Node version.
- Add bounded counters and reason codes for acceptance/replay/conflict,
  discovery/closure failures, wake backlog, and delivery retry age. Keep
  payloads, credentials, and unbounded identifiers out of metric labels.
- Confirm the unused-feature inventory, then apply additive migrations and
  deploy matching host/runtime/compiler/server artifacts with rebuilt workflow
  images. Follow the coordinated recovery/rollback policy in Phase 7.

**Done when:** the complete acceptance matrix passes with no unresolved managed
consumer, queue, ownership, or authorization gap. No backward-compatibility
adapter, event backfill, Control-agent implementation, or MinIO migration is
included in these work items.

### Implementation breakdown for the remaining work

This breakdown refines I3–I7 into dependent implementation steps. It is a plan,
not a verification record. The worktree already contains draft `StartupIntent`,
`DeliveryMode`, `HandedOff`, and `delivery/startup.rs` support; those additions
still need contract review and the tests below before channel adapters use them.
I1 and I2 remain regression gates. No backward-compatibility layer is required.

#### P1. Verify startup delivery independently of provider intake

**Files:** `api/services/session_queue/managed.rs`, `managed.lua`,
`delivery.rs`, `delivery/startup.rs`, `api/handlers/sessions.rs`, and
`tests/session_delivery.rs`, under `crates/runtara-server/src/` except the test.

1. Finalize mode/state invariants in Rust and Lua. A startup envelope holds the
   complete frozen workflow inputs, pinned version, proposed instance ID, and
   optional previous instance. It cannot carry a managed request target or input
   receipt. Managed-response envelopes cannot finish as `handed_off`.
2. Require the current lease token for every startup transition. Persist the
   launch identity before queueing; use that identity to recover the existing
   execution source before consulting mutable configuration or root liveness.
   A pending launch remains retryable. Only confirmed source handoff completes
   the startup envelope; enqueue success alone is insufficient.
3. Keep rejection and uncertainty distinct. A rejected/expired source blocks
   the envelope with its original inputs retained. A backend timeout retries
   the same launch identity. Neither path allocates another instance or binds
   the startup message to the first managed wait.
4. Update status DTOs and every state match together. Expose startup handoff as
   “Passed to workflow,” with no acceptance receipt. Disable request-selection
   resolution for startup messages. Regenerate the runtime client through
   `generate-api-runtime-offline`; do not edit generated TypeScript manually.

**Gate:** isolated Valkey tests cover mode/identity conflicts, corrupt metadata,
stale-worker transitions, handoff retention, and replacement-route compare and
swap. Server integration tests cover lost queue/handoff acknowledgements and a
workflow opening its first wait before startup acknowledgement: that wait must
remain unanswered. These source-state tests do not replace P6's compiled test.

#### P2. Persist provider intake before session dispatch

**Files:** `channels/session.rs`, the four webhook adapters, and proposed
`channels/intake.rs` with a slot-local atomic intake script.

Use a durable connection-scoped inbox before the session queue. This resolves
two problems that a direct session enqueue does not: trigger/session-mode
changes between provider retries, and new intake while a session FIFO is blocked.
The current startup enqueue's drained-queue requirement must not determine
whether an authenticated inbound message can be acknowledged.

1. Define a credential-free intake record containing tenant/connection/provider
   identity, canonical semantic content, immutable trigger/workflow/version and
   session-routing snapshot, required reply references, creation time, dispatch
   state, and lease. Derive bounded identities from an encoded tuple. Exclude
   authentication signatures, secrets, and transport retry metadata from stored
   content and equivalence checks.
2. Atomically deduplicate and append in the connection's hash slot. Look up the
   existing provider identity before resolving a new route: matching redelivery
   returns the original record even if configuration changed; changed semantic
   content conflicts. For first intake, persist the winning route snapshot in
   the same operation. Concurrent proposals must converge on that winner.
3. Persist a deterministic session ID for `per_message`; freeze the selected
   session for `per_sender` and `per_trigger` as well. Authentication remains
   current on retries. A disabled/deleted destination stops new dispatch with a
   visible blocked outcome; it never silently routes retained work elsewhere.
4. Dispatch across slots with an idempotent handoff, not a cross-slot Lua
   transaction: claim intake, enqueue the exact session message/operation,
   then mark intake dispatched under its lease. If acknowledgement is lost,
   replay the same session enqueue and recover its existing outcome.
5. Add inbox discovery to background recovery; notifications are optional.
   Retain unresolved intake and routing without expiry. Keep destination
   deduplication for as long as an unresolved source can retry dispatch, so
   completed-envelope pruning cannot recreate an already delivered message.
   Specify the release/retention handshake and test it before enabling pruning.
6. Verify each provider's inbound identity, retry status, deadline, and retry
   window against official documentation; record them in the adapter table.
   Do not infer inbound guarantees from outbound API retry guidance. Change
   HTTP acknowledgement to follow durable intake, including the Teams path
   that currently spawns work before returning success.

**Gate:** HTTP adapter tests discard the original router between attempts and
cover persistence failure, duplicate acknowledgement, changed trigger/version/
session mode, concurrent first intake, missing stable provider identity, and
crashes on either side of session enqueue. Use fake provider clients; never
send live messages. No acknowledged intake may rely on an actor's memory.

#### P3. Select delivery mode and persist collector progress

**Files:** shared managed queue/delivery modules, `channels/collector.rs`,
`channels/session.rs`; proposed collector persistence module and script actions.

1. Introduce an unclassified channel-message stage in the session queue. Under
   the head lease, freeze its mode before any launch or managed acceptance.
   Select startup only under the explicit new-execution policy; otherwise use
   authoritative request discovery. Zero targets waits or blocks according to
   that policy; multiple targets require selection. A discovery failure cannot
   authorize startup. A bound message never changes target or mode on retry.
2. Store collection state in the session's hash slot: collector identity,
   target, schema snapshot, field order, accumulated values, retry counts,
   processed message identities, response operation, and collection revision.
   Keep presentation delivery separate from workflow-response acceptance.
3. Atomically apply a reply and finish its envelope as consumed for collection,
   checking the queue lease and expected collector revision. Record invalid
   replies too: redelivery cannot consume a second validation attempt or fill
   the next field. Preserve existing required/optional, visibility/default,
   skip, cancellation, and retry-limit behavior explicitly in tests.
4. Freeze the assembled payload before the first acceptance call. After an
   uncertain result, replay that operation before checking target openness.
   Complete the collector only with its retained receipt; report closure or
   conflicts without retargeting. `/cancel` cancels collection, not the owned
   workflow request, and retains an inspectable outcome.
5. Persist prompt intent and sufficient non-secret reply-routing references
   for restart. Outbound prompts can repeat after an uncertain provider result;
   distinguish this limit from exactly-once application of collected replies.

**Gate:** restart between fields, repeated invalid input, optional skip, hidden
defaults, cancellation, target closure, competing collectors, and lost final
receipt all preserve field progress and response identity. Add tests showing
that a blocked FIFO head does not prevent durable provider intake.

#### P4. Wire adapters, owner status, and recovery as one change

After P1–P3 pass, switch all four providers to intake plus shared dispatch.
Extend authorized status/resolution to intake, startup, collector, and response
outcomes; expose references needed for diagnosis without payloads or secrets.
Explicit failure and target selection must use conditional transitions and
current ownership. A deliberate replacement of bound work gets new identities.

Register both intake and delivery recovery at server startup and stop claiming
on shutdown. Test recovery with no SSE subscriber or channel actor. Remove
`reserve_activity`/`release_activity`, destructive `push_event`/`pop_event`
delivery, `find_pending_signal_id`, and raw managed-address writes only after
the caller audit confirms all providers have moved. Update generated DTOs and
frontend tests for every added state in this same review unit.

#### P5. Close runtime lifecycle proof gaps before end-to-end approval

Complete I3's owner/fence/settlement inventory across root, composed, isolated,
published-child, and AI-tool paths. In each supported path, inject abandonment
storage failure, restore storage, and exercise the production recovery worker.
The test must prove that successful recovery cannot leave the failed wait open;
an explicit test-only termination call is not sufficient.

Add compiled repeated-wait/AI-tool and sibling-cancellation cases with tracking
off, migrate raw-only test hosts, and test child-owned park/wake ordering and
pause arbitration. Deduplicate optional lifecycle diagnostics atomically by
logical transition identity; diagnostic failure must not control input state.
Add a forward migration if required, without modifying a committed migration.

**Gate:** every supported admission path has compiled evidence and memory/
PostgreSQL parity for its persistence invariants. Unsupported managed waits
fail explicitly rather than falling back to raw signals.

#### P6. Run the final crash, load, and release matrix

P6 depends on P4 and P5; P5 may be developed independently of P1–P4. Keep each
behavior change with its focused tests, then run the combined Phase 7 matrix.

| Boundary | Required injected failure and assertion |
| --- | --- |
| Provider → intake | Lose acknowledgement after durable write; redelivery returns the original route and record. |
| Intake → session | Crash after destination enqueue; recovery finds that envelope even after normal completion/pruning would otherwise occur. |
| Startup → execution source | Lose launch/handoff acknowledgement; one instance launches and the first wait remains open. |
| Collector → acceptance | Lose receipt after commit; recovery returns the same receipt without applying another field or response. |
| PostgreSQL → queue ack | Accept in a real compiled workflow, restart delivery, then open a second wait; replay never answers the second wait. |
| Worker → storage | Exceed one scan page/batch; inject a slow scope, corrupt metadata, blocked head, and expired lease; unrelated scopes progress within bounded cycles. |
| Retention → recovery | Advance backend time; completed records prune only when no unresolved source can redeliver them, while unresolved payloads and routes remain retained. |

Run `session_delivery` with both `db-integration-tests` and
`valkey-integration-tests`, and `managed_inputs` with
`scoped-workflow-integration-tests`; otherwise Cargo can omit the required
targets. Register new integration targets with their actual service feature
gates. Use isolated server/runtime databases, pgvector where server migrations
require it, Valkey, rebuilt components, and the pinned frontend Node version.

Record commands, results, prerequisites, and unresolved failures in the existing
checkpoint. Review generated contracts, remaining raw/event-based consumers,
bounded metrics, deployed Valkey durability, and retention guarantees. Release
requires all I1–I7 gates, coordinated runtime/server/client artifacts, and the
unused-feature inventory. Implementation of Control and changes to MinIO remain
outside this plan.

### Concrete completion sequence

This sequence turns the phases below into implementation units for the current
worktree. Existing code is a starting point, not evidence that a unit has passed.
Keep tests with each unit; complete runtime ownership/lifecycle gates before
activating the migrated consumers. No compatibility layer, event backfill, or
MinIO replacement belongs in this change.

#### 1. Close runtime ownership and failure gaps (A–C)

Deliver an admission matrix covering root, embedded, published, nested,
non-durable/unknown, and resumed execution paths. For each row record the host
that supplies the logical owner, physical fence, stable wait identity, and
settlement hook. Audit `RuntimeHost` implementors and scoped factories; either
support managed waits with durable ownership or reject them explicitly. Never
fall back to root authority after child authorization fails.

Finish abandonment failure handling in the compiler, stdlib, component host,
and environment together. A failed close must prevent recovery from leaving an
actionable orphan; propagate failure and fence further managed writes. Preserve
requests during resumable teardown. Add persistence-backed diagnostic
deduplication separately from mandatory registration and acceptance, and migrate
integration test hosts that still implement only raw polling.

**Gate:** compiled tests prove repeated AI-tool identity, child cancellation with
a live sibling, closure failure during `onError`, and child-owned park/wake in
both acceptance orderings. Repeat with tracking disabled and fresh runtime
ownership after restart. Extend the existing core/store conformance suites
instead of duplicating their invariants in endpoint tests.

#### 2. Complete the web session contract (D–F)

Update `frontend/src/features/workflows/queries/chat.ts::sendSessionMessage` and
`pages/Chat/useChatStream.ts` together with `api/handlers/sessions.rs`. Send
`{ messageId, operationId, message }` for unbound text, or the mutually exclusive
`payload` form. Allocate both IDs once per intentional message; retain the IDs
and payload after an uncertain enqueue response. Editing the message creates a
new intent. A confirmed enqueue clears that retry state; another intentionally
identical message receives new IDs. Selected managed-request replies continue
using direct acceptance and its request-scoped operation tracker.

Return and validate `DeliveryStatus`, including matching message/operation IDs.
Represent enqueue success as queued, and show accepted only after a managed
receipt. Add the session submission path to the OpenAPI registry and regenerate
the runtime client through `generate-api-runtime-offline`.

Update `useChatInputs.ts` to follow an authorized session route change without
depending on a `started` SSE event. Apply the new instance only if the response
still belongs to the current session and refresh generation. Preserve uncertain
submissions against their original instance/request rather than moving their
drafts to the replacement execution.

**Gate:** HTTP contract tests catch missing IDs and mismatched acknowledgements;
hook tests prove identical retry IDs, intentional new IDs, route advancement
without SSE, and rejection of an old session's delayed response.

#### 3. Finish launch recovery and background delivery (E)

Complete `session_queue/delivery.rs`, the launch operations in `managed.lua`,
`execution_engine.rs`, and `workers/session_delivery_worker.rs` as one review
unit. Preserve this ordering:

1. Claim the FIFO head. An already bound message retries its original receipt
   before considering current session routing or instance liveness.
2. For an unbound message, discover on the current execution. Only the explicit
   terminal-session routing policy may permit another execution.
3. Persist the proposed instance ID, pinned workflow version, input snapshot,
   and previous route under the current lease before invoking the engine.
4. Recover uncertain launch acceptance through the durable outbox identity.
   Publish the new route only with a current lease and the expected prior route.
   Lease loss preserves the intent for the next worker; it never allocates a
   replacement instance ID.
5. Bind an eligible managed request and acknowledge only its acceptance receipt.
   A successful execution launch alone does not acknowledge the input message.

Validate retained launch metadata before acting on it. Keep missing/not-yet-
registered executions distinct from backend failures where the runtime contract
permits it; neither result is an empty successful discovery. Bound retries,
startup scanning, cleanup, and shutdown must retain unfinished envelopes.
Preserve an entire SCAN page even when it exceeds the processing budget. Define
fairness and a total cycle time budget so slow or idle scopes do not indefinitely
delay active sessions. Retain routing metadata while unresolved work needs it;
document a separate idle-session retention policy before adding any expiry.

Observe the retained source request's lifecycle on each unbound launch retry,
including after publishing the session route. Its identity proves a committed
source request, not that the request can still launch: expiry/cancellation or a
permanent source failure requires a blocked outcome. A message whose launch is
already recorded must not allocate another instance merely because runtime
registration is still missing. Initial session creation uses the engine's
`session:<instance>` source key; replacement intents use their retained
`session-input-launch:<instance>` key. Both must distinguish pending registration
from a rejected source or a missing previously handed-off instance.

**Gate:** isolated PostgreSQL/Valkey tests interrupt delivery before launch,
after outbox commit, before route publication, after input acceptance, and
before queue acknowledgement. A fresh worker without an SSE subscriber recovers
the same execution, request, and receipt. An expired worker cannot publish or
acknowledge after another worker reclaims its lease.

Complete the remaining launch verification in
`crates/runtara-server/tests/session_delivery.rs` in two steps:

1. Keep the existing source/outbox boundary tests as focused regressions. Add a
   fixture that runs the actual source handoff and environment launch path with
   a compiled workflow, instead of manually inserting the replacement runtime
   root and its request. Start with tracking disabled, submit a queued message,
   and assert one launched instance, one accepted request, and one receipt after
   worker restart. A workflow opening a second wait must leave that wait
   unanswered by redelivery of the first message.
2. Add deterministic worker-budget tests around a controllable delivery
   dependency: more than 16 scopes in one scan page, a slow first scope, duplicate
   scan entries, discovery failure, and shutdown during delivery. Assert that
   unprocessed scopes survive the cycle, later scopes eventually run, and
   cancelled work is recovered through its lease. Keep a real Valkey test for
   backend lease expiry; use controlled time for worker scheduling tests.

Record separately the maximum observed retry delay under the test workload and
the configured cycle budget. A five-second cycle budget alone is not a bound on
delivery latency across an unbounded number of sessions.

#### 4. Expose delivery state and explicit resolution (E–F)

Add tenant-authorized session routes with typed OpenAPI schemas:

| Proposed route | Contract |
| --- | --- |
| `GET /sessions/{sessionId}/deliveries` | Bounded cursor page of delivery statuses and the continuation cursor. |
| `GET /sessions/{sessionId}/deliveries/{messageId}` | One retained status, including reason and receipt/target IDs where present. |
| `POST /sessions/{sessionId}/deliveries/{messageId}/resolve` | Explicitly fail a blocked message or select a target for an unbound blocked message. |

Resolve session ownership before reading status. Target selection also checks
tenant/workflow association and retained request ownership; acceptance still
arbitrates current eligibility. Use an atomic blocked-state transition so
resolution cannot edit a leased message. A bound message can retry its original
target or be explicitly failed; a replacement response needs new identities.
Never expose lease tokens, internal launch inputs, or accepted payloads.

In chat, poll delivery status independently of history, display blocked/failed
reasons, and offer explicit target selection or failure. Treat cursor pages as
changing scans: deduplicate by message ID and do not imply an exact total or
complete history from one page. Keep unavailable status visibly distinct from
an empty result. Carry the same states into the channel owner's status surface.

**Gate:** API/UI tests cover foreign ownership, two simultaneous resolvers,
resolution racing a claim, immutable bound targets, blocked FIFO ordering,
pagination, reload discovery, and service unavailability.

#### 5. Migrate channel intake and collectors (D–E)

Change `channels/session.rs` and the Teams, Slack, Telegram (`webhook.rs`), and
Mailgun webhook adapters together. Derive a stable message/operation identity
from tenant, connection, conversation, and provider event identity. Persist
deduplication and enqueue atomically before acknowledging provider intake or
handing work to an in-memory task. Remove the separate reservation/release path;
a reservation or successful in-memory send cannot prove durable intake.
Specify each provider's retry response when persistence is unavailable. If a
provider requires early acknowledgement, first persist a recoverable intake
record; do not acknowledge and rely only on background enqueue.

Replace `find_pending_signal_id`, destructive queue pops, and raw managed-signal
writes with the shared delivery service. Structured collectors must retain their
chosen request and stable operation while gathering values, then submit the
validated payload. Define startup-message routing separately: when a message is
consumed as execution input, record its launch outcome explicitly rather than
inventing a managed-input receipt or delivering it again to the first wait.

**Gate:** exercise actual adapters with provider redelivery, failed enqueue,
process restart before dispatch, collector validation failure, ambiguous waits,
and acknowledgement loss. Remove the old one-hour queue expiry and helper APIs
only after the final caller is migrated; no legacy queue fallback is planned.

Implement this package as the following dependent changes. Channel paths below
are relative to `crates/runtara-server/src/channels/`.

1. **Normalize durable intake.** Extend `InboundMessage` and the intake contract
   in `session.rs` with the verified tenant/connection, trigger, conversation,
   provider identity, and intended delivery mode. Derive bounded IDs from an
   unambiguous encoded tuple, not concatenated text. `per_message` routing must
   derive the same session on provider redelivery instead of allocating a random
   session before deduplication. Resolve routing before the atomic enqueue;
   persist its immutable snapshot with the envelope. Deduplication must compare
   the canonical normalized message, excluding transport-only retry metadata.
   A reused provider identity with different semantic content is a conflict.
2. **Define adapter acknowledgement contracts.** Cover `teams_webhook.rs`,
   `slack_webhook.rs`, `webhook.rs` (Telegram), and `mailgun_webhook.rs` in a
   table of authenticated event identity, conversation identity, retry response,
   and acknowledgement deadline. Verify provider behavior against their official
   documentation when implementing this table. Challenge/verification requests
   remain separate from message intake. For supported message events, require a
   stable provider identity or a documented deterministic provider-specific
   fallback; never generate a random ID and claim redelivery deduplication.
   Authenticate first, durably enqueue second, acknowledge last. An in-memory
   actor send becomes an optional notification after durable intake.
3. **Separate startup and response outcomes.** Add a typed delivery mode for a
   message used as workflow startup input versus one awaiting managed response
   acceptance. Freeze the selected mode before its first external effect. A
   startup message retains its launch identity and source lifecycle, records
   handoff as a distinct outcome, and never also answers the first wait. Update
   all queue-state matches, status DTOs, generated clients, cleanup rules, and
   resolution controls together. Test rejected and uncertain launches for both
   modes before moving channel callers onto the worker.
4. **Make field collection recoverable.** Replace `collector.rs`'s exclusive
   dependence on an in-memory receiver. Persist a collector record with the
   instance/request, immutable schema, response operation ID, next field,
   accumulated values, and processed provider message IDs. Each incoming reply
   must be recorded with its field transition atomically before advancing the
   queue; duplicate replies cannot fill the next field. Keep collector state
   and its queue transitions in the same Valkey slot. Collector messages need
   a distinct consumed-for-collection outcome; only the final assembled response
   can have an input receipt. Once submission starts, freeze the effective
   payload and retry that exact operation after uncertain acceptance.
5. **Define collector interruption behavior.** Persist skip, validation retry,
   cancellation, and exhausted-retry transitions. Treat `/cancel` as cancelling
   this collection attempt, not the workflow-owned request. Revalidate retained
   target eligibility before gathering another value; a closed target blocks
   the collection rather than selecting another wait. On restart, resume from
   stored progress. Prompt delivery may repeat after an uncertain provider
   acknowledgement; do not claim exactly-once outbound messages. Persist only
   the reply-routing references needed for recovery, never credentials.
6. **Expose outcomes and remove old delivery.** Reuse tenant-authorized delivery
   listing/resolution for a channel owner's status surface, including collector
   and startup outcomes. Remove actor-owned raw managed-signal sends, event-based
   selection, `reserve_activity`/`release_activity`, and `push_event`/`pop_event`
   once all four adapters use durable intake. Retain actors only for presentation
   that can be reconstructed from stored state.

Add adapter tests at the real HTTP boundary with fake outbound provider clients,
isolated Valkey/PostgreSQL, and process-local router state discarded between
attempts. Required cases include duplicate intake before and after acknowledgement,
enqueue failure followed by provider retry, restart between two collected fields,
replayed invalid replies, `/cancel`, and receipt loss after final collection.
No test may satisfy recovery merely by retaining the original actor or receiver.

#### 6. Complete report and form retry semantics (D–F)

Extend the acceptance contract with the report retry context described in the
consumer checklist below: canonical caller payload, immutable effective payload,
request/operation identity, report/block, and principal. Persist the context in
the same transaction as acceptance and implement memory/PostgreSQL parity.
Use a forward migration for committed schema changes. Keep authorization current
while replaying an otherwise matching retained receipt before reevaluating
implicit defaults.

Move unresolved execution/report form intents above list-item lifetimes. A
refresh that removes an accepted action must not discard an uncertain operation
before its receipt can be retried. Keep retry state scoped to the original
tenant, instance, and request, separate from currently actionable discovery.
The baseline guarantee remains the mounted client session; browser reload
persistence requires an explicit storage/privacy design and tests.

**Gate:** test changed report defaults, revoked access, edited caller payload,
lost acknowledgement after terminal transition, and action removal during a
refresh. Only confirmed receipts clear uncertain retries as successful.

Implement report retry support from persistence outward:

1. Add a typed, optional acceptance context in core, supplied only by trusted
   server code. For reports, bind the context to canonical caller payload,
   resolved report ID (not its mutable slug), block ID, principal, and relevant
   submission filters/scope. Tenant, instance, request, and operation remain
   part of the existing acceptance identity. Define canonical equality in one
   shared helper; direct submissions retain their context-free contract.
2. Extend memory and PostgreSQL acceptance and receipt lookup together. Store
   context beside the effective accepted payload in the same transaction and
   compare it during both early replay lookup and locked acceptance. Add a
   forward migration if the preceding schema migration has already landed.
   Test concurrent same-operation report submissions with different contexts
   and rollback between context/receipt/wake writes.
3. Change `api/services/reports.rs::submit_report_workflow_action` to check
   current report/block/target authorization, then attempt contextual receipt
   replay, then evaluate `merge_report_action_payload` only for a new operation.
   Recheck the operation during atomic acceptance to cover a concurrent winner.
   Authorization failure must take precedence over returning a retained receipt.
   Public receipt DTOs must not expose either payload or the internal context.
4. Move uncertain intents from individual execution/report action components
   into their mounted page owner. Key them by tenant, instance, request, and
   operation; report intents also retain their submission context. Render retry
   controls independently of the current action list. Test two instances with
   the same request hash, refresh removing an action, out-of-order responses,
   changed form values, and switching report/session context. Explicit retries
   use the retained snapshot, while intentional edits allocate a new operation.

Add report-specific integration coverage alongside `tests/managed_actions.rs`;
the existing direct-action tests do not establish contextual replay semantics.
Regenerate runtime TypeScript only if the public contract changes, and test the
execution and report components with a lost acknowledgement followed by a
successful receipt replay after their action disappears from discovery.

#### 7. Audit, verify, and prepare the coordinated release (F)

Search all remaining event-derived target selection, raw custom-signal writes,
queue pops, and submission calls without operation IDs. Classify retained uses
as historical display or unmanaged signals and remove obsolete managed paths.
Run the Phase 7 matrix using the actual feature gates in CI, rebuild components,
regenerate contracts, and review the generated diff. Record checks actually run,
missing prerequisites, and outstanding failures; do not promote historical
checkpoint results into verification of later changes.

Add payload-free counters for blocked delivery, retry age, launch recovery,
discovery failures, closure failures, and wake backlog. Document Valkey's actual
persistence settings and the completed-envelope/deduplication window separately
from PostgreSQL receipt retention. Release only after all gates pass and the
unused-feature assumption is confirmed. Apply additive migrations first, deploy
matching server/runtime/compiler artifacts, and recompile affected workflows.
Do not activate partial consumers or introduce backward compatibility.

### Runtime and integration details

#### A. Finish execution ownership before expanding consumers

1. Audit `runtime_host/scoped.rs`, `scoped/root.rs`, and
   `scoped/invocation.rs` alongside the invocation ledger. Enumerate root,
   embedded child, published workflow agent, and resumed child admission paths.
   Give every supported child a trusted durable logical owner and execution
   fence before permitting managed registration. Do not weaken a failed child
   authorization to root authority.
2. Use the host-derived parent relation persisted by
   `begin_invocation_attempt_with_parent`; do not infer ancestry from encoded
   path strings. Cancellation/settlement must invalidate
   owned descendants atomically, even where only a descendant has registered a
   request. Siblings remain eligible. Acceptance and discovery must use the same
   ancestry validity check; closing exact-path rows alone is insufficient.
3. Keep every terminal writer covered: memory transitions close under the store
   lock, and migration 030's terminal-status trigger closes requests for any
   PostgreSQL status writer, including environment launch/recovery SQL. Retain
   direct-SQL commit/rollback tests alongside the shared lifecycle conformance
   tests when changing these transitions.
4. Preserve the host-owned teardown distinction while attaching production
   leases. `workflow/scoped_execution.rs` obtains the root's disposition before
   shutting down the execution context and calling `RootExecutionCoordinator::close`.
   `TaskHandle::drop` in `execution_host.rs` requests a physical stop with pending
   owner disposition. `IsolatedTasks` defers settlement of that stop until the
   owner chooses terminal or resumable teardown, and `TaskCleanup` propagates
   the choice to nested contexts. Do not classify intent from the root's database
   status: command finalization may not have changed it yet.
5. Keep explicit guest resource release separate from automatic Store drop.
   Explicit cancellation must remain terminal even if a later root pause requests
   resumable cleanup, including cancellation arriving during nested cleanup.
   Retain the supervisor and PostgreSQL race tests for both orderings when
   changing admission or shutdown. Keep the intent host-controlled; the guest
   cannot select a weaker closure policy for itself or its descendants.
6. Retain the production lease lifecycle in `runner/embedded/scoped.rs::execute`
   and its caller in `runner/embedded.rs` when extending child admission. Claim
   against the physical runner handle and expected epoch, bind the same lease
   to the root host and factory, and propagate parent fences to nested factories.
   Claim before guest admission within the existing launch/start authorization
   sequence, leaving final durable gate confirmation at instantiation. Cover setup
   failure, denied start, trap, cancellation, suspension, and normal completion
   with one bounded cleanup path. Revoke only the exact owned lease; never let
   stale cleanup revoke its replacement. On resumable suspension, finish child
   teardown while its fence is valid, preserve logical attempts and requests,
   and then release execution ownership. Recovery must fence the previous
   runner through the existing launch recovery protocol before admitting a
   replacement; observing a lease is not permission to steal it.
7. Record an admission matrix for root waits, ordinary embedded calls,
   published workflow agents, scoped children, and nested/resumed children.
   For each, identify the host supplying ownership and the compiler supplying
   stable signal identity. The scoped factory currently admits durable attempts
   only for `durable == Some(true)`. Define managed-wait support explicitly for
   non-durable/unknown children: either supply the required durable ownership
   without changing their ordinary checkpoint semantics, or reject unsupported
   waits during validation/admission. Never permit them to register under root
   authority as a fallback. Required supported paths need compiled integration
   coverage before this package can pass.
8. Add focused supervisor tests first, then exercise the production runner with
   compiled children and real PostgreSQL persistence. Cover local cancel racing
   root pause in both orders, automatic handle drop, a paused root with an
   unjoined waiting child, nested teardown, cleanup failure, denied start, and
   stale lease revocation. Resume under a fresh lease and verify that the same
   request and accepted bytes survive. Verify that cancelled descendants cannot
   register again and that a live sibling remains actionable.

Use this settlement table as the contract for those tests:

| Cause | Logical attempt/request outcome | Execution ownership |
| --- | --- | --- |
| Child returns a resumable suspension | Preserve active attempt and open/accepted request. | Retain while root runs; release with root suspension. |
| Root pause, breakpoint, or resumable shutdown tears down a child | Preserve resumable attempt/request; do not create a child cancellation tombstone. | Release after teardown; resume with a fresh fence. |
| Explicit child cancel or an explicit release that abandons the child | Cancel that attempt and descendants; close unanswered requests; retain receipts. | Root and siblings may continue. |
| Child completes or fails permanently | Settle its attempt and close abandoned descendant waits. | Root lease may remain active. |
| Root completes, fails, or is cancelled permanently | Close all unanswered requests and clear pending wakes. | Revoke the owned lease; retain receipts. |
| Cleanup or settlement cannot establish a durable outcome | Surface failure; do not report successful resumable cleanup. | Fence further writes and let lifecycle recovery resolve the root. |

#### B. Make the wake transaction explicit

`Persistence::park_instance_on_signals`, called by
`runner/embedded.rs::park_invoke_suspend`, now persists the wait set in the same
transaction as the suspended state. `instance_input_parks` and its in-memory
equivalent hold that association for the current park; a later park replaces it.
Keep this transaction boundary when completing child-owned wait integration.

Under the root lock, both park and acceptance must evaluate the same predicate:
the root is suspended for signals, the accepted request belongs to the current
park, its owner remains valid, and no explicit pause/breakpoint or terminal
transition prevents waking. If true, write the scheduler deadline/reason and
clear the handled intent in that transaction. Otherwise retain a recoverable
intent where the request can still resume.

| Transaction order | Required result |
| --- | --- |
| Accept, then park | Park observes the immutable response and schedules the matching wake before committing. |
| Park, then accept | Acceptance writes receipt, payload, and the matching scheduler wake atomically. |
| Pause, then accept | Receipt succeeds for an otherwise open request; no automatic wake is scheduled. |
| Accept/wake, then pause | Pause clears the obsolete wake; scheduler claim rechecks eligibility atomically. |
| Terminal transition, then new acceptance | Acceptance conflicts without payload or wake writes. |
| Acceptance, then terminal transition | Receipt remains replayable; terminal transition clears wake intent and scheduled wake. |

Add a bounded reconciliation operation to `wake_scheduler.rs` that retries
pending intents using those same locked checks. Select candidates without
reversing the root-before-request lock order. A scheduler notification is an
optimization; correctness comes from committed scheduler state. Do not clear an
intent merely because a runner read the payload: a crash before its next durable
checkpoint must still permit replay and a later matching park to progress.

#### C–F. Concrete integration checkpoints

- In `direct_wasm/compile/wait.rs`, handle every `InputState::Closed` reason.
  Only expiry maps to `WAIT_TIMEOUT`; abandonment/cancellation must follow the
  corresponding failure/unwind path. An untimed closed wait must never keep
  polling indefinitely. Wire `close-input` into handled exits that abandon a
  registered wait, preserving ordinary suspension/replay.
- Choose one durable deduplication key per request diagnostic. Replayed
  registration must not emit another logical request event. Keep diagnostic
  persistence separate from the success of mandatory state transitions.
- Replace `find_pending_signal_id` in both session implementations and all
  actionable event matching. Change `enrich_pending_input` to return `Result`
  and update its three execution-engine callers together. Add batched existence
  support to `InputRequests` rather than looping over event lookups.
- Define public submission as `{ requestId, operationId, payload }`, with target
  instance/workflow supplied by the authorized route where applicable. Return
  `{ receiptId, requestId, acceptedAt }`; do not expose the stored payload merely
  because it is part of the internal receipt. Specify stable 409 reason codes
  before regenerating TypeScript and migrating callers.
- Replace `pop_event` with typed queue operations: enqueue, claim, bind target,
  acknowledge, reclaim, and mark an explicit failure. Store message identity,
  operation identity, target instance/request, lease token/deadline, and delivery
  state. Claim, binding, and acknowledgement must check the current lease token;
  an expired worker cannot change a new worker's binding or remove its message.
- Keep an inventory of migrated submission/discovery callers in the change
  description. Search for raw custom-signal sends, event reconstruction,
  `has_pending_input`, and queue pops at the end, and classify each remaining
  use as historical display, unmanaged signal handling, or a missed consumer.

### Implementation tasks and handoffs

The checklist below makes the remaining packages executable. An unchecked item
is a delivery requirement, even if part of its implementation already exists in
the worktree. The earlier checkpoint records historical checks; rerun affected
checks after subsequent edits rather than treating those results as approval of
the final change.

#### Runtime completion (packages A–C)

- [ ] Finish the admission inventory before enabling more callers. For each
  supported execution path, record the registration host, logical owner,
  physical lease, signal-identity producer, and cancellation/settlement hook.
  Add a compiled test for each supported row; reject unsupported managed waits
  explicitly at validation or admission.
- [ ] Complete closed-state lowering in `direct_wasm/compile/wait.rs` and shared
  error construction in `runtara-workflow-stdlib/src/direct_json.rs`. Use
  `WAIT_TIMEOUT` for expiry, `WAIT_ABANDONED` for abandonment, `WAIT_CANCELLED`
  for invocation cancellation, and `WAIT_CLOSED` for other closure reasons.
  Preserve the underlying reason and signal identity in error attributes.
  Apply the same rules to timed/untimed standalone waits and AI wait tools.
- [ ] Close a registered wait before a handled runtime error leaves that wait.
  If durable closure fails, prevent the invocation from reporting successful
  recovery with an orphaned open request; propagate the failure and fence
  further managed writes. Verify this through both composed and isolated child
  paths. Normal suspension remains resumable and must not close the request.
- [ ] Add real compiled regressions: untimed closure must stop polling;
  `onError` may enter a second wait while the first stays closed; acceptance
  before expiry returns its payload even when replay happens after expiry;
  cancelling a waiting child leaves a sibling actionable. Repeat registration
  during replay and assert that request IDs and original deadlines are stable.
- [ ] Migrate every integration-test runtime host to the new managed operations
  where its workflows wait for input. Tests must exercise registration/state
  transitions rather than return canned success from unsupported imports.
- [ ] Deduplicate diagnostics at persistence using a stable key derived from
  tenant, instance, request ID, and transition kind. An idempotent diagnostic
  insertion must atomically enforce uniqueness; a guest checkpoint or an
  in-memory `already_emitted` flag is insufficient across crashes. Keep this
  optional diagnostic operation separate from mandatory registration and
  acceptance. Failure may leave a missing diagnostic, never an invalid request
  or a second logical event on retry. Add a forward migration if the event
  store needs a uniqueness key, and test both insertion/acknowledgement races.

#### Consumer migration (packages D and F)

Use the following inventory as the review checklist. Paths are relative to
`crates/runtara-server/src/` unless stated otherwise.

| Location | Required change | Regression proving the migration |
| --- | --- | --- |
| `api/services/pending_inputs.rs` | Replace actionable event matching with tenant-aware managed discovery; keep any historical matcher separate. | Removing all debug events does not change actionable results. |
| Core `InputRequests`, both stores, SDK and runtime-client adapters | Add batched existence using the same eligibility predicate as request listing; preserve typed failures through each adapter. | Running and suspended roots agree with listing; a backend failure fails the batch. |
| `workers/runtara_dto.rs` and all three `execution_engine.rs` enrichment callers | Return and propagate `Result`; batch authorized IDs rather than issue per-instance event reads. | Listing never reports `false` merely because discovery failed. |
| `api/services/workflow_runtime.rs` and `api/handlers/step_events.rs` | Pass tenant ownership explicitly, page requests after workflow association filtering, submit by request/operation identity. | A request beyond the first execution page is counted and can be submitted; foreign ownership remains undisclosed. |
| `api/services/reports.rs` and `api/services/reports/providers/workflow_runtime.rs` | Authorize the report/block target without requiring the action to remain in the current open-results set; use shared receipt replay. | An accepted report submission with a lost acknowledgement succeeds again after completion. |
| `api/handlers/sessions.rs` and `channels/session.rs` | Remove both latest-event target selectors; use zero/one/many eligible-target outcomes and the queue protocol. | Ambiguous targets retain the message; stale targets do not consume it. |
| `mcp/tools/signals.rs` and tool descriptions in `mcp/server.rs` | Migrate managed actions to request IDs and stable operation IDs; preserve raw signals only for unmanaged addresses. | A retried tool call returns the same receipt without a second delivery. |
| Frontend workflow action forms, report `ActionsBlock.tsx`, report queries, and session forms | Share one submission/retry model and regenerate runtime API types. | A timeout followed by retry reuses the original operation ID and payload; stale conflicts refresh the action. |

Before frontend migration, implement the following proposed public mapping in
one shared adapter. Keep these codes in OpenAPI and assert them in endpoint
tests; do not make clients parse backend error strings.

| Outcome | HTTP status / public code | Client behavior |
| --- | --- | --- |
| First acceptance or matching receipt replay | `200`, identical `{ receiptId, requestId, acceptedAt }` | Mark accepted; do not infer workflow completion. |
| Missing or foreign-owned target | `404 / INPUT_NOT_FOUND` | Stop submission and refresh authorized state. |
| Invalid envelope/operation identity | `400 / INPUT_INVALID_REQUEST` | Correct the request; do not automatically retry. |
| Schema-invalid payload | `400 / INPUT_INVALID_PAYLOAD` | Retain editable form values and show validation details. |
| Closed, expired, or terminal target | `409 / INPUT_INACTIVE` | Refresh/clear the stale action; never start a replacement run automatically. |
| Another operation already answered | `409 / INPUT_ALREADY_ANSWERED` | Refresh the action; do not overwrite the accepted response. |
| Operation ID reused with a different target/payload | `409 / INPUT_OPERATION_CONFLICT` | Surface the conflict; allocate a new ID only for an intentional new submission. |
| Backend unavailable or outcome uncertain | `503 / INPUT_UNAVAILABLE` | Retry with the identical operation ID, request ID, and payload. |

Internal fence, registration-identity, and raw-address conflicts stay typed on
internal transports. Do not expose execution fences or storage error details
through the public submission response. Resolve ownership before returning any
receipt or conflict information.

For report submissions with implicit viewer fields, distinguish the caller's
stable submitted payload from mutable presentation data. Define a deterministic
effective payload and preserve it across retries; replay must not recompute
time-dependent defaults or depend on the action still being discoverable.
Otherwise the same caller operation could conflict with its own accepted
receipt. Include an implicit-payload retry regression.

Implement the remaining consumer work in the following order. These are planned
changes, not additional completion claims. Frontend paths below are relative to
`crates/runtara-server/frontend/src/features/workflows/`.

1. **Finish the pending-input and signal contracts.** In
   `api/services/pending_inputs.rs`, expose the shared
   `{ instanceId, pendingInputs, count }` result, with an opaque `requestId` on
   each item. Keep `signalId` diagnostic only. Finish the draft handlers in
   `step_events.rs` and `sessions.rs`, preserving workflow/session authorization
   before discovery and tenant authorization before receipt lookup. Make all
   dependency failures use the sanitized public error mapping, including Valkey
   session lookups. Register the session response and all signal error responses
   in OpenAPI. Require `{ requestId, operationId, payload }` without aliases for
   `signalId` or `checkpointId` and without generating operation IDs on the
   server. Extend `tests/managed_actions.rs` for the signal and pending-input
   handlers; add session tests with isolated Valkey metadata.
2. **Migrate MCP signal tools.** Change schema lookup in `mcp/tools/signals.rs`
   to select by `requestId` from authoritative pending inputs. Require request
   and operation IDs for managed signal responses, and update descriptions in
   `mcp/server.rs`. Test missing identities, foreign requests, stale requests,
   and a repeated call after completion. Review remaining raw-signal tools
   separately so unmanaged custom signals keep their existing semantics.
3. **Migrate execution forms together.** Update `queries/index.ts`,
   `components/ExecutionPanel/HumanInputCard.tsx`,
   `components/ExecutionTimeline/index.tsx`, and
   `pages/WorkflowHistory/index.tsx`. Key forms by request ID and use the existing
   `utils/input-submission.ts` tracker to snapshot the payload and allocate one
   operation ID per intentional response. Keep the tracker alive across mutation
   retries and rerenders. Decode typed public errors instead of throwing away
   their codes. Keep values on validation/network errors; refresh and remove
   stale actions on inactive/already-answered conflicts. Show operation conflicts
   without automatically allocating another ID. Regenerate the runtime client
   after settling the Rust contracts, then compile every caller against it.
4. **Restore chat from authoritative discovery.** Replace the flat
   `hasPendingInput` expectation in `queries/chat.ts` with the actual response
   envelope and pending-input array. Add request identities and a pending-request
   collection to `types/chat.ts` and `stores/chatStore.ts`. Retain an explicit
   selection while it remains eligible; select automatically only when exactly
   one request exists. For several requests, render a target chooser in
   `components/ChatInput/index.tsx`. Clear the collection and selection when
   switching sessions or executions.
5. **Separate chat history from actionable state.** In `pages/Chat/index.tsx`
   and `pages/Chat/useChatStream.ts`, stop restoring forms from the last
   `waiting_for_input` event. Preserve those events as history. Fetch current
   requests on connect/reconnect, after submissions, and periodically while an
   execution can wait; optional debug events may trigger refresh but cannot be
   its only trigger. Use one non-overlapping poll with bounded backoff, cancel
   it on teardown, and ignore responses for an obsolete session/instance or
   refresh generation. A failed refresh must display unavailable/unknown state,
   not clear the list as if discovery succeeded. A successful empty result
   clears stale selections without changing historical messages.
6. **Make chat submission retryable.** Use the same tracker in
   `components/ChatInput/ChatFormInput.tsx` and the plain-text response path in
   `useChatStream.ts`. Submit an explicitly selected request directly through
   managed acceptance; ordinary unbound chat messages enter the queue protocol
   below. Preserve text, target, payload snapshot, and operation ID after an
   uncertain result, and disable duplicate clicks while the request is in
   flight. Clear accepted input only after a receipt, then refresh discovery.
   Do not turn an ambiguous or stale reply into a new execution. State the
   retry guarantee explicitly: the current in-memory tracker covers a mounted
   client session; surviving a browser reload requires separately retained
   operation state and must not be claimed without implementing it.
7. **Resolve report retry preparation.** `merge_report_action_payload` currently
   evaluates implicit viewer fields against the current block and authentication
   context on every attempt. Persist enough acceptance context to distinguish
   the canonical caller payload from the immutable effective payload actually
   accepted. Bind that context to tenant, instance, operation, request,
   report/block, and submitting principal. Store it atomically with acceptance,
   rather than in a best-effort write afterward. On an authorized matching
   retry, compare the original caller payload and return the retained receipt
   before reevaluating mutable defaults. Continue checking current access;
   receipt replay must not bypass revoked permissions. Changed caller payload,
   request, or submission context conflicts. Do not expose accepted payloads
   through the receipt API. Add the narrow persistence extension and forward
   migration needed for this context, preserving direct-submission semantics.
8. **Remove obsolete actionable helpers.** Once all callers are migrated, remove
   `has_open_inputs`, bounded event-fetch helpers used only for discovery, and
   both `find_pending_signal_id` implementations. Preserve the pure historical
   matcher and its identity tests only where history still needs them. Audit
   every remaining signal send and waiting-event consumer before closing this
   package.

The consumer regression suite must establish these outcomes:

| Scenario | Required assertion |
| --- | --- |
| Suspended wait with no debug events | Instance API, session API, action/report lists, and pending flags agree on eligibility. |
| Missing or foreign session/workflow/request | No receipt, payload, or existence information leaks across ownership boundaries. |
| Malformed JSON or missing operation ID | Every submission surface returns the same typed invalid-request outcome. |
| Accepted response followed by completion and lost acknowledgement | Direct, signal, MCP, and report retries return the original receipt. |
| Report defaults change after acceptance | An otherwise authorized identical retry returns the original receipt; edited caller input conflicts. |
| Report/block access is revoked | A prior receipt does not bypass current authorization. |
| Two chat requests or a stale historical waiting event | No implicit latest-event selection; an explicit eligible target is required. |
| Out-of-order refresh or session switch | An older response cannot replace the current execution's pending requests. |
| Discovery outage or uncertain submission | UI retains the draft and retry identity and visibly reports the failure. |

#### Queue and release handoff (packages E and F)

- [ ] Introduce typed envelope/lease/target records and atomic queue operations
  before replacing delivery loops. Exercise scripts against isolated Valkey,
  including expired-worker bind/ack attempts after another worker reclaims.
- [ ] Replace every `pop_event` delivery caller together with its error handling.
  Use one delivery routine for sessions and channels so target selection,
  binding, retry, and acknowledgement cannot drift between them.
- [ ] Prove the crash boundary end to end: accept in PostgreSQL, lose the HTTP
  acknowledgement, reclaim the Valkey lease in a second worker, replay the same
  receipt, and acknowledge exactly that envelope. Assert that a newly opened
  wait receives no duplicate message.
- [ ] Regenerate contracts only after the Rust DTO/error mapping is settled;
  update MCP descriptions and consumer tests in the same reviewable change.
- [ ] Run the combined Phase 7 checks and record commands, feature gates,
  results, and any skipped prerequisites. Audit remaining event reconstruction,
  raw managed-address writes, queue pops, and missing operation IDs.
- [ ] Complete the coordinated rollout inventory described below. No legacy
  adapter or event backfill is required under the unused-feature assumption.
  Keep activation blocked until runtime, consumers, queue delivery, and
  generated clients all satisfy their exit criteria.

Implement queue recovery as a service independent of an attached SSE stream.
One shared delivery routine should accept a tenant/session and transport-neutral
envelope, then claim, resolve or reuse the target, bind, submit, and acknowledge.
Session and channel adapters supply routing/presentation context; they must not
implement separate acceptance or lease rules. Arrange bounded worker retries
and startup recovery so a disconnected client or quiet channel cannot strand an
already queued message.

In addition to the Phase 6 operations, include these implementation details:

- Define typed delivery states: queued, leased, retry-scheduled, blocked,
  accepted, and explicitly failed. Store the original payload, stable message
  and operation IDs, enqueue time, optional immutable target, retry count/time,
  and reason code. Lease tokens and deadlines are server-owned fields. Return
  structured outcomes for no target, ambiguity, schema mismatch, stale target,
  lease loss, and backend failure.
- Use tenant/session-scoped keys with an encoded or hashed session hash tag;
  caller-provided braces must not control the cluster slot. Keep enqueue
  deduplication, pending order, envelope storage, lease, and outcome transitions
  within the script's slot. Provider deduplication and enqueue must be atomic;
  a reservation created before a failed enqueue cannot count as delivery.
- Remove undelivered-message expiry **and** the independent one-hour expiry of
  routing metadata in `set_session_meta`. Retain the authorized session binding
  while any unresolved envelope requires it. A bound envelope preserves its
  instance/request even if the session later points to a replacement execution.
  Unbound messages follow the explicit session-routing policy, never an inferred
  target from expired or missing metadata.
- Expose blocked/failed outcomes through the session API/UI and channel owner
  status path. Provide explicit resolution operations to choose an unbound
  target or fail the message. Resolution rechecks ownership and uses a
  conditional state transition; a concurrently leased message cannot be edited.
  A bound message cannot be silently retargeted. Its deliberate replacement
  receives a new message and operation identity.
- Test recovery without a live SSE subscriber, loss of session metadata,
  provider redelivery after enqueue failure, expired-worker writes, and a
  blocked FIFO head. Add the PostgreSQL-acceptance/Valkey-ack crash test only
  after individual script tests pass. Document completed-message retention and
  actual Valkey persistence configuration as limits of the guarantee.

Review and land the work in the package order above, keeping tests with each
behavior change. After runtime packages A–C, split consumer work into contracts
and endpoint tests, execution/MCP clients, chat discovery/submission, and report
retry context. Follow with queue primitives, shared delivery/recovery, and final
integration verification. These review units may be separate changes, but all
remain part of one coordinated release; none closes the overall acceptance
checklist on its own.

### Phase 1: core contract and persistence

Primary locations: `crates/runtara-core/src/persistence/`, its in-memory backend
and conformance suite, and `crates/runtara-store-postgres/src/`.

1. Add domain types for a registered request, invocation ownership, closure
   reasons, acceptance receipt, and typed outcomes. Distinguish not found,
   identity mismatch, closed/expired, already answered, idempotency conflict,
   invalid payload, and storage failure. Do not collapse storage errors into an
   empty list or a conflict.
2. Add persistence operations for idempotent registration, authorized lookup,
   paged actionable discovery, batched existence checks, receipt lookup,
   conditional acceptance, non-destructive accepted-response reads, and
   conditional closure. Give timeout closure a result that can return an
   already accepted response; it must not independently overwrite acceptance.
3. Add a forward PostgreSQL migration using the next available migration number.
   `src/migrations.rs` embeds the migration directory with `sqlx::migrate!`;
   verify the new migration is included rather than adding a manual registry.
   Do not alter committed migrations.
   Proposed `instance_input_requests` data:

   - A bounded opaque request key, tenant/instance ownership, full signal
     identity, logical invocation identity, and current fence association.
   - Immutable schema, schema format/revision, step/tool display metadata,
     action key, correlation, context, registration time, and optional deadline.
   - State, closure reason/time, accepted operation ID, canonical accepted
     payload, acceptance time, stable receipt ID, and pending-wake metadata.

   The accepted fields on this row are the receipt; a separate receipt table is
   unnecessary for one accepted response per request. Enforce unique operation
   IDs per tenant/instance and valid state/field combinations.
4. Follow the existing invocation store's lock order: tenant-owned instance row
   first, invocation state next when needed, request rows last in stable order.
   All registration, acceptance, raw writes, closure, and terminal transitions
   must participate. Mirror atomicity in the in-memory backend with one critical
   section, rather than a sequence of independently locked maps.
5. Use a bounded digest for indexing opaque signal/invocation identities where
   their full length exceeds PostgreSQL btree limits, retaining and comparing
   the full value. Detect a digest collision explicitly. Reuse the existing
   invocation-path identity rules instead of introducing string-prefix tests
   for ancestry. Index tenant/instance/state and actionable pagination fields.
6. Make registration replay-safe: return the existing state when immutable
   metadata matches; reject conflicting metadata; never reopen accepted or
   closed rows. Fence transfer after legitimate suspension/replay may update
   execution ownership, but an old fence cannot mutate the request. A new
   semantic invocation must have a different request identity; verify how
   invocation retry generations map to the compiler's existing signal IDs.
7. For a raw mailbox value that predates registration at the same address,
   return an explicit registration conflict. Do not silently import, delete, or
   accept that unvalidated payload. Serialize raw writes with registration so
   this rule holds in both race orders.
8. Keep request/receipt deletion coupled to instance deletion, not event cleanup.
   Add migration registration/version checks and in-memory/PostgreSQL
   conformance cases before wiring runtime consumers.

Exit criterion: both backends implement the same state machine, including
deterministic races, without a workflow or HTTP handler being involved.

### Phase 2: atomic acceptance and lifecycle arbitration

Implement the shared acceptance service over the new persistence contract.
Keep schema validation reusable below HTTP handlers; preserve both supported
schema formats instead of assuming every request contains JSON Schema.

The submission sequence is:

1. Validate the envelope and authorize tenant, instance, workflow, and request
   ownership. Do not call DTO enrichment or require a currently open action just
   to establish ownership.
2. Look up the operation receipt. If request identity and canonical payload
   match, return the original receipt immediately, even after termination. A
   reused operation ID with a different request/payload is a conflict.
3. For a new operation, load the immutable registered schema and validate the
   payload. Pass a validated response carrying the request/schema revision into
   persistence, rather than exposing an unchecked acceptance path to callers.
4. In one transaction, lock the tenant-owned root and request, repeat receipt
   lookup to resolve concurrent retries, and verify schema revision, instance
   eligibility, logical invocation validity, request state, and deadline.
   Sample database time after acquiring the locks, so waiting for a lock cannot
   admit an expired response using a transaction-start timestamp.
5. Atomically change `open` to `accepted`, store the immutable payload/receipt,
   and persist the wake intent described in Phase 4. Commit before acknowledging
   success. Another operation against an accepted request conflicts even if its
   payload happens to be equal.

Receipt lookup inside the transaction is still required when pre-validation
appeared to fail: a concurrent matching operation may already have succeeded.
Only new acceptance needs current liveness/schema checks. Return typed outcomes
through every adapter; retrying a receipt must not create another wake intent.

Integrate closure at these persistence boundaries:

- Root completion, failure, cancellation, and terminal timeout close all open
  requests in the same transaction and clear wake intent. Accepted receipts
  remain readable. Include command acknowledgement, cancellation backstops, and
  administrative terminal paths, not just the ordinary completion handler.
- Invocation cancellation/settlement closes open requests owned by that logical
  invocation and abandoned descendants while allowing siblings to continue.
  Extend `runtara-store-postgres/src/invocations.rs` and the corresponding
  in-memory transitions. A resumable suspension or temporary lease release is
  not abandonment; it must preserve the logical request across restart.
- A timed-out or abandoned wait closes itself before routing to `onError` or
  continuing another branch. Deadline expiry makes it non-actionable even if
  the workflow has not resumed to publish closure. A bounded cleanup pass may
  materialize expired state, but correctness cannot depend on that pass.
- If acceptance committed before timeout closure, the closure operation returns
  the accepted payload and the wait follows its response path. If closure won,
  late submission conflicts. Persistence decides the race, not guest clock skew
  or the eventual order of debug events.

Exit criterion: acceptance, every terminal path, per-wait timeout, and invocation
cancellation share one serialization rule, with no partial receipt/payload write.

### Phase 3: mandatory runtime request lifecycle

Primary locations: the canonical workflow runtime WIT, `RuntimeHost` in
`runtara-component-host`, `runtara-environment/src/runtime_host.rs` and its scoped
implementations, `runtara-workflow-runtime`, `runtara-sdk`, and the direct WASM
compiler/stdlib.

1. Add typed runtime operations for registering a request, reading its accepted
   response/state, and conditionally closing it. Do not interpret arbitrary
   `custom-event` JSON as privileged control writes. The host supplies tenant,
   root, invocation ownership, and fence from its execution context; guest
   arguments cannot impersonate another invocation.
2. Thread the operations through native host imports, scoped root/child hosts,
   deferred-terminal wrappers, the published-agent capability bridge, the runtime
   component and SDK/internal transport used by CLI execution, and test hosts.
   Search all `RuntimeHost` implementors,
   generated import indices, linkers, and ABI registries. Update canonical WIT
   sources and regenerate dependent artifacts through the repository tooling.
3. Update both standalone wait lowering and `emit_ai_wait_tool_arm` in
   `direct_wasm/compile/wait.rs`. Register when the wait becomes available, before
   the first response poll or park, regardless of `track_events` or workflow ABI.
   Preserve `onWait` ordering and nested-wait local state. Do not expose the outer
   wait before it is ready merely to simplify registration.
4. Reuse full deterministic signal identity for the logical request, and include
   actual AI tool/call and invocation metadata in the registration. Carry that
   identity into diagnostic request/end events as well. Existing AI waits have
   no deadline; preserve that behavior unless timeout support is separately
   added. Timed standalone waits reuse their original durable absolute deadline
   and the registration result, never mint a fresh deadline on replay.
5. Poll the immutable accepted-response record for managed waits instead of the
   raw mailbox. Return the same bytes after restart and after a checkpoint miss.
   Preserve normal raw polling for unmanaged signals. Recording consumption may
   aid diagnostics but must not delete the response needed for replay.
6. Replace the guest-only timeout decision with the conditional close/read
   outcome from Phase 2. Close on handled failures and exits that abandon an
   already registered wait; use host invocation/root closure as the backstop for
   traps, cancellation, and unwinding. Do not close on ordinary suspension,
   explicit pause, or host shutdown intended to resume.
7. Preserve `external_input_requested` and useful closure/acceptance diagnostics
   for history, with stable request IDs and replay deduplication. A diagnostic
   write failure must not undo or obscure a successful durable transition. No
   discovery consumer may depend on receiving that diagnostic event.

Exit criterion: compiled standalone, nested, published-agent, and AI tool waits
work with debug tracking disabled, and replay returns the same request/response.

### Phase 4: wake scheduling and raw-signal boundary

Primary locations: `runtara-environment/src/handlers.rs`,
`runner/embedded.rs`, wake scheduling, and core/store lifecycle operations.

1. Keep acceptance payloads in the request record, and reject raw writes to every
   registered managed address at persistence, not only in HTTP. Audit server
   signal handlers, MCP, runtime clients, and SDK delivery adapters. The raw HTTP
   path must also verify tenant ownership before delegating.
2. Acceptance of a parked request must persist its scheduler wake deadline in
   the acceptance transaction, conditional on the root still being suspended
   for that request. Factor the scheduler SQL into a transaction-aware helper;
   the existing read-status-then-`schedule_wake` sequence is insufficient for a
   pause racing the write.
3. Persist the identities of waits responsible for a park alongside the park
   state, so an unrelated accepted request cannot wake a different suspension.
   Preserve the existing signal-before-park reread, extended to managed response
   records. Check for already accepted responses in the park transaction where
   possible, so acceptance followed by a process crash cannot strand the wait.
4. Persist a retryable wake intent with acceptance. Add bounded reconciliation
   for acceptance-before-park and failed wake scheduling; clear the intent only
   after the matching wake is durably scheduled, the response is consumed, or
   the owning wait/root can no longer resume. Reconciliation and scheduler
   claims must remain idempotent. Do not rely on a caller retry to repair wakes.
5. An explicitly paused or breakpoint-stopped root retains accepted data without
   an automatic wake. Pause/terminal transitions atomically invalidate obsolete
   scheduled wakes. Explicit resume permits response consumption and normal
   reconciliation. An inactive runner lease during a durable park alone must
   not make the logical request invalid.

Exit criterion: fault injection after acceptance commit, during park, and before
scheduler notification cannot lose a response or bypass a pause. Preserve the
existing scheduler's single-claim protection against duplicate launches.

### Phase 5: discovery, public contracts, and clients

1. Replace actionable reconstruction in `api/services/pending_inputs.rs` with
   tenant-aware request discovery through the runtime client/environment facade.
   Use the same eligibility predicate for list and existence queries: root
   non-terminal, valid logical invocation, state open, deadline not elapsed.
   Verify ownership even when the expected result would be empty.
2. Update `list_instance_actions`, `list_workflow_actions`, and
   `submit_workflow_action` in `api/services/workflow_runtime.rs`. Map stored
   metadata into action DTOs; do not require a successful instance-list flag or
   an event lookup to submit a known request.
3. Page workflow-wide results by requests after workflow/tenant eligibility
   filtering, not by instances. Preserve the current offset/size contract but
   compute `total_count` and `has_next_page` over all matching requests, with
   deterministic ordering by registration time and bounded request key. Count
   and page share one database snapshot. If workflow association resides outside
   core tables, add an explicit batched association/filter query rather than
   scanning only the first execution page. Pagination across calls remains a
   changing snapshot; document that concurrent answers can move offset results.
4. Replace `enrich_pending_input` with a batched authoritative existence query
   covering eligible running and suspended instances. Change its signature and
   callers to propagate failure; terminal rows can return false directly.
   Avoid one event-history query per execution.
5. Migrate per-instance action handlers, session pending-input endpoints, report
   providers, channels, and MCP action consumers to the same service. Keep the
   low-level event matcher isolated to diagnostics; if it remains exposed there,
   page its complete available input and mark retention gaps as incomplete.
6. Add required `operationId` to submission contracts and return a stable receipt
   with request ID and acceptance time. Map unauthorized/missing ownership to the
   existing non-disclosing not-found behavior, malformed/schema-invalid input to
   400, stale/answered/idempotency conflicts to 409 with machine-readable codes,
   and storage failures to a retryable service error. A receipt replay returns
   the same successful result; success promises acceptance only.
7. Regenerate OpenAPI/runtime TypeScript with
   `generate-api-runtime-offline`. Search frontend action forms, report actions,
   session forms, and SDK/MCP adapters for all submission call sites. Create the
   operation ID once per user submission and retain it across uncertain network
   retries. On a stale-request conflict, refresh/clear the form; never silently
   start a replacement run. An intentional different response uses a new ID.

Exit criterion: every actionable surface agrees, including pending-input flags,
and request counts remain correct beyond the old event and execution page caps.

### Phase 6: reliable queued chat/channel delivery

Primary locations: `api/services/session_queue.rs`, `api/handlers/sessions.rs`,
and `channels/session.rs`.

1. Replace destructive `LPOP` delivery with an atomic claim/ack protocol. Keep
   Valkey as the queue backend: use an atomic script to move an envelope into
   an in-flight collection with a lease token and deadline, plus reclaim and
   token-checked acknowledgement operations. Preserve FIFO for each session
   with one active delivery lease. Do not emulate a claim with separate pop and
   push calls.
2. Give every envelope a stable message ID and operation ID at enqueue time.
   Use a stable upstream event identity for provider redelivery where available.
   Surface corrupt envelopes; never deserialize them to an apparent empty queue.
3. Discover verified requests before binding a message to a target. Bind the
   chosen request durably in the in-flight envelope before submission, and reuse
   that target and operation ID on redelivery. A lost acknowledgement must not
   retarget the same message to the next wait after the first was answered.
4. Submit through the shared acceptance service and acknowledge the queue only
   after accepted/replayed success. Reclaim expired leases after crashes. If
   acceptance committed but queue acknowledgement failed, replay the receipt
   and acknowledge without another delivery or wake.
5. On no target, ambiguity, stale request, schema mismatch, or temporary failure,
   retain the envelope with an explicit retry or user-resolution state. Do not
   busy-loop permanent conflicts or silently feed the message into another
   execution. Preserve explicit terminal-session routing separately from retries
   of a message already bound to a request.
6. Replace the current silent one-hour expiration for undelivered messages with
   a documented failure policy: queued/in-flight envelopes remain until accepted
   or explicitly failed, with failure visible to the session/channel owner.
   Apply retention only after that outcome. State the existing Valkey durability
   limits; this change does not make Valkey and PostgreSQL one transaction.

The current session/channel idle loops also start replacement executions. Move
that routing into the shared delivery worker without turning a delivery retry
into a new run. For an unbound message whose explicit routing policy permits a
new execution, persist a launch intent (stable proposed instance ID, workflow
version, and launch input snapshot) under the current queue lease **before**
calling the execution engine. Reuse the engine's instance/idempotency contract
when recovering an uncertain launch, and conditionally publish the resulting
session route under the same lease. A response already bound to an input request
must skip this path and retry its original receipt, even if the session route or
root status changed. Distinguish workflows receiving a message as initial input
from workflows whose message still needs managed-wait acceptance; an execution
launch is not an input-acceptance receipt.

Implement the queue changes in this order: define the envelope and typed outcomes;
add atomic scripts and isolated Valkey tests; migrate session delivery; migrate
channel delivery; then remove destructive pop callers. Keep all keys used by one
script in the same session hash slot where clustered Valkey is supported. Use
backend time for lease deadlines. Each transition must preserve the envelope's
original message ID, operation ID, and any previously bound target.

| Operation | Atomic precondition and effect |
| --- | --- |
| Enqueue | Deduplicate a stable upstream/message identity and append one envelope without an undelivered-message expiry. |
| Claim | Lease the oldest eligible message only when the session has no live delivery lease; return an opaque lease token. |
| Bind | Require the current unexpired lease; set the target once, or return the identical existing binding. Reject a different binding. |
| Renew/reclaim | Renew only the current unexpired token, or replace an expired lease atomically; preserve the binding and message order. |
| Acknowledge | Require the current unexpired token and successful acceptance/replay; retain enough completed identity to deduplicate supported redeliveries. |
| Retry/block/fail | Require the current lease and record a reason plus retry time or visible resolution state; never discard the envelope implicitly. |

Resolve a blocked head message explicitly before delivering later session
messages, preserving the stated FIFO policy. Bound retries for transient errors
with backoff; ambiguity and schema/stale-target conflicts require resolution
instead of repeated submission. A bound stale request may be explicitly failed,
but must never be rebound silently. A deliberately new delivery uses a new
message/operation identity. Document the deduplication retention window alongside
completed-message retention; do not promise indefinite provider deduplication.

Exit criterion: two delivery workers and crashes on either side of acceptance
cannot lose a message, deliver it to two requests, or consume it on conflict.

### Phase 7: verification and delivery

Add tests with each phase, then run the combined acceptance matrix before
enabling the new consumer path. Prefer barriers and fault injection over timing
sleeps in concurrency tests.

| Layer | Required checks |
| --- | --- |
| Core/store conformance | Registration replay/conflict; immutable accepted bytes; operation reuse; both ordering outcomes for accept/close/cancel/raw-write races; cross-tenant denial; deadline boundary; receipts after terminal state; memory/PostgreSQL parity. |
| PostgreSQL integration | Independent connections contending on the same root; rollback between receipt and wake writes; exact count/page snapshots; deletion/retention; long identities; migration registration. |
| Compiled runtime | Tracking on/off; root and published child waits; repeated AI tools, loop iterations, embedded call sites; nested `onWait`; timeout with `onError`; child cancellation with sibling continuation; replay and restart. |
| Environment | Signal-before-park in both orders; accepted-before-deadline response read after deadline; explicit pause racing acceptance/wake; scheduler restart; failed launch retry; stale fences unable to register/close another attempt's request. |
| API/reports/frontend | Missing/foreign/terminal instances; suspended waits; schema formats; lost-ack retry after cancellation; counts beyond 100 executions and 1,000 historical ends; lookup failure propagation; stale form recovery. |
| Queue/channels | Competing claims; stable target on redelivery; crash before/after acceptance; failed ack; lease reclaim; ambiguity; corrupt envelope; stale target; explicit failure retention. |

Use isolated PostgreSQL databases and Valkey instances. After focused tests pass,
run the relevant CI commands with the feature gates that actually include these
targets:

```sh
cargo fmt --all -- --check
cargo test -p runtara-core --features test-support -- --test-threads=1
cargo test -p runtara-store-postgres --features db-integration-tests -- --test-threads=1
cargo test -p runtara-sdk
cargo test -p runtara-workflows
cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1
cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests -- --test-threads=1
scripts/build-agent-components.sh
cargo test -p runtara-component-host --features component-integration-tests --tests
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute
cargo test -p runtara-environment --features scoped-workflow-integration-tests --test scoped_runner_test --test cooperative_stop_test --test managed_inputs -- --test-threads=1
```

Use the component-directory setup and service prerequisites from CI, including
storage emulators for the full component-host suite. Run the feature-gated
Clippy matrix from `.github/workflows/ci.yml`, and frontend tests,
lint, and build from `crates/runtara-server/frontend`. Regenerate SQLx metadata
only if compile-time checked SQL changes. Report actual results and any missing
services; these commands are planned verification, not checks already run.

Suggested reviewable changes, in dependency order:

1. Core request contract, forward migration, both persistence backends, atomic
   acceptance/closure/raw-write protection, and conformance tests.
2. Runtime/WIT/compiler lifecycle, fenced ownership, park/wake recovery, rebuilt
   components, and compiled integration tests.
3. Shared discovery/submission, API/report/MCP/frontend migration, generated
   contracts, and stable queue delivery with Valkey integration tests.
4. Remove obsolete actionable event reconstruction and document deployment and
   operational behavior after the combined acceptance suite passes.

Treat these as dependent changes, not independently deployable partial fixes.
Apply the additive migration first, deploy the new runtime/host/compiler and
server artifacts together, and recompile affected workflow images. The release
assumption is that there are no existing users or active waits requiring the old
feature contract; verify that assumption as a release check. No legacy backfill,
dual-read period, or compatibility adapter is planned. If the inventory disproves
the assumption, stop activation and revise the rollout instead of inferring
authoritative requests from debug history or resetting live executions. No
destructive cleanup is part of this plan. Reverting application code after new
managed requests exist needs a coordinated drain/recovery procedure; an old raw
writer is not a safe rollback.

Add bounded operational counters for acceptance/replay/conflict, request closure,
discovery failure, wake reconciliation backlog, and queue lease/retry age. Log
identifiers and reason codes, not response payloads. Completion means all
actionable consumers use this contract and the acceptance criteria above pass;
the Control agent itself remains a separate follow-up.

## Scope boundary

Complete these lifecycle, discovery, and acceptance guarantees before exposing
pending-input capabilities through [Control](control-agent.md).
