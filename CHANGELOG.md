# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> This file tracks release notes starting at **1.7.0**. Earlier tagged releases
> (`v1.0.21` through `v1.6.18`) are available as git tags; their history lives
> in `git log`.

## [Unreleased]

### Added

- **Workflows can declare `stateSchema`**, the typed state a run exposes,
  next to `inputSchema` and `outputSchema`: a map of schema fields with
  labels, formats and enums (DSL 3.4.0). It is stored with each version,
  returned by the workflow and version-schemas endpoints, editable in the
  workflow settings and through the MCP `get_state_schema`/`set_state_schema`
  tools and the `set_state_schema` graph mutation. Number fields accept the
  `currency` format hint. W081 warns when a state field sets `required`,
  `default` or `visibleWhen`, which have no effect for state.
- **Runs keep queryable state** (DSL 3.5.0). A `SetState` step merges values
  into the run's state (shallow; `null` clears a field), each checked against
  `stateSchema` (E134 without one, E135 for an undeclared field, E023/E024
  for literal values, `STATE_INVALID_VALUE` at run time; date-times are
  stored in UTC). A `GetState` step reads it; its output is the state object.
  A write applies once per step and a read is checkpointed, so replays change
  nothing and read-modify-write loops take the same path. Only the outer
  durable run publishes its state: an embedded child, a published
  workflow-agent and a non-durable workflow keep local state that readers
  never see (W082, W083); a durable Split carries the local state its body
  wrote in its cached result. Readers never wake the run: the control agent
  gains `get-state` and a `state` filter on `query`
  (`[{field, op, value}]`, op `eq`, `ne`, `in`, `lt`, `lte`, `gt`, `gte` or
  `exists`), the single-run executions endpoint returns `state` and
  `stateUpdatedAt`, and `POST /api/runtime/executions/query` (and MCP
  `list_executions` with `state`) lists executions by state without
  returning it. Compiled workflows import the new `runtara:workflow/state`
  interface only when they publish state. **Upgrade note:** the first boot
  migrates the runtime database (`043_instance_state`: `instance_state` and
  `instance_state_writes`, deleted with their run; pruning a finished child
  keeps its state).
- **The `control` agent reads runs of the tenant from a workflow**, on every
  pricing tier and whatever the agent allowlist says. `get` returns one run's
  state with its output inlined up to 1 MiB and its error up to 64 KiB
  (larger values are flagged omitted, with their size); `query` pages runs by
  creation or finish time (page size 1-100); `list-pending-signals` lists open
  WaitForSignal requests of a run or a workflow. Failures carry `CONTROL_*`
  codes. Control runs only the installed control bytes in fresh host stores:
  a compiled workflow pins those bytes (`runtara:builtin-artifacts/control-…`),
  the host audits every composed component that imports `runtara:control`
  when the artifact is prepared, and every call re-checks both against the
  approved history. The server approves its bundles' control bytes at boot
  (runtime table `approved_builtin_artifacts`, migration
  `20260927000000`); an operator revokes a version by setting `revoked_at`,
  which takes effect at the next boot, after which calls through it are
  `CONTROL_DENIED` and workflows pinning it no longer become ready. Runs
  already parked on a revoked version still load and fail at their next
  control call, and a run pinned to an older, still approved version keeps
  working after the control agent is upgraded. Composed artifacts that import
  a component or core module, or whose agents import
  `runtara:workflow-operation`, are now refused at preparation.

- **The `control` agent answers signals and pauses, resumes and cancels
  runs from a workflow.** `send-signal` answers the one open request of a
  WaitForSignal step, validated against its response schema, for a child, an
  ancestor, or any run whose request opted in with `action.key` when the step
  passes the same `actionKey`; `cancel` (reason, grace 0-3600 s, default 5 s),
  `pause` and `resume` reach direct children only, and nothing targets the
  calling run. Each mutation step runs in a
  compiler-emitted operation scope (`runtara:workflow-operation`), durable or
  not, so a retried or replayed step never applies twice: its receipt is kept
  per calling run and step operation (runtime migration
  `034_instance_control_receipts`, deleted with the caller), a replay returns
  `replayed: true`, and different arguments are `CONTROL_REPLAY_CONFLICT`.
  Signals it sends carry `control:` operation ids, a prefix now refused on
  every public submission path (signals, report actions, sessions,
  channels). Every attempt is recorded in `audit_events`
  (`control.send_signal`, `control.cancel`, …) without payloads. Workflows
  without control or suspending steps compile to the same bytes as before.

