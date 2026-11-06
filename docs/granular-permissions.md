# Granular API-key permissions & MCP tool scoping — implementation plan

**Status:** All four phases implemented & verified across three repos.
- **runtara** (`cargo check` clean; full lib suite **1230 passed, 0 failed**): Phase 1 granular scope + create-time clamp; Phase 2 deny gate + durable event-driven revocation (control-event stream consumer + startup reconcile sweep + admin endpoint); Phase 3 MCP tool gate + `list_tools` filter.
- **runtara-cloud** (`cargo check -p runtara-cloud-customer` clean): Phase 2 producer — `publish_member_removed` `XADD {tenant}:control` on member removal.
- **runtara-frontend** (`tsc -b` + `eslint` clean): Phase 4 scope picker; client regenerated from the dumped OpenAPI (no stopgap).
- **Integration test** (`tests/control_events_revocation.rs`, gated `db-integration-tests,valkey-integration-tests`): **2 passed** against live Postgres + Valkey — the full `XADD → consume → revoke → XACK` round trip, and the startup reconcile sweep (revokes a former member, preserves a current one).

Requirements 1–4 satisfied end-to-end, including durable revocation on owner removal via the Valkey control-event stream and the startup reconcile-sweep backstop. No known gaps remain. See §13.
**Tracking:** SYN-939 (API keys grant full owner scope).
**Owner:** _tbd_
**Related:** [`security/user-management-contracts.md`](./security/user-management-contracts.md) (authoritative role→permission contract), [`deployment/auth-modes.md`](./deployment/auth-modes.md), [`deployment/entitlements.md`](./deployment/entitlements.md).

---

## 1. Goal

Make API keys (and MCP sessions authenticated by them) carry **least-privilege, owner-bounded scopes** instead of acting as a full-power owner credential. Concretely, the desired end state:

1. **Owner-bounded.** A key can never exercise more than the user who issued it currently can. Demote or remove the user and the key degrades or dies with them.
2. **Deactivated on owner removal.** When the issuing user loses tenant membership, their keys stop working — promptly, not just "eventually".
3. **Granular scopes.** A key is created with a chosen set of capabilities — workflows read / run / write, connections read / write, database read / write, triggers read / write, history & analytics — not an all-or-nothing owner grant.
4. **Same mechanism for MCP.** When a key drives the MCP server, the identical scope set decides **which tools the session can see and call**. The user choosing a key's scope is choosing its MCP toolset.

This plan reuses the existing authorization primitives wherever possible; most of the work is *extending* well-factored code plus closing two lifecycle/config gaps.

---

## 2. Current state (what already exists on `main`)

The foundation is already in place — the deployed dev env (`v10.0.0`, `scope: null`) is simply running older code. On `main`:

| Primitive | Where | Behaviour |
|---|---|---|
| `Permission` vocabulary (21) | `crates/runtara-server/src/authz/mod.rs:100` | Colon-style wire names (`workflow:read`, `connection:create`, …); single source of truth. |
| Static role→access map | `authz/mod.rs:251`–`344`, `access_for` `:348` | `Owner`/`Admin`/`Member`/`Viewer` → `Allow`/`Own`/`Deny`. Pinned to the contract by tests. |
| Live role resolution | `middleware/auth.rs:330` (`get_member_role`) | Key's role is read from Valkey `member:{issuing_user_id}` **every request** — not from the token. |
| `ApiKeyScope` (narrowing) | `authz/scope.rs` | Per-key narrowing (`Full`/`ReadOnly`), enforced **before** the role gate, **never widens**. |
| Two-gate authorization | `middleware/authorization.rs:564` (scope, Gate 1), `:586` (role, Gate 2) | Gate 1 = `scope.permits(permission, method)`; Gate 2 = `access_for(role, permission)`. |
| MCP reuses REST authz | `mcp/server.rs:1111` (`call_tool`), `tools/internal_api.rs:251` | Each tool does its work via in-process REST calls on `internal_router`, which carries the same `authorize` layer → scope + role gates already apply. |

**Consequence:** requirement 1 is already *architecturally* true wherever the role gate is active — effective access is `role ∩ scope`, and scope can only subtract. The gaps are not in the model; they are in **enforcement mode**, **scope granularity**, and **MCP tool visibility**.

