# QuickBooks Online Integration & Agent — Implementation Plan

_Status: draft plan (2026-07-06). All API/OAuth values and code-path claims below were adversarially verified against the real runtara source and Intuit's production discovery/docs. Where a point remains unverified it is marked **TO CONFIRM**._

> **Post-verification update (2026-07-06):** Two previously-flagged items were checked directly against the source and are now resolved:
> - **Static connection-type registry — CONFIRMED required.** `crates/runtara-agents/src/static_registry.rs:140` defines `CONNECTION_TYPES`; line 153 lists HubSpot; `registry.rs:80` iterates it in `find_connection_type`. Adding the one registry line (§6) is genuinely required.
> - **Refresh-token rotation persistence — CONFIRMED as a real gap, and larger than "same code path".** The refresh path returns only access_token+expiry and caches it **in-memory** (`token_cache.rs`); the rotated `refresh_token` is discarded and never written back to `connection_parameters`. `token_cache.rs` holds **no `ConnectionRepository` handle**, so fixing this needs new plumbing (a write-back callback/repo threaded into `resolve_deferred_auth`, or an event the connections service persists), not a one-liner. This is the single largest runtime change in the plan — see §3(b)/§9.

---

## 1. Summary & feasibility

Adding QuickBooks Online (QBO) is **mostly a configuration + new-agent job, plus a handful of well-scoped OAuth-subsystem changes** — most small, but one (refresh-token **rotation persistence**) is a genuine, non-trivial runtime change because the token cache has no DB write-back path today. What already exists and is reused verbatim: the full 3-legged OAuth 2.0 authorization-code flow (`GET /api/runtime/connections/{id}/oauth/authorize` → CSRF `state` in `oauth_state` → public `GET /api/oauth/{tenant_id}/callback` → code exchange → AES-256-GCM-encrypted `connection_parameters` → automatic request-time token refresh via `provider_auth.rs` + `token_cache.rs`); the `#[derive(ConnectionParams)]` macro that auto-generates all connection metadata **including the `OAuthConfig`**; the connection-type registry, API exposure, and OAuth config lookup, which are all automatic once one struct + one registry line are added; and the WASM integration-agent pattern (HubSpot) with `_connection` injection and proxy-routed HTTP that never lets the component see credentials. What is **net-new**: (a) the `quickbooks_online` connection-params struct + one registry line; (b) **realmId capture** in the OAuth callback (a new query field threaded into the param merge — QBO returns `realmId` only as a callback query param, never in the token JSON); (c) **HTTP Basic auth at the Intuit token endpoint** for *both* the initial code exchange *and* refresh (runtara currently form-encodes `client_id`/`client_secret` in the body, which Intuit rejects); (d) **refresh-token rotation persistence** — write Intuit's rotated `refresh_token` back to `connection_parameters` on every refresh (today's in-memory cache discards it; the non-trivial one); (e) **environment- and realm-aware base-URL resolution** in `describe_connection_auth`; and (f) the `runtara-agent-quickbooks` WASM component with a generic-core-plus-thin-typed capability set. None of this requires schema migrations (connection_parameters is JSONB) or smo-management changes (bring-your-own OAuth app, no central registry).

---

## 2. Connection type design

### 2.1 The `#[connection]` params struct

Add to `crates/runtara-agents/src/agents/extractors/connection_types.rs`, modeled on `HubSpotPrivateAppParams` (lines ~686–695):

