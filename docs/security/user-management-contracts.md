# User-management contracts (SYN-437)

**Contract version:** `1`
**Status:** authoritative — this document is the single source of truth for the
cross-service contracts that SYN-437 introduces. runtara *enforces* these
contracts; smo-management and the Auth0 Post-Login Action *produce* the data
they describe.

Three services share state without calling each other on the request hot path:

- **Auth0** issues JWTs and owns user identity, tenant membership, and role
  assignment (via Organizations + Organization Roles).
- **smo-management** is the control plane. It mutates Auth0 (invite, remove,
  change role) and writes the resulting membership into each tenant's Valkey.
- **runtara** verifies JWT signatures offline, reads membership and revocation
  from the tenant's Valkey on every authenticated request, and enforces a static
  role → permission map. It never calls Auth0 or smo-management to authorize a
  request.

If any of the contracts below change in a way that is not backwards-compatible,
bump the contract version and coordinate the rollout across all three services.

See also: [`auth-modes.md`](../deployment/auth-modes.md) for the deploy-time auth
provider modes, and [`entitlements.md`](../deployment/entitlements.md) for
tenant-level feature gating (orthogonal to per-user authorization).

---

## 1. Source of truth

Where each piece of state lives. Every contract below assumes this split.

| Concern | Owner | Notes |
|---|---|---|
| User identity (`sub`, email, password, MFA, SSO) | **Auth0** | No `users` table in any of our databases. |
| Tenants | **Auth0 Organizations** | Auth0 `org_id` = tenant id. |
| Tenant membership (user ∈ tenant) | **Auth0 Organization Members** | Native multi-tenant-user support. |
| Role per user per tenant | **Auth0 Organization Roles** | Owner / Admin / Member / Viewer. |
| Tenant business state (slug, `valkey_url`, plan, settings, provisioning state) | **smo-management DB** | `auth0_org_id` joins back to Auth0. |
| Invitations | **smo-management DB** | Auth0 invitation API doesn't fit our flow. |
| Audit log | **smo-management DB** | Aggregates runtara- and smo-management-emitted events. See §6. |
| **Hot-path role per request** | **Per-tenant Valkey** (`member:{sub}`) | Written by smo-management, read by runtara. See §3. |
| **Token revocation** | **Per-tenant Valkey** (`token:revoked:{jti}`) | Written on revoke/removal; read by runtara. See §3. |
| Permission map (role → permissions) | **runtara code** (static) | Not data. Exported as JSON for the admin UI; runtara is the enforcer. See §4. |

Key consequences:

- **No `users` / `memberships` / `roles` tables in any of our databases.** Auth0
  owns those. smo-management reads Auth0 via the Management API when it needs to
  render the admin UI; it never persists a membership row that could drift.
- **Auth0 is never on the request hot path.** Every authenticated request — JWT,
  `rt_*` API key, `smo_*` API key, MCP session — resolves the caller's role from
  per-tenant Valkey only. Auth0 is called *only* by smo-management, off the hot
  path, when an admin mutates membership or roles.
- **API keys and MCP sessions inherit the issuing user's current role from
  Valkey.** No per-key role or permission scope. Demote the user → their keys are
  demoted on the next request. Remove the user → their keys stop working on the
  next request.

---

## 2. JWT claims contract

Auth0 issues a deliberately thin access token. The Post-Login Action injects the
custom claims; runtara validates the signature offline and reads the claims
below.

```json
{
  "sub": "auth0|abc123",
  "email": "user@acme.com",
  "name": "Ada Lovelace",
  "org_id": "org_abc",
  "tenant_slug": "acme",
  "jti": "a1b2c3d4-...",
  "iat": 1700000000,
  "exp": 1700003600
}
```

| Claim | Required | Source | Used by runtara for |
|---|---|---|---|
| `sub` | yes | Auth0 | User identity; key for `member:{sub}` lookup. |
| `org_id` | yes | Auth0 Organization id | Tenant id. Must equal the request's `TENANT_ID`; mismatch → `403`. |
| `jti` | yes¹ | Auth0 Action | Token identity; key for `token:revoked:{jti}` lookup. |
| `email` | no | Auth0 | Structured logging; `/me` response. |
| `name` | no | Auth0 | Structured logging; `/me` response. |
| `tenant_slug` | no | smo-management → Auth0 org metadata | Human-readable log field; `/me` response. **Not** used for routing — `org_id` is the tenant key everywhere internally. |
| `iss` | yes | Auth0 | Validated against `OAUTH2_ISSUER`. |
| `aud` | conditional | Auth0 | Validated against `OAUTH2_AUDIENCE` when configured. |
| `exp` | yes | Auth0 | Standard expiry validation. |

