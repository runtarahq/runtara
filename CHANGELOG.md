# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> This file tracks release notes starting at **1.7.0**. Earlier tagged releases
> (`v1.0.21` through `v1.6.18`) are available as git tags; their history lives
> in `git log`.

## [Unreleased]

### Added

- **Workflow images live in a content-addressed artifact store.** A new
  `runtara-artifact-store` crate defines `ArtifactStore` (put, get, exists,
  local path and delete by SHA-256 digest), with a filesystem store at
  `<DATA_DIR>/artifacts/blobs/sha256/<digest>` as the default. An image row
  records `binary_digest` instead of a path (environment migration
  `20261027000000`, backfilled from the checksum every image recorded), and
  `LaunchOptions` carries `workflow_digest`, which the runner resolves through
  its own store (`RunnerParts::artifacts`; a host supplies its own through
  `ServerOptions::artifacts`). Images with the same bytes share one artifact,
  released when the last image naming it is deleted. On boot, images stored
  the old way (`images/<id>/binary`) are moved into the store, except a file
  whose bytes no longer match the checksum its image recorded.
  `e2e/test_image_artifact_store.sh` covers both.

- **Platform agents load from a release in the artifact store.** Each
  agent's `.wasm` and `.meta.json` are one record in the store, a manifest of
  the content digest and the metadata's exact bytes, written after the
  content (`agents/<id>/<record>.json`; shared components under
  `components/`), so the two can no longer drift. A release index
  (`agents/index/<release>.json`) pins every record by digest. At boot the
  components directory is published as the server's release
  (`RUNTARA_AGENT_RELEASE`, default the server version), and the component
  dispatcher, agent catalog and entitlement allowlist load from that index
  instead of a directory listing. Without `RUNTARA_AGENT_COMPONENTS_DIR`, the
  server runs the release the store already holds. A record whose content is
  missing or does not match its digest is refused.

- **Workflows compile against the release.** The release is materialized for
  composition under `<DATA_DIR>/releases/<index digest>`, every component
  verified against the index, and the compiler reads its agents and shared
  components there unless `RUNTARA_DIRECT_WASM_COMPONENTS_DIR` overrides it.
  An image records `workflow.componentRelease` (the release and the digest of
  its index) beside the digest of every component it was composed from, and
  the release is part of the compile cache identity, so a new release
  recompiles a workflow instead of reusing an artifact built from other
  agent bytes.

- **Published workflow-agents are records in the artifact store.** Publishing
  records the composed `.wasm` and its `.meta.json` (byte for byte, with the
  lowering it was built with) under
  `tenants/<hex tenant>/workflow-agents/<slug>.json`, then writes the staging
  dir, which is now a cache: at boot it is rebuilt from the records, so a
  lost `DATA_DIR/workflow-agents` loses nothing, and agents staged before the
  store are recorded from their files. Unpublishing removes the record first.

- **Artifacts can live in object storage.** `RUNTARA_ARTIFACT_STORE_URL`
  (`s3://bucket/prefix`, `gs://…`, `az://…`, `file://…`) puts every artifact
  (workflow images, the release and its agent records, published
  workflow-agents) in a bucket through the `object_store` crate, configured
  from the provider's standard environment variables (`AWS_ENDPOINT`,
  `AWS_ACCESS_KEY_ID`, …). The layout is the filesystem store's, content is
  verified against its digest on every read, and local readers (the runner,
  the compile view) get files from a verified cache under
  `<DATA_DIR>/artifact-cache`. A server with an empty data dir and no
  components directory loads its agents from the bucket, and compiles and
  runs workflows there (`e2e/test_object_store.sh`, against MinIO). TLS is
  rustls on ring, so no C crypto build is added.
  `ArtifactStore::presigned_get` signs a credential-free GET URL for an
  artifact's content on S3, GCS and Azure stores (the filesystem store
  returns none), for readers such as fleet nodes that must not hold the
  store's credentials.

- **A server serves an explicit set of tenants.** `TENANT_ID` is the primary
  tenant and `RUNTARA_SERVED_TENANTS` (comma-separated) adds more; one by
  default. Requests and work for any other tenant are refused: an OIDC token
  or an API key whose org is not served gets 403 (an API key's org was not
  checked before), and an HTTP trigger webhook runs under its trigger's own
  tenant only when that tenant is served (404 otherwise); the sync webhook
  finds the workflow among the served tenants. Product events from
  background work carry the work's tenant, and a run's `RUNTARA_TENANT_ID`
  is its own tenant, not the process's. Each served tenant gets its own
  trigger-stream consumers, cron scheduler (triggers no tenant owns run once,
  as the primary tenant), session delivery, default object-model connection
  and workflow-agent restage. Membership roles are read per tenant: the
  primary tenant keeps `member:{user}`, another served tenant uses
  `member:{tenant}:{user}`. MCP tools act for the calling request's tenant,
  not the one the server was built for. Channel webhooks and the intake
  worker take the tenant from the channel connection, which must be served,
  and Teams reply refs carry it. Control and instance waits accept any served
  tenant. `e2e/test_served_tenants.sh` serves two tenants from one process
  and checks neither reads the other's workflows, versions or runs.