```rust
/// QuickBooks Online (Intuit) — OAuth 2.0 authorization-code connection.
/// Bring-your-own Intuit app: client_id/client_secret entered per connection.
#[derive(Debug, Clone, Serialize, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "quickbooks_online",
    display_name = "QuickBooks Online",
    description = "Intuit QuickBooks Online Accounting API (v3) — customers, invoices, bills, payments, items, reports.",
    category = "erp",
    auth_type = "oauth2_authorization_code",
    oauth_auth_url  = "https://appcenter.intuit.com/connect/oauth2",
    oauth_token_url = "https://oauth.platform.intuit.com/oauth2/v1/tokens/bearer",
    oauth_default_scopes = "com.intuit.quickbooks.accounting"
)]
pub struct QuickBooksOnlineParams {
    /// Intuit app client id (from the Intuit developer dashboard).
    #[field(display_name = "Client ID", secret = false)]
    pub client_id: String,

    /// Intuit app client secret.
    #[field(display_name = "Client Secret", secret = true)]
    pub client_secret: String,

    /// OAuth scopes (space-delimited). Default: accounting scope only.
    #[field(display_name = "Scopes", optional = true)]
    pub scopes: Option<String>,

    /// Target Intuit environment. "sandbox" or "production". Default sandbox.
    #[field(display_name = "Environment", optional = true)]
    pub environment: Option<String>,

    /// QuickBooks company id (realmId). Populated automatically from the
    /// OAuth callback; may be left blank at create time.
    #[field(display_name = "Realm ID (Company ID)", optional = true)]
    pub realm_id: Option<String>,

    /// API minor version to pin (default 75). Configurable to ride schema bumps.
    #[field(display_name = "Minor Version", optional = true)]
    pub minor_version: Option<String>,

    // --- Populated by the OAuth callback, not user-entered ---
    #[field(skip)]
    pub access_token: Option<String>,
    #[field(skip)]
    pub refresh_token: Option<String>,
    #[field(skip)]
    pub token_expires_at: Option<String>,
}
```

Notes on field choices (verify exact `#[field(...)]` attribute names against the macro at `crates/runtara-agent-macro/src/lib.rs:811-1011` and the HubSpot struct before finalizing — **TO CONFIRM** the precise `secret`/`optional`/`display_name` spellings):
- `access_token` / `refresh_token` / `token_expires_at` are written by the existing callback+refresh machinery into `connection_parameters`; mark them `#[field(skip)]` so they are not user-facing (same pattern as any provider).
- `realm_id` is nominally optional/user-editable but is **normally populated by the callback** (see 2.3). Keeping it in the schema lets an operator paste it manually as a fallback.

### 2.2 Category

Use **`category = "erp"`** — this is the VERIFIED-valid category value. `ConnectionCategory::Erp` exists (`crates/runtara-dsl/src/agent_meta.rs:620-632`; macro mapping at `crates/runtara-agent-macro/src/lib.rs:902-938`). `"accounting"` and `"finance"` do **not** exist and would fail macro expansion. (`"api"` would also compile and is arguably "more honest" for a REST wrapper, but `"erp"` is the semantically correct classification for QuickBooks.)

### 2.3 OAuth endpoint strings (VERIFIED Intuit values)

| Attribute | Value |
|---|---|
| `oauth_auth_url` | `https://appcenter.intuit.com/connect/oauth2` |
| `oauth_token_url` | `https://oauth.platform.intuit.com/oauth2/v1/tokens/bearer` (same URL for `authorization_code` and `refresh_token`) |
| `oauth_default_scopes` | `com.intuit.quickbooks.accounting` |
| (revocation, for later) | `https://developer.api.intuit.com/v2/oauth2/tokens/revoke` |

The macro generates the inline `OAuthConfig` from `oauth_auth_url` + `oauth_token_url` automatically (`crates/runtara-agent-macro/src/lib.rs:975-990`), so no OAuth config is hand-written or duplicated.

### 2.4 How `realm_id` gets populated

`realmId` (== QuickBooks company id, the `{realmId}` path segment) is returned by Intuit **only as a query parameter on the callback redirect** (alongside `code` and `state`), and **only when the accounting/payments scope was requested**. It is **not** in the token JSON. Therefore:

