# Consolidate Monitor into Runs

Status: implemented on `feat/operations`, 2026-09-29.
Based on the implementation at `937a514f` on `feat/operations` (PR #283).
This updates the navigation in `operations.md`; its Monitor screenshot
remains a reference for information to preserve, not a separate destination.

## Outcome

Operations has three navigation items, in this order:

- **Overview:** where should I look?
- **Queues:** what needs an answer?
- **Runs:** what happened, what is happening, and what can I do about it?

Runs becomes the single execution investigation page. Keep one paginated table;
do not place a second failure or waiting list below it. Workflow-specific saved
run views and request queues remain available with their existing semantics.

## Baseline and gaps addressed

- Runs reuses `InvocationHistoryTable`. It already provides search, workflow,
  status and date filters, sorting, pagination, parent links, Replay, Resume,
  Stop, and detailed execution links. Preserve these capabilities.
- Monitor adds process counts, compact error summaries, a waiting list, a time
  selector and 30-second refresh. It makes four count queries per workflow.
- The execution API already supplies `error`, `errorSummary`,
  `suspensionReason` and `hasPendingInput`. The history adapter drops the error
  fields. Preserve and render them instead of fetching details per row.
- Runs synchronizes only some filters with the URL: date bounds and sort are
  currently omitted. Complete this before wiring Overview drill-downs.
- Monitor mixes creation-time counts, completion-time failures, and all-time
  waiting runs. Give each Runs query one explicit filter context.
- Waiting age is unavailable. Existing Monitor ages are measured from run
  creation, not entry into the current waiting state.
- Operations and workflow history have separate Replay components with different
  confirmation/navigation behavior. Consolidate the behavior used in Operations
  while preserving supported actions elsewhere.

## Runs experience

### Filters and counts

Use a compact row of quick filters above the existing table:

| Filter | Statuses |
| --- | --- |
| All | Every status, including queued, compiling and cancelled |
| Running | running |
| Waiting | suspended; show the specific suspension reason on each row |
| Failed | failed and timeout |
| Completed | completed |

Keep queued, compiling, cancelled and custom combinations accessible in the
existing detailed status filter. For a custom combination, show its active
filter explicitly rather than incorrectly highlighting All. Waiting includes
paused runs; it does not imply that every row needs an answer.

Counts reflect workflow, search, exact label, parent and date filters, but ignore
the selected status and pagination. Each chip shows what selecting it would
return. Use the same public status mapping as the list, including timeout and
queued mapping; do not count raw database status values differently.

Default to All statuses and All time to preserve the history page's coverage.
Offer Last 24 hours, Last 7 days, All time and custom dates, with an explicit
**Started / Completed** date basis. Started initially means the existing
`createdAt` field; document this as run creation rather than worker start.
Existing links with both date ranges remain representable as advanced filters.

Changing status does not silently change dates or sorting. Default sort remains
newest created first. Provide an Oldest first choice for finding long-lived
waiting runs. Overview failure links explicitly select Failed, Completed in
the last 24 hours, and newest completed first. Overview active-run links use
All time so old running/waiting runs remain visible.

Persist filter context and sorting in the URL, including relative range presets
and date basis. Resolve a relative range once per refresh cycle and pass the
same bounds to counts and rows. Reload and browser back/forward restore the
same context; changing filters resets pagination.

### Row content and actions

- Primary identity: business label, or shortened run ID when absent; workflow
  name underneath, and a way to copy the full ID. The primary link opens the
  Operations run page. Keep an explicit Open execution link for technical details.
- Status context: readable failure message or waiting reason. Put technical
  error code/category on the execution details page; avoid large colored error boxes.
  Handle structured errors, plain host errors and missing error details.
- Timing: preserve created/completed dates and execution duration. Label elapsed
  run age as **Started … ago**, never **Waiting for …**. Never show completion
  timestamps for active runs.
- Waiting requests: link to the Operations run page to review its actionable
  requests. Preserve the existing chat-input link when applicable. A boolean
  pending-input flag must not be interpreted as a queue/action-key identifier.
- Retain Resume only for resumable paused runs and Stop for eligible active
  runs. Preserve backend-authorized Replay behavior; retryability is explanatory
  metadata, not a new restriction on previously valid Replay actions.
- Operations Replay uses the existing confirmation explaining that it starts
  from the beginning and repeats side effects. Preserve the business label,
  prevent duplicate submits, show the resulting run and invalidate rows/counts.
  Apply `workflow:execute` to mutations and `invocation_history:read` to reads.

Add manual Refresh and a visible 30-second auto-refresh toggle, enabled initially.
Refresh rows and counts together, pause polling in background tabs, and retain
filters, page, scroll position and open confirmations. A failed background
refresh keeps the last successful data, marks it stale and offers retry. Failed
count requests display unavailable values rather than zero. Do not discard an
open row interaction if polling changes that run's position or status.

### Overview and compatibility

Remove the Processes area from Overview, including its workflow/stage summaries.
Do not replace it with Monitor's process table or per-workflow health counts.
Workflow inspection belongs in Runs, using its workflow filter and status counts.

Overview keeps concise top-level totals and Needs attention: actionable requests,
overdue requests where a view defines a due field, and recent failures. Use the
available space for attention items without introducing a second run browser.
Label time bases explicitly: current active work is all-time; recent terminal
outcomes use completion time. Run totals and failure links open Runs with matching
status/date filters; request items open the relevant queue or run request form.
Keep request counts distinct from run counts. Preserve workflow-specific saved
run views and access from Queues; only the Overview Processes area is removed.

Remove Monitor from the sidebar. Redirect `/operations/monitor` to
`/operations/runs`, preserving compatible query parameters and the hash; default
to All when no filters are present. Preserve `/invocation-history` redirects,
run detail routes, workflow history routes, and saved-view routes. Replace all
internal Monitor links and the header's hard-coded Monitor breadcrumb fallback.

## Implementation sequence (completed)

1. **Define shared query semantics and aggregate counts.** Add a tenant-scoped
   `POST /api/runtime/executions/summary` read endpoint reusing validated listing
   predicates and permission checks. Accept relevant non-status filters, return
   total and counts by public status. Workflow filtering is sufficient; no
   grouped per-workflow response is needed. Omit sorting/pagination and reject
   unsupported fields. Reuse
   the list's database predicates and status conversion; aggregate in the
   database without fetching every run or enriching individual errors. Overview
   may use separate aggregate calls for active work and recent outcomes, but
   request count must not grow with workflow count. Regenerate the API client.
2. **Complete Runs query state.** Add URL parsing/serialization for dates,
   relative presets and sorting; add quick filters, aggregate counts and refresh
   controls. Include tenant and full filter context in query keys. Preserve
   existing table pagination and advanced filters. Avoid a broad feature rename.
3. **Add operational row context.** Extend the adapter/types with failure
   metadata, render compact errors and waiting reasons, and connect run details.
   Reuse formatting/error helpers and consolidate Operations action behavior.
4. **Simplify Overview and remove Monitor.** Remove the Processes area and its
   dedicated queries/components. Retain attention items and top-level totals,
   wire filtered drill-downs, use aggregate counts, add the compatibility redirect,
   remove the unused Monitor page and update navigation, breadcrumbs,
   documentation and E2E routes.
5. **Validate the complete journey.** Run the checks below and inspect the local
   application against populated, empty, failed-refresh and read-only fixtures.

The summary endpoint adds API behavior but should require no data migration.
Counts and the separately fetched list can change between requests as workflows
advance. Both use common time bounds, without promising a cross-request snapshot.
Counts follow displayed statuses: compiling aliases queued, and timeout aliases
failed, matching the existing list API; totals never double-count these aliases.
Global authentication/maintenance handling still applies to 401/503 responses.

## Acceptance and verification

- Repository/API tests: tenant isolation and permissions; identical list/count
  filters; public status mapping for queued/timeout; inclusive date bounds;
  totals unaffected by page size or selected status.
- Frontend tests: URL round trips and redirects, custom status combinations,
  date-basis preservation, relative-range refresh, stale/error states, refresh
  toggle, permission-gated actions, correct waiting reasons and missing errors.
- Browser scenario: Overview failure count → matching Runs filter → readable
  error → Replay → new run with the same label. Cover paused versus other
  waiting reasons, pending-request links, parent filters and legacy redirects.
- Overview has no Processes area or per-workflow status table. Attention links
  still reach the correct queues/runs, and saved workflow views remain reachable.
- Layout checks at 1440, 1000 and 390 pixels: readable identity/error text,
  aligned actions, accessible filter controls, and no page-level overflow.
- Verify network request count stays bounded as workflows increase; no per-row
  failure fetches or per-workflow status-count requests. Investigate representative
  aggregate query plans before adding indexes.
- Run focused Rust tests for changed API/repository boundaries, formatting and
  lint; relevant frontend tests, lint and production build; local API and browser
  acceptance. Report any skipped broader CI checks.

## Deferred

True waiting duration, wait-age filtering and “stuck” thresholds need a durable
timestamp for entering the current suspension episode, reset on resume and
re-entry. Keep this separate from consolidation; do not approximate it from
`updatedAt`, creation time or the age of an individual request. No alerting,
notifications, infrastructure metrics or new saved-view schema in this change.

## Verification completed

- 1,373 frontend tests passed; after the final polling refinements, all 42
  invocation-history tests passed again. Production build passed.
- Frontend lint: zero errors, 29 existing warnings. Focused lint adds no warnings.
- Seven Operations database integration tests passed, including filtered totals,
  aliases, tenant boundaries, inclusive dates and exclusion of suspend timestamps
  from completion filters. 57 authorization tests and the summary request-shape
  test passed. Workspace formatting and focused Rust Clippy passed.
- Live API acceptance passed against the isolated local PostgreSQL/Valkey server
  and compiled workflows, including summary totals and invalid-filter rejection.
- Five Chromium browser tests passed: bulk answers and Replay; three layout
  widths (1440, 1000, 390); and Runs date-bound consistency, bounded request count,
  legacy Monitor redirects, history/reload, paused polling while reading errors,
  stale rows on refresh failure, unavailable counts, and recovery.
- Manually inspected the desktop and narrow-screen Runs layouts. Overview has
  no Processes area; failure totals link to the matching Runs time/status filter.
- Inspected an aggregate query plan against the local fixture: one grouped scan,
  with unnecessary image joins eliminated for a tenant/time-only query. This is
  a small-fixture check, not a production load benchmark; no new index was needed.

The complete CI matrix and unrelated external-service E2E suites were not run
locally. The waiting-duration work described above remains deferred.


## Runs presentation follow-up

The default table is Run, Status/context, Started, Duration and Actions. Business
labels and workflow names occupy two lines, with copy-ID in the right-hand actions.
Started uses relative time with an exact timestamp on hover. Duration is neutral;
there are no arbitrary speed colors. Eye, Replay and applicable live-run actions
remain visible.

Rows do not expand. The eye icon opens execution details for full errors and
metadata; exact timestamps remain available on hover. The Columns menu can reveal
Completed, Parent and Version; an incoming
completion sort automatically reveals Completed. Desktop sorting uses accessible
column headers; narrow screens retain a sort selector. Date basis and period are
grouped in the time-range popover, with refresh controls together on the right.

Follow-up verification: all 1,374 frontend tests, scoped ESLint and the production
build passed. Five Chromium checks passed, covering 1440/1000/390 layouts, compact
row heights, optional columns, keyboard sorting, expanding/collapsing details,
URL context, refresh failures and recovery. No backend behavior changed.

The subsequent simplification removes row disclosure and its polling state.
Copy run ID is a read-only action beside the eye icon, outside execution permission
checks. Actions stay on one line, including waiting runs with chat and Stop.

This simplification passed all 43 invocation-history tests, scoped ESLint,
production build, and five browser checks. Browser checks cover no expanders,
copy-ID in the action cell, single-line waiting-run actions, optional columns,
keyboard sorting, responsive layouts and refresh recovery.


## Overview presentation follow-up

Needs attention contains two distinct compact groups: Requests requiring input
and Recent failures. Each group has its own total and destination link. Request
rows show business label, request, workflow, due time and a review icon; overdue
requests sort first, oldest due time first. Failure rows show business label,
workflow, one-line error summary, completion age and eye/copy-ID/replay icons.
Recent failures are sorted by completion time within the existing 24-hour range.

Desktop columns align across rows, while narrow screens stack the same content.
Redundant Waiting/Failed badges, large action buttons and inline error expansion
are removed. Replay retains its permission and transient-error eligibility rules;
its confirmation uses a dialog holding the selected run across list refreshes.
Full errors remain on the execution details page.

Verification: 55 focused Operations and invocation-history tests, scoped ESLint,
production build and three browser layout checks passed. Browser checks cover
1440/1000/390 widths, overdue priority, separate destinations, compact rows,
action alignment and opening/cancelling Replay without submitting it.

## Overview request metric drilldowns

Waiting for a decision opens `/operations/requests`; Overdue opens
`/operations/requests?filter=overdue`. View all requests uses the same list.
Waiting/Overdue tabs and the page number survive URL sharing and reload. Browse
queues remains a separate destination for queue configuration and saved views.

The full list shares queue discovery, due-role selection and overdue predicates
with Overview. It counts and paginates requests across queue partitions, grouped
by workflow, using each queue's existing tenant-scoped API. The initial page of
each queue supplies its total; additional pages are fetched only where needed
for the current 25-row page. Saved presentation filters do not narrow these
metric drilldowns. Queues without a configured due field are excluded from the
overdue list. A failed queue request fails the aggregate visibly instead of
showing a misleading partial total.

Verification: 16 Operations tests, scoped ESLint and production build passed.
Pagination covers 132 requests across three queues, including multiple requests
on the same run, partition boundaries and out-of-range pages. Five browser checks
passed: the cards open the matching four waiting/two overdue requests at desktop
and mobile widths, Overdue survives reload, and existing Operations layouts work.

## Custom queue management

Queues now lists only saved, user-created configurations. The automatic queue
list and separate Shared views section are removed. Existing shared views remain
available as queues without a migration; their previous `/operations/views/:id`
links still work. The canonical destination is `/operations/queues/:id`.

Create queue selects one workflow and either requests for an action key or
workflow runs. Queue settings include name, state filters, run status, columns,
sorting, display formats, field roles, and answer controls. Edit and delete are
available on both the list and queue detail page under `workflow:update`.
The workflow cannot change after creation, matching the existing API contract.
Removed action keys remain selectable for an existing queue.

Deletion confirms the named queue and removes only its saved configuration.
Workflows, runs, and pending requests remain available. Edits and deletion retain
the revision originally presented, allowing the API to reject concurrent changes.
Waiting and Overdue still use internal request discovery independently of saved
queues; the waiting metric describes workflows rather than automatic queues.

Verification: 63 focused Operations and invocation-history tests, scoped ESLint,
and production build passed. Seven browser checks passed, including complete
create/edit/delete flows at desktop and mobile widths, persistence after reload,
cancelled deletion, request metric destinations, and existing Operations layouts.

Run status is a set of selectable tags using the same known statuses as Runs.
Multiple tags can be selected; toggling a tag removes it and All statuses clears
the filter. Existing saved status filters load into the selection. Desktop and
mobile queue CRUD checks cover multiple selections, persistence after reload,
deselection, and reset. Production build and four focused queue tests passed;
scoped lint reported no errors and one pre-existing Fast Refresh warning.