### 2.1 The two teeth that are missing today

- **`MembershipPolicy` defaults to `Disabled`** (`auth/mod.rs:138`, `from_env`). Under `Disabled`/`Logging`, a missing `member:{sub}` does **not** deny (`auth.rs:283`) and the role gate goes dormant (`role = None` → `decision_for` returns `Pass`, `authorization.rs:211`). So on a default deployment, neither the owner-clamp nor the deactivate-on-removal actually bites — only `Required` makes them real. See [§6](#6-requirement-2--lifecycle-deactivate-on-owner-removal).
- **Self-hosted (non-OIDC) hard-codes `role = Owner`** with no membership lookup (`auth.rs:260`). By design — a single-owner box has no "owner's permissions" to clamp to — so there, `ApiKeyScope` is the *only* lever. This is acceptable and must stay documented, not "fixed".

### 2.2 MCP specifics

- MCP sessions authenticate through the **same** pipeline as REST (`server.rs:2795`), producing the same `AuthContext` (with `role` + `api_key_scope`).
- Enforcement is **real but indirect**: it keys on whichever REST route a tool happens to hit, not on the tool name. `execute_sql` is blocked for a `ReadOnly` key only because it POSTs to `object-model/sql/execute` (mapped to `DatabaseDelete`, `authorization.rs:442`).
- **Tool visibility is not filtered.** `list_tools` emits a single static global registry (`SmoMcpServer::tool_router()`, `server.rs:55`) identically for every session. A scoped key is denied at call time, but still *sees* every tool.
- There is **no tool→`Permission` map** to reuse; `require_feature`/`require_agent` (`mcp/entitlement.rs:26`) gate on tenant *entitlements*, orthogonal to role/scope.

---

## 3. Target design

### 3.1 Scope becomes a set of permissions

Replace the coarse two-value `ApiKeyScope` with a scope that is **a set of `Permission` grants** drawn from the existing 21-permission vocabulary — no second vocabulary, and MCP gets the same map for free.

```
ApiKeyScope =
  | Full                     // NULL column — no narrowing (back-compat)
  | Restricted(Set<Permission>)   // the key may exercise exactly these ∩ the owner's role
  | Unrecognized             // fail-closed (row from a newer build; denies everything)
```

- `ReadOnly` is retained as a **named preset** that expands to `{ all *:read }` — existing `read_only` rows keep working.
- `permits(permission, method)` becomes:
  - `Full` → `true`
  - `Restricted(set)` → `permission.map(|p| set.contains(p)).unwrap_or(method.is_safe())` — mirrors today's "mapped route needs the permission; ungated route needs a safe method" rule (`scope.rs:101`), so POST-shaped reads keep working and a restricted key still can't `POST /api/runtime/api-keys` to mint itself a wider one.
  - `Unrecognized` → `false`.
- Effective access is unchanged in shape: **Gate 1 (scope set) ∩ Gate 2 (live role)**. Scope still only ever subtracts.

### 3.2 UI presets → permission expansion

The create dialog offers grouped presets; each is just a bundle of permissions. Presets are a **UI/API convenience** — the stored value is always the expanded permission set, so there is one enforcement path.

| Preset | Expands to |
|---|---|
| Workflows — Read | `workflow:read` |
| Workflows — Run | `workflow:read`, `workflow:execute` |
| Workflows — Write | `workflow:read/create/update/delete`, `workflow:folder_rename` |
| Connections — Read / Write | `connection:read` / `+ create,update,delete` |
| Database — Read / Write | `database:read` / `+ create,update,delete` |
| Triggers — Read / Write | `trigger:read` / `+ create,update,delete` |
| History — Read | `invocation_history:read` |
| Analytics — Read | `analytics:read` |
| Full | _NULL — no narrowing_ |

> `user_management:access` is **never** grantable to a key — it is a UI-only capability (`authz/mod.rs:125`) and keys have no UI. It is excluded from every preset and rejected if requested explicitly.

### 3.3 Owner-bounded at creation *and* at request time

- **Request time (already true):** `role ∩ scope`, role resolved live. Nothing to add beyond making the role gate bite ([§6](#6-requirement-2--lifecycle-deactivate-on-owner-removal)).
- **Creation time (new, defense-in-depth):** clamp the requested scope to the **creator's own current permissions**. A `Member` cannot mint a key that names `workflow:folder_rename` (which `Member` lacks) → `403`. This keeps the stored scope honest even if the creator is later promoted/demoted, and gives a clear create-time error instead of a silent runtime denial.

---

## 4. Data model & migration

`api_keys.scope` currently holds a single keyword (`NULL`=full, `read_only`). It must hold a **set**.

**Chosen representation (implemented):** the existing column is already `TEXT` and nullable, so **no structural migration is needed** — it now also holds a JSON array of permission wire-names. (The original plan proposed `jsonb`; `TEXT` is simpler, maps directly to the existing `scope: Option<String>` row field, and avoids a sqlx type change. Only the column *comment* is refreshed — migration `20261105000000_api_keys_scope_permission_set.sql`.)

- `NULL` → `Full` (unchanged; every pre-scope key keeps its behaviour).
- `"read_only"` (string) → the `ReadOnly` preset — the legacy wire form still works.
- `["workflow:read","workflow:execute", …]` (array of permission wire-names) → `Restricted`.
- Anything else → `Unrecognized` (fail-closed; preserves the rollback-safety property in `scope.rs`).

**Representation in code:** `ApiKeyScope` keeps its `Full` / `ReadOnly` / `Unrecognized` variants (zero churn at existing call sites) and gains `Restricted(PermissionSet)`, where `PermissionSet` is a `u32` bitset over `Permission::ALL` — this keeps `ApiKeyScope: Copy`, which `ScopeDenial`, `GateDecision` and `AuthContext` all rely on. `to_column` returns `Option<String>` (was `Option<&'static str>`).

**Migration:** additive and backward-compatible — no backfill, no type change. Write path emits `NULL` for `Full`, `read_only` for the preset, and the JSON array for `Restricted`.

Touched read/write sites: `api/handlers/api_keys.rs` (`ApiKey` row, `create_api_key` `:200`, `list_api_keys`, `validate_api_key_by_hash`), `middleware/auth.rs:240` (`from_column`).

---

## 5. Requirement 3 — granular scope (Phase 1)

**Files:** `authz/scope.rs`, `authz/mod.rs`, `api/handlers/api_keys.rs`, `middleware/auth.rs`, `middleware/authorization.rs`.

1. **`authz/scope.rs`** — redefine `ApiKeyScope` per [§3.1](#31-scope-becomes-a-set-of-permissions). Update: `permits`, `from_column`/`to_column`, custom serde (accept keyword *or* array; reject unknown permission names), `utoipa` schema, `is_narrowing`, `SELECTABLE` → preset catalogue. Keep `Unrecognized` fail-closed. Extend the existing unit tests (the file already has strong coverage to mirror).
2. **`authz/mod.rs`** — add a preset→permission expansion table and a `permissions_for_role(role)` helper (derivable from `grants`) for the create-time clamp. Keep the `Permission` enum and contract test as the source of truth.
3. **`api/handlers/api_keys.rs`** — `CreateApiKeyRequest.scope` accepts a preset list or explicit permission list; expand → clamp to `permissions_for_role(creator_role)` → `403` with the offending permissions if it exceeds; persist the set. Reject `user_management:access`.
4. **`middleware/authorization.rs`** — Gate 1 already calls `scope.permits(permission, method)`; it keeps working unchanged against the richer type.
5. **Tests** — scope set round-trips through the column and wire; `Restricted` permits exactly its set (POST-shaped reads included); create-time clamp rejects over-grants; `Unrecognized` still denies all; legacy `read_only` and `NULL` unchanged.

---

## 6. Requirement 2 — lifecycle (deactivate on owner removal)

Two layers, belt-and-suspenders:

### 6.1 Make the live gate bite — ALREADY ENABLED in the cloud deploy

- For **OIDC** deployments, `MembershipPolicy::Required` must be the effective mode so a missing `member:{sub}` → `403 NOT_A_MEMBER` (`auth.rs:283`, `:387`) and the role gate clamps.
- **Verified: it is already set.** The cloud deploy config pins it: `RUNTARA_AUTH_MEMBERSHIP_POLICY=required` in `runtara-cloud/deploy/env/service.env.example:36` and the ansible template `runtara-cloud/deploy/ansible/roles/operator/templates/service.env.j2:18` (with `AUTH_PROVIDER=oidc`). So the hosted env (incl. `api.runtara.dev`) already enforces the gate — the owner-clamp and deactivation bite today.
- **Do NOT change the in-code default:** `MembershipPolicy::default_for` (`auth/mod.rs:153`) is deliberately `Disabled` for config-free/self-hosted deploys (documented rationale: an earlier draft that derived `Logging` from `oidc`+Valkey made a config-free deploy emit a shadow-denial warn on every request). The cloud sets `required` explicitly via the env above; self-hosted stays quiet unless an operator opts in. Nothing to change.
- Self-hosted (non-OIDC) is intentionally exempt (`auth.rs:260`); document, don't change.
- **Deployment/config action, partly outside this repo** — see [§9](#9-cross-repo--deployment-impacts).

> **Why runtara must not auto-revoke on a `NotAMember` miss (implementation finding).** It is tempting to have the auth middleware flip a key to `is_revoked` the moment a request resolves `NotAMember`. That is unsafe: `member:{sub}` can be *transiently* absent while smo-management rewrites it, so a cache-timing miss would permanently kill a legitimate key. Removal must be driven by the authoritative *event*, not inferred from a lookup miss. The safe behavior already exists: under `Required`, a key whose owner has no membership is denied (`403 NOT_A_MEMBER`) for as long as the membership is absent, and works again if it reappears — tested by `api_key_required_denies_when_issuer_not_member_403` (`middleware/auth.rs`). So runtara's in-repo Phase 2 is "the gate, already present"; durable revocation is §6.2.

### 6.2 Durable revocation via a Valkey control-event stream

Two things are true and separate:

- **Deny is immediate** (already works): removal deletes `{tenant}:member:{sub}`; under `Required` runtara denies the user's keys on the next request. Nothing below changes that.
- **The stored record must also become truthful.** `is_revoked` stays `false` on removal, so the key *reads* active even though it's dead. Fixing that needs runtara (the only component that owns `api_keys`) to flip the column — triggered by an authoritative signal from the control plane.

The control plane cannot reach runtara by HTTP (no authed path) or DB (separate database), so the signal rides the **shared Valkey** both already use — as a **Stream with a consumer group**, which gives reliable delivery + explicit "processed" (`XACK`).

**Producer — `runtara-cloud`.** On remove/disable, in addition to the membership write, append a control event:

```
XADD {tenant}:control * type member_removed sub <sub> at <unix_ms>
```

`at` is the removal instant. The same event fires for disable (same revocation effect). It is a general control channel — future event types (`tenant_suspended`, cache-invalidations) reuse it.

**Consumer — `runtara`.** A background task per process reads its served tenants' `{tenant}:control` streams via a consumer group (`XREADGROUP BLOCK`), and for `member_removed` / `member_disabled` runs the **terminal, timestamp-bounded** revoke, then `XACK`s:

```sql
UPDATE public.api_keys SET is_revoked = TRUE
WHERE org_id = $1 AND issuing_user_id = $2
  AND created_at <= $3            -- event.at: only keys that existed at removal
  AND is_revoked = FALSE
RETURNING jti, expires_at;
-- then deny-list each returned jti (same path as revoke_api_key)
```

Three properties make this correct:

- **Terminal:** `is_revoked` is one-way; nothing is ever restored. A re-added user mints *new* keys; their old ones stay dead. (Matches the existing model — there is no un-revoke.)
- **Race-safe:** the `created_at <= event.at` bound means a fast *remove → re-add → make a new key* sequence does **not** catch the new key — only keys that existed at the removal instant are revoked. This is why the authoritative signal is the timestamped **event**, not "current membership is absent" (which a re-add would mask).
- **Idempotent:** reprocessing revokes the same closed set — a no-op the second time — so at-least-once delivery + `XACK`-after-write is safe. Never ack before the DB write; recover stranded entries with `XAUTOCLAIM`.

**Retention + backstop.** Streams trim by length/age (`XADD … MAXLEN ~`, or `XTRIM MINID` by time — there is no per-entry TTL), so the retention window is the "TTL". The one edge is runtara being **down longer than retention** → a trimmed event is missed. Backstop (**implemented**: `reconcile_revocations` in `valkey/control_events.rs`, runs once at the top of each per-tenant consumer's `run()` before it listens): a coarse reconcile sweep revokes keys whose `issuing_user_id` has no current membership — confirmed *absent* via `EXISTS {tenant}:member:{sub}`, never on a transient read error (safe precisely because revocation is terminal and only ever *adds* revocations). Being keyed on current membership, it reconstructs the common "removed and still gone" case but not a lost event's precise timestamped revocation for a removed-then-re-added user; `revoke_user_keys_before` is idempotent, so re-running is a no-op. Stream = fast, acked, race-safe path; sweep = can't-miss net; membership gate = immediate deny meanwhile. Both the stream round trip and the sweep are covered by the live integration test `tests/control_events_revocation.rs`.

**Reused code:** the handler body is the timestamp-bounded form of `revoke_user_keys` (`api/handlers/api_keys.rs`) — extracted into a shared `revoke_user_keys_before(pool, valkey, tenant, sub, before)` the HTTP endpoint and the stream consumer both call.

**Contract:** `{tenant}:control` (producer: runtara-cloud, consumer: runtara) is a new shared-Valkey structure → add a section to `docs/security/user-management-contracts.md` and bump its contract version.

---

## 7. Requirement 4 — MCP tools gated by the same scope

> **Implementation finding — enforcement is already delivered by Phase 1.** Every MCP tool does its work through in-process REST calls on `internal_router`, which carries the same `authorize` layer (Gate 1 scope + Gate 2 role), and `build_request` (`tools/internal_api.rs:275`) re-injects the caller's `AuthContext` including `api_key_scope`. So a granular `Restricted` key used over MCP **already** has its scope enforced on every tool call, with no new code — "the same mechanism" is literally the same code path. Everything below (call-time tool gate, `list_tools` filtering) is an **optional UX refinement** that makes denials earlier/clearer and hides unusable tools; it is not required for the security property.

**Files:** `mcp/server.rs`, `mcp/entitlement.rs`, (+ a tool→permission table).

### 7.1 Tool → `Permission` (derived from the route, not a parallel table)

There are **106** `#[tool]` functions. Hand-maintaining a 106-row tool→permission table would duplicate `permission_for(method, path)` and drift from it. Instead, **derive** each tool's permission from the REST route it already calls through `internal_api::api_*`: that route + method is exactly what `permission_for` maps, and it is already what enforces the tool at call time today. The derivation is the single source of truth for both the call-time gate (§7.2) and visibility (§7.3).

Concretely: give each tool a declared `(method, path-template)` (or read it from the one `api_*` call it makes), run it through `permission_for`, and cache the result. A tool whose route is ungated (`None`) is treated as metadata — always visible, method-safe-gated like the REST ungated set.

### 7.2 Call-time gate

In `call_tool` (`server.rs:1111`), which already recovers the caller's `AuthContext` (incl. `api_key_scope`), check the tool's required permission against the caller's scope set **before** dispatch — a clean gate keyed on the tool name, closing the "tool that doesn't hit a gated route" gap. Add `require_permission(server, Permission)` alongside `require_feature`/`require_agent` in `mcp/entitlement.rs`. The downstream REST gates stay as the backstop (defense-in-depth).

### 7.3 Per-session tool visibility

Filter `list_tools` / `get_info` (`server.rs:1097`) by the caller's scope set so the session only *sees* tools it may use — this is what "the user chooses which tools are available" means at the protocol level: the key's scopes are the toolset. This is net-new (no existing filter hook); it needs the `ServerHandler` to become identity-aware via the same `AuthContext`.

> Guard the synthetic fallback context (`internal_api.rs:275`, user `mcp-internal`, `role = None`, scope `Full`): it must remain unreachable by real callers. The multi-tenant check at `server.rs:1138` already rejects an unbound caller when >1 tenant is served; the new gates must not weaken that.

### 7.4 Optional — explicit per-key tool allow/deny list

Beyond permission presets, a per-key tool-level allow/deny list gives finer control (e.g. expose `list_executions` but not `execute_sql` though both are `database:read`). **Deferred**: ship scope-driven filtering first; add tool-level overrides later if needed.

---

## 8. Phasing & sequencing

| Phase | Scope | Repo | Requirement | Status |
|---|---|---|---|---|
| 1 | Granular scope type + create-time clamp + migration + tests | runtara | 3, 1 (create-time) | done |
| 2 | Deactivate on removal: `Required` (already in deploy) + member-key deletion (control plane, exists) + deny gate (runtara, tested) + admin `revoke_user_keys` endpoint | runtara + runtara-cloud deploy | 2, 1 (teeth) | done |
| 3 | MCP tool→permission map, `call_tool` gate, `list_tools` filtering | runtara | 4 | done |
| 4 | Create-key dialog scope picker + regenerated client | runtara-frontend | 3, 4 (UX) | done |

All four phases are implemented and verified. The only thing deliberately NOT built is an automatic control-plane→runtara revoke call (§9) — superseded by the membership gate, and unjustified without new service-auth infrastructure.

---

## 9. Cross-repo & deployment status

- **Deployment/config — DONE:** `RUNTARA_AUTH_MEMBERSHIP_POLICY=required` is already set for OIDC in `runtara-cloud/deploy/env/service.env.example` and the ansible template `service.env.j2`. No change needed (§6.1).
- **Control plane removal trigger — IMPLEMENTED via a Valkey control-event stream (§6.2).** On remove, `runtara-cloud`'s `CustomerPlane::remove` emits `publish_member_removed` → `XADD {tenant}:control … type=member_removed sub at`. runtara's per-tenant `valkey::control_events` consumer reads the stream (consumer group, `XAUTOCLAIM` recovery, shutdown-aware) and runs the terminal, timestamp-bounded `revoke_user_keys_before`, then `XACK`s. No new HTTP auth or cross-DB access — it rides the shared-Valkey contract both services already use (producer: runtara-cloud; consumer: runtara). The admin endpoint `POST /api/runtime/api-keys/users/{user_id}/revoke` remains for explicit off-boarding.
- **runtara-frontend — DONE:** key-creation scope picker; client regenerated from the dumped OpenAPI (separate repo/PR).
- **OpenAPI / generated TS client — DONE:** `ApiKeyScope` is now `oneOf(keyword, permission[])`; the runtime client was regenerated to match.

---

## 10. Backward compatibility & rollout

- `NULL` scope = `Full` forever; every existing key is unaffected.
- Legacy `read_only` keyword still reads and enforces as the read preset.
- `Unrecognized` fail-closed preserves rollback safety: a `Restricted` row written by a newer build is denied (not widened to `Full`) if seen by an older one.
- Phase 2's `Required` flip is the behaviour-changing step — stage it per tenant via `Logging` first (shadow-logs denials without blocking, `auth/mod.rs:120`) to find any key whose owner is already non-member before enforcing.

---

## 11. Testing strategy

- **Unit:** scope set serde/column round-trip; `permits` membership incl. POST-shaped reads and ungated-write escalation; create-time clamp; `Unrecognized` denies all; preset expansion; `user_management:access` rejected.
- **Contract:** extend `permission_map_matches_contract` discipline — if presets reference the permission table, pin them with a test so drift fails loudly.
- **Integration (`tests/`):** `api_key_required_denies_when_issuer_not_member_403` already covers removal under `Required`; add active-revocation coverage (removed user's key → `is_revoked`/denylist); scoped key denied on out-of-scope REST route; scoped key denied on out-of-scope MCP tool at call time **and** absent from `list_tools`.
- **Manual:** against a dev tenant, mint a read-only workflow key, confirm it can list but not delete workflows and that its MCP session exposes only the read tools.

---

## 12. Open decisions

1. **Scope model** — reuse the 21 `Permission`s with UI presets (recommended; one vocabulary, MCP map for free) vs. a separate coarse capability token set. _Plan assumes the former._
2. **Requirement 2** — live `Required` gate only vs. **also** active revocation on removal. _Plan assumes both._
3. **MCP tool choice** — scope-driven filtering only (recommended first) vs. also an explicit per-key tool allow/deny list ([§7.4](#74-optional--explicit-per-key-tool-allowdeny-list)). _Plan ships scope-driven first._

---

## 13. Implementation status

| Phase | State | Notes |
|---|---|---|
| 1 — granular scope + create-time clamp + migration + tests | **done & verified** | `authz/scope.rs` (`PermissionSet`, `ApiKeyScope::Restricted`, serde/column/`permits`, 14 unit tests), `authz/mod.rs` (`permissions_for_role`, `PermissionSet` export), `api/handlers/api_keys.rs` (create-time clamp + `user_management:access` reject + `Unrecognized` reject), migration `20261105000000_api_keys_scope_permission_set.sql`. `cargo check -p runtara-server` → Finished, 0 errors; `cargo test -p runtara-server --lib authz::` → 24 passed, 0 failed. |
| 4 (enforcement) — MCP tools honour the key's scope | **done (falls out of Phase 1)** | Verified: in-process tool calls carry `api_key_scope` through the same `authorize` gate (`tools/internal_api.rs:275`). No new code. |
| 1 (req 1, request-time) — key ≤ owner's live role | **already in architecture** | Gate 1 (scope) ∩ Gate 2 (live role). Phase 1 adds the create-time clamp on top. Bites wherever the role gate is active (see Phase 2). |
| 2 — lifecycle: deactivate on owner removal | **done & verified end-to-end** | **Deny:** `Required` already on in deploy (`service.env.j2:18`); control plane deletes `member:{sub}` on removal (`members.rs`); runtara denies the removed user's keys (`api_key_required_denies_when_issuer_not_member_403`). **Durable revocation (event-driven, §6.2):** runtara-cloud `publish_member_removed` `XADD {tenant}:control …` on remove (`members.rs` + `lib.rs` `CustomerPlane::remove`); runtara per-tenant consumer `valkey::control_events` (group + `XAUTOCLAIM` + shutdown-aware) runs terminal, timestamp-bounded `revoke_user_keys_before` + `XACK`. Both repos `cargo check` clean; runtara full lib suite 1230 green; authz decision unit-tested (`may_revoke_user_keys`). Admin endpoint `POST …/users/{id}/revoke` also available. Auto-revoke-on-`NOT_A_MEMBER` deliberately NOT done (transient-miss false positives — the timestamped event is the authoritative signal). **Startup reconcile sweep (built & verified):** `reconcile_revocations` runs once per tenant at consumer start, revoking keys of users with no current membership — the backstop for events lost to stream-retention during long runtara downtime (§6.2). **Live integration test `tests/control_events_revocation.rs` — 2 passed** against Dockerized Postgres + Valkey: the `XADD → consume → revoke → XACK` round trip and the reconcile sweep (revokes former member, preserves current). No known gaps. |
| 3 — MCP tool→permission map + `call_tool` gate + `list_tools` filter | **done & verified** | `mcp/tool_permissions.rs` classifies all 103 tools → `Permission` (grouped, audited); `call_tool` fails fast on a scope-denied tool; `list_tools` hides tools the scope can't use. `cargo check` green; `mcp::tool_permissions::` tests pass incl. `every_tool_is_classified` (guards against an unmapped new tool). Advisory layer — the in-process route gate stays authoritative. |
| 4 — frontend create-key dialog (granular scope picker) | **done & verified** | `runtara-frontend`: `CreateApiKeyDialog` gains an Access selector (Full / Read-only / Custom permission checkboxes grouped by resource), sending `scope` as a keyword or permission array; backend utoipa schema upgraded to `oneOf(keyword, permission[])`. The generated `ApiKeyScope` type + `revokeUserKeys` client method were taken from a **real regen** (`cargo run --bin dump_openapi` → `swagger-typescript-api` against the spec file), applied surgically to avoid unrelated drift from the stale committed client — no hand-stopgap. `tsc -b` + `eslint` clean. Lives in a separate repo (separate PR/branch). |

**Env note:** the dev box this was implemented on has no `cargo`/`rustc` on `PATH`; builds run in Docker with the pinned 1.97.0 toolchain (`docker run --rm -v <worktree>:/work -w /work rust:1.97-bookworm cargo …`). Re-run `cargo check -p runtara-server` and `cargo test -p runtara-server --lib authz::` to reproduce.