¹ `jti` becomes mandatory for `oidc` mode once the Auth0 Action is live. During
rollout it is gated by `RUNTARA_AUTH_REQUIRE_JTI` (default `false` in Stage 0,
`true` from Stage 1). See the rollout plan in the implementation plan.

**`roles` is intentionally NOT in the token.** runtara reads the live role from
`member:{sub}` in Valkey on every request, so demotions and removals take effect
within a Valkey round-trip. A `roles` claim would be cached for the token's
lifetime (1–24h), defeating immediate revocation.

**Claim namespacing.** Auth0 may require namespaced custom-claim keys (e.g.
`https://runtara.io/org_id`) depending on the API/audience configuration.
**runtara owns normalization:** `jwt_validator` accepts both the raw
(`org_id`) and namespaced (`https://runtara.io/org_id`) forms and maps them to a
single internal name. The Action may emit whichever shape the configured
audience permits; no other service depends on a specific shape.

---

## 3. Valkey key schema (per tenant)

The Valkey instance *is* the tenant boundary — no tenant prefix on keys. In the
current deployment model each tenant host runs Valkey on `localhost:6379`;
runtara reads it on every authenticated request (two `GET`s, sub-millisecond).

### `member:{uid}` — membership and role

- **Key:** `member:{uid}` where `{uid}` is the Auth0 `sub`.
- **Writer:** smo-management (on invite-accept, role change, reconciliation).
- **Reader:** runtara (every authenticated request).
- **Value:** JSON object.

```json
{ "role": "owner" | "admin" | "member" | "viewer", "updated_at": "2026-05-28T12:34:56Z" }
```

- `role` — one of the four built-in roles (see §4). Required.
- `updated_at` — RFC 3339 / ISO 8601 UTC timestamp of the last write. Advisory;
  used for drift diagnostics, not for authorization decisions.
- **No TTL.** Membership persists until smo-management deletes it (user removed)
  or overwrites it (role changed). Valkey AOF persistence plus the hourly
  reconciliation job protect against loss.
- Parse as JSON — do **not** string-split. This is a cross-service contract;
  unknown fields are ignored, an unknown `role` value fails closed.

### `token:revoked:{jti}` — revocation denylist

- **Key:** `token:revoked:{jti}` where `{jti}` is the token's `jti` claim (JWT)
  or the API key's stored `jti`.
- **Writer:** smo-management (on member removal, token revoke) and runtara (on
  local `rt_*` key revoke).
- **Reader:** runtara (every authenticated request that carries a `jti`).
- **Value:** JSON object.

```json
{ "revoked_at": "2026-05-28T12:34:56Z" }
```

- **TTL = remaining token lifetime.** The denylist entry only needs to outlive
  the token it blocks; once the token would have expired anyway, the entry
  self-cleans. For API keys with no expiry, choose a long TTL or rely on the
  key's local `is_revoked` DB flag as the permanent record.
- Presence of the key means *revoked*. runtara may use `EXISTS`/`GET`; the body
  is advisory (diagnostics only).

---

## 4. Permission map (P0)

Roles and permissions are **static code in runtara** — `access_for(role,
permission) -> Access`. They are not stored in any database. Auth0 stores only
"which role does user X hold in organization Y"; the mapping from role to
permissions is enforcement logic that lives in runtara.

### Access semantics

```
Access = Allow | Own | Deny
```

- **Allow** — permitted on any resource in the tenant.
- **Own** — permitted only on resources the caller created (`created_by ==
  caller.sub`). Owner and Admin bypass the ownership check (they get `Allow`
  semantics for `Own` rows). A resource with `created_by = NULL` (legacy /
  un-backfilled) is treated as *not owned* — only Owner/Admin may act on it.
- **Deny** — never permitted.

Ownership enforcement (the `Own` resource check) lands in Phase 2; this contract
defines the table that Phase 2 enforces.