- **The UI switches tenants.** Where the UI is served at the bare mount, each
  served tenant also has its own copy at `/ui/{tenant}/`, with that tenant's
  base href, id and entitlements, so one server's SPA works for any of its
  tenants without a gateway. The SPA takes its tenant from that path. A user
  signed in with a token that names no tenant is sent to their only workspace or
  picks one, and the sidebar offers a workspace switcher when they belong to
  several (`GET /api/runtime/me/tenants`). `RUNTARA_UI_OIDC_ORGANIZATIONS=false`
  stops the login from sending Auth0's `organization` parameter. A served tenant
  that inherits the primary's entitlements now reports its own `tenantId`.

- **The request names the tenant; membership authorizes it.** A path
  `/api/runtime/{tenant}/…` or `/mcp/{tenant}` naming a served tenant is routed
  as the tenant-less path, for that tenant (the SPA's org-prefixed URLs now work
  without a gateway stripping them). With `RUNTARA_AUTH_MEMBERSHIP_POLICY=required`,
  an OIDC token no longer needs an `org_id` claim: it acts for the tenant the
  request names (else the primary), authorized by `{tenant}:member:{sub}` there.
  A token or API key bound to one tenant is refused under another's path. New
  `GET /api/runtime/me/tenants` lists where the caller may act, with the role.
  Under any other policy `org_id` is still required, so existing deployments
  behave as before. The JWT issuer is recorded on the auth context.

