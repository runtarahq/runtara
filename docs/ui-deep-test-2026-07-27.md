# Runtara UI deep-test findings

Instance: http://localhost:7001/ui/org_p0IkAFnrVqVOvQw9 (v8.7.0, commit 7ab389bdd9e5, authMode=local)
Date: 2026-07-27. Driven through the real UI with Playwright; every finding cross-checked against the
runtime API or MCP before being recorded. All test artifacts created during the run were deleted.

## Summary

29 findings. The single highest-leverage fix is **F4/F5/F9/F10** — one hardcoded `pageSize: 100`
with four distinct user-visible failures.

### Fix first (data is wrong or the user is blocked)
| # | Finding | Area |
|---|---|---|
| F4 | Folder counts read "0 workflows" for folders holding 2 — capped at 100 | Workflows |
| F5 | 24 workflows unselectable in every workflow picker | Triggers |
| F9 | Triggers list shows raw UUIDs instead of workflow names (3 of 6) | Triggers |
| F10 | Triggers bound to those workflows are permanently uneditable | Triggers |
| F14 | Inline cell editing in Database never saves — silent data loss | Object model |
| F20 | Versions panel puts the "Active" badge on the wrong version | Workflows |
| F21 | Log steps are invisible in every execution Events view | History |
| F26 | 5 of 6 Analytics trend percentages are fabricated | Analytics |
| F2 | "Sign in" ejects you to an external Auth0 tenant that 403s | Global |
| F16 | A filter used only by a block `condition` is unsettable in the UI | Reports |

### Then
F1 (CSP blocks the anti-FOUC script), F12 (errors on a pristine form), F14b (stale write hits a
deleted row), F15 (phantom "+ Add row"), F17 (no absolute time ranges), F23 (new workflow ignores
the folder you're in), F24 (UI steps get UUID ids), F27 (Chat swallows messages), F29 (API keys have
no scope/expiry).

### Polish
F3, F6, F7, F8, F11, F13, F18, F19, F22, F25, F28.

### Largest areas that worked correctly
Workflow authoring → compile → run → Activity Log; report rendering and URL-state hygiene; the
object-model schema/instance form paths; connections (22 types, secrets never returned); invocation
history; delete confirmations throughout. No XSS found. Details in the
"verified working" sections below.

---

## F1 — CSP blocks the anti-FOUC theme script on every page load
Severity: medium (visible flash for every dark-mode user + console error on every load)
- `build_html_csp` (crates/runtara-server/src/api/handlers/ui.rs:379) hashes only the injected
  `runtara-runtime-config` script.
- `index.html` (crates/runtara-server/frontend/index.html:9) ships a SECOND inline script — the
  pre-paint theme applier. Its hash `sha256-PLsRbGzv7FQG+KzvOy90Gm3JPwotV829hoG3EoWEPCw=` is never
  added to script-src, so the browser blocks it.
- Net effect: the script that exists solely to avoid a light flash never runs.

## F2 — "Sign in" button ejects you to an external Auth0 tenant in local auth mode (HIGH)
Severity: high (unrecoverable navigation away from the app)
- `AuthSidebar` (src/shared/layouts/AuthSidebar.tsx:12) branches only on OIDC `isAuthenticated`,
  never on `config.authMode`. In `authMode: "local"` there is no OIDC session, so it always renders
  "Sign in".
- Clicking it calls `signinRedirect()` → navigates to
  `https://auth.syncmyorders.com/authorize?...&audience=https://api.syncmyorders.com&organization=org_p0IkAFnrVqVOvQw9`
  which returns **HTTP 403**. The user is thrown out of the app; only the browser Back button recovers.
- Two sub-issues:
  - The shipped `dist` has a *specific* Auth0 tenant (`auth.syncmyorders.com`, client_id
    `AppofmEZR4nkYJTPyHRgQakdUaV7AdA4`) baked in at build time.
  - `redirect_uri` is `http://localhost:7001/ui/` — the tenant segment `org_p0IkAFnrVqVOvQw9` is
    dropped, so even a successful login would land on the wrong base path.
- Also: the sign-in and sign-out states both render the **same `LogOut` icon**, so the control looks
  identical in both states.

## F3 — "Manage billing" button is dead on self-hosted/local deployments
Severity: low-medium
- `Sidebar.tsx:189` renders it unconditionally. Clicking POSTs
  `/api/management/billing-dashboard` → **404 Not Found**, then a generic
  "Failed to create billing portal session" toast.
