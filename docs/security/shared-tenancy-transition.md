# Shared-tenancy transition plan

Status: proposed implementation and rollout plan. No shared-tenancy capability is enabled by this document.

Source baseline: repository commit `849cf856`, inspected on 2026-09-22. Deployment configuration, live databases, and the external management service were not inspected. Existing tests were read, not executed for this planning task.

The objective is to make each release useful and safe for today's one-environment/one-tenant deployments, while progressively establishing enough isolation to admit multiple mutually untrusted tenants into one environment. Removing the configured-tenant authentication check is a late activation step, not the starting point.

The [core API comparison](runtara-core-tenancy-api.md) specifies the operation-level design: core receives a mandatory `tenant_id: &TenantId` argument from the host on every tenant-specific operation; the environment enumerates assigned tenants; requests, scheduling, recovery, and cleanup use the same operations with that explicit argument. No tenant-bound persistence wrapper is required. Platform identity bootstrap and infrastructure administration remain separate from that lifecycle API.

**Target architecture and scope**

The first supported shared environment has one server/runtime deployment serving several explicitly assigned tenants, shared platform/execution PostgreSQL storage, shared Valkey, and shared worker capacity. Tenant object databases remain separately credentialed databases, possibly on the same PostgreSQL cluster. This still delivers shared-environment tenancy; putting every customer's arbitrary SQL and dynamic object tables into one database is a separate project.

| Boundary | Target for the first shared release |
|---|---|
| Public identity | Verified identity selects one tenant; deployment assignment and membership authorize it |
| Platform data | Explicit tenant scope throughout services, with PostgreSQL RLS as a second line of defense |
| Object data and raw SQL | Explicit tenant-owned database binding and restricted database role; no shared default fallback |
| Guest authority | Host-bound tenant and instance identity; existing WASM restrictions retained |
| Valkey and caches | Tenant-aware identity, mutation, eviction, and consumer scope |
| Scheduling | Global safety bounds plus per-tenant limits and fair access |
| Tenant configuration | Tenant-resolved settings; process settings remain deployment ceilings |
| Operations | Privileged operator paths separated from tenant APIs; tenant-specific drain, restore, and migration |

The trust model includes a trusted platform operator and trusted host implementation. RLS, WASM, and logical namespaces do not protect tenants from a fully compromised server process or database administrator. The first shared release must bound tenant-generated load, but still shares a process-level crash/OOM boundary. Tenants requiring independent host failure boundaries remain on dedicated deployments or separately isolated worker pools; do not market the first release as process/container isolation.

All new configuration names, interfaces, tables, and commands described below are proposals, not existing features.

**Compatibility contract for every step**

- Existing `TENANT_ID`, auth modes, API paths, object database configuration, and workflows continue to work in dedicated mode. Dedicated mode remains the default and remains supported after shared mode ships.
- Preserve valid operations and response contracts. Denying access to another tenant's resource is an intentional security correction, not a supported behavior to preserve.
- A new tenant resolver initially adapts existing environment variables into exactly one tenant configuration. Do not require a management service to keep self-hosted installations running.
- Add schema and protocol support before changing writers. Backfill and validate before enforcing constraints. Keep old-readable formats during the declared rollback window. Never edit committed SQL migrations.
- Prove compatibility for each adjacent release pair using a production-shaped, sanitized fixture, including queued, running, sleeping, and suspended executions. Do not infer compatibility solely from fresh-install tests.
- Compatibility fallbacks may operate only within an explicitly bound dedicated environment. Missing tenant identity in shared mode is an error, never permission to select a default tenant.
- Normal dedicated upgrades require no data move or endpoint replacement. Transitioning a tenant to another deployment is an explicit operation with durable intake handling and, if necessary, a brief write pause; this plan does not promise an unproven zero-downtime database merge.
- Each step's tests are cumulative. Do not defer security tests to the final release, and do not enable production multi-tenant admission until every mandatory gate passes.

**Rollout states and rollback rules**

| State | Allowed tenancy | Compatibility behavior | Rollback boundary |
|---|---|---|---|
| D0: current dedicated | One configured tenant; physically dedicated resources | Existing behavior | Existing release procedure |
| D1: hardened dedicated | One tenant, new ownership checks/context/adapters | Existing configuration and wire contracts | Previous compatible dedicated binary while expansion schemas remain compatible |
| D2: shared-ready dedicated | One tenant exercising strict namespaces, policies, and resource binding | Legacy adapters retained but not used by strict paths | Only a tested binary that understands the enabled policies and schema |
| S1: shared canary | Explicit small tenant allowlist | No ambiguous legacy fallback | A shared-capable release, or fenced tenant evacuation |
| S2: shared production | Explicit admitted tenants within measured capacity | Same strict rules as S1 | Shared-capable release or fenced evacuation |

Add an authoritative deployment assignment/mode record and a minimum supported runtime version before S1. Admission must check both configuration and persisted assignment; setting an environment variable must not turn a populated shared database back into a dedicated database. Unknown/stale assignments fail closed for new admission.

An old binary cannot enforce a record it does not understand. Enforce the version floor in deployment tooling as well as new code, and revoke the old database/Valkey service credentials before production sharing. Do not grant those credentials access to the shared deployment. Rollback never means disabling RLS or restoring unprefixed auth reads on a live shared environment.

**Step 1 — Establish the isolation inventory and upgrade regression harness**

Change:

1. Inventory every tenant-facing route and MCP tool, its permission, resource ownership check, service/repository entry point, and storage backend. Include downloads, debug data, streaming, reports, signals, API keys, OAuth, channel webhooks, and admin endpoints.
2. Inventory durable tables, foreign keys, tenantless child records, Valkey keys/consumer groups, in-memory caches, artifact paths, and uses of global tenant configuration. Label intentionally global resources explicitly.
3. Build fixtures for two tenants, a user belonging to both with different roles, a user belonging to only one, API keys with different scopes, and foreign resource IDs known to the caller.
4. Add an upgrade fixture from the current schema and workflow artifact formats. Capture valid single-tenant behavior before changing implementations. Record discovered gaps as tracked failures; fix them before making their corresponding security gate pass.

Compatibility and rollout: test/documentation additions only. The production runtime remains unchanged. This step makes existing guarantees measurable; later steps must not regress them.

Acceptance: the inventory accounts for every registered route/tool and background consumer, including feature-gated surfaces. The baseline fixture exercises completion, pause/resume, scheduled wake, retry, cancellation, webhook intake, compilation, connection refresh, and report/object access.

Rollback: none needed for runtime behavior. Keep the regression tests even if a later implementation is reverted.

Starting points: [server routing](../../crates/runtara-server/src/server.rs), [MCP tools](../../crates/runtara-server/src/mcp/tools), and [CI feature matrix](../../.github/workflows/ci.yml).

**Step 2 — Close instance ownership gaps immediately**

Change:

1. Introduce tenant-scoped instance access methods and use them for stop, pause, resume, signals, checkpoints, step events, scopes, summaries, debug payloads, and downloads.
2. Validate instance ownership before returning any content or changing state. When the route also supplies a workflow ID, check that relationship rather than trusting the path.
3. Carry the explicit `&TenantId` argument into the mutation/read itself and enforce it in storage. Never authorize one ID and operate on another; tenant transfer must not mutate a live instance's tenant.
4. Use the same tenant-scoped lifecycle methods for runtime/recovery paths. The environment enumerates its assigned tenants and passes each `&TenantId` into those methods; do not add unrestricted lifecycle variants. Preserve current response shapes, using non-disclosing missing-resource behavior for foreign resources.

Compatibility and rollout: public URLs and request bodies do not change. Correct single-tenant calls behave identically. This can ship as an ordinary security release before the rest of the plan.

Acceptance: tenant A cannot read or control B's instance even knowing its UUID; permitted A operations still work. Cover REST and MCP, concurrent status changes, wrong workflow IDs, and negative responses containing no foreign status or metadata.

Rollback: no data conversion. Prefer a forward fix; reverting restores a known authorization gap and is never permitted on shared storage.

Starting points: [workflow handlers](../../crates/runtara-server/src/api/handlers/workflows.rs), [step events](../../crates/runtara-server/src/api/handlers/step_events.rs), [step summaries](../../crates/runtara-server/src/api/handlers/step_summaries.rs), and [execution engine](../../crates/runtara-server/src/workers/execution_engine.rs). The existing `get_execution` ownership check is a useful pattern.

**Step 3 — Make tenant authority explicit without changing deployment behavior**

Change:

1. Introduce a validated tenant identity type and distinct request, execution, and system-operation contexts. Identity construction belongs at authentication, verified external ingress, or durable job resolution—not arbitrary request JSON.
2. Replace optional tenant arguments in tenant-facing services with required context. Core/persistence methods take mandatory `&TenantId` arguments, with no tenant-bound persistence wrapper. Maintenance enumerates assigned tenants outside core and passes each ID into the same operations; `None` must not mean both “missing identity” and “all tenants.” Keep genuinely platform-wide infrastructure administration separate from tenant lifecycle operations.
3. Pass tenant context through report providers, connection resolution, compilation, child workflow execution, and agent testing. Audit spawned tasks: copy explicit authority rather than relying on request-local state surviving a spawn.
4. Centralize deployment admission checks for both JWTs and API keys. In dedicated mode, the authenticated tenant must equal configured `TENANT_ID`, including the API-key fast path.
5. Replace role-less production in-process identity fallbacks with an explicit system principal or real caller identity. Test-only constructors must not become externally reachable authentication bypasses.

Compatibility and rollout: adapt `OrgId`, existing host callbacks, and the configured tenant to the new types. Keep wire formats and SDK/WIT interfaces unchanged where possible; do not make old WASM artifacts obtain or supply their own authority.

Acceptance: all valid single-tenant auth modes and API keys pass. A foreign key in an accidentally shared database is denied. Missing request identity fails closed; legitimate recovery still works through the system interface. Track and remove unreviewed raw/global-tenant usage from tenant paths.

Rollback: adapter-level changes require no data migration. Preserve Step 2 checks. Shared admission remains disabled.

Starting points: [auth middleware](../../crates/runtara-server/src/middleware/auth.rs), [tenant extractors](../../crates/runtara-server/src/middleware/tenant_auth.rs), and [MCP in-process client](../../crates/runtara-server/src/mcp/tools/internal_api.rs).

**Step 4 — Add tenant configuration and deployment assignment with a legacy adapter**

Change:

1. Separate deployment settings (listeners, global pool/CPU/memory ceilings, trusted services) from tenant settings (entitlements, limits, database bindings, retention, public endpoint mapping).
2. Add a resolver whose initial implementation produces a single record from today's environment variables. Add a persistent resolver behind explicit configuration, plus assignment, lifecycle state, configuration revision, and migration ownership generation.
3. Bootstrap the configured tenant idempotently. Never silently assign unexplained existing rows to it; produce an operator-readable inventory of discrepancies without exposing data or credentials.
4. Resolve entitlement decisions by tenant. Apply the minimum of tenant allowance and deployment safety ceiling. Pin the revision needed for reproducible execution policy, but always honor live suspension/revocation and current safety ceilings.
5. Define cache freshness and outage behavior: use a bounded validated cache where appropriate; missing/expired authority must deny new work. Do not turn a registry outage into a fallback to the configured tenant.