- **One Valkey can serve many tenants.** Every key a server uses names its
  tenant: membership `{tenant}:member:{uid}`, revocation
  `{tenant}:token:revoked:{jti}`, connection rate limits
  `{tenant}:rate_limit:{connection}`, the compilation queue
  `{tenant}:compilation:*` (a server consumes only its served tenants' queues)
  and the admission counter `{tenant}:admission:reserved` (seeded and
  reconciled only for served tenants; previously a server reseeding or
  reconciling would reset every tenant's counter on the same Valkey). The
  user-management contract is now version 2 (`docs/security/user-management-contracts.md` §3,
  including every runtara key). Version 1 keys still work: `member:{uid}` answers
  for the primary tenant unless `RUNTARA_AUTH_LEGACY_MEMBER_KEYS=false`, which a
  shared Valkey must set, and `token:revoked:{jti}` still revokes; the old
  compilation queue is drained into the new one at boot. Tenant ids may not
  contain `:`. `e2e/test_shared_valkey.sh`.

- **OAuth callbacks and connection events belong to the connection's tenant.**
  An OAuth state is redeemed only at the callback URL of the tenant it was
  issued for (`/api/oauth/{tenant}/callback`); one presented at another
  tenant's URL is refused and spent. Connection lifecycle product events
  (created, deleted, OAuth started/completed/failed, token refreshed) carry
  the connection's tenant instead of the process's primary tenant:
  `ConnectionEventSink::emit` now receives it (`None` for a callback that
  failed before its state matched, which the server files under its primary
  tenant rather than trust the URL), and
  `provider_auth::resolve_connection_auth` takes it.

- **Connection credentials are bound to their tenant and connection.** New
  writes store a version 2 envelope whose AES-256-GCM associated data is the
  tenant and connection id, so a ciphertext copied to another connection or
  tenant does not decrypt; version 1 envelopes still decrypt, unbound, until
  `runtara-server reencrypt-connections` rewrites them. The cipher is a
  keyring: the current key (`RUNTARA_CONNECTIONS_ENCRYPTION_KEY`, `_KEY_ID`)
  encrypts, and retired keys (`RUNTARA_CONNECTIONS_RETIRED_KEYS`,
  `kid:base64,…`) still decrypt the envelopes that name them, so a key is
  rotated by adding the new one, re-encrypting, then dropping the old.
  `e2e/test_credential_binding.sh`.

- **Every instance-scoped table records its tenant.** Store migration 053
  adds `tenant_id` to checkpoints, events, signals, state, waits, leases,
  attempts, continuations and control receipts, and environment migration
  `20261028000000` to workflow API receipts. Existing rows are backfilled
  from their instance (or launch), and a `BEFORE INSERT` trigger fills new
  rows the same way, so no write path changes. Store migration 054 and
  environment migration `20261028000100` then enable and force row-level
  security on every tenant-bearing table: a connection that names a tenant
  (`runtara.tenant_id`, set by `PostgresPersistence::for_tenant`) reads and
  writes only that tenant's rows, so a read of another tenant's fails closed,
  while a connection that names none is the system path (today's pools and
  the cross-tenant workers) and is unchanged. Row-level security does not
  bind a superuser or a `BYPASSRLS` role, so a tenant-scoped pool must log in
  as a plain role.

- **Entitlements are per tenant.** Every entitlement check (features,
  agents, limits, API keys, SQL, trusted calls, concurrent executions, bulk
  sizes) reads the tenant it acts for through
  `config::entitlements_for(tenant)`. The default source is the environment:
  the primary tenant's `RUNTARA_PRICING_TIER` / `RUNTARA_ENTITLEMENTS_JSON` /
  `RUNTARA_ENTITLEMENT_OVERRIDES_JSON` as before, and for another served
  tenant its entry in `RUNTARA_TENANT_ENTITLEMENTS_JSON`
  (`{"<tenant>": {"pricingTier", "entitlements", "overrides"}}`), else the
  primary's. A host supplies its own through `ServerOptions::entitlements`
  (`config::EntitlementSource`). `GET /entitlements` answers for the caller's
  tenant.

- **The platform object-model database is optional.** Without
  `OBJECT_MODEL_DATABASE_URL` the server has no default database and creates
  no default `object_model` connection. The object model, SQL steps, CSV
  import/export and the MCP database tools then work through a tenant's own
  Postgres connection, and a request for the default answers that one is
  needed. Tenant database connections pass an address guard, enforced by
  default without a platform database (`RUNTARA_DATABASE_CONNECTION_GUARD`):
  loopback, private, metadata and local-socket addresses are refused before
  anything connects (allowlist: `RUNTARA_PROXY_ALLOWED_HOSTS`), a URL that
  names its target in a `host` or `hostaddr` parameter is refused, and the
  connection is pinned to the vetted address, checked by re-parsing it the
  way the connector does. `OBJECT_MODEL_TENANT_POOL_MAX`
  (default 16) caps the pools one tenant's connections hold open. With the
  variable set, behaviour is unchanged. `e2e/test_optional_platform_db.sh`.

- **A runner can report an executor lost.** `RunExitReport::Lost` (for a
  node or link that went away) hands the run to recovery instead of failing
  it: recovery suspends it (`termination_reason = executor_lost`, a new value
  added by store migration 052), which revokes its root lease, and relaunches
  it from its checkpoints, under the crash-loop cap. A lost executor that
  keeps running is stopped at its next durable write, and its workflow API
  calls answer `fenced` (not a retryable `unavailable`) from then on.
  `e2e/test_lost_executor_fenced.sh` loses a launch mid-run and checks the
  run finishes once.

- **A stale executor cannot acknowledge a command or post an outcome.**
  Lifecycle acks (pause, cancel, shutdown) are admitted by the run's root
  lease like every other guest write (`Persistence::apply_owned_lifecycle_command`,
  with a conformance case), so a superseded launch leaves the command pending
  for the launch that holds the run. `POST /v1/run/outcome` reads the lease
  first and answers `fenced` to a launch that lost it.

- **The workflow runner is pluggable.** A host binary can boot the stock
  server with its own runner: `runtara_server::boot::run(ServerOptions {
  runner_factory })`, where the factory builds a `Runner` from the parts the
  embedded one is built from (`runtara_environment::runner::RunnerParts`;
  `build_embedded_runner` is the default). `runtara-runner-delegate` is an
  out-of-tree runner delegating to the embedded one, with a
  `runtara-server-delegating` binary the e2e suites run against.
  `RUNTARA_RUNNER` is no longer read (it was ignored already).

- **DSL 3.7.0: a step id an `onWait` graph shares with the graph around it is
  rejected (E152).** An `onWait` graph runs at its WaitForSignal step's
  position, so a step there with the id of a step around it, or of a step in
  another `onWait` graph at that position, shared that step's checkpoints:
  one could return the other's result, read the other's child rows, or
  delete its saved work. E152 names the id and where it is used. It counts
  only steps that keep checkpoints: every step but Finish, Log, Error,
  Conditional, Switch, Filter and GroupBy, which count too when they have a
  breakpoint. So a `finish` in the workflow and one in its `onWait` stay
  valid. A Split or While body is a position of its own, and the `onWait` of
  a WaitForSignal that only an AiAgent tool reaches never runs, so neither
  is compared with the graph around it. The editor never produced such ids,
  so this reaches only graphs written through MCP or the API:
  - saving a workflow (`PUT`/`POST …/update`, MCP `deploy_workflow`),
    `validate_graph` and MCP `deploy_latest` fail with E152;
  - an MCP graph edit still saves, with E152 as a warning like every other
    error, since a sequence of edits may pass through a reused id;
  - compiling (`compile_workflow`, `set_current_version`) does not validate,
    so a workflow saved before keeps compiling and running.

  Validation covers a workflow's embedded children, so a parent whose child
  reuses such an id fails to save until the child is renamed and saved.

- **DSL 3.6.0: fixes from the DSL audit** (`docs/dsl-audit-2026-10-14.md`).
  Graphs the compiler cannot build, non-durable Delays, missing Split or
  Switch config and mistyped variables are rejected at save (E138–E151)
  instead of failing at compile or run time, step ids and variables that
  shadow runtime bindings are rejected (E136, E137), and W084–W092 flag keys,
  settings and references that are ignored or do something unexpected. `workflow.inputs` is the run's input in every scope, onError
  handlers route their own failures, a value condition can carry a type hint,
  and AiAgent turns retry and report provider throttling as `AI_RATE_LIMITED`.
  Millisecond fields are now `timeoutMs`, `retryDelayMs` and `turnTimeoutMs`;
  the old `timeout`, `retryDelay` and `turnTimeout` still read and warn
  (W093), and the editor rewrites them to the new names the next time a
  workflow is saved. Type hints gain `object` and `array` (`json` is
  deprecated), variables gain `connection`, and GroupBy outputs `totalGroups`
  beside the deprecated `total_groups`. The changelog at
  `/api/runtime/specs/dsl/changelog` lists the deprecated spellings that DSL
  4.0 is to remove.

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
  codes; a control call is bounded by the step's deadline and 90 s
  (`CONTROL_TIMEOUT`), and `CONTROL_UNAVAILABLE` is retryable. Control is an
  ordinary composed agent: it calls the host's control service from the run's
  own store, and only the built-in control agent may import `runtara:control`,
  which the compiler enforces. A run parked across an upgrade keeps the control
  version composed into it; a new version reaches existing workflows when they
  recompile. The runtime table `approved_builtin_artifacts` (migration
  `20260927000000`) holds the approved history of trusted built-ins.

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
  calls running on the control agent composed into it: every shipped host
  interface version stays linked (the released `runtara:control`,
  `runtara:workflow-operation`, `runtara:workflow-wait` and
  `runtara:agent-suspension` 0.1.0 interfaces are frozen and tested).
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

- **Breaking: workflows and agents reach the platform only through the
  workflow API, so every workflow must be recompiled.** A compiled workflow
  now carries the `runtara:api` component and calls the server's `/v1`
  workflow API for its run services: checkpoints, signals, events, sleeps,
  operations, instance waits, state, connections, HTTP, SQL, control and the
  child runs of isolated and scoped agents (`/v1/tasks`).
  Agents do the same through the agent SDK. The host interfaces they
  imported before are no longer linked: `runtara:workflow/*` (`runtime`,
  `operation`, `waits`, `state` and `tasks`), `runtara:host/*`,
  `runtara:control/*`, `runtara:trusted/executor` and
  `runtara:agent/continuation`.
  - Recompile every workflow after upgrading (compile it, or set its current
    version again). An artifact built before is refused at load: "built for
    the removed host ABI … recompile the workflow".
  - Runs are not carried over. A run parked, paused or waiting on an old
    artifact cannot wake on an upgraded server; let such runs finish, or
    cancel them, before upgrading.
  - Third-party agents built against the old WIT must be rebuilt with the
    agent SDK. A component that imports any of the interfaces above is
    refused when a workflow is compiled against it, and at load.

- **Wasmtime 50 (from 46).** The WASI 0.3 interfaces are unchanged, so agents
  need no rebuild. Wasmtime 50 drops the `cancellable` waits
  (component-model #716): only the event loop of a callback-lifted export
  delivers a caller's cancel.
  - Published workflow-agents are now lifted async with a callback. The
    emitted body is unchanged; the compiler instruments it so that each wait
    returns to the event loop and resumes where it stopped. Workflow-agents
    published before this release are republished from their versions when
    the server boots: their artifacts use `cancellable` waits that Wasmtime
    50 refuses to load. One that cannot be republished is logged, and
    compiling a workflow that calls it asks for a republish.
  - Raw `wasi:http@0.2` stays denied; `wasi:http@0.3` reaches only the
    workflow API.
  - A workflow API request is now served on a task of its own, so it makes
    progress while the guest runs CPU-bound code. A cleanup alarm armed just
    before such code starts its clock at once rather than at the guest's
    next yield.

- **A workflow-agent that can park runs as a child run of its own.** Before,
  a sleeping or waiting workflow-agent parked its whole calling tree; now its
  caller parks on a wait for it, and the caller's other children keep going.
  - Publishing records whether the version parks. Agents published before
    this release stay composed into their caller until republished.
  - The step's `timeoutMs` is the child's deadline: past it the caller
    wakes, cancels the child and fails the step with `AGENT_TIMEOUT`.
  - A parallel Split starts up to 100 such children per window (its
    `parallelism`), then parks on one wait over all of them.
    Parallel branches headed by such calls start all their children before
    the first branch parks.
  - Each call is a run with its own instance row, costing a launch instead
    of an in-process call.
  - A start refused because the tenant's limit (or control's share of it)
    is full is not an error: the caller parks for the refusal's retry hint
    and starts the child again.

- **Breaking: a workflow's memory now limits its runs, and it is a number.**
  A workflow version has a memory in MiB (`memoryMb`, 1 to 65536, 256 by
  default), which caps the sum of a run's linear memories; each child Store
  of an isolated or scoped agent gets the same cap. The S/M/L/XL tiers were
  stored but never applied; they are now presets (8, 64, 128, 256 MiB) that
  fill in the number, and existing versions keep their tier's value. A run
  that grows past its memory fails with "workflow trapped: guest memory limit
  exceeded".
  - Compiling records the workflow's memory floor, what it allocates when it
    starts, and refuses a memory below it ("below this workflow's memory
    floor of N MiB"). The editor shows a MiB field with the presets, the
    floor, and disables presets below it.
  - API: create and update take `memoryMb`; `memoryTier` is still accepted
    as a preset (deprecated), and workflows report `memoryMb` instead of
    `memoryTier`. MCP `update_workflow` takes `memory_mb` (or `memory_tier`).
  - `RUNTARA_INSTANCE_MEMORY_MAX_BYTES` stays the operator ceiling (1 GiB by
    default), now for the sum of a run's memories rather than for each
    memory. A workflow's memory never raises it.
  - A workflow that needs more than its memory fails where it ran before:
    raise its memory. A version compiled before this release runs under the
    ceiling alone until it is recompiled.
- **BREAKING: a Telegram Channel trigger that holds no webhook secret
  rejects inbound calls (401) instead of accepting them unauthenticated.**
  Since 9.0.0 runtara stores a webhook's secret before it calls Telegram, so
  a trigger it registers holds the secret Telegram was sent. **A bot that
  worked only because such calls were accepted goes silent on upgrade:**
  - a Telegram webhook set up by hand, outside runtara;
  - a trigger that lost its secret to a failed registration racing another
    one;
  - from releases before 9.0.0, which stored the secret only after Telegram
    answered, a trigger whose registration timed out, or whose follow-up
    write failed, after Telegram had applied the secret.

  Before upgrading, list the affected triggers on the server database:

  ```sql
  SELECT t.id FROM invocation_trigger t
  JOIN connection_data_entity c ON c.id = t.configuration->>'connection_id'
  WHERE t.trigger_type = 'CHANNEL' AND t.active
    AND c.integration_id = 'telegram_bot'
    AND COALESCE(t.configuration->>'webhook_secret', '') = '';
  ```

  To repair one, set `WEBHOOK_BASE_URL` and switch the trigger off and on, so
  runtara registers its webhook with a new secret.
- **A finished EmbedWorkflow or workflow-agent call keeps its result, not
  its child's rows.** Recording a durable EmbedWorkflow's result, or the
  result of an Agent step calling a workflow published as an agent, now
  deletes in the same transaction the checkpoints its child wrote, nested
  children included, and the step's own attempt, retry and retry-sleep rows:
  replay reads the result first and never runs the child again. An
  EmbedWorkflow's deadline rows go too; an Agent's deadline row stays, since
  it cannot be matched to the Agent's result. `A → EmbedWorkflow → B` and
  `A → workflow-agent → B` keep 3 rows whatever the child does, or 4 when the
  Agent has a timeout. The same goes for an isolated
  workflow-agent child, and for an EmbedWorkflow called as an AiAgent tool.
  Because a checkpoint key does not say whether an agent is a workflow, every
  durable Agent that retried and then succeeded drops its `::attempt::N`,
  `::retry_sleep::N` and `::retry::N` rows too, so the checkpoints listing
  (`GET …/instances/{id}/checkpoints`) no longer shows those retry records or
  their error messages once the step succeeds. A run killed or parked inside
  the child keeps the child's rows until the step finishes. As with Split,
  this applies to workflows already compiled with structured checkpoint keys,
  from runs started on this release; a `durable: false` step saves no result
  and collapses nothing. A step in an `onWait` graph that reuses the id of an
  Agent or EmbedWorkflow around it shares that step's child namespace, so
  such namespaces are never collapsed: Environment reads, when it starts a
  run, the step ids that occur inside `onWait` graphs anywhere in the
  workflow, its embedded children and the workflow-agents it calls, from the
  manifests compiled into the artifact, and keeps their child namespaces (the
  steps' own retry rows still go). This covers workflows compiled before this
  release. When the artifact's manifests cannot all be read, the run keeps
  every child namespace. **Upgrade note:** the first boot migrates the
  runtime database (`051_instance_shared_child_steps`, a nullable
  `instances.shared_child_steps` column, catalog-only); runs started before
  it keep every child namespace. Postgres now runs the delete in the statement
  that points the instance at the new result, as two ranges over the
  `checkpoints.scope` index, so it adds no round trip and never scans the
  run's other rows, even before the table has statistics (a Split's result
  could do that before). `A → Split[10000]{Agent} → B` takes as long as it
  did without the new rules.
- **An AiAgent keeps its latest turn, then one checkpoint row.** A durable
  AiAgent tool loop no longer keeps a snapshot and a model response per turn,
  each holding the whole conversation. It saves each turn whose tools ran in
  one of two alternating slots, and recording one deletes, in the same
  transaction, the other slot, the turn's model responses and its tool calls'
  checkpoints, so the stored bytes no longer grow with the square of the
  turns. A resume reads the two slots instead of every turn. Once the loop
  and the memory save finish, the step saves its result (its output's fields,
  not the conversation) and deletes its last slot, responses and memory
  calls' rows: `A → AiAgent → B` keeps 3 rows. A replay of a finished AiAgent
  now calls no model, loads and saves no memory, and no longer bills the
  memory summary again; a summarize compaction also saves its output, so a
  resume before the step's result does not repeat it either. Where a store
  cannot collapse the slots (an address over 2 KB), the loop also saves the
  turns under their per-turn keys and replays them as before. This applies
  to workflows compiled on this release; an older artifact keeps and replays
  every turn as before until it is recompiled. A run that started on an
  older artifact and resumes after the workflow is recompiled replays the
  per-turn snapshots it saved, and keeps them and the model responses it
  saved before this release until retention: a row keeps the address it was
  written with, so the new rules do not match it.
- **A finished While keeps one checkpoint row, and a resume skips it.** A
  While in a durable graph now checkpoints its result, `{iterations,
  outputs}`, once its loop exits, and deletes in the same transaction the
  checkpoints of its iterations and its loop-deadline rows. A replay takes the
  result from that row instead of running every iteration again, so a resume
  after a long loop no longer gets slower with each iteration, and
  `A → While[1000] → B` keeps 3 rows instead of one per step per iteration.
  A loop that fails, times out, is handled by `onError` or is suspended saves
  no result, and a run parked inside an iteration resumes from its rows as
  before. A loop body that writes local state (in an embedded child or a
  workflow-agent) carries a snapshot of it in the result. This applies to
  workflows compiled on this release; an older artifact replays every
  iteration as before until it is recompiled.
- **A finished Split keeps one checkpoint row.** Recording a durable Split's
  result now deletes, in the same transaction, the checkpoints of its
  iterations and its own attempt, retry and loop-deadline rows: replay reads
  the result first and never needs them. `A → Split[10000] → B` keeps 3 rows
  instead of 10,003. A run killed or parked inside an iteration keeps the
  Split's rows until the Split finishes. Once it has, the checkpoints
  listing (`GET …/instances/{id}/checkpoints`) no longer shows the
  iterations' rows, including the `::retry::N` records (with their error
  messages) of items that retried. This applies to workflows already
  compiled with structured checkpoint keys, from runs started on this release
  (rows written before migration 049 are not collapsed); a `durable: false`
  Split saves no result and collapses nothing.
- **Checkpoints record where they sit in their run.** Migrations 049 and 050
  add an indexed `checkpoints.scope`: an address derived from a structured
  (version 2) checkpoint key when the row is written, NULL for any other key.
  The rows inside one composite step share a prefix of it, so recording the
  step's result can delete them in the same transaction. No step collapses
  its rows yet; that lands per composite step. Building the index reads the
  whole `checkpoints` table once, and checkpoint writes wait while it does.
  Crash-loop recovery now measures a run's progress with
  `Persistence::checkpoint_progress` (the highest checkpoint row id) instead
  of the row count, which falls once rows are collapsed. Recovery markers are
  now stored as `id:<n>`. A marker in another form is not comparable and
  counts as progress, so an instance's first recovery after the upgrade, or
  after a rollback to an earlier release (which cannot read the new form),
  never counts against `RUNTARA_MAX_AUTO_RESTARTS`.
- **Core's HTTP instance API is off by default.** The loopback listener on
  `RUNTARA_CORE_HTTP_PORT` (8003) starts only with
  `RUNTARA_CORE_INSTANCE_API=true`. Workflows never used it; they reach Core
  in-process. An instance registered through its `register` route has no launch
  or registration in the Environment, so the heartbeat monitor recovered it as
  orphaned two minutes after it started, however healthy. Enable it for tests
  and debugging only. `RUNTARA_MAX_CONCURRENT_INSTANCES` and
  `RUNTARA_CORE_SHUTDOWN_GRACE_MS` only ever applied to this API, so they now
  have no effect unless it is enabled; the server warns when they are set
  without it.
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

- **The session delivery worker handles only its own tenant's queues.** It
  scanned every session queue in Valkey, so a server sharing Valkey with
  other tenants tried to deliver their queued responses through its own
  runtime, and warned about their corrupt queues. It now skips queues another
  tenant owns, and corrupt owner metadata it cannot trace to its own tenant.
  A corrupt queue of its own is still reported, but the same warning for the
  same queue now repeats at most once an hour instead of on every scan pass;
  the passes in between log at debug. A session that recovers warns again
  the next time it fails.
- **A Channel trigger whose workflow was deleted no longer takes over its
  bot's inbound messages.** Inbound calls were checked against, and routed
  to, the newest active Channel trigger on the connection, even when its
  workflow was deleted, while webhook registration skips such a trigger and
  sends the secret of the newest live one. A bot whose newest trigger
  belonged to a deleted workflow then rejected every Telegram update with
  401, and on other platforms its messages were acknowledged and dropped,
  since the deleted workflow has no version to run. Validation and routing
  now use the same newest live trigger as registration, so these messages
  reach that trigger's workflow.
- **A published workflow-agent that carries a built-in agent's id no longer
  lifts the allowlist for that built-in.** Saving (`/update`, MCP
  `deploy_workflow`), the graph patch (`PUT …/versions/{version}/graph`),
  compiling through `POST …/versions/{version}/compile` and
  `graph/validate` exempted every id the tenant had published as a
  workflow-agent, including one a built-in agent shipped with after the
  publication. A step with that id runs the built-in, so a tenant whose
  allowlist leaves the built-in out could save and compile a graph that
  calls it. These checks now exempt a published id only when no built-in
  agent has it, as agent lookup and the MCP step tools already did.

  A saved workflow, or an embedded child, with a step that calls such an id
  now fails to save, validate and compile with `AGENT_NOT_ENABLED`, even
  when it is already compiled. To keep it, enable the built-in for the
  tenant, or republish the workflow-agent under another slug and point the
  step at it.
- **Graph validation applies the checks saving applies.**
  `POST /api/runtime/workflows/graph/validate`, and so the MCP
  `validate_graph` and `preflight_compile` tools, could call a graph valid
  that `/update` then rejected. Validation now also:
  - checks every agent in the graph and its embedded children against the
    tenant's allowlist (`AGENT_NOT_ENABLED`), with published
    workflow-agents exempt as on save;
  - checks that every connection a step pins exists;
  - rejects a description over 1000 characters;
  - takes an optional `workflowId` and validates the graph under that id.
    A child that embeds the workflow is then checked against this graph,
    not its stored version, so a cycle the graph would close is found.

  The MCP `validate_graph` tool takes the id as `workflow_id`. The MCP
  deploy tools pass it.
- **Compiling checks the agent allowlist on every path, and refuses what it
  cannot check.** The MCP `compile_workflow` tool did not check the stored
  graph against the tenant's allowlist: a version saved before an agent left
  the allowlist still compiled, and its cached image was still returned as
  compiled. It now runs the check the REST compile runs, over the graph and
  its embedded children, before the cached image can answer
  (`AGENT_NOT_ENABLED`). The REST compile checked only the root graph when
  the embedded children failed to load, and skipped a child whose graph did
  not parse. Both compiles now refuse in those cases, and when the version's
  own graph fails to load or parse, as saving does. They also refuse when an
  embedded child no longer resolves (deleted, or its pinned version gone):
  the cached image still carries that child's agents, which the check can no
  longer see. And a compile of a version that embeds other workflows hands
  back its cached image only when the image records the children just
  checked (the same child versions and manifests); otherwise it rebuilds it.
  The image keeps the children it was built with, which may have changed
  since ("latest" or "current" moved, or a version edited in place). A
  rebuild that finishes after an MCP compile timed out is reused by the
  retry.
- **Running a workflow checks the agent allowlist.** Nothing between a run's
  request and its launch checked it: a version compiled before an agent left
  the allowlist kept running from its cached image, and one not compiled yet
  was compiled on the first run. Every launch (HTTP, MCP, cron, webhooks,
  events, sessions, chat, replay and `control` children) now runs the
  compile's check before the image starts or a compile is queued. A refused
  run never starts: its execution reads "The run never started" with the
  `AGENT_NOT_ENABLED` message. Starting a run over HTTP or MCP, and
  `control:start`, refuse it up front instead (`AGENT_NOT_ENABLED`, HTTP
  403). An image that passed is remembered for the life of the process, so
  later runs of it do not repeat the check. A compiled image is judged by
  the agents it recorded when it was registered, since it keeps the
  children it was built with, whatever the stored ones say now: an agent in
  it that the allowlist leaves out refuses the run, unless it is a
  workflow-agent the tenant publishes and no built-in has its id. The
  `ai-tools` agent every AiAgent step brings is not gated there. An image
  registered without that record is rebuilt once, before it runs; without
  Valkey, where nothing can rebuild it, it is left in place and its runs
  wait until the workflow is compiled with `forceRecompile`. A version not
  compiled yet is checked as stored and refused only for a forbidden agent
  the check finds; a graph it cannot read is left to the compile, which
  reports it as before.
- **`preflight_compile` checks the mappings of the right version.** It read
  its version from fields the workflow response does not have, so it
  reported version 0, and its mapping check of version 0 failed unseen. It
  now uses the requested or latest version, as `deploy_latest` does. Both
  tools now return an error when the mapping check fails to run, instead of
  reading it as no mapping errors.
- **`deploy_latest` and `preflight_compile` validate the graph of the
  version they work on.** Without a `version` they validated the graph of
  the deployed version while compiling and deploying a newer draft. A draft
  that fixed an error in the deployed version could not be deployed: the
  error in the deployed graph stopped it. They now fetch and validate the
  draft.
- **Agent lookup sees the tenant's published workflow-agents.**
  `GET /api/runtime/agents` did not list them, and
  `GET /api/runtime/agents/{name}` and `…/capabilities/{capability_id}`
  answered `AGENT_NOT_ENABLED` for them. The MCP `get_agent`,
  `get_capability` and `add_agent_step` tools failed the same way, and
  `list_unmapped_inputs` failed on a step that calls one. So a parent of a
  workflow-agent could be deployed through MCP as a whole graph but not
  built one step at a time. The editor's validator, which loads its agents
  from the same list, reported `E020 unknown agent` for such a step. The
  endpoints now serve a tenant's published workflow-agents to that tenant,
  exempt from the agent allowlist as on save. A built-in agent's id still
  wins, and other tenants do not see them.
- **The MCP deploy tools stop when graph validation fails to run.**
  `deploy_workflow`, `deploy_latest` and `preflight_compile` read a failed
  `graph/validate` call as a graph with no errors. Such a call fails, for
  example, with a 500 when embedded children cannot be loaded. The deploy
  tools then went on to save and compile the graph, and `preflight_compile`
  reported it ready. They now return the error.
- **Step-debug events show large step outputs.** With step events on, a
  step with a large output (a stored step result of about 16 KiB or more)
  recorded an internal reference such as `{"$wfref":1,"$wfnonce":…}` as its
  `step_debug_end` outputs, so the debug view showed nothing usable for it.
  The event now holds the step's result with each large field, such as
  `outputs`, replaced by the same `_truncated` summary as other large
  values: its type and its keys, length or size. This applies to workflows
  compiled on this release; recompile a workflow to pick it up.
- **Breakpoint events no longer show internal references for large
  values.** When a debug run stopped at a breakpoint, any earlier step
  result, input field or variable of about 16 KiB or more appeared in the
  breakpoint event as an internal reference such as
  `{"$wfref":1,"$wfnonce":…}`. The debug view then showed no outputs for
  that step, and a Conditional or EmbedWorkflow step paused at the
  breakpoint showed the references in its inputs. These parts of the event
  now use the same limits as step-debug events: each field of a step
  result, and each entry of `data` or `variables`, that is over 8 KiB or is
  an array of more than 50 items is replaced by a `_truncated` summary of
  its type and its keys, length or size. Other values appear in full. This
  applies to workflows compiled on this release; recompile a workflow to
  pick it up.
- **Numbers no longer change as they pass between workflow steps.** A value
  is re-read from JSON at every step, and the JSON parser could round a float
  to the neighbouring value, so `0.9093910740085555` reached the next step as
  `0.9093910740085556`. About one random value in ten could change this way,
  which matters for amounts, IDs and equality checks. Floats are now parsed
  exactly everywhere: in the server, the workflow runtime and every agent.
  The workflow runtime and agents are built into each compiled workflow, and
  an unchanged workflow keeps its compiled copy after an upgrade, so an
  already-deployed workflow keeps the old behaviour until it is recompiled:
  `POST /api/runtime/workflows/{id}/versions/{version}/compile?forceRecompile=true`
  or MCP `compile_workflow` with `force: true`. A workflow whose definition
  has changed gets the fix the next time it is compiled.
- **A workflow-agent whose slug has a word starting with a digit can be
  called.** A workflow named like `Q3 2024 sync` gets the slug
  `q3-2024-sync` and publishes as an agent, but every parent that called it
  failed to compile with `failed to parse direct workflow wac document:
  unexpected token was encountered`: the document that composes the parent
  requires every word of a name to start with a letter. It now writes such a
  word in a form it accepts, and the agent keeps its slug. Recompile a parent
  that failed this way; already-published workflow-agents need no change.
- **Graph validation sees the tenant's published workflow-agents.**
  `POST /api/runtime/workflows/graph/validate` checked a graph against the
  built-in agents only, so a parent whose Agent step calls a published
  workflow-agent failed with `E020 unknown agent`. That stopped the MCP
  `deploy_workflow` and `deploy_latest` tools before they saved or compiled
  the graph, and the MCP `validate_graph` and `preflight_compile` tools
  reported the same error, although `/update` accepts the graph. Validation
  now uses the same tenant-scoped catalog as saving.
- **An EmbedWorkflow inside an embedded child no longer breaks the child
  around it.** Running the inner child overwrote the enclosing child's
  variables, and nothing put them back. With step events on, the run failed
  with `unknown direct step '<id>'`. With them off, the enclosing child's
  later steps and its `onError` handler read the inner child's `variables.*`,
  and its durable steps checkpointed under the inner child's namespace. An
  AiAgent inside an embedded child that called an EmbedWorkflow tool also
  lost the child's variables and data to the tool's. A run resumed after the
  inner EmbedWorkflow had recorded its result replayed it from its
  checkpoint, but the enclosing child's later steps read the inner child's
  input as `data.*`. The enclosing context is now restored after each inner
  run, whether it ran or replayed. This applies to workflows compiled on this
  release; recompile a workflow to pick it up.
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