- **Primary mechanism: OAuth-callback capture.** The public callback handler must read `realmId` off the query string and merge it into `connection_parameters` in the same write that stores the tokens. See §3(a) for the exact patch.
- **Fallback: user-entered.** The `realm_id` field in the struct lets an operator paste a company id manually if capture ever fails (e.g. re-authorizing an existing connection). This is a safety net, not the happy path.
- No `oauth_state` schema change is needed — `realmId` arrives directly on the callback URL and does not need round-trip persistence through the `state` row.

---

## 3. OAuth flow gaps to close

Three concrete runtime changes are required **beyond** declaring the connection type. Each is assessed as real-change vs. already-supported.

### (a) realmId capture in the callback — **REAL CODE CHANGE (small)**

Currently `OAuthCallbackQuery` (`crates/runtara-connections/src/handler/oauth.rs:111-117`) has exactly four fields (`code`, `state`, `error`, `error_description`) — **no `realm_id`**. The callback never sees it, and `OAuthService::handle_callback` (`crates/runtara-connections/src/service/oauth.rs:140-212`) never persists it. Minimal patch (4 edits, no migration — `connection_parameters` is JSONB):

1. `handler/oauth.rs:112-117` — add `pub realm_id: Option<String>,` to `OAuthCallbackQuery`.
2. `handler/oauth.rs:~170` — pass it through: `service.handle_callback(&oauth_state, &code, params.realm_id)`.
3. `service/oauth.rs:140` — widen the signature: `handle_callback(&self, state: &str, code: &str, realm_id: Option<String>)`.
4. `service/oauth.rs:182-198` — in the params-merge block, after the token fields, insert:
   ```rust
   if let Some(realm) = realm_id {
       obj.insert("realm_id".to_string(), Value::String(realm));
   }
   ```
   before `connection_repo.update_parameters_and_status(...)` seals and stores the JSON.

### (b) HTTP Basic auth at the Intuit token endpoint — **REAL CODE CHANGE (both paths)**

Intuit **requires** `Authorization: Basic base64(client_id:client_secret)` with `Content-Type: application/x-www-form-urlencoded`; credentials go in the **Basic header, not the body**. runtara today does the opposite in **both** token calls:

- **Initial code exchange** — `exchange_code()` (`crates/runtara-connections/src/service/oauth.rs:223-244`) hardcodes `grant_type=authorization_code&code=…&client_id=…&client_secret=…&redirect_uri=…` in the body, no `Authorization` header.
- **Refresh** — `refresh_oauth_access_token()` (`crates/runtara-connections/src/auth/token_cache.rs:167-191`) form-encodes `client_id`/`client_secret` in the body, no `Authorization` header.

Both will fail against Intuit's endpoint. The precedent for Basic auth already exists: Mailgun builds a Basic header via `base64::Engine::encode()` in `describe_connection_auth` (`provider_auth.rs:171-172`), so the base64 machinery is present.

**Chosen approach — provider-aware auth strategy (not a Mailgun-style hardcode).** Introduce a small `TokenEndpointAuth` enum (`FormBody` | `BasicHeader`) resolved by `integration_id`:
- Thread the strategy into `exchange_code()` and `refresh_oauth_access_token()`. Default `FormBody` (preserves HubSpot behavior exactly); `quickbooks_online` ⇒ `BasicHeader`.
- When `BasicHeader`: set `Authorization: Basic <base64(client_id:client_secret)>`, keep `Content-Type: application/x-www-form-urlencoded`, and **drop `client_id`/`client_secret` from the body** — body carries only `grant_type` + (`code`+`redirect_uri` | `refresh_token`).
- The `DeferredAuth::OAuth2RefreshToken` variant (`token_cache.rs:30-39`) should carry the strategy (or an integration_id it can map from) so refresh knows which mode to use.

Wire the strategy selection through `describe_oauth_refresh_auth` / `describe_connection_auth` in `provider_auth.rs` (the same place that already special-cases per integration).