- **The `control` agent starts child runs.** `start` durably admits a run
  of another workflow as a child of the calling run and returns
  `{instanceId, workflowId, version, runLabel, replayed}` once it is
  accepted, without waiting for it. The author must choose a
  `parentClosePolicy` (`cancel`, which the step editor preselects, or
  `leave_running`); the editor now pre-fills every required enum input
  without a default with its first value. Admission is replay-safe per step
  operation: the same step replays the same child (`replayed: true`) and
  other arguments are `CONTROL_REPLAY_CONFLICT`. A run label names one child
  per parent for the parent's lifetime (`CONTROL_LABEL_CONFLICT`); lineage
  stops at depth 16 (`CONTROL_INVALID`); a missing workflow or version is
  `CONTROL_NOT_FOUND` and a permanently failed compilation
  `CONTROL_NOT_RUNNABLE`, while a workflow not compiled yet is admitted and
  retried until the request deadline (then `launch_deadline_not_compiled`).
  Children count against the concurrency limit, and control's children may
  hold at most `max(1, floor(0.8 x limit))` slots: beyond that `start` fails
  with retryable `CONTROL_CAPACITY_RATE_LIMITED` (3-8 s hint); a limit of at
  most 1 is `CONTROL_CAPACITY_UNSATISFIABLE`, with a boot warning. The parent
  link now decides which runs are children and ancestors for `send-signal`,
  `cancel`, `pause` and `resume`; a child still in admission reads as
  `queued` and cannot pause or resume yet. `get` reports `parentInstanceId`,
  `query` filters by `parentInstanceId` or the caller's children (merging
  children still in admission, paged by admission time), and
  `list-pending-signals` covers the caller's children. The executions API
  (`WorkflowInstanceDto.parentInstanceId`, `GET /api/runtime/executions
  ?parentInstanceId=`) and MCP `list_executions` (`parent_instance_id`) expose
  the link. **Upgrade note:** the first boot migrates the runtime database
  (`035`-`037`: parent columns on `instances`, briefly locked, and
  `idx_instances_parent_admitted`) and the server database
  (`20260927000100`/`000101`: child columns, checks and the unique per-parent
  label index on `execution_requests`). Migrations are forward-only; do not
  downgrade past this release.

- **Control's children are owned by their parent.** A finished child stays
  readable until its parent is terminal: instance cleanup keeps it until both
  are past `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS` (one level deep; a missing
  parent counts as terminal) and logs `pinned_terminal_children` per pass.
  With `parentClosePolicy: cancel`, a child is cancelled whenever its parent
  ends (completed, failed, cancelled, or gone; a suspended parent has not
  ended), with a 5 s grace and the reason `parent <id> terminated
  (<status>)`, even when the parent crashed or was stopped from outside;
  `leave_running` children are left alone. `cancel` works in every
  admission state: a child not launched yet is cancelled before it ever
  runs and frees its slot, one mid-launch is stopped as soon as its launch
  is accepted. A child that never launched gets exactly one fenced outcome,
  `not-started` or `cancelled` with its reason, which `get` and
  `query(parent)` report and the public executions list does not; a launch
  and that outcome exclude each other, so no running child ever reads
  `not-started`. Image cleanup no longer stalls on images a launch still
  references. **Upgrade note:** the first boot migrates the runtime database
  (`038`: `instance_external_outcomes` and two partial indexes on
  `instances`) and the server database (`20260927000200`: a partial index on
  `execution_requests`).

- **Parallel approvals: the `WaitForInstances` step.** A new step type,
  displayed as "Wait for Instances", parks the run until its direct children
  finish: `instanceIds` (at most 1000 distinct), `mode` `all` (default) or
  `any`, and an optional `timeoutMs` business deadline. It outputs `{mode,
  resolution: satisfied|deadline|empty, finished[], remaining[], deadlineMs}`,
  each finished child with its status and its output or error (inlined up to
  256 KiB of output and 16 KiB of error per child, 3 MiB per wait; larger
  values are omitted and flagged). The run parks without holding a runner or
  a concurrency slot and is woken in the same commit as the child that
  satisfies it, by any writer including raw SQL; a wake that could not be
  stamped at once is recovered by the wake scheduler, and a paused run is
  never woken. The wait registers once per step and loop position, so a
  replayed `any` keeps its choice and the first deadline stands; a deadline
  settles the step with what finished and never cancels children. An empty
  list settles at once as `empty`; the run itself, ancestors, other runs,
  unknown ids and too many ids fail with `INSTANCE_WAIT_*` codes and register
  nothing. The workflow must be durable (E028), the step cannot sit in
  onError, onWait or AiAgent tools or memory (E131), and a bad literal
  `instanceIds` or `timeoutMs` is E133. Compiled workflows call the new
  `runtara:workflow-wait@0.1.0` host interface; the workflow editor and the
  MCP authoring schema author the step. **Upgrade note:** the first
  boot migrates the runtime database (`039`: termination reason
  `waiting_instances`; `040`: `instance_waits`, `instance_wait_targets`,
  `instance_input_parks.wait_ids`, three triggers on finishing runs, and a
  named wake-reason CHECK replacing the unnamed one from `024`; `041`
  validates it).