- No entitlement/authMode gate, unlike `reports`/`database`/`api` which do have `EntitlementRoute`.

## F4 — Folder workflow counts are wrong: hard-capped at 100 workflows (HIGH)
Severity: high (visibly wrong data on the app's landing page)
- `Workflows/index.tsx:66` calls `getWorkflows`, which (`features/workflows/queries/index.ts:80`)
  requests `{ recursive: true, pageSize: 100 }` with the comment
  "Use max page size to get all workflows for dropdowns".
- `folderWorkflowCounts` (Workflows/index.tsx:76) then tallies folders **client-side** from that
  single truncated page.
- This tenant has **124** workflows, so 24 are silently dropped. Because the list is sorted
  `updated desc` and the foldered workflows are the oldest, *every* folder-resident workflow falls in
  the dropped tail.
- Observed vs actual (API `?path=…&recursive=false`):
  | folder | UI shows | actual |
  |---|---|---|
  | Commerce | 0 workflows | 2 |
  | Customer | 0 workflows | 2 |
  | Microsoft Azure | 0 workflows | 2 |
  | Operations | 0 workflows | 2 |
  | Demo | 1 workflow | 1 direct / 6 recursive |
- Clicking into "Commerce" (which reads "0 workflows") immediately shows 2 workflows — the list view
  and the count disagree on the same screen.
- Second-order: the count is *direct children only*, so `/Demo/` reads "1 workflow" while it holds 6
  including `/Demo/Test/`. Even un-truncated, a folder full of nested workflows reads "0 workflows".

## F5 — `getWorkflows`' 100-cap also truncates every workflow picker
Severity: high (workflows become unselectable)
- The same capped query feeds the trigger dropdowns (`features/triggers/queries/index.ts:51,68`,
  `CreateTrigger/index.tsx:27`, `EditTrigger/index.tsx:48`).
- On this tenant that makes 24 workflows impossible to pick when creating/editing a trigger.
- No "showing first 100" affordance — the list just ends.

## F6 — Workflow search leaves folders unfiltered and shows the wrong empty state
Severity: low
- Typing a query filters workflows but **not** folders: searching `zzzznomatch` still renders all 5
  folder rows.
- The empty state is hardcoded `No workflows in this folder yet.`
  (`WorkflowsGrid/index.tsx:502`) — shown at the *root* with an active search, where neither
  "in this folder" nor "yet" is true. A no-search-results state is missing.

## F7 — Folder rows repeat on every page of the workflow pagination
Severity: low
- On "Page 2 of 11" all 5 folders render again above the workflows. Folders are not part of the
  paginated set but are re-emitted with each page.

## F8 — List state (search, page, page size) is not in the URL, but folder is
Severity: low (inconsistency)
- `?folder=/Demo/` is a URL param and survives reload/back-forward.
- Search text, current page and page size are component state only — reload resets to page 1 and
  clears the query, and no list state beyond folder is shareable.

## F9 — Triggers list shows raw UUIDs instead of workflow names (same 100-cap root cause)
Severity: high (3 of this tenant's 6 triggers are affected — 50%)
- The NAME column falls back to the raw `workflow_id` when the workflow isn't found in the
  truncated 100-item list. Verified exactly:
  | trigger | shows | actual workflow |
  |---|---|---|
  | 56d4fd7b | `206d0835-99a0-…` | Inventory sync — invoicing ↔ Shopify |
  | 736e58b7 | `57c9ffd2-1640-…` | SEO enrichment & market intelligence |
  | 9f8a9a08 | `6041ae3e-e36d-…` | HS test |
- The three affected triggers are *exactly* the three whose workflow_id is outside the first 100.

## F10 — A trigger whose workflow is outside the top 100 is permanently uneditable (HIGH)
Severity: high
- Repro (verified on a disposable trigger I created and repointed via the API):
  1. Trigger references a workflow not in the first 100 (sorted `updated desc`).
  2. Open `/invocation-triggers/:id`. The Workflow field renders **blank**
     (`select.selectedIndex === -1`) because the bound workflow isn't among the 100 options.
  3. Click Save → `Please choose a Workflow.` (zod `workflowId.nonempty`), and the PUT never fires.
- Confirmed no data corruption — validation holds and the stored `workflow_id` is unchanged.
- But the trigger can never be edited again through the UI: changing its schedule is impossible
  without rebinding it to one of the 100 visible (wrong) workflows.

## F11 — Workflow combobox has no placeholder, and its shadow native select disagrees with it
Severity: low
- On `/invocation-triggers/create` the Workflow field renders **completely empty** — no
  "Select a workflow…" placeholder, unlike the adjacent Trigger Type which shows "HTTP".
- The hidden native `<select>` meanwhile reports `value = <first workflow id>`,
  `selectedIndex = 0` while form state is `''`. The two controls disagree.
- Net UX: an empty unlabeled box; clicking Create returns "Please choose a Workflow." for a field
  the user has no cue to fill.

## F12 — Connection create form shows required-field errors on a pristine, untouched form
Severity: medium (UX)
- Opening `/connections/sftp/create` immediately renders four red (`rgb(239,67,67)`) errors —
  "Title is required", "Host is required", "Username is required", "Password is required" —
  before the user has typed a single character.
- Validation is eager rather than on-blur/on-submit. The edit page (populated values) shows none, so
  this hits every *new* connection, i.e. exactly the first-run experience.

## F13 — Schema-form enum option values are JSON-encoded (low / latent)
Severity: low (works today; latent mismatch)
- The Authentication Mode `<option value>` is `"password"` *including literal quote characters*,
  not `password`. Stored value (API `editProjection.values.auth_mode`) is plain `password`.
- Round-trips correctly today because the form encodes and decodes through the same path, so this is
  cosmetic — but any client writing the plain value where the form expects the quoted form (or a
  future change to one side only) would silently fail to match and render a blank select.

## Connections — verified working
- All 22 API connection types render in the picker, correctly grouped by category.
- Agent→connection mapping has no orphans: `sharepoint`→`microsoft_entra_client_credentials`,
  `sqs`/`bedrock`→`aws_credentials`, `s3-storage`→`s3_compatible`.
- Secrets are never returned by the API — `editProjection.secretState` exposes only
  `{configured, clearable}` booleans. Good.
- Edit form round-trips stored values correctly and shows no spurious errors.
- Gap (minor): the connections list has no search and no pagination, unlike the workflows list which
  has both — inconsistent for a list that will grow.

## F14 — Inline cell editing in the Database instance table never saves (HIGH)
Severity: high (silent data loss from the primary data-editing surface)
- Repro on a fresh schema with one persisted record (`probe_field = "created-via-form"`):
  1. Click the cell → inline editor opens.
  2. Type a new value (verified with real per-character keystrokes, not programmatic `fill`).
  3. Commit by any gesture — **Enter**, **click outside the table**, or type-then-click-outside.
  4. **No PUT/PATCH is ever issued.** Verified three ways: Playwright network log, an injected
     `fetch`/`XHR` spy (`window.__net` stayed empty), and reading back via MCP
     `list_object_instances` (value unchanged).
- Extra damage: after the failed commit the cell renders **blank**, hiding the correct stored value,
  so it looks like the field was wiped. A reload restores the true value — data is not corrupted.
- No error, no toast, no console message.
- Enter also fails to close the editor: `handleKeyDown` (EditableCell.tsx:134) calls `onBlur()` but
  never `setIsEditing(false)` (the Escape branch does).
- Save path: `handleUpdate` (ObjectInstancesTable/index.tsx:342) only marks the row dirty; the real
  write happens in `saveDirtyRow` (:288), triggered from `handleClickOutside` (:399), which
  early-returns unless `lastFocusedRowRef.current` is set (:402). That ref is only populated via
  `onCellFocus` → `handleCellFocus` (:526). The chain never completes at runtime.
- Working alternatives (so this is the inline path specifically):
  - `/objects/:typeName/create` form → persists correctly.
  - Bulk "Edit N selected" dialog → works.

## F14b — The deferred inline-edit write can fire late, against an already-deleted row
Severity: medium
- Observed sequence in the network log: the pending edit's `PUT .../instances/{schemaId}/{id}` was
  flushed *after* a bulk `DELETE .../bulk` of that same row, returning **404 Not Found** and a
  console error.
- So the write isn't dropped — it's queued indefinitely and can be flushed by an unrelated later
  click, potentially targeting a row that no longer exists.

## F15 — "+ Add row" creates a phantom row and inflates the record count
Severity: medium
- Clicking "+ Add row" inserts a client-only `PENDING_<ts>` row with fabricated created/updated
  timestamps and a blank ID, and the footer count jumps ("0 records" → "1 record") even though the
  API still reports `totalCount: 0`.
- Because inline editing never commits (F14), the draft can never be turned into a real record — the
  row and the inflated count simply vanish on reload.

## Object model — verified working
- Schema creation persists correctly; table name auto-derives from schema name (`UiTestProbe` →
  `uitestprobes`).
- Column types offered: string, integer, decimal, boolean, timestamp, json, enum, tsvector, vector.
- **No XSS**: a description of `Probe <name> & "quotes" <script>alert(1)</script>` was stored
  verbatim (no over-escaping on write) and rendered as inert text (no `<script>` in the DOM).
- The pre-existing `&lt;name&gt;` in the `RuntaraTenant` description is **stored** that way already —
  it is not a UI render bug.
- Minor a11y: the ID cell is an icon-only copy button with a `title` but no `aria-label`; the bulk
  edit modal uses `role="alertdialog"` for a non-destructive data-entry form.

## F16 — A filter consumed only by a block `condition` is invisible in the UI and unsettable (HIGH)
Severity: high (renders a shipped dashboard chart permanently empty)
- `isFilterVisible` (features/reports/components/ReportFilterBar.tsx:484) hides any filter whose
  `appliesTo` array is empty:
  ```js
  const appliesTo = filter.appliesTo ?? [];
  if (appliesTo.length === 0) return false;
  ```
- But a filter can also be consumed by a block through `source.condition`. In the shipped
  "Commerce operations dashboard", the `period` filter has `appliesTo: []` yet the `stock_trend`
  block's condition references it: `{"filter": "period", "path": "from"}` / `"to"`.
- Result: **Period never appears in the "+ Filter" picker** (only Vendor and Category do), so the
  user cannot set it, and "Distributor stock over time" permanently shows
  *"No chart data for the current filters."*
- The visibility heuristic inspects only `appliesTo`; it never walks block `condition` references.
- **The report editor documents the opposite contract.** Its own Filters panel says verbatim:
  > "Empty applies-to means the filter targets all blocks via their source's condition."
  So the authoring UI tells you empty `appliesTo` means *targets everything via condition*, while the
  viewer treats it as *targets nothing, hide the control*. The two halves of the product disagree.

## F17 — Time-range filters offer only relative presets, so absolute ranges are unreachable
Severity: medium
- `TIME_RANGE_PRESETS` (features/reports/utils.ts:20) = Today, Yesterday, Last 7 days,
  Last 30 days, This month. There is **no custom / absolute from–to option**.
- The backend fully supports absolute ranges — `get_report_block_data` with
  `{"period": {"from": "2026-01-01", "to": "2026-03-31"}}` returns **90 rows, status "ready"**,
  while `{"period": "last_90_days"}` returns 0 rows.
- The tenant's `DailySkuStock` data spans 2026-01-01 → 2026-03-31 (3.6M rows) and "today" is
  2026-07-27, so *every* available preset misses the data entirely. Combined with F16 the chart can
  never be populated from the UI.

## F18 — Inconsistent date formatting across list pages
Severity: low (polish)
- Workflows list: relative + `18 Jul, 2026 8:29 AM`
- Object types list: `22 Jul, 2026 12:10 PM`
- Reports list: `7/26/2026, 2:41:00 PM` (US locale, with seconds)
- Triggers list: `7/27/2026, 8:00:19 AM`
- Three different formats across four tables in the same console.

## F19 — "Explore" is always offered but dead-ends for reports without a semantic dataset
Severity: low-medium
- The report header always renders an **Explore** button. On the Commerce dashboard it navigates to
  `/reports/:id/explore` which renders only:
  *"This report does not expose a semantic dataset for Explore."*
- The button should be hidden or disabled when the report has no semantic dataset, rather than
  routing to a dead end.

## Reports — verified working
- Rich rendering is solid: markdown, metric cards, bar/donut/scatter charts, pill/avatar table
  formats, 39 inline SVGs, no console errors beyond the global CSP one.
- Product search deep-links correctly (`&q=Alpine`) and filter chips serialize to the URL
  (`&vendor=Alpine+Gear+Co`) — better URL-state hygiene than the workflows list (F8).
- KPI cards and the vendor donut *not* responding to the Vendor filter is **correct** — the report
  definition's `vendor.appliesTo` deliberately lists only `products_table`, `price_vs_stock` and
  `by_category`. Verified against the stored definition, not assumed.
- Search scoping to the catalog table only is likewise documented in the report's own intro text.

## F20 — Versions panel shows the "Active" badge on the WRONG version (HIGH)
Severity: high (misreports which version is live)
- After saving a change, the Versions panel renders:
  - `v2 · just now · Compiled · wasm 2.6 MB · [Rebuild] [Activate]`
  - `v1 · 2 min ago · Not compiled · **Active**`
- The API says the opposite: `GET .../versions` → v1 `isActive: false`, **v2 `isActive: true`**.
- The execution that followed is attributed to **v2** in both the History panel and Invocation
  History — confirming v2 is genuinely the live version.
- So the UI labels the stale, uncompiled v1 as "Active" and offers to "Activate" the version that
  already is. A user is told their newly saved work is not live when it is.

## F21 — Log steps are invisible in every execution "Events" view (HIGH)
Severity: high (the main debugging step type is missing from the main debugging screen)
- The history page's Events tabs (Timeline / Graph / List) read **only** step *summaries*
  (`GET .../instances/{id}/steps`); they never read `.../step-events`.
- Log steps produce no step summary, so they are silently dropped from all three views.
- Landing Demo run `39137e4c`: the workflow has **6** steps but the Timeline header reads
  **"Steps 3"** and lists only Delay, WaitForSignal and Finish. All three Log steps are missing.
- A workflow made *only* of Log steps renders a completely empty Events view — verified on a
  workflow I created: `/step-events` returns 2 events (including the `workflow_log` payload
  `"ui-probe hello"`) while `/steps` returns `count: 0`.
- The data is not lost — the separate **View Logs** page shows all 6 entries correctly, including
  every Log message. Only the Events views are wrong.

## F22 — Completed runs show "still running" empty states
Severity: low (misleading copy)
On a run whose status is **Completed**:
- Events tab: *"No Timeline Events Yet — Timeline events will appear here as your workflow executes.
  If your workflow is still running, events may appear soon."*
- Output Data: *"No output data yet — Output will be available once the workflow completes."*
Both imply the run is in progress. Neither distinguishes "not finished" from "finished, nothing to
show".

## F23 — Creating a workflow from inside a folder puts it at the root
Severity: medium
- Browsing `?folder=/Demo/Test/` and clicking **New workflow** navigates to `/workflows/create`
  with the folder context dropped.
- The created workflow lands at `path: "/"` (verified via API), not `/Demo/Test/`, with no warning.
- The create form has only a **Name** field — no description and no folder picker — even though the
  list shows a Description column and `createWorkflow` sends a description.

## F24 — UI-created steps get a raw UUID as their step id
Severity: medium (authoring quality)
- Adding a Log step through the picker produced step id
  `b7514123-3222-42f4-8349-a5577f9d450b`, displayed verbatim in the canvas and in diagnostics:
  *"[W003] Step 'b7514123-3222-42f4-8349-a5577f9d450b' (Log) has no outgoing edges…"*
- DSL/MCP-authored workflows get readable slugs (`receive_order`, `sync_systems`, `apply_rules`).
- Consequence: references to a UI-created step read
  `steps.b7514123-3222-42f4-8349-a5577f9d450b.outputs`.

## F25 — Instance-list pagination is 0-indexed while the workflow list is 1-indexed
Severity: low (API inconsistency, surfaced in the UI)
- `GET /workflows/{id}/instances?page=0` → 1 row; `?page=1` → **empty** (`number: 1`).
- `GET /workflows?page=0` and `?page=1` both return the *first* page (`number: 0`); `?page=2` is the
  second. So one endpoint is 0-indexed and the other clamps 0→1.
- Also seen: an immediate `GET .../instances/{id}` right after starting a run returns **404** once
  before succeeding on retry (harmless race, but it logs a console error on every run), and the
  history page fires the identical `/steps` request **three times** on load.

## Workflow editor — verified working
- Create → add step → save → compile → run works end to end (compiled to a 2.6 MB wasm, ran in
  0.23s, status Completed).
- The step picker exposes 12 step types + all 27 entitled agents, matching the entitlement list
  exactly; Start/Finish/Agent are handled by dedicated affordances rather than the picker.
- Step editor is good: required-field marking, level select, DSL Context mapping with an
  "Edit as JSON" escape hatch, Output and Execution sections, and a clear
  "Configuring in the panel — not added yet" state.
- Problems panel gives real coded diagnostics (`W003` with an explanation and a "Go to step" link).
- Start/Debug are correctly disabled with the tooltip "Please save your changes before starting
  execution" until the graph is saved.
- Canvas (React Flow) renders 7 nodes / 6 edges with a fitted viewport; the **Activity Log** page is
  complete and accurate.
- Minor: a linear workflow is laid out in a single left-to-right row, so `fitView` zooms *out*
  (scale 0.84) and node labels truncate ("Apply business ...", "Complete and a...") even on a
  1600px viewport with ~85% of the canvas empty.
- Minor a11y: editor toolbar buttons carry `title` but no `aria-label`.

## F26 — Five of six Analytics KPI trend percentages are fabricated (HIGH)
Severity: high (invented numbers presented as measured analytics)
- In `features/analytics/pages/Usage/index.tsx` only `executionsChange` is computed from real data
  (first-half vs second-half of the metric series). Every other card derives its "change" as a fixed
  multiple of that one number:
  | Card | line | "change" value |
  |---|---|---|
  | Success Rate | :385 | `Math.abs(trends.executionsChange * 0.1)` |
  | Avg Duration | :392 | `Math.abs(trends.executionsChange * 0.05)` |
  | Avg Memory | :399 | `Math.abs(trends.executionsChange * 0.08)` |
  | Failed Executions | :406 | `Math.abs(trends.executionsChange * 0.15)` |
  | Cancelled | :413 | `Math.abs(trends.executionsChange * 0.12)` |
- Observed live with `executionsChange = +100%`: +10.0% / -5.0% / -8.0% / -15.0% / +12.0% — exactly
  the multipliers.
- The visible tell: **"Cancelled: 0, +12.0%"** — a 12% rise on a metric that is zero.

## F27 — Chat is offered on every workflow and silently swallows messages
Severity: medium
- Every workflow row has a **Chat** action, and `/workflows/:id/chat` opens a working-looking chat
  ("Type a message to begin chatting with this workflow") that even allocates an instance id.
- On a workflow with no chat-capable entry point (my Log-only probe), sending "hello" echoes the
  message into the transcript and then makes **zero network requests** — no reply, no spinner, no
  error, no "this workflow doesn't support chat".
- Same shape as F19 (Explore): an always-offered action that dead-ends for most objects.

## F28 — The 404 route renders a bare unstyled "404"
Severity: low
- `router/index.tsx:517` is `element: <>404</>`. An unknown URL renders the literal string `404`
  inside the app chrome — no message, no link back, no styling.

## F29 — API key creation offers no scopes and no expiry
Severity: low-medium (security posture)
- The Create API Key dialog has a single **Name** field. These keys are described as granting MCP /
  external-integration access to the tenant, but there is no scope restriction and no expiry option.
- (I deliberately did not mint a key, so this is from the form and its copy, not a created credential.)

## Analytics / settings — verified working
- `/analytics/system` reports real host stats (16 cores aarch64, memory, disk).
- `/analytics/rate-limits` lists all 17 connections with per-connection limit + 24h request counts.
- `/invocation-history` is accurate and — unlike the Triggers list (F9) — resolves workflow names
  correctly for *every* run, including workflows outside the first 100.
- Workflow delete has a proper confirmation ("Are you absolutely sure? This action will delete the
  workflow …") and really deletes (GET → 404 afterwards).
- Object-instance bulk delete also confirms first ("This action cannot be undone").

## Not bugs (checked and cleared)
- `APPLICATION` trigger type missing from the create dropdown — intentional
  (`CreateTrigger/index.tsx:136` passes `hiddenTriggerTypes={['APPLICATION']}`).
- New triggers defaulting to Inactive — matches declared `initialValue: false` (TriggerItem.tsx:142).
- Invalid cron / malformed static-JSON — validated correctly and submission is blocked
  (button stays enabled, but no bad request is sent).
- Sortable table headers appearing nameless in the a11y tree — snapshot artifact; real DOM has text
  and correct `aria-sort`.