Compatibility and rollout: the environment adapter remains the default, with the same entitlement values. Persistent mode can first be exercised with one tenant and compared to the adapter before becoming authoritative. Adding a second record does not enable serving it.

Acceptance: old configuration produces equivalent decisions; two test tenants receive different entitlements; reductions stop excess new admission without corrupting already running work. Startup diagnoses contradictory mode/assignment state. Registry loss and stale revisions behave as specified.

Rollback: revert to the adapter only on a dedicated environment with an equivalent exported configuration. Keep additive tables. Do not erase assignments or bypass suspended state.

Starting points: [configuration](../../crates/runtara-server/src/config.rs), [entitlements](../../crates/runtara-server/src/entitlements.rs), and [admission decisions](../../crates/runtara-server/src/workers/execution_engine.rs).

**Step 5 — Repair ownership data and enforce tenant-consistent relationships**

Change:

1. Add resumable, idempotent inspection/backfill tooling. Record counts, unresolved rows, and relationship checks; do not log payloads or credentials.
2. In a verified dedicated database, assign NULL-owned legacy triggers to the configured tenant. Retain a dedicated-only legacy reader during mixed-version rollout; the final pass occurs after old writers stop. Do not guess ownership in a database already containing multiple tenants.
3. Require explicit ownership for newly written triggers. Remove `OR tenant_id IS NULL` from strict tenant paths. If global templates are needed, represent them separately as read-only platform resources with tenant-owned copies.
4. Audit composite foreign keys across workflows, images, instance bindings, connection defaults, reports, triggers, and outbox records. Add missing tenant-consistency constraints and supporting indexes through forward migrations.
5. For instance child tables, either enforce access through the owning instance or add a backfilled tenant column with a composite foreign key. Select the approach per table based on measured query plans; redundant tenant columns must not be independently writable identities.
6. Audit global uniqueness constraints for tenant-local names and preserve globally unique IDs that existing APIs depend on. Add constraints in an expand/backfill/validate sequence with bounded lock waits and retryable backfill batches.

Compatibility and rollout: old binaries tolerate additional tables/indexes/columns. Delay NOT NULL and incompatible write constraints until every writer is compatible, or provide a narrowly bound dedicated compatibility writer. Apply any required index creation outside a transactional migration through supported tooling; do not assume all DDL is nonblocking.

Acceptance: an upgrade preserves existing trigger behavior and identifiers; foreign tenant relationships are rejected; backfills can resume after interruption. Test actual locks and timing on representative data. Ambiguous ownership blocks strict-mode promotion, not unrelated tenants' normal upgrades.

Rollback: stop backfill and use the still-supported dedicated reader. Validated ownership assignments remain; do not rewrite them to NULL. Schema contraction waits until after the compatibility window.

Starting points: [trigger repository](../../crates/runtara-server/src/api/repositories/triggers.rs), [server migrations](../../crates/runtara-server/migrations), and [execution migrations](../../crates/runtara-store-postgres/migrations/postgresql).

**Step 6 — Migrate Valkey contracts without stale authorization fallback**

Change:

1. Establish a versioned, unambiguous key encoding for deployment/tenant identity. Cover membership, revocation, sessions, deduplication, rate limits, compilation queues/progress, admission, and product events. Preserve already-correct tenant keys unless a specific defect requires changing them.
2. Coordinate a versioned writer/consumer contract with smo-management and every other writer. This is an external implementation dependency; Runtara cannot complete it unilaterally.
3. While each tenant still has dedicated Valkey, deploy dual-writing producers first and keep one authoritative auth reader. Mirror updates, removals, role downgrades, and revocations with ordering/version semantics. Repair partial writes through reconciliation; do not acknowledge a removal until the currently authoritative authorization store enforces it.
4. Backfill namespaced state, preserving expiry. Compare exact membership/role versions and revocation coverage. Flip the whole tenant to the new namespace only at a verified convergence barrier. Never use “missing new membership key means read the old key”: deletion may be the intended denial.
5. Move queue consumers with a cutover watermark. One consumer protocol owns execution at a time; drain old queues or bridge through existing durable idempotency. Product-event dual delivery needs event-ID deduplication, not double counting.
6. Retain old keys in dedicated environments for the rollback window, maintained by dual writers. Shared Valkey has only strict tenant-scoped authority. Namespace cleanup and counter reconciliation must operate only on assigned tenants.

Compatibility and rollout: old readers remain functional while producers dual-write on dedicated Valkey. Roll out one tenant at a time. A namespaced authorization outage must not downgrade to legacy keys.

Acceptance: the same user has independent roles in A and B; deletion and revocation survive partial writes, restart, and rollback. Lost Valkey is rebuilt from authoritative state without granting access. Queue migration causes no extra launches beyond documented at-least-once delivery/idempotency semantics; product-event totals reconcile.

Rollback: switch the dedicated reader back only after legacy state is proven current, including removals/revocations. Keep strict readers on shared Valkey; repair forward or evacuate.

Starting points: [auth keys](../../crates/runtara-server/src/valkey/auth.rs), [product events](../../crates/runtara-server/src/product_events.rs), [admission counters](../../crates/runtara-server/src/workers/admission_counter.rs), and [external auth contract](user-management-contracts.md).