**Legacy ownership.** Rows created before per-user ownership was captured carry a
placeholder `created_by` — either `NULL` or the literal `"jwt-user"` (the old
hard-coded value, now replaced by the real caller on create). These never match a
caller's `sub`, so under `Own` enforcement they are **managed by Owner/Admin
only** (via the bypass); a Member cannot act on a legacy resource they originally
created, because the original creator's identity was never recorded and cannot be
recovered. This is an accepted, documented limitation — **not** backfilled:
mapping `"jwt-user"` to the tenant Owner would only relabel rows the Owner can
already manage, without restoring the real creator. Legacy rows age out as keys
expire and resources are recreated. A one-off backfill is run only if a specific
tenant requires it.

### The map

`own` = allowed only on resources the caller created.

| Permission | Owner | Admin | Member | Viewer |
|---|---|---|---|---|
| `workflow:read` | Allow | Allow | Allow | Allow |
| `workflow:create` | Allow | Allow | Allow | Deny |
| `workflow:update` | Allow | Allow | Own | Deny |
| `workflow:delete` | Allow | Allow | Own | Deny |
| `workflow:execute` | Allow | Allow | Allow | Deny |
| `invocation_history:read` | Allow | Allow | Allow | Allow |
| `database:read` | Allow | Allow | Allow | Allow |
| `database:create` | Allow | Allow | Allow | Deny |
| `database:update` | Allow | Allow | Own | Deny |
| `database:delete` | Allow | Allow | Own | Deny |
| `report:read` | Allow | Allow | Allow | Allow |
| `report:create` | Allow | Allow | Allow | Deny |
| `report:update` | Allow | Allow | Own | Deny |
| `report:delete` | Allow | Allow | Own | Deny |
| `trigger:read` | Allow | Allow | Allow | Allow |
| `trigger:create` | Allow | Allow | Allow | Deny |
| `trigger:update` | Allow | Allow | Own | Deny |
| `trigger:delete` | Allow | Allow | Own | Deny |
| `connection:read` | Allow | Allow | Allow | Allow |
| `connection:create` | Allow | Allow | Allow | Deny |
| `connection:update` | Allow | Allow | Own | Deny |
| `connection:delete` | Allow | Allow | Own | Deny |
| `analytics:read` | Allow | Allow | Allow | Allow |

Notes:

- This is a **resource-only** permission vocabulary. Member, billing, and tenant
  administration are owned by smo-management (Auth0 Organizations + its admin
  surface), not by runtara's enforced map. Consequently **Owner and Admin grant
  the same set** in P0 — the distinction has no resource permission to bite on
  yet. When member/billing/tenant permissions are added, Admin gets a narrower
  list.
- Reads (`*:read`, including `invocation_history:read` and `analytics:read`) are
  open to every role, Viewer included.
- Create and `workflow:execute` require Member or above.
- Update/delete are `Own` for Member (own resources only; Owner/Admin bypass the
  ownership check) and denied for Viewer.
- `connection:read` is metadata only — connection secrets are never returned by
  read endpoints.

### Identifier shape

The Rust `Permission` enum is the canonical source of truth. On the wire,
permissions serialize as the **colon-style strings** above (`workflow:read`,
`invocation_history:read`, …) via a custom serde implementation — not the default
serde rename, which can't produce the underscore-in-resource segment. Roles
serialize as their lowercase names (`owner`, `admin`, `member`, `viewer`),
matching the `member:{uid}` value in §3. This keeps the wire form identical across
runtara code, the Valkey value, and the admin UI.

### Distribution

runtara exposes the static map as JSON at:

```
GET /api/runtime/permissions
```

The endpoint is the same for every tenant (the map is global), so it is
unauthenticated or tenant-scoped-read only. smo-management's backend fetches and
caches it; the management SPA gets a typed copy via a small TS generator off the
JSON shape. **No shared Rust crate** — the JSON endpoint decouples deploy cycles.
runtara remains the single enforcement point; the admin UI may disable controls
by role for UX but is never the source of truth.

---

## 5. Built-in roles

Four roles, modeled in Auth0 as Organization Roles, ordered most- to
least-privileged:

| Role | Wire / Valkey value | Summary |
|---|---|---|
| Owner | `owner` | Full access to every resource permission. |
| Admin | `admin` | Same resource permissions as Owner in P0 (no member/billing/tenant split in the enforced map). |
| Member | `member` | Read / create / execute on any resource; update & delete only on its **own** resources. |
| Viewer | `viewer` | Read-only across the tenant. |

Custom roles and granular permissions are out of scope for P0 (deferred to P2).

---