- **Typed agent suspension.** A capability declared `suspends` may return
  `suspended {wakes, state}`: the host keeps `state` (at most 64 KiB) per
  step operation and attempt, parks the run at the earliest wake bounded by
  the step timeout, and re-invokes the capability with its state on wake. It
  is an extension point for long-polling agents; no built-in agent uses it.
  A suspension may wake only on a timer (`at`): an instance wake is refused
  with `AGENT_INVALID_SUSPENSION`. Such steps compile in branch arms, Split
  and While bodies (a parallel Split runs them sequentially), embedded
  workflows and retrying steps (each attempt keeps its own state). A
  suspension within one second of the step timeout fails with
  `AGENT_TIMEOUT`; one the host refuses fails with `AGENT_INVALID_SUSPENSION`;
  a failed step discards its state, so a retry starts afresh
  (`AGENT_CONTINUATION_REJECTED` when a capability refuses its saved state).
  **Upgrade note:** the first boot migrates the runtime database
  (`042_agent_continuations`: `instance_agent_continuations`, deleted with
  its run).

- **Parked and pinned runs survive cleanup and upgrades.** Image cleanup
  keeps the compiled package of a parked run and of a child its parent still
  pins, and a parent parked on a WaitForInstances step resumes after the
  server binary or the control agent bundle is upgraded, its later control
  calls running on its older, still approved control pin: every shipped host
  interface version stays linked (the released `runtara:control`,
  `runtara:workflow-operation`, `runtara:workflow-wait` and
  `runtara:agent-suspension` 0.1.0 interfaces are frozen and tested), and
  approved control versions are only ever revoked, never deleted.
- **Control and suspension across the surfaces.** The executions API reports
  `WorkflowInstanceDto.suspensionReason` (`paused`, `waiting_signal`,
  `waiting_instances` for a WaitForInstances step, `sleeping` or `shutdown`)
  for a suspended run, and the
  step summaries endpoint (`GET /api/runtime/workflows/{id}/instances/{iid}/steps`,
  MCP `get_step_summaries`) reports an unfinished step of a suspended run as
  `suspended` and accepts `status=suspended`. The invocation history gains a
  Parent column and filter, the execution detail shows who started a run and
  its child runs, Resume is offered only for an explicitly paused run, the
  step editor marks suspending capabilities and asks for their timeout and
  durability, and a capability that only works inside a run cannot be tested
  from the editor. MCP `test_capability` answers `CONTROL_REQUIRES_INSTANCE`
  for such capabilities without running them, `add_agent_step` takes
  `timeout` and `durable` and hints what a suspending capability needs, and
  `get_workflow_authoring_schema` carries the control agent reference
  (`controlAgent`), also in `docs/control-agent.md`.

- TLS support for the Valkey connection: `VALKEY_TLS=1` switches every server
  connection to `rediss://`; `VALKEY_TLS_CA_CERT=/path/cert.pem` trusts a
  self-signed or private-CA certificate with full verification;
  `VALKEY_TLS_INSECURE=1` (strict opt-in — only `1/true/yes/on`) skips
  verification for local testing, and a configured CA always wins over
  insecure mode. Connection credentials are now percent-encoded in the URL so
  passwords containing `@ / : # %` work, IPv6 hosts are bracketed, a CA file
  without a PEM certificate block is rejected at client build with an error
  naming `VALKEY_TLS_CA_CERT`, and debug-logging `ValkeyConfig` or the
  compilation worker config redacts credentials instead of printing them.