**Step 7 — Make object databases, credentials, caches, and artifacts explicitly owned**

Change:

1. Register today's default object database as an explicit binding for the configured tenant. In strict mode, missing binding is an error; prohibit falling back to a process-wide URL or `from_pool` store shared by tenants.
2. Provision distinct database roles for tenant object databases. Verify actual server/database/role identity and effective privileges, not merely URL inequality; different URLs can reach the same data. Customer-owned connections retain their explicit semantics, but platform-managed defaults must be disjoint.
3. Key pools, credential caches, refresh locks, negative caches, resource handles, and eviction by tenant plus connection/binding identity and credential revision as needed. Validate ownership before cache access. Use collision-resistant cache identities rather than a bare short URL hash as an isolation decision.
4. Migrate credential encryption through a read-old/read-new keyring. Imported tenants may have different existing keys: support those envelopes before import, rewrap incrementally, and verify decryptability without printing values. A shared process key is compatible initially; per-tenant envelope keys improve rotation/deletion but do not isolate a compromised host.
5. Resolve artifact/staging/run directories through an opaque storage identity. Prohibit traversal, symlink escape, and cross-tenant writable directories. Keep existing paths through a validated legacy mapping until artifacts and pinned suspended executions are migrated. Do not reinterpret existing tenant IDs as arbitrary filesystem paths or URL fragments.

Compatibility and rollout: existing credentials need no user re-entry, object tables need no tenant-column rewrite, and compiled workflows retain their IDs/checksums. Single-tenant fallback remains available only through the explicitly bound legacy adapter.

Acceptance: A cannot resolve B's connection or obtain a cache hit for it; rotation/deletion invalidates the right tenant only. A's raw SQL role cannot read/write B or platform tables, including explicit cross-schema references. Same-shaped artifact names do not collide. Existing encrypted credentials and suspended artifacts remain usable.

Rollback: retain old key decryptors, legacy path mappings, and binding records. Delay encryption formats unreadable by the rollback binary until the version floor advances. Never copy all tenants back into one default object database.

Starting points: [object store manager](../../crates/runtara-server/src/api/repositories/object_model.rs), [native SQL boundary](../../crates/runtara-server/src/api/services/database.rs), [credential cache](../../crates/runtara-connections/src/auth/token_cache.rs), [cipher factory](../../crates/runtara-connections/src/crypto/factory.rs), and [workflow agent artifacts](../../crates/runtara-server/src/workflow_agents.rs).

**Step 8 — Authenticate internal operations and bind guest capabilities**

Change:

1. Audit the connections-admin listener and the separate core instance protocol. Implement request-time service authentication and operation authorization, not just a boot-time check that a secret exists.
2. Keep operator administration distinct from workflow callbacks. A workflow must never receive an all-tenant administrative secret. Remote callbacks use short-lived instance-bound authority or authenticated host mediation, with tenant/instance/operation scope and replay/expiry rules.
3. Preserve host-import execution, where instance identity is already bound by the host. Verify tenant binding for outbound HTTP, SQL, trusted agents, child execution, and signals; guest-provided identifiers cannot replace it.
4. Inventory retained SDK/native/older artifact callbacks. Add a versioned authenticated protocol while retaining the old protocol only behind dedicated isolation. Drain, upgrade, or bridge legacy artifacts through a scoped adapter before moving that tenant to shared mode.
5. Keep SSRF and credential destination controls enforced on all outbound paths. In shared mode, private-network exceptions must be explicit deployment/tenant policy and must never expose platform admin, metadata, or neighboring tenant services.

Compatibility and rollout: update internal callers before enabling enforcement on a dedicated listener. Existing remote integrations may retain their isolated legacy endpoint during this transition; no automatic activation that cuts off old workflows. Shared readiness rejects unauthenticated legacy callbacks rather than granting a global bypass.

Acceptance: unauthenticated admin/callback requests fail; an A callback cannot address B; expired/replayed authority fails as defined; legacy workflows complete/resume in the dedicated compatibility path. Sandbox tests prove no raw credential/network/filesystem escalation.

Rollback: revert protocol enforcement only on the isolated dedicated legacy endpoint. Never reopen unauthenticated administrative routes within the shared workload boundary.

Starting points: [listener construction](../../crates/runtara-server/src/server.rs), [bind guards](../../crates/runtara-server/src/bind.rs), [core HTTP protocol](../../crates/runtara-server/src/core_runtime/http_server.rs), and [host-bound runtime](../../crates/runtara-environment/src/runtime_host.rs). Current documentation explicitly notes that the internal shared-secret setting is checked at startup, not validated on requests: [auth modes](../deployment/auth-modes.md).

**Step 9 — Make durable workers operate on assigned tenant context**

Change:

1. Refactor cron, trigger consumption, outbox relay, compilation, launch dispatch, wakes, recovery, retention, and reconciliation to resolve the tenant from authoritative records and deployment assignment.
2. Preserve existing tenant fields and durable identity contracts. If a new envelope field is needed, version it and deploy readers first. A tenantless legacy payload may be interpreted only by its known dedicated queue adapter.
3. Check tenant consistency among queue message, durable request, instance, image, connection, and configuration. A poisoned/mismatched item is quarantined with bounded diagnostics; it must not block other tenants indefinitely.
4. Make assignment ownership generation part of claims/handoffs and state-changing callbacks. Old workers cannot resume, enqueue, or checkpoint after a migration transfer. Recheck suspension and assignment at admission and dispatch, not only when the process starts. Define a storage-enforced write fence: write transactions validate/lock the active generation, and a transfer barrier waits for admitted transactions before revoking it. A cached assignment check alone is not fencing.
5. Resolve cron schedules and worker configuration per tenant. Preserve schedule/timezone/idempotency identity during handover. The environment obtains assigned tenants from its directory, then passes each `&TenantId` to discovery and claim operations on the shared persistence implementation. Core needs no all-tenant discovery or recovery methods.
6. Remove accidental process-global tenant propagation, including the extra `RUNTARA_TENANT_ID` environment value supplied at launch. Guest-visible metadata must agree with host authority.

Compatibility and rollout: one assigned tenant yields the same work as today. Retain existing durable leases/checkpoint formats. Mixed-version execution is allowed only while all writers/readers understand active formats; generation enforcement begins after legacy workers are drained.

Acceptance: interleave A/B jobs through crash, retry, sleep/wake, cancellation, restart, and lease expiry; assert the original tenant throughout. Simulate duplicate delivery, stale owners, and suspension. No wrong-tenant recovery or simultaneous ownership after transfer.

Rollback: fence and drain new claims before reverting a dedicated worker. Do not let a pre-fencing worker reconnect after ownership transfer. Queued work stays durable.

Starting points: [cron](../../crates/runtara-server/src/workers/cron_scheduler.rs), [trigger worker](../../crates/runtara-server/src/workers/trigger_worker.rs), [outbox](../../crates/runtara-server/src/workers/execution_outbox.rs), [launch dispatcher](../../crates/runtara-environment/src/launch_dispatcher.rs), and [runtime client](../../crates/runtara-server/src/runtime_client.rs).

**Step 10 — Enforce PostgreSQL isolation beneath tenant services**

Change:

1. Separate migration/owner credentials, tenant-operation credentials, and narrowly privileged system credentials. The tenant role is neither table owner nor superuser and cannot bypass RLS or assume a privileged role. Restrict default grants, function execution, and schema creation.
2. Provide a transaction-scoped database API that establishes tenant context with transaction-local settings. Every statement in that operation uses the same transaction; cancellation, rollback, and pool reuse must clear context. Missing tenant context denies access.
3. Add and test both read predicates and write checks for tenant-owned tables, including child records reached through an instance. Ensure ownership is immutable through ordinary updates. Apply equivalent isolation to server metadata and execution stores even if they currently use different pools/databases.
4. Audit views, SQL functions, analytics, raw repository queries, and joins for policy bypass. Assignment enumeration stays in the host directory; scheduler/recovery queries use scoped transactions for each tenant. Any genuinely platform-wide administrative lookup needs its own narrowly justified authority, not an unrestricted instance-operations interface.
5. Handle identity bootstrap explicitly: API-key hash resolution, OAuth callback state, and public endpoint resolution happen before a tenant is known. Expose narrow, audited lookup functions/services, not a general privileged pool to request handlers. Harden function ownership/search paths and return only necessary authority.
6. Enable enforcement table-by-table on a dedicated canary with the new role. Measure query plans and backfill supporting indexes. Ordinary object SQL continues using separate customer roles and cannot access these platform functions.

Compatibility and rollout: create roles/policies first, move new code to scoped transactions, and enable policies only after every participating caller is compatible. Do not unexpectedly subject an old dedicated runtime to policies it cannot satisfy. A legacy broad database credential must not remain available once the environment becomes shared.

Acceptance: deliberately omit an application tenant predicate and verify RLS still blocks B; test INSERT/UPDATE/DELETE, views, child tables, missing context, connection reuse, concurrent tenants, failed transactions, and system jobs. Test using the real non-owner production role. RLS is defense against missing scope, not protection from a fully compromised host that can choose arbitrary tenant context.

Rollback: within the dedicated canary, return to the prior tested credential/code combination only if all data remains single-tenant. Shared rollout requires a policy-aware rollback release; do not disable policies to make an old binary run.

Starting points: [PostgreSQL store](../../crates/runtara-store-postgres/src), [server repositories](../../crates/runtara-server/src/api/repositories), and [API-key bootstrap query](../../crates/runtara-server/src/api/handlers/api_keys.rs).

**Step 11 — Resolve every external entry point and session to one tenant**

Change:

1. Add shared-mode tenant selection based on verified JWT organization or the validated API-key row, then require active deployment assignment and membership. A path/host/header may select a candidate but cannot override verified identity. Reject conflicting sources.
2. Keep dedicated OIDC's configured-tenant restriction. For the first shared release, require OIDC/API-key authentication with enforced membership; keep local and existing trust-proxy modes dedicated-only. A future authenticated proxy-tenancy protocol is separate work.
3. Introduce strict shared-mode JWT issuer/audience/subject validation and revocation policy without changing legacy dedicated defaults blindly. Upgrade token producers and allow old tokens to expire/reissue before promoting a tenant; do not permanently require missing legacy API-key fields without a migration path.
4. Resolve webhooks/channels through a stored endpoint or connection binding, then verify the provider signature/secret and replay rules before invoking tenant work. Unsigned public triggers remain possible only as explicitly public endpoints with bounded intake. Preserve old endpoints through routing aliases; an unqualified path in a shared environment cannot fall back to a default tenant.
5. Bind OAuth state to tenant, connection, initiator where applicable, redirect destination, expiry, and single use. Preserve valid outstanding callbacks across a move or route them to their original owner until completion.
6. Bind MCP live/recovered sessions to tenant and caller; reauthorize each call. Replace fixed `server.tenant_id` direct reads and production synthetic-auth fallback. A session ID alone cannot authorize access, and role revocation must affect existing sessions.
7. Include tenant identity in frontend cache/storage namespaces; cancel old requests and clear tenant-sensitive state on identity/tenant change. Ensure late responses cannot refill the new tenant's cache. Preserve existing gateway prefixes and embedded UI base paths through aliases.