## 6. `audit_events` schema

Defined **once** here. Both runtara's local `audit_events` table (Phase 1.11) and
smo-management's aggregated `audit_events` table (Phase 3.2) MUST use this exact
column shape. runtara's table is *also* the wire format when smo-management
ingests runtara-emitted events — the only differences between the two tables are
ingest/transport, never column shape.

| Column | Type | Null | Notes |
|---|---|---|---|
| `id` | UUID | no | Primary key. |
| `tenant_id` | TEXT | no | Tenant id (= Auth0 `org_id`). |
| `actor_user_id` | TEXT | yes | Auth0 `sub`. NULL for system actions. |
| `source` | TEXT | no | One of `runtara` \| `smo-management` \| `auth0`. |
| `event_type` | TEXT | no | e.g. `workflow.create`, `member.role_change`. |
| `resource_type` | TEXT | yes | e.g. `workflow`, `connection`, `token`. |
| `resource_id` | TEXT | yes | Identifier of the affected resource. |
| `payload` | JSONB | no | Event-type-specific data. |
| `created_at` | TIMESTAMPTZ | no | Default `now()`. |

Indexes (both tables):

- `(tenant_id, created_at DESC)`
- `(tenant_id, actor_user_id, created_at DESC)`
- `(tenant_id, resource_type, resource_id)`

---

## 7. Failure stance

The system fails **closed**. A request that cannot be positively authorized is
rejected.

| Failure | Behavior | Error code |
|---|---|---|
| Bad JWT signature / expired / wrong issuer or audience | `401 Unauthorized` | — |
| Missing required claim (`org_id`, or `jti` once required) | `401 Unauthorized` | — |
| `org_id` ≠ request `TENANT_ID` | `403 Forbidden` | — |
| `member:{sub}` missing in Valkey (not a member) | `403 Forbidden` | — |
| Unknown/unparseable `role` value | `403 Forbidden` | — |
| `token:revoked:{jti}` present | `401 Unauthorized` | — |
| Valkey unreachable | fail-closed; loud alert | `AUTH_MEMBERSHIP_UNAVAILABLE` |
| Tenant not in `ready` provisioning state | `503 Service Unavailable` | `TENANT_NOT_READY` |

Distinct codes let operators tell infrastructure failure
(`AUTH_MEMBERSHIP_UNAVAILABLE`, `TENANT_NOT_READY`) apart from bad credentials.

---

## 8. Revocation semantics

All revocation propagates through Valkey and takes effect on the **next**
runtara request — no token reissue, no Auth0 hot-path call.

| Action | smo-management writes | Effect in runtara |
|---|---|---|
| Remove user from tenant | delete `member:{sub}` **and** write `token:revoked:{jti}` for active sessions/keys | next request fails membership check (`403`); in-flight tokens also fail the denylist check |
| Change role | overwrite `member:{sub}.role` | next request uses the new role |
| Revoke API token | write `token:revoked:{jti}` with TTL | token rejected immediately |
| Auth0 session revoke | (Auth0 only) | separate; forces re-login across browsers |

**Defense in depth on removal.** smo-management's `member:remove` follows three
independent paths so that cache staleness, a missed message, or a partial Valkey
write is caught by another layer:

1. Delete `member:{sub}` — next request fails the membership check.
2. Write `token:revoked:{jti}` for every active session/key of the user (TTL =
   remaining token lifetime) — in-flight tokens fail the denylist check even
   before membership lookup.
3. (Future, if an in-process cache is ever added) publish on
   `tenant:{id}:member-updates` to invalidate cached entries.

**Direct Auth0-console edits bypass smo-management** and leave Valkey stale until
the hourly reconciliation job runs. The sanctioned path for any role/membership
change is smo-management.

---

## 9. Changing these contracts

This document is versioned (see the header). Treat every section as a
cross-service interface:

- **Backwards-compatible additions** (a new optional JWT claim, a new permission,
  a new audit `event_type`) can ship without a version bump, but should be noted
  here in the same PR that introduces them.
- **Breaking changes** (renaming/removing a claim, changing a Valkey value shape,
  changing the permission identifier wire form, altering an `audit_events`
  column) require a contract-version bump and a coordinated rollout across
  runtara, smo-management, and the Auth0 Action.

The permission **table** itself is additionally pinned by an exact-match unit
test in runtara (Phase 1.4) so that any drift between this document and the code
fails CI.
