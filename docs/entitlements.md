# Entitlements Implementation Plan

## Goal

Add pricing-tier based feature gating to Runtara without depending on Stripe or
any external billing provider in the platform core.

The first implementation should be local, deterministic, and single-tenant:
the running `runtara-server` process reads the configured tenant's tier and
entitlements from environment-derived configuration at startup, exposes the
resolved snapshot to the UI, and enforces access on every backend entry point.

This document uses "entitlement" for long-lived product access and "feature
flag" only for temporary rollout or kill-switch behavior. Pricing access should
not be implemented as scattered rollout flags.

## Current Context

Runtara is single-tenant per process today:

- `TENANT_ID` is required in `crates/runtara-server/src/config.rs`.
- `Config::from_env()` parses environment at startup and stores the result in a
  process-wide `OnceLock`.
- Auth middleware inserts `AuthContext { org_id, user_id, auth_method }` into
  request extensions.
- Public REST routes, object-model routes, file routes, and MCP routes are
  mounted separately in `crates/runtara-server/src/server.rs`.
- MCP tools call the internal router with a pre-injected `AuthContext`, so MCP
  must be gated explicitly and not rely only on external HTTP middleware.
- The frontend already has runtime config injection through
  `window.__RUNTARA_CONFIG__`, and the sidebar menu is currently static.

These constraints make an env-backed entitlement snapshot a good MVP. It will
require process restart for plan changes, which is acceptable for the current
single-tenant deployment model.

## Non-Goals

- No Stripe integration in `runtara-server`.
- No live multi-tenant billing control plane in this phase.
- No OpenFeature, LaunchDarkly, Unleash, or other remote flag provider in this
  phase.
- No frontend-only enforcement.
- No compile-time Cargo feature changes for pricing tiers. Cargo features remain
  for platform build shape, dependency footprint, and target support only.

## Product Surface

Initial feature keys are simple booleans (`true` = enabled, `false` = disabled). There are no read/write/execute access levels — for the first pricing model the product distinction is on/off, and per-capability gating for agents is handled by an explicit allowlist (below).

| Feature key | Product surface | Backend areas |
| --- | --- | --- |
| `reports` | Reports UI and report MCP tools | Report REST handlers and MCP report tools |
| `database` | Object model / Database UI | Object-model REST, SQL, CSV, internal workflow object-model access |
| `api` | External API access (API-key authenticated requests) | API key management and any handler reached via API-key auth |
| `mcp` | MCP server access | MCP router and all MCP tools |

Agents are not a single feature toggle. The snapshot carries an explicit `enabled_agents` allowlist of agent module IDs (e.g. `http`, `csv`, `xml`, `openai`). The list is built directly from env — no per-agent entitlement metadata is required, and gating decisions are made by set membership against the registered dispatcher modules. Absence of the field means all known agents are enabled (current default). The Agents UI and MCP tool surface is derived from this list: visible when at least one agent is enabled, hidden when the list is explicitly empty.

## Configuration Model

The server process is **already per-tenant**: `TENANT_ID` is required in env and resolved once into `OnceLock<Config>` in `Config::from_env()`. Entitlements ride the same channel — a single per-process snapshot built from env at startup, tied to that one `TENANT_ID`. There is no per-tenant lookup at request time; the env *is* the tenant's entitlement source of truth.

Environment variables:

```sh
RUNTARA_PRICING_TIER=pro
RUNTARA_ENTITLEMENTS_JSON='{
  "features": {
    "reports": true,
    "database": true,
    "api": true,
    "mcp": true
  },
  "agents": ["http", "csv", "xml", "openai", "anthropic"],
  "limits": {
    "maxWorkflows": 100,
    "maxObjectSchemas": 50,
    "maxApiKeys": 10,
    "objectModelBulkRequestLimit": 1000,
    "maxConcurrentExecutions": 8
  }
}'
```

`agents` semantics:

- Field omitted → all known agent modules are enabled (preserves current behaviour).
- Field present (even empty) → exact allowlist. `[]` disables all agents.
- Each ID is validated against the dispatcher's registered agent modules at startup; unknown IDs fail with `ConfigError::Invalid`.

Optional follow-up variable:

```sh
RUNTARA_ENTITLEMENT_OVERRIDES_JSON='{
  "features": { "mcp": true },
  "agents": ["http", "csv"],
  "limits": { "maxApiKeys": 20 }
}'
```

The MVP can support only `RUNTARA_ENTITLEMENTS_JSON`; if a built-in tier catalog
is added in the same patch, use this precedence:

1. Built-in tier defaults from `RUNTARA_PRICING_TIER`.
2. `RUNTARA_ENTITLEMENTS_JSON`.
3. `RUNTARA_ENTITLEMENT_OVERRIDES_JSON`.

All parsing happens in `Config::from_env()` or a helper it calls. Invalid JSON, unknown feature keys, non-boolean feature values, unknown agent module IDs, negative limits, and overflowing numeric values fail startup with `ConfigError::Invalid`.

### Limit merge semantics (current state)

The merge function on `EntitlementLimits` treats missing keys and explicit JSON `null` identically: both leave the lower-precedence layer's value in place. This means higher layers can only *impose* a stricter cap, not *lift* a cap set by a lower layer back to "uncapped".

Concretely, given a `RUNTARA_PRICING_TIER=starter` baseline with `maxApiKeys = 2`, neither of these `RUNTARA_ENTITLEMENT_OVERRIDES_JSON` payloads will uncap the limit:

```json
{ "limits": {} }
{ "limits": { "maxApiKeys": null } }
```

Both resolve to `Some(2)`. To effectively remove a cap, operators must restate the limit as a large explicit value (e.g. `"maxApiKeys": 4294967295`). This is a known limitation — implementing a proper tri-state (`missing` / `null` / `value`) would require custom deserialization for ~100 lines of code, and no operator flow today reaches it (tier definitions are placeholders pending product input, and the override layer is rare in practice).

Revisit when real tiers ship and operators need a non-workaround way to express "lift this cap". Until then, treat `null` in any entitlement-JSON layer as "inherit from below", not "clear".

Local development default:

- If no entitlement env is set, default to all features `true` and `enabled_agents = None` (i.e. all known agents allowed).
- This preserves current developer behavior.
- Production packaging can set an explicit default tier later.

## Data Structures

Create `crates/runtara-server/src/entitlements.rs`.

Core types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKey {
    Reports,
    Database,
    Api,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementLimits {
    pub max_workflows: Option<u32>,
    pub max_object_schemas: Option<u32>,
    pub max_api_keys: Option<u32>,
    pub object_model_bulk_request_limit: Option<usize>,
    pub max_concurrent_executions: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementSnapshot {
    pub tenant_id: String,
    pub pricing_tier: String,
    pub features: BTreeMap<FeatureKey, bool>,
    /// `None` = all known agent modules are allowed.
    /// `Some(set)` = exact allowlist (may be empty to disable all agents).
    pub enabled_agents: Option<BTreeSet<String>>,
    pub limits: EntitlementLimits,
}
```

Runtime API:

```rust
impl EntitlementSnapshot {
    pub fn is_enabled(&self, feature: FeatureKey) -> bool;
    pub fn require(&self, feature: FeatureKey) -> Result<(), EntitlementError>;
    pub fn agent_enabled(&self, module_id: &str) -> bool;
    pub fn require_agent(&self, module_id: &str) -> Result<(), EntitlementError>;
}
```

There is intentionally no `AccessLevel` enum. Every feature is either on or off, and agent-level granularity is handled by `enabled_agents`. If a future tier needs finer splits (e.g. database read-only), it can be added then without rewriting call sites.

Add `pub entitlements: EntitlementSnapshot` to `Config`, plus convenience accessors
similar to existing config accessors:

```rust
pub fn entitlements() -> &'static EntitlementSnapshot;
pub fn entitlement_limits() -> &'static EntitlementLimits;
```

## Limit Composition

Pricing limits should never raise infrastructure caps. Compose limits with the
stricter value:

```text
effective_limit = min(configured_infra_limit, entitlement_limit)
```

Initial effective limits:

| Limit | Existing config / enforcement point | Behavior |
| --- | --- | --- |
| `objectModelBulkRequestLimit` | `OBJECT_MODEL_BULK_REQUEST_LIMIT` | Use stricter value when creating `ObjectStoreConfig` |
| `maxConcurrentExecutions` | `MAX_CONCURRENT_EXECUTIONS` | Use stricter value in config accessor and execution engine |
| `maxApiKeys` | API key create handler | Count active keys before insert |
| `maxObjectSchemas` | Object schema create handler | Count tenant schemas before create |
| `maxWorkflows` | Workflow create/clone handler | Count active workflows before create |

Do not silently truncate tenant-owned resources. If a create request exceeds a
tier limit, return `403` with a stable entitlement error.

## Error Model

Use `403 Forbidden` for an authenticated tenant that lacks an entitlement.

Feature gate:

```json
{
  "error": "Entitlement required",
  "code": "ENTITLEMENT_REQUIRED",
  "feature": "reports",
  "message": "Reports are not enabled for this tenant."
}
```

Agent allowlist:

```json
{
  "error": "Agent not enabled",
  "code": "AGENT_NOT_ENABLED",
  "agent": "openai",
  "message": "Agent 'openai' is not enabled for this tenant."
}
```

Tier-limit exhaustion:

```json
{
  "error": "Tier limit exceeded",
  "code": "ENTITLEMENT_LIMIT_EXCEEDED",
  "limit": "maxApiKeys",
  "maximum": 10,
  "message": "This tenant can create at most 10 API keys."
}
```

Keep codes (`ENTITLEMENT_REQUIRED`, `AGENT_NOT_ENABLED`, `ENTITLEMENT_LIMIT_EXCEEDED`) stable so the UI and MCP clients can switch on them.

### Exception: `/api/internal/agents/{module}/{capability_id}` returns 200

One route deliberately breaks the "always 403" rule above: the internal agent dispatcher at `/api/internal/agents/{module}/{capability_id}`. When a module is denied by the allowlist, the response is:

```
HTTP/1.1 200 OK
content-type: application/json

{"success": false, "code": "AGENT_NOT_ENABLED", "error": "Agent '<module>' is not enabled for this tenant."}
```

The `code` value is the same stable string every other denial uses, but the HTTP status is 200 and the body is wrapped in the `{success, error|output}` envelope that the WASM workflow runtime has always used for *any* agent failure (normal errors, panics, denials). This is a private contract between the server and the in-process WASM runtime: the runtime treats every non-2xx response as a transport failure, so emitting a 403 here would force a runtime-side change to recognise denial as a distinct channel. We chose to keep the envelope stable instead.

Callers MUST discriminate on `code`, not on HTTP status, for this route. The audit log (`WARN entitlement denial code=AGENT_NOT_ENABLED ...`) fires the same way as every other denial, so the observability story is unchanged.

This exception applies *only* to `/api/internal/agents/{module}/{capability_id}`. REST routes under `/api/runtime/`, MCP tools, and all other internal routes (including `/api/internal/object-model/*`) follow the 403 contract above.

### Exception: `/api/internal/object-model/sql/{query,execute}` are status-coded

The workflow raw-SQL routes (backing the `object-model:query-sql` / `object-model:execute-sql` capabilities) deliberately deviate from the sibling internal object-model handlers' 200-envelope pattern in the *other* direction: SQL failures return real HTTP status codes (Validation → 400, NotFound → 404, Conflict → 409, DatabaseError → 500) via the same `raw_sql_error_response` mapper the runtime SQL routes use. Rationale: the object-model WASM agent's `check_status` classifies status codes into permanent/transient step errors (400 → permanent, 5xx → retryable-for-reads) with zero envelope-parsing agent code; a 200-envelope would make every SQL failure look like a "successful" step invisible to `onError`. Entitlement denials on these routes are unaffected — the `require_database` route layer still emits the standard 403 before the handler runs.

Operational notes for these two routes:

- **Gating:** inherited `database` gate only (owner decision 2026-07-03: no dedicated feature flag). Any database-entitled tenant process can run workflow raw SQL.
- **Guard rails (server-side):** query runs in a `READ ONLY` transaction (writes rejected by Postgres, SQLSTATE 25006); both routes run under `SET LOCAL statement_timeout` (`RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS`, default 60 000, clamped below the instance execution timeout); query results stream against a row cap (`RUNTARA_RAW_SQL_MAX_ROWS`, default 10 000) and byte cap (`RUNTARA_RAW_SQL_MAX_RESPONSE_BYTES`, default 64 MB) that error rather than truncate. Zero/invalid knob values fail boot.
- **Warning:** raw SQL bypasses per-schema authorization, property validation, and soft-delete filtering. It sees raw table rows on the connection's database. The remaining boundary is the per-tenant process + per-tenant DB + the Postgres connection role — provision a least-privilege role for SQL connections (DML + TRUNCATE on derived tables only) if DDL should be impossible. `TRUNCATE` needs the TRUNCATE privilege; DDL needs ownership (denials surface as permanent 400s, SQLSTATE 42501).
- **Audit:** one structured `tracing::info!` line per request at target `runtara::raw_sql_audit` (tenant, connection, capability, sql sha256 + 256-char prefix, params count, duration, outcome, rows). Full SQL text only at `debug`. Process-log-only; a durable audit table is future work.
- **Internal-surface exposure:** like every internal route these are authenticated only by `X-Org-Id` and protected by the loopback bind enforcement (`enforce_internal_listener_safe`); they are in scope for the open "internal bind" hardening follow-up (F6, connection-proxy plan).

## Backend Enforcement Points

### Middleware Helpers

Add small Axum extractors / helpers:

```rust
pub struct RequireFeature(pub FeatureKey);
pub struct RequireAgent(pub String);
```

Prefer explicit handler/service checks over route-layer middleware. Route layers are easy to miss for MCP and internal routes; explicit checks are easier to audit.

### REST Routes

| Surface | Gate |
| --- | --- |
| All `/api/runtime/reports*` routes (list, get, create, update, delete, edit, validate, preview, render, blocks/data, datasets/query, workflow-action submission) | `reports` |
| All `/api/runtime/object-model/*`, `/api/runtime/sql/*`, and CSV import/export routes | `database` |
| All `/api/runtime/api-keys*` (create, list, revoke) | `api` |
| `/mcp/*` transport entry | `mcp` |

Any handler that references a specific agent module (agent metadata read, agent test invocation, workflow step validation, etc.) calls `entitlements().require_agent(module_id)` before doing work.

### API-key bypass guard

The `api` entitlement does **not** gate every authenticated route — only requests whose `AuthContext.auth_method == ApiKey`. After auth succeeds, a small post-auth check rejects API-key-authenticated requests with `ENTITLEMENT_REQUIRED` when `api` is disabled. Session-cookie / OAuth users on the same routes are unaffected. This keeps `api` cleanly meaning "external automation surface", not "the whole HTTP API".

API-key management routes (`/api/runtime/api-keys*`) are gated regardless of auth method — disabling `api` should also hide the management UI.

> **Gotcha — `mcp=true` + `api=false` (SYN-433 Finding 5).** The hosted MCP transport at `/mcp/*` is only reachable via API-key auth. The API-key bypass guard above runs *before* the `mcp` transport gate, so when `api` is disabled every hosted MCP client is rejected with `ENTITLEMENT_REQUIRED` even though `mcp` reads as enabled. There is no session/OAuth auth path to the hosted MCP endpoint today, so in practice `api=false` also disables hosted MCP. This combination boots cleanly but emits a startup `WARN` (`EntitlementSnapshot::warn_risky_combinations`, predicate `mcp_unreachable_without_api`). To actually serve MCP, keep `api` enabled. Decoupling MCP auth from the `api` gate (or adding a session auth path for MCP) is tracked as a follow-up.

### MCP

- Gate the `/mcp` transport with `mcp`.
- Gate every tool group explicitly **in addition** to the transport gate, so in-process calls (no transport) and future transport changes cannot bypass.
- Report tools check `reports`; object-model tools check `database`; agent metadata/test tools and workflow mutation tools that touch agent steps check `enabled_agents` membership for the specific module.
- 403 responses from `Router::oneshot()` inside MCP tools are translated to `rmcp::ErrorData` with the original `code` preserved.

### Files / Connections / Triggers / Analytics / Invocation History

Always enabled. These are not entitlement-gated and do not have feature keys. Decision is firm for the first pricing model — they ship with every tier.

### Internal Runtime Routes (`/api/internal/*`)

Internal routes are reached either from in-process MCP (`Router::oneshot()` with pre-injected `AuthContext`) or from workflow runtime callbacks with `X-Org-Id`.

Because the process is per-tenant and the entitlement snapshot is per-process, enforcement is straightforward — **there is no per-request entitlement lookup**. The route layer / handler simply consults the single global `entitlements()` snapshot.

Apply:

- `/api/internal/object-model/*` → `database`. This includes the workflow raw-SQL routes `/api/internal/object-model/sql/{query,execute}` (see the status-code exception section above) — `database` therefore also gates the `object-model:query-sql` / `object-model:execute-sql` workflow capabilities.
- `/api/internal/agents/{module}/{capability}` → `enabled_agents` membership for `module`. Reject with `AGENT_NOT_ENABLED`.
- `/api/internal/proxy` → not gated in first pass (Connections deferred).

**Note on `X-Org-Id`:** internal handlers read this header to scope SQL queries and connection lookups, but they do **not** validate it against the configured `TENANT_ID`. The security boundary for internal routes is the localhost socket, not the header value — these routes have no JWT auth and assume the caller has already cleared the OS-level perimeter (workflow containers via pasta networking, in-process MCP calls via `Router::oneshot()`). In a single-tenant runtime, a mismatched `X-Org-Id` results in queries against an empty scope (no data leak); the entitlement gate above is unaffected because the snapshot is process-global. A strict `X-Org-Id == TENANT_ID` check would be load-bearing under a future multi-tenant model and is tracked as a follow-up there.

## Frontend Plan

### Snapshot delivery

The snapshot reaches the SPA through **two paths**:

1. **Inlined into `window.__RUNTARA_CONFIG__.entitlements`** at server-render time, in `crates/runtara-server/src/api/handlers/ui.rs` (next to the existing `runtime_config_json()` call that already populates `window.__RUNTARA_CONFIG__`). This is the primary path — it removes first-paint flicker for hidden menu items and gated routes.
2. **`GET /api/runtime/entitlements`** for refresh, MCP clients, and any non-HTML consumer.

Both surfaces return the same shape:

```json
{
  "tenantId": "tenant_123",
  "pricingTier": "pro",
  "features": {
    "reports": true,
    "database": true,
    "api": true,
    "mcp": true
  },
  "agents": ["http", "csv", "xml", "openai", "anthropic"],
  "limits": {
    "maxWorkflows": 100,
    "maxObjectSchemas": 50,
    "maxApiKeys": 10,
    "objectModelBulkRequestLimit": 1000,
    "maxConcurrentExecutions": 8
  }
}
```

`agents` in the rendered snapshot is always a concrete array: the internal `None` ("all known agents") is materialised against the dispatcher's registered modules before serialisation, so the frontend never has to reason about an implicit-all sentinel.

### Frontend tasks

1. Read the inlined snapshot first via `window.__RUNTARA_CONFIG__.entitlements`; fall back to `useEntitlements()` query if absent (graceful degradation for older HTML or refresh).
2. Add helpers:
   - `isEnabled(feature: FeatureKey): boolean`
   - `agentEnabled(moduleId: string): boolean`
3. Filter sidebar menu entries in `shared/config/index.tsx` or in `shared/layouts/Sidebar.tsx`.
4. Protect direct routes with an entitlement-aware route wrapper. Do not rely only on hidden navigation.
5. Show an upgrade/disabled-state page for direct navigation to gated routes.
6. Map backend `403` responses by `code` (`ENTITLEMENT_REQUIRED`, `AGENT_NOT_ENABLED`, `ENTITLEMENT_LIMIT_EXCEEDED`) to user-readable messages.

Frontend gating targets:

| Route | Required entitlement |
| --- | --- |
| `/reports*` | `reports` |
| `/objects*` | `database` |
| `/settings/api-keys` | `api` |
| Agent test controls inside workflow editor | `agentEnabled(moduleId)` per step |

MCP is normally not a UI route, but settings/help surfaces that mention MCP
should read `mcp`.

## MCP Plan

Add helper functions in `mcp/tools`:

```rust
fn require_feature(
    server: &SmoMcpServer,
    feature: FeatureKey,
) -> Result<(), rmcp::ErrorData>;

fn require_agent(
    server: &SmoMcpServer,
    module_id: &str,
) -> Result<(), rmcp::ErrorData>;
```

Apply to tool groups:

| Tool group | Gate |
| --- | --- |
| Report tools | `reports` |
| Object-model tools | `database` |
| Agent metadata/test tools | `enabled_agents` membership for the targeted module |
| Workflow mutation tools that add/test agent steps | `enabled_agents` membership for each referenced module |
| MCP transport itself | `mcp` |

Since MCP internally calls REST routes through `Router::oneshot()`, REST enforcement is the second line of defense. Tool-level checks come first because they (a) surface clearer errors as typed `rmcp::ErrorData` with stable codes, (b) avoid roundtripping into the router for a denied call, and (c) protect against future transport changes that bypass the HTTP router. 403s that *do* come back from `oneshot()` must be translated to `rmcp::ErrorData` with `code` preserved.

## Implementation Phases

### Phase 1 - Types and Config

- Add `entitlements.rs`.
- Add env parsing and validation to `Config::from_env()`.
- Add all-enabled development default (all features `true`, `enabled_agents = None` → all known agents).
- Add unit tests for:
  - missing env default
  - valid JSON
  - unknown feature key
  - non-boolean feature value
  - unknown agent module ID (must be rejected against the dispatcher's registered set)
  - explicit empty `agents` array (allowlist disables all)
  - invalid limits
  - pricing tier field propagation
  - precedence: tier defaults < `ENTITLEMENTS_JSON` < `OVERRIDES_JSON`

### Phase 2 - Public Snapshot Endpoint

- Add DTO and handler for `GET /api/runtime/entitlements`.
- Add OpenAPI schema annotations.
- Add route under authenticated tenant routes.
- Add tests that the endpoint returns the configured tenant's snapshot.

### Phase 3 - Backend Gates

Split into six PR-sized sub-phases, layered so each one builds on the previous. The full goal is unchanged: every disabled feature, every disallowed agent, and every exceeded tier limit must produce a stable 403 response on every authenticated entry point.

#### Phase 3.1 - Error helpers (foundation)

- Add a shared module that builds the three documented 403 responses by `code` (`ENTITLEMENT_REQUIRED`, `AGENT_NOT_ENABLED`, `ENTITLEMENT_LIMIT_EXCEEDED`).
- Provide both an HTTP variant (for REST handlers) and an `rmcp::ErrorData` variant (for MCP tools), so 3.5 doesn't have to reinvent the wire shape.
- Unit tests assert status, stable `code`, and JSON shape for each helper.
- **Out of scope:** any actual gate; nothing changes route behavior in this phase.

#### Phase 3.2 - REST feature gates

- Apply `reports`, `database`, `mcp` (transport only), and `api` (management routes only — `/api/runtime/api-keys*`) gates on the matching REST handlers using the 3.1 helpers.
- Per-handler explicit checks, not route-layer middleware (see "Backend Enforcement Points" above).
- Integration tests: denied + allowed for each surface.
- **Out of scope:** API-key auth bypass on non-admin routes (3.3); per-agent allowlist (3.4); MCP tool-level checks (3.5); numeric limits (3.6).

#### Phase 3.3 - API-key bypass guard

- Add a post-auth check that rejects any request whose `AuthContext.auth_method == ApiKey` when `api` is disabled, on every tenant route — not only `/api/runtime/api-keys*`.
- Session-cookie / OAuth users on the same routes must still pass.
- Integration tests cover both the deny path and a session-authenticated control case.
- **Out of scope:** anything other than the API-key bypass.

#### Phase 3.4 - Agent allowlist on REST + workflow compile

- Add `enabled_agents` membership checks on every REST handler that references a specific agent module (test, execute, metadata, capability call).
- At workflow create / update, walk the step graph and reject any step whose `agent.module` is not in `enabled_agents`. Return `AGENT_NOT_ENABLED` so the UI sees the same code as runtime rejections.
- **Compile-time walk is best-effort, not load-bearing.** The compile handler may also re-walk the persisted graph and short-circuit with `AGENT_NOT_ENABLED` when the snapshot rejects an agent. This is a UX nicety — it lets the "redeploy" button surface the right error instead of producing a binary that will fail at runtime — but it is **not** the security gate. Because the persisted JSON can drift from the current DSL (every step struct uses `deny_unknown_fields`), a strict deserialize-then-walk check may fall through silently on perfectly-valid older graphs. That is acceptable: the runtime dispatcher gate in Phase 5.2 is the actual hard stop and runs regardless of what compiled. If you implement the compile-time walk, fail open on parse error and rely on Phase 5.2; do not invest in a parallel-walker design just to make this gate strict.
- Integration tests for the create/update graph walk and the per-request handler checks. Compile-time walk does not require tests beyond a happy-path smoke (the security guarantee is asserted in Phase 5.2's tests, not here).
- **Out of scope:** MCP tool-level enforcement (3.5); dynamic dispatcher rejection at `/api/internal/agents/{module}/{capability}` (Phase 5).

##### Stale workflows after entitlement changes (expected behavior)

When `enabled_agents` shrinks across a restart (or a JSON-layer override removes an agent), previously-persisted workflows that reference the now-forbidden agent **remain in the database unchanged**. This is intentional — we do not retroactively mutate or delete tenant-owned resources on a config change. The behavior surfaces as follows:

| Action on a stale workflow | Behavior | Why |
| --- | --- | --- |
| `GET /workflows`, `GET /workflows/{id}` | Returns the workflow normally | Read-only access to existing tenant data is not gated; entitlement changes don't make resources disappear. |
| `POST /workflows/{id}/update` (any change) | `403 AGENT_NOT_ENABLED` | The 3.4 graph walk runs over the whole graph, not just the diff. The workflow is **effectively frozen for editing** until either the entitlement is restored or the forbidden step is removed in a separate write that *also* fails the walk — i.e. removal requires temporary entitlement restoration. |
| `PUT /workflows/{id}/versions/{v}/graph` (incremental patch) | `403 AGENT_NOT_ENABLED` | Same walk, same outcome. |
| `POST /workflows/{id}/compile` | Best-effort `403 AGENT_NOT_ENABLED`; may succeed for stale workflows whose persisted JSON no longer parses against the current DSL | The compile-time walk is a UX nicety, not a security gate (see Phase 3.4 note above). When it does fire, it returns the standard `AGENT_NOT_ENABLED` shape so the UI can surface a clear error. When it falls through (parse failure on drifted JSON, or simply not implemented for a given path), the binary is built but the stale workflow still cannot **run** — Phase 5.2's runtime dispatcher gate rejects every forbidden-agent step at execution. The hard guarantee is "stale workflows are unrunnable", not "uncompilable". |
| Execute an already-compiled stale workflow | Fails with `AGENT_NOT_ENABLED` at the step that calls the forbidden agent (Phase 5.2) | The runtime dispatcher gate at `/api/internal/agents/{module}/{capability}` is the hard gate. A stale binary keeps running for all steps that *don't* reference forbidden agents; the first forbidden-agent step fails and the workflow terminates with that error. |

This is the intended end-state: a stale workflow is **visible** (so the tenant doesn't lose data), **uneditable**, and **unrunnable** (Phase 5.2). Compile may or may not block — that's intentionally a soft gate. Restoring the entitlement re-enables everything. The frontend will surface this state explicitly once Phase 4 inlines the snapshot — the workflow editor can mark forbidden steps and the workflow list can flag affected workflows.

A workflow's removal of a forbidden step requires a brief entitlement restoration: change the env, restart, edit, restart again with the restricted entitlements. This is friction we accept rather than build a "destructive update bypass" path that could be misused.

#### Phase 3.5 - MCP tool gates

- Add tool-level helpers `require_feature(server, feature)` and `require_agent(server, module_id)` returning `rmcp::ErrorData` built from 3.1.
- Apply: report tools → `reports`; object-model tools → `database`; agent metadata/test tools → allowlist for the targeted module; workflow mutation tools that add or test agent steps → allowlist per referenced module.
- Translate 403 responses bubbling out of in-process `Router::oneshot()` calls into `rmcp::ErrorData` with the original `code` preserved.
- **Out of scope:** transport-level `/mcp` gate (already in 3.2); per-internal-route enforcement (Phase 5).

#### Phase 3.6 - Numeric limits

- Apply the five caps (`maxWorkflows`, `maxObjectSchemas`, `maxApiKeys`, `objectModelBulkRequestLimit`, `maxConcurrentExecutions`) at the enforcement points listed under "Limit Composition".
- Composition rule is `effective_limit = min(configured_infra_limit, entitlement_limit)`.
- Returns `ENTITLEMENT_LIMIT_EXCEEDED` (helper from 3.1). Do not silently truncate.
- Integration tests on the "limited" fixture from the Test Matrix.
- **Out of scope:** anything other than numeric caps.

### Phase 4 - Frontend Gating

Split into six PR-sized sub-phases mirroring Phase 3's layering. Each builds on the previous and is independently shippable — 4.1 ships the snapshot but adds no behavior, so the SPA continues to work end-to-end even if 4.2+ slip.

Only 4.1 touches Rust (`api/handlers/ui.rs`); everything else is SPA-only. `GET /api/runtime/entitlements` already exists from Phase 2, so no new REST routes are added in Phase 4.

#### Phase 4.1 - Snapshot delivery

- Extend `runtime_config_json()` in `api/handlers/ui.rs` to add an `entitlements` key whose value is `EntitlementsDto::from(crate::config::entitlements())` serialised as JSON.
- The existing CSP-hash chain (`build_index_html` → SHA-256 over the inline script body) covers the new payload automatically; the inline script body still depends only on `OnceLock`-stable env, so the "compute once at startup" invariant holds.
- Update `runtimeConfig.ts`'s `RuntimeConfig` type with `entitlements?: EntitlementsSnapshot` (interim handwritten type — 4.2 swaps it for the generated one).
- Backend tests: `inlined_script_contains_entitlements_payload` and `csp_hash_covers_entitlements_payload`.
- **Out of scope:** any consumer of the snapshot; any gating behavior.

#### Phase 4.2 - Types, hook, and helpers

- Regen the frontend API client via the `regen-frontend-api` skill so `EntitlementsDto` lands in `generated/RuntaraRuntimeApi.ts`.
- Add pure helpers `isEnabled(snapshot, feature)` and `agentEnabled(snapshot, moduleId)`.
- Add `useEntitlements()` hook: returns the inlined snapshot synchronously when `window.__RUNTARA_CONFIG__.entitlements` is present; falls back to `GET /api/runtime/entitlements` via TanStack Query when absent; falls back to a permissive default (everything on, all agents allowed) when both are missing. Permissive default matches the backend's "no entitlement env set" behavior, so a misconfigured server doesn't black-screen the UI.
- Unit tests for helpers and for all three hook paths (inlined / fetched / fallback).
- **Out of scope:** any UI consumer of the hook.

#### Phase 4.3 - Sidebar / menu filtering

- Extend `shared/config/index.tsx` menu entries with an optional `requiresFeature?: FeatureKey`. Wire `objects → database`, `reports → reports`. Workflows / Triggers / Connections / Analytics / Invocation History stay always-on (consistent with "Files / Connections / Triggers / Analytics / Invocation History" decision above).
- In `Sidebar.tsx#AppMenu`, call `useEntitlements()` and filter the menu by `isEnabled`.
- The settings gear stays visible regardless of `api`; the API-keys sub-page itself is route-guarded in 4.4.
- RTL tests for hidden / shown menu entries against fixture snapshots.
- **Out of scope:** route guards (4.4); workflow-editor surfacing (4.6).

#### Phase 4.4 - Route guards + disabled-state page

- Add `router/EntitlementRoute.tsx` — wrapper around a feature key that renders a `FeatureDisabled` page when `isEnabled` is false and `children` otherwise.
- Add `shared/pages/FeatureDisabled.tsx` — minimal "not enabled for this tenant" page with a back-to-workflows link. No upgrade CTA in MVP (single-tenant, no billing flow yet).
- Compose order in `router/index.tsx`: `PrivateRoute > EntitlementRoute > Suspense > Component`, so unauthenticated users still hit the login flow on gated URLs.
- Apply to `/reports*` → `reports`, `/objects/*` → `database`, `/settings/api-keys` → `api`.
- Tests: direct navigation to each gated path under a disabling fixture shows `FeatureDisabled`; same path under an enabling fixture renders the real page.
- **Out of scope:** 403-toast mapping (4.5); workflow-step gating (4.6).

#### Phase 4.5 - 403 error-code mapping

- Extend `handleError` in `shared/hooks/api.ts` to branch on `error.response?.status === 403 && error.response?.data?.code` for the three stable codes:
  - `ENTITLEMENT_REQUIRED` → "{Feature} not enabled" toast with the body's `message`.
  - `AGENT_NOT_ENABLED` → "Agent '{agent}' not enabled" toast.
  - `ENTITLEMENT_LIMIT_EXCEEDED` → "Tier limit reached" toast naming `limit` and `maximum`.
- Narrow `ApiError` to expose the three code-specific body fields (`feature`, `agent`, `limit`, `maximum`).
- Unit tests in the existing `api.test.ts` — one case per code — assert the right toast copy and that the generic "Error: 403" fallback is suppressed.
- **Out of scope:** anything other than the toast mapping.

#### Phase 4.6 - Agent visibility (Step Picker, editor, and any agent-test surfaces)

All UI surfaces that mention a specific agent module consult `agentEnabled()` from the resolved snapshot. Phases 4.3 and 4.4 covered *feature*-level gating; this sub-phase covers *agent*-level gating, which is finer-grained and shows up in more places. The deliverable is consistent behavior: a disabled agent should never appear as a pickable option, and any existing reference to a disabled agent should be visibly flagged.

- **Step Picker (`features/workflows/components/WorkflowEditor/NodeForm/StepPickerModal.tsx`):** filter the listed capabilities so agent modules absent from `useEntitlements().agents` are **hidden entirely**. Decision made up-front: hide rather than gray-out, matching the sidebar-filtering pattern in Phase 4.3. Rationale: prevents users from picking a step they can't save and keeps the picker free of upsell noise in a single-tenant deployment without a billing flow. If/when a multi-tenant billing model lands, revisit and add a tier-aware "available at higher tiers" hint.
- **Workflow editor (existing steps):** for each step whose `agent.module` is not in the allowlist, render an inline "Agent disabled" warning badge and disable the per-step Test control. This is the UI feedback for the management-plane lock from Phase 3.4 (see "Stale workflows after entitlement changes" above).
- **Workflow list:** surface a "needs attention" pill on rows that reference a forbidden agent. Scoped to the workflow detail page if the list endpoint doesn't carry agent module IDs (defer a list-side change to a follow-up if so).
- **Other agent-test surfaces:** audit `features/` for places that invoke a specific agent module (test buttons in settings, dev panels, capability previews) and gate them with `agentEnabled()`. Disable the control + show a short tooltip explaining why.
- **No new save-time error handling** — Phase 4.5's toast already covers any `AGENT_NOT_ENABLED` response from the backend that slips past the UI hints.
- **Tests:** RTL editor test under a snapshot that excludes one agent; RTL Step Picker test that asserts disabled modules are filtered out; Playwright smoke under a server started with the same fixture so the full end-to-end (env → snapshot → picker → editor) is covered once.
- **Out of scope:** any auto-fix or destructive UI on stale workflows — fixing requires the manual entitlement-restore flow documented above.

### Phase 5 - Runtime/Internal Enforcement

Closes the last open acceptance criterion: "Disabled features cannot be accessed via internal workflow runtime routes." The compile-time graph-walk bullet that was originally listed here was delivered earlier in Phase 3.4 (see `walk_graph_for_agents` in `crate::middleware::entitlement` and its invocation sites in `api/services/workflows.rs`), so Phase 5 narrows to the runtime side only.

Two PR-sized sub-phases. Each is independently shippable.

#### Phase 5.1 - Internal object-model gate

- Apply the existing `require_database` middleware as a `route_layer` on the `internal_object_model_routes` builder in `server.rs`. No handler changes — the gate short-circuits with `403 ENTITLEMENT_REQUIRED` before any handler runs. Mirrors the tenant-side `object_model_routes` setup.
- Integration test: hit any `/api/internal/object-model/*` path under a `database=false` snapshot and assert the standard `ENTITLEMENT_REQUIRED` body; control case under `database=true` reaches the underlying handler.

#### Phase 5.2 - Internal agent dispatcher allowlist

- In-handler check at the top of `execute_agent_capability` (`api/handlers/internal_agents.rs`): call `entitlements().require_agent(&module)` before any connection lookup or `spawn_blocking`. One route, one explicit line — easier to grep than a custom extractor.
- **Response shape** stays consistent with the existing handler's failure contract: `HTTP 200` with body `{ "success": false, "error": "Agent '<module>' is not enabled for this tenant.", "code": "AGENT_NOT_ENABLED" }`. The `code` field is the discriminator for callers; the HTTP envelope is unchanged so the WASM workflow runtime treats this as any other agent failure. Rationale: the internal route is a private contract with the WASM runtime, and changing the HTTP status would change observed behaviour for previously-working modules.
- Extracted helper `gate_internal_agent(module, snapshot)` (or similar) is unit-testable without spinning up the agents registry. Integration test hits `/api/internal/agents/<excluded-module>/<cap>` under a snapshot that excludes the module and asserts the 200 + `AGENT_NOT_ENABLED` shape; control case with the module enabled reaches the registry.
- **Out of scope:** structured-error mapping in the workflow runtime that would let the execution-history view show "Agent disabled" instead of generic "execution failed". Owned by the runtime/UI side, not platform-core.

`X-Org-Id` validation on internal routes (originally drafted as Phase 5.3) is tracked separately under future multi-tenant work — see the note in "Internal Runtime Routes" above. In a single-tenant runtime, a mismatched header produces empty-scope queries rather than data leakage, the entitlement gate is unaffected (the snapshot is process-global), and the OS-level localhost perimeter is the actual boundary. The check becomes load-bearing only when one server process serves more than one tenant.

### Phase 6 - Hardening and Operator Docs

- Document env examples in deployment docs.
- Add startup log line summarizing pricing tier, enabled features, and the materialised agent allowlist size.
- Add audit logs for entitlement-denied requests. Fields split across two layers:
  - **On the denial event itself** (emitted by `EntitlementDenial::audit_log`): the stable `code`, `tenant_id`, and one of `feature` / `agent` / `limit`+`maximum` per variant. These are the fields a denial line carries unconditionally.
  - **On the surrounding request span** (picked up by subscribers that flatten parent-span fields onto each event — JSON formatter, OTLP exporter): `method`, `uri` (from `TraceLayer`), plus `user_id` and `auth_method` (from the `request_auth` span the auth middleware wraps the request future with). Operators using a text formatter that doesn't flatten will need to correlate via request-id instead.
  - **MCP tool name** is not yet on the audit line. Each tool function knows its name but doesn't attach it to a span today; track as a follow-up if operators need denial-line tool attribution. The parent MCP request span provides indirect context in the meantime.
- Add a local test matrix script or fixture set for common tiers.

## Test Matrix

Minimum fixtures to cover:

| Fixture | Snapshot | Expected behaviour |
| --- | --- | --- |
| `all_enabled` | default | Current behaviour preserved |
| `no_reports` | `features.reports = false` | Reports menu hidden, every report REST/MCP path returns `ENTITLEMENT_REQUIRED` |
| `no_database` | `features.database = false` | Database menu hidden, every object-model REST/internal/SQL/CSV path returns `ENTITLEMENT_REQUIRED` |
| `agents_empty` | `agents = []` | Agents UI hidden; agent MCP tools deny; workflow create / update with any agent step rejected with `AGENT_NOT_ENABLED`; internal agent dispatcher returns `AGENT_NOT_ENABLED` at runtime |
| `agents_subset` | `agents = ["http"]` | `http` agent works end-to-end; calls referencing `openai` (MCP tool, internal route, workflow step at create/update, workflow step at runtime) all return `AGENT_NOT_ENABLED`. Compile of a stale workflow that already references `openai` may or may not block — the hard stop is the runtime dispatcher gate (Phase 5.2), not compile. |
| `no_api` | `features.api = false` | API key management UI hidden; API key create/list/revoke denied. API-key authenticated requests to *any* route denied with `ENTITLEMENT_REQUIRED` after auth. Session/OAuth users on the same routes still pass — include this as a control case. |
| `no_mcp` | `features.mcp = false` | `/mcp` transport rejected; if reachable in-process, individual tools also reject |
| `limited` | `limits.maxApiKeys = 1` (and others) | Creates fail with `ENTITLEMENT_LIMIT_EXCEEDED` after the limit; existing resources keep working |

Backend tests must cover both UI-facing REST routes and MCP tool paths.

## Acceptance Criteria

- A tenant's product access is fully described by env-derived entitlements.
- Server startup fails on malformed entitlement configuration.
- Disabled features cannot be accessed through:
  - direct REST calls
  - hidden-but-direct frontend routes
  - MCP tools
  - internal workflow runtime routes
- The frontend hides unavailable product surfaces and handles direct-route
  access gracefully.
- Existing local development behavior remains all-enabled unless entitlement env
  vars are set.
- No Stripe dependency is introduced into platform core.
- Future migration to DB-backed or remote-provider entitlements can happen by
  replacing the `EntitlementSnapshot` provider, not by rewriting call sites.

## Scope Decisions (for the record)

These were considered and explicitly **not** in scope for the first pricing model — call them out so a future contributor doesn't re-open them as "missing":

- `Files`, `Connections`, `Triggers`, `Analytics`, and `Invocation History` ship always-on with every tier. No feature keys.
- No separate workflow-execution gate. Workflow authoring and execution are bundled and always-on; `enabled_agents` is the only per-step lever.
- No split between API key management and runtime API-key auth. A single `api` feature key governs both. MCP-only tenants do not need API keys.
- Plan changes require a process restart. The env-backed `OnceLock` snapshot is intentional — a DB-backed or hot-reloadable snapshot can be added later by swapping the snapshot provider without touching call sites.