Compatibility and rollout: ship new resolvers while still admitting only the existing tenant. Old client URLs and key formats work via unambiguous aliases. Additional tenants remain disallowed in production until Step 16.

Acceptance: conflicting tenant selectors, copied sessions, revoked membership, cross-tenant callbacks, and same-user/different-role cases fail safely. Existing CLI/API/UI/MCP flows still work. Switch A→B while A requests are in flight; no A data appears in B. Test gateway Host/forwarded-header trust explicitly.

Rollback: preserve URL aliases and callback/session compatibility during the dedicated window. Once shared, revert only to a release with the same tenant-binding rules or drain affected sessions/endpoints safely.

Starting points: [event ingress](../../crates/runtara-server/src/api/handlers/events.rs), [channels](../../crates/runtara-server/src/channels), [MCP server](../../crates/runtara-server/src/mcp/server.rs), [session store](../../crates/runtara-server/src/mcp/session_store.rs), and [frontend query keys](../../crates/runtara-server/frontend/src/shared/queries/query-keys.ts).

**Step 12 — Bound resource consumption and schedule tenants fairly**

Change:

1. Retain current durable admission reservations and global worker safety limits. Resolve per-tenant limits from Step 4 and enforce them across replicas; multiplying replicas must not multiply a tenant's allowance accidentally.
2. Add fair tenant selection ahead of FIFO ordering within a tenant, with bounded bursts and bounded idle-tenant bookkeeping. Apply it to compilation/preparation and execution, not just one stage. Preserve durable lease/cancellation semantics.
3. Budget aggregate guest memory and active work, host-side buffers, HTTP/SQL concurrency and deadlines, queue depth/bytes, artifact/cache capacity, retained checkpoint/event bytes, and tenant API/webhook rates. A per-guest memory cap alone is not a process memory budget.
4. Reserve capacity for heartbeats, checkpoint completion, cancellation, and recovery so overloaded intake cannot prevent existing executions from reaching safe states. Bound connection creation and pool churn across all tenants.
5. Define consistent backpressure: reject before acknowledging non-durable work, return a retryable response where appropriate, and keep accepted work durably queued within its limit. Do not silently discard accepted events when a tenant exceeds quota.

Compatibility and rollout: keep current dedicated limits as defaults. Measure new aggregate budgets first, then choose limits that admit existing supported workloads before enforcing. Shared mode requires validated finite budgets; merely logging would-be limits is insufficient there.

Acceptance: tenant A floods compilation, execution, SQL, ingress, and large payload paths while B runs representative work. Before testing, publish the supported tenant count/workload and numeric bounds for B's queue latency, p95 response latency, memory, file descriptors, pool count, and disk growth. Pass those bounds under sustained load and cancellation/restart; do not invent a universal capacity number without measurements.

Rollback: disable a new scheduling algorithm only in dedicated mode or replace it with another bounded fair implementation. Keep hard shared safety ceilings and already-accepted work intact.

Starting points: [launch selection](../../crates/runtara-environment/src/launch_queue.rs), [dispatcher limits](../../crates/runtara-environment/src/launch_dispatcher.rs), [embedded runner](../../crates/runtara-environment/src/runner/embedded.rs), and [admission counter](../../crates/runtara-server/src/workers/admission_counter.rs).

**Step 13 — Separate tenant observability and implement lifecycle controls**

Change:

1. Split tenant dashboards/streams from operator-wide host/pipeline diagnostics. Apply tenant authorization before subscribing and again when authority expires or changes. Tag audit/product events with originating tenant rather than a process default.
2. Define provisioning, active, draining, suspended, migrating, and deleting states with explicit permissions for new starts, callbacks, resumes, reads, and maintenance. An intake pause must still let already-running work checkpoint safely; a security suspension can have a stricter policy.
3. Implement tenant-specific retention, cache/session invalidation, export, and deletion with dry runs, manifests, resumability, and dependency order. Privileged cleanup must select one tenant explicitly and never delete a shared built-in artifact still referenced elsewhere.
4. Add tenant restore into an isolated destination and a controlled activation step. Define backup retention and delayed physical deletion semantics; do not claim tenant deletion instantly removes data from old backups.
5. Track internal isolation denials, fallback use, scope failures, queue age, and ownership-generation conflicts. Keep high-cardinality identity in bounded logs/audit stores rather than unbounded metrics labels; do not put payloads/credentials into telemetry.

Compatibility and rollout: existing dedicated dashboards may retain host diagnostics under the current operator authority; shared tenant dashboards use scoped responses. New lifecycle operations are opt-in and do not delete or suspend anything during ordinary upgrade.

Acceptance: A cannot observe B's events/usage/debug information; suspending A leaves B running. Delete/export/restore A with a populated B fixture and prove B's rows, files, credentials, sessions, and scheduled work are unchanged.

Rollback: stop lifecycle workers safely, retain manifests and tombstones, and resume on a compatible version. Do not reverse a deletion by clearing flags without a restore procedure.

