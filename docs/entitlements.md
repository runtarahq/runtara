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

### MCP

- Gate the `/mcp` transport with `mcp`.
- Gate every tool group explicitly **in addition** to the transport gate, so in-process calls (no transport) and future transport changes cannot bypass.
- Report tools check `reports`; object-model tools check `database`; agent metadata/test tools and workflow mutation tools that touch agent steps check `enabled_agents` membership for the specific module.
- 403 responses from `Router::oneshot()` inside MCP tools are translated to `rmcp::ErrorData` with the original `code` preserved.

### Files / Connections / Triggers / Analytics / Invocation History

Always enabled. These are not entitlement-gated and do not have feature keys. Decision is firm for the first pricing model — they ship with every tier.

### Internal Runtime Routes (`/api/internal/*`)

Internal routes are reached either from in-process MCP (`Router::oneshot()` with pre-injected `AuthContext`) or from workflow runtime callbacks with `X-Org-Id`.

Because the process is per-tenant and the entitlement snapshot is per-process, enforcement is straightforward — **there is no per-request entitlement lookup**:

1. The existing `X-Org-Id == TENANT_ID` check already runs.
2. Then check the single global `entitlements()` snapshot.

Apply:

- `/api/internal/object-model/*` → `database`.
- `/api/internal/agents/{module}/{capability}` → `enabled_agents` membership for `module`. Reject with `AGENT_NOT_ENABLED`.
- `/api/internal/proxy` → not gated in first pass (Connections deferred).

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
- At workflow create / update / compile, walk the step graph and reject any step whose `agent.module` is not in `enabled_agents`. Return `AGENT_NOT_ENABLED` so the UI sees the same code as runtime rejections.
- Integration tests for both the static (graph-walk) and per-request (handler) checks.
- **Out of scope:** MCP tool-level enforcement (3.5); dynamic dispatcher rejection at `/api/internal/agents/{module}/{capability}` (Phase 5).

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

- Inline the snapshot into `window.__RUNTARA_CONFIG__.entitlements` in `api/handlers/ui.rs`.
- Add generated or handwritten entitlement types.
- Add entitlement read hook (prefers inlined snapshot, falls back to `GET /api/runtime/entitlements`).
- Add `isEnabled(feature)` and `agentEnabled(moduleId)` helpers.
- Filter static menu items.
- Add route guard for gated pages.
- Map `403` responses by error `code` to user-readable messages.
- Add frontend tests for hidden menu entries, direct-route blocked states, and the inlined-vs-fetched fallback path.

### Phase 5 - Runtime/Internal Enforcement

- Gate `/api/internal/object-model/*` with `database`.
- Gate `/api/internal/agents/{module}/{capability}` with `enabled_agents` membership for `module`. Reject with `AGENT_NOT_ENABLED`.
- In workflow create/update/compile, walk the step graph and reject any step whose `agent.module` is not in `enabled_agents`. Surface the same `AGENT_NOT_ENABLED` error so the UI can show a consistent message.
- Add tests for both the static (compile-time) rejection of forbidden agent steps and the dynamic (execution-time) rejection at the internal dispatcher route.

### Phase 6 - Hardening and Operator Docs

- Document env examples in deployment docs.
- Add startup log line summarizing pricing tier, enabled features, and the materialised agent allowlist size.
- Add audit logs for entitlement-denied requests with tenant, feature or agent ID, route/tool, and user/auth method.
- Add a local test matrix script or fixture set for common tiers.

## Test Matrix

Minimum fixtures to cover:

| Fixture | Snapshot | Expected behaviour |
| --- | --- | --- |
| `all_enabled` | default | Current behaviour preserved |
| `no_reports` | `features.reports = false` | Reports menu hidden, every report REST/MCP path returns `ENTITLEMENT_REQUIRED` |
| `no_database` | `features.database = false` | Database menu hidden, every object-model REST/internal/SQL/CSV path returns `ENTITLEMENT_REQUIRED` |
| `agents_empty` | `agents = []` | Agents UI hidden; agent MCP tools deny; workflow create with any agent step rejected at compile; internal agent dispatcher returns `AGENT_NOT_ENABLED` |
| `agents_subset` | `agents = ["http"]` | `http` agent works end-to-end; calls referencing `openai` (MCP tool, internal route, workflow step at compile, workflow step at runtime) all return `AGENT_NOT_ENABLED` |
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