- Two new `object-model` agent capabilities give workflows raw SQL against the
  tenant object-model database: `query-sql` (reads, DB-enforced `READ ONLY`
  transaction, optional `result_schema` for typed decoding of columns generic
  decoding rejects) and `execute-sql` (one write statement per call — DML,
  `TRUNCATE`, `INSERT...SELECT`, DDL if the connection role permits). Both use
  typed positional params (`$1..$n`), wire-compatible with the MCP
  `query_sql`/`execute_sql` tools, and run server-side guard rails: a
  statement timeout (`RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS`, default 60 s) and
  streaming row/byte caps on reads (`RUNTARA_RAW_SQL_MAX_ROWS` /
  `RUNTARA_RAW_SQL_MAX_RESPONSE_BYTES`) that error rather than truncate.
  `execute-sql` never auto-retries server errors (at-least-once semantics —
  write idempotent SQL); `query-sql` retries transient failures including
  transport blips. **Note for operators:** there is no dedicated feature flag —
  the capabilities are live for every `database`-entitled tenant on upgrade
  (the same tenants can already run identical SQL via MCP `execute_sql`). Raw
  SQL bypasses per-schema authorization and soft-delete; scope the Postgres
  connection role accordingly. Each request emits an audit line at target
  `runtara::raw_sql_audit`.
- New `runtara-agent-encoding` crate: a single, WASM-safe character-encoding
  vocabulary shared by the text, csv, and xml agents. It wraps `encoding_rs`
  (the WHATWG label/alias table and decoder) and `chardetng` (statistical
  detection), so a *detected* encoding name and a *requested* encoding name are
  guaranteed to be the same set. Exposes an `Encoding` input type that accepts
  any standard label (e.g. `UTF-8`, `windows-1252`, `Shift_JIS`, plus aliases
  like `utf8`/`latin-1`/`iso-8859-1`/`cp1252`) or `Auto`, and advertises a
  curated dropdown of common names to the Step Picker.
- New `text` agent capability `detect-encoding`: detects the character encoding
  of bytes via BOM sniffing then chardetng, returning the canonical encoding
  name plus `confident` and `bom` flags. The returned name can be fed directly
  into any encoding-sensitive capability (csv `from-csv`/`get-header`, xml
  `from-xml`, text `from-base64`).
- `runtara-environment` HTTP error responses now additively include
  structured fields (`category`, `severity`, `retry_hint`,
  `retry_after_ms`, `attributes`) when the underlying error carries them
  (typically `CoreError` → `StructuredError`). Existing `{error, code}`
  fields are preserved verbatim; clients that read only those two keep
  working unchanged.
- `InvocationCleanupWorker` (runtara-server) deletes terminal
  `workflow_executions` older than `RUNTARA_INVOCATION_CLEANUP_MAX_AGE_DAYS`
  (default 3) and `workflow_metrics_hourly` rows older than
  `RUNTARA_INVOCATION_CLEANUP_METRICS_MAX_AGE_DAYS` (default 365). Events
  and side-effect usage rows cascade with their parent execution. Tune via
  `RUNTARA_INVOCATION_CLEANUP_ENABLED`, `_POLL_INTERVAL_SECS`,
  `_MAX_AGE_DAYS`, `_METRICS_MAX_AGE_DAYS`, `_BATCH_SIZE`.
- `CleanupWorker` (runtara-environment) now reads configuration from env:
  `RUNTARA_RUN_DIR_CLEANUP_ENABLED`, `_POLL_INTERVAL_SECS`, `_MAX_AGE_DAYS`.
- All four cleanup workers (`InvocationCleanupWorker`, `DbCleanupWorker`,
  `ImageCleanupWorker`, run-dir `CleanupWorker`) now perform an **eager
  first cleanup pass on startup**, before entering the poll loop.
  Previously, each worker waited a full `poll_interval` (default 1 hour)
  before its first run, so frequent server restarts could prevent cleanup
  from ever happening.
- `runtara-server` integration tests in CI now run against a real Postgres
  service (previously skipped via `skip_if_no_db!()` because the CI
  workflow had no DB). New regression test
  `test_run_performs_eager_cleanup_on_startup` (in
  `crates/runtara-server/tests/invocation_cleanup_test.rs`) seeds an old
  terminal execution and asserts it is swept within seconds — guarding
  the eager-pass behavior described above.

### Changed

- **Raw SQL read routes are read-only and bounded.** `sql/query`,
  `sql/query-one`, `sql/query-raw` and the MCP `query_sql*` tools run in a
  `READ ONLY` transaction under the raw SQL statement timeout and row/byte caps,
  so a write or DDL sent through them (including a data-modifying CTE) now fails
  with 400 instead of running with a read-scoped key. `sql/execute` gets the
  statement timeout too, and a timeout is a 400.