Starting points: [analytics endpoints](../../crates/runtara-server/src/api/handlers/analytics.rs), [product events](../../crates/runtara-server/src/product_events.rs), [audit](../../crates/runtara-server/src/audit), and [retention worker](../../crates/runtara-environment/src/db_cleanup_worker.rs).

**Step 14 — Build and rehearse tenant transfer before moving production data**

Change: implement the following operator runbook as resumable tooling with a manifest and explicit source/destination assignment. Test against sanitized copies; never merge live databases by a generic dump restore.

1. Verify both deployments run the compatible version floor. Inventory tenant rows, globally keyed IDs, queued requests, cron state, callback/OAuth/session state, object bindings, encryption key IDs, artifact checksums, and unfinished instances.
2. Check destination collisions, including global IDs and historical uniqueness constraints. Preserve externally visible IDs. If a collision or ownership ambiguity exists, stop this tenant's move and resolve it explicitly; do not overwrite or silently regenerate IDs.
3. Pre-copy immutable artifacts and prepare object database access/keyrings. Copy platform data with explicit tenant filters and a dependency manifest, including instance children and all referenced images. If sequence-backed IDs collide, use a tested internal mapping or destination-generated IDs only where no persisted/external reference depends on them.
4. Pause source tenant intake, cron firing, queue consumption, and automatic wake/recovery claims. Route new requests into a durable bounded gateway buffer if one is implemented; otherwise return explicit retryable maintenance responses. Do not acknowledge events that are not durably saved.
5. Let active work finish or reach a supported durable suspension, then fence source ownership and verify no in-flight external operation remains. Never hard-cut an ambiguous external side effect; reconcile it before retrying elsewhere. Long-sleeping instances are transferred with checkpoints and pinned artifacts, not waited out.
6. Take the final consistent tenant snapshot/delta after the write barrier. Include pending outbox/reservations/signals and authoritative revocation state. Rebuild derived counters rather than treating them as primary data. Re-establish worker leases under the new generation; do not inherit a source process's live lease.
7. Validate row counts, ownership/FK closure, checksums, decryptability, schema versions, and object database privileges. Test read-only destination resolution while destination execution remains disabled.
8. Transfer assignment atomically through an authoritative generation/CAS operation. Switch API/webhook aliases and consumer ownership, then enable destination execution. Source stale generations must fail at claims and callback writes; gateway routing alone is insufficient fencing. Separate source/destination databases do not share a transaction: require durable proof that the source write fence is installed before destination activation, and keep both sides inactive if ownership transfer is uncertain. Test coordinator/network failure at every handover boundary.
9. Replay durably buffered intake with original idempotency identities. Reconcile cron/wake due times and prove there is no lost schedule or simultaneous ownership. Drain or invalidate old MCP sessions safely; preserve pending OAuth callback routing.
10. Observe the tenant through the agreed acceptance window. Keep source copies fenced and read-only during rollback retention; delete them only under a separate reviewed retention operation.

Compatibility and rollout: the tenant keeps organization, workflow, connection, and externally referenced instance IDs and endpoints. Object databases can stay in place during the server move, avoiding an unnecessary second data migration. Hosts need secure access to the retained databases.

Acceptance: rehearse transfer and transfer-back with queued, running, sleeping, failed/retryable, and suspended instances, plus a webhook storm and credential refresh. Demonstrate no lost accepted requests and no duplicate external side effects introduced by migration. This does not claim arbitrary workflows acquire exactly-once external effects; existing idempotency/reconciliation contracts still apply.

Rollback: before destination writes, restore assignment to source only after proving destination never activated. After destination writes, freeze/fence destination, transfer the new state back or move back using the same shared-capable binary/storage, validate, then transfer ownership. Never reactivate a stale source snapshot.

Safety improvement: this establishes tenant-scoped recovery and controlled relocation even for installations that remain dedicated.

**Step 15 — Run strict single-tenant canaries and certify the shared profile**

Change:

1. Run a production-shaped dedicated tenant with every strict mechanism enabled: scoped repositories/RLS, namespaced auth, explicit database bindings, authenticated callbacks, assignment fencing, bounded resources, and scoped observability.
2. Run the complete two-tenant suite in a disposable shared staging environment, including restart/restore and concurrent failures. Exercise all enabled features, not just workflow creation/execution.
3. Implement a readiness report and enforced promotion check. Check active policies/roles, assignment, key namespace version, token producer compatibility, worker protocol versions, unresolved ownership rows, database bindings, artifact compatibility, migration rehearsal, and measured capacity.
4. Make the report distinguish machine-checked state from release certification evidence. Passing a startup flag check alone cannot prove isolation or fairness. Any unknown required item blocks shared promotion.
5. Record the shared-capable rollback release and retire old service credentials. Keep existing dedicated installations on their compatible mode until individually promoted.

Compatibility and rollout: production still contains one tenant per environment. This validates stronger protections without changing customer tenancy. Reverting strict features is possible only under the dedicated rollback rules already established.

Acceptance: all gates below pass, normal traffic stays within published latency/error budgets, and the compatibility adapters show no unexpected use in strict mode. Canary duration must include real scheduled/recovery paths, or explicitly exercise them; elapsed time alone is not proof.

Rollback: retain the single-tenant deployment and revert to the last tested dedicated/shared-ready release as allowed by active schema/policy formats. Diagnose failures without admitting another tenant.

**Step 16 — Admit a second tenant, then expand deliberately**

Change:

1. Promote a certified environment with an explicit tenant allowlist and persisted shared mode. Start with synthetic tenants in staging, then a small production cohort after Step 15 certification. Use Step 14 for existing tenants; new tenants can be provisioned directly.
2. Admit the second tenant only when all readiness gates pass and headroom is measured. Initially limit tenant count and workload, not just the number of deployments.
3. Monitor isolation denials, configuration revision/assignment mismatches, per-tenant queue latency, resource saturation, auth freshness, and lifecycle failures. Exercise tenant-scoped suspension without disturbing neighbors.
4. Expand in cohorts within certified capacity. Retain the dedicated deployment option; do not automatically co-locate tenants with incompatible security/network/retention requirements.
5. After the declared compatibility window, remove obsolete strict-path fallbacks and old write formats. Keep the supported dedicated environment adapter. Schema contraction/key deletion is a separate release after old readers/writers and rollback dependencies are proven absent.

Compatibility and rollout: shared mode is an explicit per-environment promotion, never a changed installation default. Existing dedicated deployments continue independently. The first tenant's URLs, identities, stored workflows, and database bindings remain valid when the second tenant is admitted; verify that assertion with the same regression fixture used before promotion.

Acceptance: two real tenants complete the supported feature matrix concurrently, with independent permissions/data/limits, successful restart/recovery, bounded interference, and a rehearsed evacuation route. Review every isolation anomaly before expanding the cohort.

Rollback: stop new tenant admission; suspend or drain the affected tenant; use a certified shared-capable release or evacuate via Step 14. Never fall back to a pre-isolation binary on the populated shared stores.

**Release gates: what “acceptable to share” means**

| Gate | Required evidence before S1 |
|---|---|
| Tenant authority | Every public route/tool and callback has a reviewed identity source; missing/conflicting identity is denied |
| Data isolation | A/B negative tests for every resource family and operation; SQL omission tests pass with the actual non-owner role |
| Authentication | Different roles for the same user work; deletion/revocation propagate correctly; no legacy namespace fallback |
| Storage | Explicit verified object bindings; no foreign role access; cache/path collision and rotation tests pass |
| Execution | Host-bound capabilities, authenticated callbacks, child calls, cancellation, checkpoint/resume, and image ownership tests pass |
| Workers | Mixed-tenant recovery, poisoned messages, duplicate delivery, lease loss, and stale assignment tests pass |
| Resource sharing | Published numerical capacity/interference targets met under sustained adversarial load; control operations remain responsive |
| Sessions/ingress | UI/MCP/streams/OAuth/webhooks cannot cross tenants; old URLs remain unambiguous and verified |
| Operations | Tenant suspension, export, deletion, restore, transfer, and transfer-back preserve neighbors |
| Compatibility | Existing dedicated configurations and stored artifacts pass upgrade tests; safe rollback version and fencing proven |
| External dependencies | Management writers/consumers, token producers, gateway routing, and deployment admission tooling support the active contracts |

Zero unauthorized cross-tenant reads/writes/control operations is a hard gate. Performance thresholds are workload-specific and must be fixed before the load test, not chosen afterward to make the result pass. Do not waive a failing safety gate by calling the first customers a canary.

**Verification commands and environments**

The authoritative matrix remains [CI](../../.github/workflows/ci.yml). The following are implementation-stage commands, not checks run for this document. Supply isolated PostgreSQL/Valkey services and test configuration as CI does; never run migration or destructive fixtures against production. Use the repository-pinned Rust and frontend Node versions.

```sh
cargo fmt --all -- --check
cargo test -p runtara-server --features db-integration-tests,valkey-integration-tests -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1
cargo test -p runtara-store-postgres --features db-integration-tests -- --test-threads=1
cargo test -p runtara-connections -- --test-threads=1
cargo test -p runtara-object-store --features db-integration-tests --test integration --test native_database -- --test-threads=1
```

Run focused tests as each step lands, then the complete relevant CI feature matrix before release. Feature flags matter: a bare workspace-default `cargo test` does not establish coverage. Add the new isolation/upgrade tests to those jobs, rather than relying on existing tests to cover new behavior. Extend CI with distinct tests for migration role vs tenant role, old/new release pairs, control-plane contract versions, and multi-replica ownership transitions.

For host/component changes, run `scripts/build-agent-components.sh`, followed by CI's `runtara-component-host` component/database tests, server outbound-HTTP integration test, `runtara-workflows` direct-WASM tests, and environment scoped/cooperative runner tests with their required features. For frontend changes, run `npm test`, `npm run lint`, and `npm run build` in `crates/runtara-server/frontend`, plus browser tests for tenant switching and stale requests. Regenerate the runtime client with `generate-api-runtime-offline` if the public API changes. Update SQLx offline metadata only through its tooling where checked queries change.

The rollout additionally needs staging load/fault tests and migration rehearsals; unit tests and green CI alone cannot certify those operational properties. Record the release, schema/policy version, role configuration, workload, measurements, and actual rollback result in the promotion evidence.

**Implementation ordering and completion boundary**

Implement Steps 1–5 first as dedicated-environment hardening. Complete namespace/storage/internal-protocol work in Steps 6–8 before tenant-aware worker rollout, then database enforcement and external-surface work in Steps 9–11. Steps 12–14 supply operational safety. Steps 15–16 are the only production promotion path. Each numbered step can be split into small reader-first, writer, backfill, enforcement, and cleanup PRs; do not combine those into a single irreversible deployment.

The planning deliverable is complete when this sequence covers the known boundaries, compatibility, per-step tests, rollback, and final promotion evidence. Implementation is complete only when those tests and promotion gates have actually passed. This document records proposed work; it does not certify the current runtime for shared tenants.
