# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> This file tracks release notes starting at **1.7.0**. Earlier tagged releases
> (`v1.0.21` through `v1.6.18`) are available as git tags; their history lives
> in `git log`.

## [Unreleased]

### Added

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
  which takes effect at the next boot, after which calls are denied and
  workflows pinning it no longer become ready. Composed artifacts that import
  a component or core module, or whose agents import
  `runtara:workflow-operation`, are now refused at preparation.

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
- **Validation checks where suspending and control-agent steps may appear.**
  A step whose capability suspends must be durable (E028) and set a timeout
  above zero (E029), and may not run in an onError handler, a WaitForSignal
  `onWait` or as an AiAgent tool or memory provider (E131); a control-agent
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