- **The object-model database gets its own non-superuser role.** Raw SQL runs as
  the role in `OBJECT_MODEL_DATABASE_URL`; the Compose files and
  `bootstrap-install.sh` now create `runtara_objects`, which owns only
  `runtara_objects` and cannot connect to the server or runtime databases. The
  server warns at boot when that role is a superuser or shares the server's
  database. Existing installs keep their role until migrated; the SQL is in
  [docs/install.md](docs/install.md#object-model-database-role).
- **OAuth token, code-exchange and revocation endpoints are re-checked for https
  just before credentials are sent**, with the same allow-list as when a
  connection is saved. An `http` or private-address endpoint that is not
  allow-listed is now refused without sending anything.
- **Pausing a waiting run pauses it immediately** (public API, MCP and
  control). A run parked on a timer, a WaitForSignal, a restart or (with
  WaitForInstances) its children used to report `already paused` and resume on its own
  when its wait ended; it now
  becomes explicitly paused at once, loses its wake, and only an explicit
  resume relaunches it. Answers to its signals are kept and seen after the
  resume. The stop, pause and resume responses now carry `data.outcome`
  (`requested`, `applied`, `unchanged` or `already_terminal`), and each is
  scoped to the caller's tenant. Resuming a failed or cancelled run returns
  400 `Instance not resumable` (`code: NotResumable`) instead of trying to
  relaunch it; replay it instead. When a pause and a resume race, the latest
  one wins.

- **Run labels accept up to 1024 bytes** (was 250), still printable ASCII with
  at least one non-space character, on start, the `runLabel` filter, reports
  and MCP. The first boot after upgrading runs core migrations 032 and 033:
  they drop the unused trigram index `idx_instances_run_label_search`, widen
  `instances.run_label`, which rebuilds the exact-match btree
  `idx_instances_tenant_label_created` while `instances` is locked, then
  validate the label constraint and refresh its statistics. The substring
  `search` filter keeps its 250-character limit.
- **Every workflow recompiles once after upgrading.** The direct-WASM lowering
  tag now adds `on-signal-remap=v1` and `wide-result-errors=v1` (the latter
  fixes error messages from clock and delay host calls being read from the
  wrong offset), so cached artifacts built by an older server no longer match and each workflow is rebuilt on its next compile or
  launch. Expect a one-off burst of compilation work after the upgrade; nothing
  needs to be done by hand.
- **Validation checks where suspending, WaitForInstances and control-agent
  steps may appear.** A WaitForInstances step or a step whose capability
  suspends needs a durable workflow (E028), and may not run in an onError
  handler, a WaitForSignal `onWait` or as an AiAgent tool or memory provider
  (E131); a suspending step must also set a timeout above zero (E029); a control-agent
  step is refused in `onWait` and AiAgent tools and memory (E132). Embed call
  sites are judged against what their workflows call. New warnings: a literal
  `runLabel` on a control `start` in a loop (W074), such steps in a parallel
  Split or branch group, which now run serialized (W075), under a retrying
  Split or EmbedWorkflow (W076), a non-literal `start` target (W077) and a
  suspending step timeout within the 1 s deadline margin (W078). The rules
  are listed in the workflow authoring schema. Workflows containing such steps
  cannot be published as workflow-agents, and **the `composed` runtime binding
  (`RUNTARA_DIRECT_RUNTIME_BINDING=composed`) cannot compile them**; compile
  them with the default host-imported runtime.
- **Trusted agent pins are checked at the call, not at load.** A trusted
  agent upgrade no longer stops workflows that pin the previous version from
  loading; their trusted calls fail with `TRUSTED_VERSION_REQUIRED` before any
  credential lookup. Compiles record their trusted pins (server migration
  `20260926000100_compiled_trusted_pins`), and a workflow whose recorded pin is
  no longer installed recompiles when it next becomes ready. A parent built on
  a stale published workflow-agent recovers once the agent is republished.
- **Runs parked on an older trusted agent version keep working after an
  upgrade.** Boot now records the installed S3 and Azure trusted versions in
  the approved history (`approved_builtin_artifacts`, environment migration
  `20260927000300_approved_trusted_artifacts`). When a run parked under an
  older, still approved version is woken or resumed (paused waits included),
  its trusted calls run the installed bytes instead of failing with
  `TRUSTED_VERSION_REQUIRED`. A new start under an older pin still fails (and
  readiness still recompiles it), as does any launch under a revoked or never
  approved pin, always before any credential lookup. Revoking the installed
  version itself denies every call to that agent until a new version is
  installed. The launch kind comes
  from the durable launch queue, never from the workflow; the pin never
  chooses which bytes run, and credentials are still resolved per tenant,
  connection and type. To cut off runs parked on a version, revoke its pin
  (`revoked_at`); it takes effect at the next boot.
- **Finished children kept for a live parent are pruned.** Each cleanup pass,
  after deleting, strips a finished child that retention keeps only because
  its parent is still running or parked, once it is past
  `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS` by its own finish: its checkpoints,
  signals, closed input requests, invocation state, input and stderr are
  removed. Its row and outcome (status, output, error, parent link, run label),
  events and accepted input receipts stay, so `get`, WaitForInstances and replayed
  `send-signal` calls answer as before. No new setting; the pass logs
  `pruned_children`. A pruned child's checkpoints and input can no longer be
  inspected.
- **Upgrading is one-way.** This release migrates the runtime database
  (`032`-`042`) and the environment and server databases (`20260926000100`,
  `20260926000200`, `20260927000000`-`20260927000300`), adds
  `termination_reason` labels (`start_gate_failed`, `waiting_instances`) and
  stores control state (parent links, admission outcomes, command receipts,
  instance waits, agent continuations) that older servers do not read.
  Migrations are forward-only: do not downgrade past this release. A
  downgraded server would not decode those labels, would strand runs parked
  on a WaitForInstances step, and would re-run the compile burst below.
- **Agent components are now held to an import allowlist at composition.** A
  bundled or third-party agent component may import only `wasi:*`,
  `runtara:agent/types@*` and the host interfaces the component host links
  (host-io timers, outbound HTTP, the trusted executor, the connection
  resolver, and the object-model database). Any other import fails composition
  with an error naming the agent and the import. Published workflow-agents
  staged by the server for a tenant skip the list, but may never import
  `runtara:control/*` or `runtara:workflow-operation/*`; a `workflow-agent`
  tag in the sidecar of a component in the primary components dir does not
  count. **Operators shipping custom agent components should check their
  imports (`wasm-tools component wit`) before upgrading.**
- **A parent calling a published workflow-agent needs that agent in a staging
  or extra components dir.** The parent re-raises a workflow-agent's reserved
  park and suspend codes, so an agent the catalog calls a workflow-agent but
  whose component sits in the primary components dir now fails composition.
  The server already stages published workflow-agents per tenant. With
  `runtara-compile`, move the agent's `.wasm` and `.meta.json` out of
  `--components-dir` and pass their dir with the new repeatable
  `--extra-components-dir` flag.
- **`RUNTARA_REQUEST_TIMEOUT_MS` is deprecated as a runtara-server setting; use
  `RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS`.** One name was read by two unrelated
  components meaning two different things in two different units: in runtara-sdk
  it is the per-request HTTP client timeout, in milliseconds; in runtara-server
  it was divided by 1000 and used as the default execution timeout for a workflow
  instance that names no `executionTimeoutSeconds` of its own — a kill deadline,
  enforced by stopping the instance and recording it `failed` with
  `termination_reason: timeout`. Setting it for one silently moved the other, and
  neither site said so. The SDK keeps the name; the server's timeout is now named
  in the unit it is kept in, with no division. **The old name is still read when
  the new one is unset, so no deployment changes behavior on upgrade**, but the
  server logs a deprecation warning quoting both the milliseconds found and the
  seconds derived. Note the old conversion truncates: a value under `1000`
  becomes a zero-second timeout, under which every workflow is killed the instant
  it starts. The resolved timeout is now logged at startup regardless of which
  name supplied it, and `RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS=0` is refused
  rather than honored.

- **BREAKING: `GET /api/runtime/workflows` now takes a 0-based `page`**, matching
  every other paginated endpoint in the runtime API (`/executions`,
  `/workflows/{id}/instances`, `/checkpoints`, `/actions`). It was the only
  1-based one, while still reporting a 0-based `number` in the response — so
  `number + 1` re-requested the page just read and never reached page 2, and
  `?page=0` and `?page=1` both returned the first page. `number` now echoes the
  page that was asked for. **Callers passing `page=1` for the first page must
  change it to `page=0`, or they will silently skip the first page.** The MCP
  `list_workflows` tool takes the same 0-based `page`.
- **Encoding-sensitive agents (text, csv, xml) now share one encoding
  vocabulary** via the new `runtara-agent-encoding` crate. Their `encoding`
  inputs become a curated dropdown of standard names plus `Auto` (detect),
  while still accepting any standard label or alias. Notable behavior changes:
  - The `text` agent now decodes `ISO-8859-1` / `LATIN-1` as **windows-1252**
    (WHATWG aliasing). This matches the `csv` agent's pre-existing behavior and
    correctly handles bytes `0x80`–`0x9F`; the previous text-agent path mapped
    them as raw code points.
  - The `csv` agent no longer silently falls back to lossy UTF-8 when given an
    unrecognized encoding label — unknown labels are now rejected at input
    validation. It also strips a leading BOM (previously left in the first
    field).
  - The `xml` agent now supports non-UTF-8 encodings (previously UTF-8 only,
    with a TODO) and BOM stripping.
  - Decoding is lossy across all three (malformed bytes become U+FFFD) rather
    than erroring; `Auto` enables automatic detection (BOM + chardetng).
- **Retention defaults are now on and aligned at 3 days.** Previously
  `DbCleanupWorker` and `ImageCleanupWorker` were off-by-default and the
  `CleanupWorker` ran at a 24-hour retention. All four workers now default
  to enabled, 3-day retention. Operators who relied on long retention (or
  silent cleanup) must opt out before upgrading:
  - `RUNTARA_DB_CLEANUP_ENABLED=false` / `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS=<n>`
  - `RUNTARA_IMAGE_CLEANUP_ENABLED=false` / `RUNTARA_IMAGE_CLEANUP_MAX_AGE_DAYS=<n>`
  - `RUNTARA_RUN_DIR_CLEANUP_ENABLED=false` / `RUNTARA_RUN_DIR_CLEANUP_MAX_AGE_DAYS=<n>`
  - `RUNTARA_INVOCATION_CLEANUP_ENABLED=false` / `RUNTARA_INVOCATION_CLEANUP_MAX_AGE_DAYS=<n>`
  On first run after upgrade, existing data older than 3 days in terminal
  states will be deleted.

### Removed

- **The internal API listener (`INTERNAL_PORT`, default `7002`) is gone.**
  Running workflows stopped calling it when agent, object-model and proxy calls
  moved to in-process host imports; all it still served was connection
  re-encryption plus duplicates of `/health` and `/ready`. `runtara-server` now
  binds only the public port. `INTERNAL_PORT`, `INTERNAL_HOST` and
  `RUNTARA_INTERNAL_SHARED_SECRET` are ignored. Point any health check that
  targeted `7002` at the public port's `/health` instead.
- `POST /api/internal/connections-admin/reencrypt` is replaced by a CLI
  subcommand that uses the same database and key configuration as the server:
  `runtara-server reencrypt-connections [--tenant-id <id>]`. It prints the
  scanned/re-encrypted/unchanged/failed counts and exits non-zero if encryption
  is off or any row fails.

### Fixed

- **Step summaries list each step once.** A resumed run replays its
  completed steps and re-enters a parked one, which emitted their start and
  end events again, so `GET .../instances/{iid}/steps` repeated those steps
  (up to four rows for one step). Each step and scope is now one row, from
  its first start to the first end after it; a step parked in a Delay or
  WaitForInstances keeps its original start time.
- **A guest error carrying the reserved `__rt_on_signal__` code no longer
  parks its parent.** Like the other reserved codes it is remapped so the
  step fails, and agent guests no longer receive `RUNTARA_HTTP_URL` in their
  environment.
- **A run whose start gate fails now fails promptly** instead of later as
  `launch_queue_timeout`. The instance row records termination reason
  `start_gate_failed`. That label was written without ever existing in the
  `termination_reason` enum, so the update failed and the run waited for the
  launch queue to time it out; a forward migration adds the label.
- **`GET /workflows/{id}/instances/{instanceId}/checkpoints` now honors
  `page`.** The handler normalized the page number and then dropped it, asking
  the store for a limit and no offset — so every page re-read the first `size`
  rows and `?page=1` returned page 0 again, however large the instance's
  checkpoint history. The page now also keeps the store's documented order —
  `(created_at, checkpoint_id)` descending, newest first — instead of being
  re-sorted by checkpoint id under a comment calling that chronological: ids
  are step-derived (with `::retry::{n}` / `::attempt::{n}` suffixes), so that
  sort was neither a time order nor one the pages themselves followed.
  **`seq` changes meaning**: it now numbers the row within the full
  ordered list (page 2 of size 20 starts at 40) rather than restarting at 0 on
  every page — and it is no longer stamped before a re-sort that then scrambled
  it against the order the rows it was attached to came back in.

- `*_CLEANUP_ENABLED` env-var parsing across all four cleanup workers
  previously treated **any** value other than `"true"` or `"1"` as
  disabled — including misconfigurations like `"yes"`, `"on"`, `"True"`,
  or a typo, all of which silently turned cleanup off. The parse is now
  inverted: cleanup is enabled by default and only an explicit false-like
  value (`"false"`, `"0"`, `"no"`, `"off"`, `"disabled"`,
  case-insensitive) disables it. Anything else — unset, malformed, or
  truthy in any common spelling — leaves cleanup running.

## [3.0.0]

### Changed (BREAKING)

- Renamed the core primitive `Scenario` → `Workflow` across the whole
  codebase — Rust types, REST endpoints, database schema, MCP tools,
  frontend routes, and generated API clients.
- Renamed the nested-workflow DSL step `StartScenario` → `EmbedWorkflow`.
  Serde discriminator `"stepType": "StartScenario"` becomes
  `"EmbedWorkflow"`; struct fields `childScenarioId` / `childScenarioVersion`
  become `childWorkflowId` / `childWorkflowVersion`.
- REST API: every `/api/runtime/scenarios/*` path moves to
  `/api/runtime/workflows/*`; path param `:scenarioId` becomes `:workflowId`.
- MCP tool names: `scenarios.*` → `workflows.*`.
- Error codes: `CHILD_SCENARIO_FAILED` → `CHILD_WORKFLOW_FAILED`.
- Telemetry env var: `SCENARIO_ID` → `WORKFLOW_ID` (OTel resource attribute).
- Database schema: tables `scenarios`, `scenario_definitions`,
  `scenario_executions`, `scenario_execution_events`,
  `scenario_compilations`, `scenario_metrics_hourly`, and
  `scenario_dependencies` are renamed to their `workflow*` equivalents,
  along with every `scenario_id` column. Forward migration
  `20260419000000_rename_scenarios_to_workflows.sql` performs the rename
  idempotently with `ALTER TABLE ... RENAME`.

### Migration notes

- No backward-compat shims. SDK consumers, REST clients, frontends, and
  operators must update together. If you have operators exporting
  `SCENARIO_ID=...` for OTel, switch to `WORKFLOW_ID`.
- Historical rows in `error_history.error_code = 'CHILD_SCENARIO_FAILED'`
  are left as-is; only new errors use the new code.
  **Correction:** `error_history` was dropped in
  `crates/runtara-store-postgres/migrations/postgresql/017_drop_structured_errors_and_schedules.sql`.
  Nothing ever wrote to the table, so there were no historical rows to leave
  as-is and nothing to migrate. Comments elsewhere that plan a rewrite of
  those rows — including one in
  `crates/runtara-server/migrations/20260419000000_rename_scenarios_to_workflows.sql`,
  which is applied and checksummed and so cannot be corrected in place —
  describe work that does not exist.

## [1.8.0] - 2026-04-13

### Added

- Compilation queue for serialized scenario compilation.

### Changed

- Internal API default port moved from `7001` to `7002`. `7001` remains the
  public `runtara-server` HTTP API port; `7002` is the internal service port.

### Fixed

- Agent testing dispatcher routing.
- `Default` impl for `ExecutionGraph` so downstream tests compile.
- sqlx offline cache miss in CI by switching `sqlx::query!` → `sqlx::query_as`.

## [1.7.0] - 2026-04-10

### Added

- Automatic rate-limit honoring for integrations: 429 responses (and equivalent
  provider codes) trigger durable sleep until the indicated `retry_after`
  without consuming the normal retry budget. Configurable via
  `AUTO_RETRY_ON_429`, `MAX_429_RETRIES`, and `MAX_RETRY_DELAY_MS`.

## Earlier releases

Tagged releases `v1.0.21` (2026-04-15) through `v1.6.18` predate this
changelog. Notable platform-level changes during that period — reconstructed
from the workspace crates and configuration — include:

- **New crates:** `runtara-server` (HTTP API server embedding environment +
  core), `runtara-connections` (connection/credential management),
  `runtara-object-store` (schema-driven dynamic PostgreSQL object model),
  `runtara-http` (portable HTTP client for native/WASI/browser-wasm),
  `runtara-ai` (WASM-first LLM completion client), `runtara-text-parser`
  (Slack/SMS/CLI text-channel adapter).
- **Scenario compilation targets WASM by default.** The default
  `RUNTARA_COMPILE_TARGET` is `wasm32-wasip2`; the native-musl path is
  retained as a fallback and flagged for cleanup.
- **Valkey/Redis is now a required runtime dependency** for
  `runtara-server` scenario execution (`VALKEY_HOST` env var).
- **Wasm is the default runner** in `runtara-environment`; OCI, Native, and
  Mock runners remain available.
- **Removed:** `runtara-protocol` crate (never existed on main; the reference
  in earlier documentation was stale).