> **Refresh-token rotation caveat (must-fix — CONFIRMED gap, highest-effort change in this plan):** Intuit **rotates the refresh token on every refresh** — each `/tokens/bearer` refresh response returns a *new* `refresh_token` and invalidates the previous one. **Verified 2026-07-06:** the current refresh path does NOT persist it. `refresh_oauth_access_token()` (`token_cache.rs:167-189`) returns a `CachedAccessToken` carrying only `access_token`+`expires_at`; `resolve_cached_token` (`token_cache.rs:126-139`) stashes that in an **in-memory DashMap** via `cache_token`; the rotated `refresh_token` in the response body is **parsed away and discarded**. HubSpot never noticed because its refresh tokens don't rotate. For QBO this means the connection **permanently breaks ~1 hour after the first refresh**. Fixing it is **not** a same-function tweak: `token_cache.rs` has **no `ConnectionRepository` handle** (its `resolve_deferred_auth` only receives `client`, `events`, `connection_id`, `integration_id`). The fix requires new plumbing — either (i) thread a write-back callback / repo handle into `resolve_deferred_auth` so a rotated `refresh_token` (and fresh `token_expires_at`) is sealed back into `connection_parameters`, or (ii) emit a new lifecycle event carrying the rotated token that the connections service persists. Treat this as its own sub-task in Phase 1, with a dedicated regression test (§8 step 5).

### (c) Environment (sandbox/prod) base-URL resolution — **REAL CODE CHANGE (in `describe_connection_auth`)**

See §4. This is real because neither existing base-URL mechanism can vary the host by a stored `environment` param **and** embed the `realmId` path segment.

---

## 4. base_url / auth resolution

Two separate mechanisms exist; only one is authoritative at proxy request time:

- `HttpConnectionExtractor::extract()` → `HttpConnectionConfig.url_prefix` (`crates/runtara-agents/src/agents/extractors/mod.rs:31-50`) runs at **schema-discovery time**, is synchronous, and **the proxy never reads it**. For HubSpot the hardcoded `url_prefix` is effectively dead code on proxy-routed calls.
- `describe_connection_auth()` (`crates/runtara-connections/src/auth/provider_auth.rs:69-328`) runs **per request** and **always wins**: the proxy (`crates/runtara-server/src/api/handlers/internal_proxy.rs:270-271, 310`) calls `resolve_connection_auth()` → `describe_connection_auth()` and pins the request URL to its returned `base_url`, and injects the `Authorization: Bearer <access_token>` there too (after automatic refresh).

**Therefore, for QuickBooks put ALL URL + auth logic in `describe_connection_auth`, not the extractor.** Add a `quickbooks_online` arm that:
1. Reads `environment` from connection params → host = `https://sandbox-quickbooks.api.intuit.com` (sandbox) or `https://quickbooks.api.intuit.com` (production). **These are genuinely different hosts** (a prior draft wrongly used identical URLs).
2. Reads `realm_id` from params.
3. Returns `base_url = "{host}/v3/company/{realm_id}"` so the proxy pins requests to the correct company-scoped root. The agent then appends only `/{entity}...` + `?minorversion=…`.
4. Sets the Bearer token from the (auto-refreshed) `access_token` param, exactly as the HubSpot arm does.

**Known architectural limitation to document:** the connection system has **no runtime path-template interpolation** — it cannot inject request-time path params. We sidestep this by folding the *stored* `realm_id` into `base_url` inside `describe_connection_auth`. That works because `realm_id` is a **stored connection property**, not a per-request argument. If a single connection ever needed to address multiple realms per request, that would require proxy-level `{realmId}` templating (not planned; one connection == one realm is the intended model).

---

## 5. The agent (WASM component)

### 5.1 Structure (matches VERIFIED HubSpot agent)

- **Crate location:** `crates/agents/runtara-agent-quickbooks/` (standalone crate — **not** a module in `runtara-agents`, which is now legacy host-native only). Package name `runtara-agent-quickbooks`, `crate-type = ["cdylib", "rlib"]`.
- **Files:** `Cargo.toml` (copy HubSpot's; deps: `wit-bindgen-rt`, workspace `serde`/`serde_json`/`base64`, `runtara-agent-macro`, `runtara-dsl` default-features-off, `runtara-http` with `native`/`wasi` per-target features; `[package.metadata.component]` → `package = "runtara:agent-quickbooks"`, world `agent`, dep on `runtara-agent-wit`). `build.rs` copied verbatim (auto-generates `wit/agent.wit` from `CARGO_PKG_NAME`). `src/lib.rs` with capabilities. `wit/agent.wit` auto-generated — never hand-edit.
- **Connection injection:** every input struct carries
  ```rust
  #[field(skip)]
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub _connection: Option<RawConnection>,
  ```
  and each capability begins `let connection = require_connection(&input._connection)?;`.
- **HTTP mechanism:** `runtara_http::HttpClient::with_timeout(...)`, set the `X-Runtara-Connection-Id: {connection.connection_id}` header, then `.call_agent()` to route through the host proxy. The proxy resolves the connection, applies §4 base_url, and injects the Bearer token server-side. **The component never sees credentials, the realmId host, or the token.** Provide typed helpers `qbo_get` / `qbo_post` / `qbo_post_query` / `qbo_upload` mirroring `hubspot_get`/`hubspot_post`.
- **Capability macro:** `#[capability(module = "quickbooks", module_display_name = "QuickBooks Online", module_supports_connections = true, module_integration_ids = "quickbooks_online", module_secure = true, module_has_side_effects = <per verb>, display_name = "...", description = "...")]`. `module_integration_ids` **must** equal the `integration_id` registered in §2 (`quickbooks_online`).
- **Content-Type per operation (do NOT blanket application/json):** CRUD + batch = `application/json`; `POST /query` = `application/text` (raw SQL body); CDC request = `text/plain` (JSON response); upload = `multipart/form-data`. Auth header is injected by the proxy on every call.

### 5.2 Capability set (hybrid: generic core + thin typed layer)

All ids kebab, agent id `quickbooks`. **9 generic capabilities are the only runtime code paths** (cover all ~40 entities day one); the **typed layer is thin presets over the same helpers**, costing schema JSON not new HTTP handlers. Surface `{ id, syncToken }` at top level on read/create/update so DSL refs like `steps.read.outputs.syncToken` work without digging into nested Intuit JSON. Model an `entity` **enum** (committed QBO entity list) so the Step Picker renders a dropdown, not free text.

**Generic core (9):**

| Capability | Inputs | Outputs |
|---|---|---|
| `quickbooks-query` | `{ entity, where?, orderBy?, startPosition?, maxResults? }` **or** raw `query`; `minorVersion?` | `{ items[], count, startPosition, maxResults }` — wraps `GET/POST /query` (SQL `SELECT … FROM <Entity>`; PascalCase entity in `FROM`; page default 100, max `MAXRESULTS` 1000) |
| `quickbooks-read` | `{ entity, id, minorVersion? }` | `{ entity, id, syncToken }` — `GET /{entity}/{id}` |
| `quickbooks-create` | `{ entity, body, minorVersion? }` | `{ entity, id, syncToken }` — `POST /{entity}` |
| `quickbooks-update` | `{ entity, id, syncToken, body, sparse?=true, minorVersion? }` | `{ entity, id, syncToken }` — `POST /{entity}` with `Id`+`SyncToken` (+`sparse:true`). Full update is read-modify-write (unspecified writable fields reset) |
| `quickbooks-delete` | `{ entity, id, syncToken, minorVersion? }` | `{ id, status }` — `POST /{entity}?operation=delete`. **Name-list entities (Account/Customer/Item/Vendor/Class…) are NOT hard-deletable** — deactivate via sparse update `Active:false`; optionally alias `quickbooks-deactivate` |
| `quickbooks-report` | `{ report_name, params:{ start_date,end_date,accounting_method,… } }` | `{ report }` — `GET /reports/{name}` (Intuit Header/Columns/Rows envelope) |
| `quickbooks-batch` | `{ operations:[{ bId, operation, entity?, body?, query? }] }` (**≤30**) | `{ results:[{ bId, entity?/queryResponse?, fault? }] }` — per-item faults, chunk larger sets |
| `quickbooks-cdc` | `{ entities:[string], changedSince: ISO8601 }` | `{ changes:{ <entity>:{ items[], deleted[{id}] } }, cdcTimestamp }` — `GET /cdc`; ~30-day look-back, ≤1000 objects |
| `quickbooks-upload-attachment` | `{ file:{ bytes/ref, contentType, fileName }, attachTo?:{ entityType, entityId }, note? }` | `{ attachableId, fileAccessUri?, tempDownloadUri? }` — `POST /upload`, multipart (distinct code path) |

**Thin typed layer (5–6, presets over the core — better picker/mapping UX for the dominant write paths):**

| Capability | Purpose |
|---|---|
| `quickbooks-create-invoice` | Hides `Line[].DetailType/SalesItemLineDetail`; `{ customer_ref, line_items[], txn_date?, due_date?, currency?, doc_number?, … }` → `{ invoice, id, syncToken, doc_number, total_amt, balance }` |
| `quickbooks-upsert-customer` | `{ match_by (display_name\|email\|id), customer{…} }` → query-then-create-or-update, avoids duplicate-name faults → `{ customer, id, syncToken, created }` |
| `quickbooks-record-payment` | Applies a Payment to invoices via `LinkedTxn`; `{ customer_ref, total_amt, line[{ invoice_id, amount }], … }` → `{ payment, id, syncToken }` |
| `quickbooks-create-bill` | `{ vendor_ref, line_items[{ amount, account_ref\|item_ref, description? }], … }` → `{ bill, id, syncToken }` |
| `quickbooks-get-customer` | (optional) typed read convenience by id/display_name/email |

Keep the typed set ≤6; do **not** grow toward per-entity completeness (that path is ~200 steps and floods the picker for little gain, since QBO's own API is already generic).

---

## 6. Registration & build steps

The registration flow is **highly automated** — the macro generates all metadata and the inline `OAuthConfig`; discovery, API exposure, and OAuth lookup are automatic. Concretely:

**Connection type (2 hand-edits):**
1. **Struct** → add `QuickBooksOnlineParams` (§2.1) to `crates/runtara-agents/src/agents/extractors/connection_types.rs`. The `#[derive(ConnectionParams)]` macro auto-emits `__CONNECTION_META_QuickBooksOnlineParams` with all field + OAuth metadata.
2. **Registry line** → add one entry referencing that generated static to the `CONNECTION_TYPES` slice in `crates/runtara-agents/src/static_registry.rs` (~lines 140-160; HubSpot's line 153 is the template). **This file IS hand-maintained and IS required** (the design is intentional: an explicit list, no runtime reflection, for deterministic WASM builds). After this, discovery (`registry.rs:79-96 find_connection_type`), API exposure (`handler/connections.rs:489-499 get_all_connection_types`), and OAuth lookup (`service/oauth.rs:85-91`) all pick it up automatically — **no further edits**.

**Runtime OAuth patches (§3):** edits in `handler/oauth.rs`, `service/oauth.rs`, `auth/token_cache.rs`, `auth/provider_auth.rs` (realmId capture, Basic-auth strategy, refresh-token rotation persistence, env/realm base_url arm). These are the only changes outside the new agent crate.

**Agent crate (create + 3 wiring edits):**
1. Create `crates/agents/runtara-agent-quickbooks/{Cargo.toml, build.rs, src/lib.rs, wit/}` (§5.1).
2. Root `Cargo.toml` → add `"crates/agents/runtara-agent-quickbooks",` to `[workspace] members` (auto-discovered by `scripts/build-agent-components.sh` via the `crates/agents/runtara-agent-` grep).
3. `crates/runtara-agent-bundle-emit/Cargo.toml` → add `runtara-agent-quickbooks = { path = "../agents/runtara-agent-quickbooks" }` to `[dependencies]`.
4. `crates/runtara-agent-bundle-emit/src/main.rs` → add `("quickbooks", runtara_agent_quickbooks::agent_info()),` to the `agents()` list. (If omitted, the build script's wasm-count vs meta-count check fails loudly: "some agent must be added to runtara-agent-bundle-emit's agent list".)

**Build / emit / run:**
```bash
./scripts/build-agent-components.sh     # builds runtara_agent_quickbooks.wasm + emits .meta.json sidecar
export RUNTARA_AGENT_COMPONENTS_DIR=target/wasm32-wasip2/release
# capabilities then appear in /api/runtime/agents and the Step Picker
```
`meta.json` is **generated** from the macro statics via `emit-meta` — never hand-written. After adding the connection type / capabilities, run the **`regen-frontend-api`** skill so the generated frontend client + types (Step Picker, Connection UI forms) stay in sync.

---

## 7. Phased delivery plan

**Phase 1 — thin vertical slice (connection + OAuth gaps + read path).** Add the `quickbooks_online` connection type (§2) and registry line; land the three OAuth runtime patches (§3: realmId capture, Basic-auth strategy on exchange+refresh with rotation persistence, env/realm base_url arm in `describe_connection_auth`). Ship the agent crate with just `quickbooks-query` and `quickbooks-read`. Goal: create a sandbox connection, complete the interactive OAuth authorize→callback, confirm tokens + `realm_id` land in `connection_parameters`, and run a workflow that queries a Customer end-to-end. This is fully e2e-verifiable and proves every hard part (Basic auth, realmId, sandbox host).

**Phase 2 — writes + reporting.** Add `quickbooks-create`, `quickbooks-update` (Id+SyncToken, sparse/full), `quickbooks-delete`/deactivate, `quickbooks-report`, `quickbooks-batch`, `quickbooks-cdc`. Exercise the read→update SyncToken chain and 5010 stale-token handling (re-read on conflict).

**Phase 3 — typed high-value layer + attachments.** Add `quickbooks-create-invoice`, `quickbooks-upsert-customer`, `quickbooks-record-payment`, `quickbooks-create-bill` (+ optional `quickbooks-get-customer`) as presets over the core, and `quickbooks-upload-attachment` (multipart). Consider the revocation endpoint + a `disconnect` path here.

---

## 8. Testing / e2e

Use the **`e2e-verify`** skill (boots the full server stack with the embedded WASM runner + separate DBs and drives the server HTTP API) as the mandatory pre-commit gate — unit tests alone are insufficient for agent/OAuth/runtime changes (per repo policy). Use **`iterate-capability`** for the ~10s inner loop while developing a single capability, and **`inspect-connection`** to confirm stored params (secrets masked), token state, and expiry after OAuth.

**Prerequisites:** an Intuit **developer sandbox company** and a sandbox Intuit app whose **Redirect URI is registered** as `{public_base_url}/api/oauth/{tenant_id}/callback` (must match `service/oauth.rs:107-111` exactly). Enter that app's `client_id`/`client_secret` into the runtara connection, `environment = "sandbox"`.

**E2E scenario:**
1. Create a `quickbooks_online` connection (client_id/secret/environment).
2. `GET /api/runtime/connections/{id}/oauth/authorize` → follow the Intuit consent → callback. Assert connection `status = ACTIVE`, and that `access_token`, `refresh_token`, `token_expires_at`, **and `realm_id`** are present in `connection_parameters` (via `inspect-connection`).
3. Compile + run a workflow with `quickbooks-query` (`SELECT * FROM Customer`) → assert non-empty `items`.
4. Create→read→sparse-update a sandbox Customer to exercise the SyncToken chain.
5. **Force a token refresh** (e.g. simulate/wait past the 55-min mark or clear the cached access token) and re-run a call → assert success **and** that the stored `refresh_token` changed (proves rotation persistence). This is the highest-value regression test.
6. Verify the proxy allowlist admits all three Intuit hosts (`appcenter.intuit.com`, `oauth.platform.intuit.com`, and the api host `quickbooks.api.intuit.com` / `sandbox-quickbooks.api.intuit.com`).

Use **`trace-instance`** / **`tail-logs`** to debug any failing run.

---

## 9. Risks & open questions

- **Refresh-token rotation + persistence (highest risk — CONFIRMED gap).** Intuit rotates `refresh_token` on **every** refresh and invalidates the old one. Verified: `token_cache.rs` refreshes into an in-memory cache and **discards** the rotated `refresh_token` — it is never written back to `connection_parameters`, and the module has no DB handle to do so. QBO therefore breaks ~1h after the first refresh unless rotation-persistence plumbing is added (see §3(b)). This is the largest single runtime change; do it in Phase 1 with the §8-step-5 regression test.
- **Refresh-token expiry window.** Rolling ~100-day inactivity window, refreshed on use, capped by an absolute **5-year** hard limit (Nov-2025 policy; first expiries ~Oct 2028). Proactively refresh each access-token cycle so connections don't silently die from inactivity; treat the 5-year cap as a hard re-consent boundary the UI should surface.
- **Access-token TTL = 3600s.** Cache with a safety margin (refresh at ~55 min). Confirm the existing `token_cache` refresh-margin logic is aggressive enough.
- **Basic-auth strategy must not regress HubSpot.** The `TokenEndpointAuth` default must remain `FormBody`; only `quickbooks_online` opts into `BasicHeader`. Cover both in tests.
- **Sandbox vs production hosts are different** (`sandbox-quickbooks.api.intuit.com` vs `quickbooks.api.intuit.com`) — driven by the stored `environment` param, resolved in `describe_connection_auth`. Ensure a connection can't silently mix a sandbox token with the prod host.
- **realmId lifecycle.** One connection == one realm. realmId arrives only on the callback query (accounting scope). Re-authorizing must refresh/confirm the stored realmId; a token bound to realm A must never be used against realm B.
- **`minorversion` pinning.** Default 75 but Intuit increments it; omitting it falls back to a 2014-era schema. Keep it a configurable connection param and document the current known-good value; treat the exact newest number as time-sensitive.
- **SyncToken concurrency.** Update/delete require the current `SyncToken`; the agent must read-before-write and handle 5010/stale-token faults by re-reading. Surfacing `syncToken` as a top-level output is what makes the DSL chain ergonomic.
- **Rate limits.** ~40 req/min per realm, 500 req/min per app, and batch ≤30 ops. CDC look-back ~30 days / ≤1000 objects; query `MAXRESULTS` cap 1000. Incremental sync must window queries; batch must chunk. Consider connection-level `rate_limit_config`.
- **Multi-tenant redirect_uri registration.** The Intuit app must whitelist the exact `{public_base_url}/api/oauth/{tenant_id}/callback`. Since redirect_uri is tenant-scoped, confirm the Intuit app allows the set of tenant callback URLs in use (or that a single public_base_url callback pattern is registered). **TO CONFIRM** how per-tenant redirect URIs map to Intuit's allowed-redirect list for a bring-your-own-app model.
- **Content-Type per operation.** The HTTP layer must set `application/json` (CRUD/batch), `application/text` (query POST), `text/plain` (CDC), `multipart/form-data` (upload) — not a blanket JSON header.
- **Exact macro attribute spellings** (`secret`, `optional`, `display_name`, `#[field(skip)]` semantics) — **TO CONFIRM** against `crates/runtara-agent-macro/src/lib.rs:811-1011` and the live HubSpot struct before writing the final `#[connection]`/`#[field]` attributes.
