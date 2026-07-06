# Extending runtara's OAuth 2.0 Mechanism — Implementation Plan

_Status: draft plan (2026-07-06). Grounded in first-hand reads of `runtara-connections` + two adversarial design-review workflows (token caching/rotation; provider-extensibility + central app registry). Code-path and provider facts were verified against source and provider docs; items still open are marked **TO CONFIRM**._

## 1. Where we are

runtara already ships a **working 3-legged OAuth 2.0 authorization-code flow**, entirely inside `runtara-connections`:

- **Authorize** — `GET /api/runtime/connections/{id}/oauth/authorize` (JWT) mints a CSRF `state` (stored in `oauth_state`, 10-min TTL) and returns the provider authorize URL for a popup.
- **Callback** — public `GET /api/oauth/{tenant_id}/callback` consumes `state`, exchanges the `code`, and stores `access_token`/`refresh_token`/`token_expires_at` into the connection's **AES-256-GCM-encrypted** `connection_parameters` (JSONB), flipping status to `ACTIVE`.
- **Use** — access tokens are auto-refreshed at request time via `provider_auth.rs` + `token_cache.rs`.

Three facts define the gap between this and "a real, extensible OAuth platform":

1. **Only HubSpot is wired.** The flow is generic, but the only connection type declaring an interactive `OAuthConfig` is `hubspot_private_app`. Adding a provider today means a new hardcoded arm in `describe_connection_auth`.
2. **Bring-your-own credentials only.** Every connection carries its own `client_id`/`client_secret`; there is **no central app registry**. smo-management (the control plane) has zero OAuth today — though it *had* HubSpot OAuth entities before they were removed in the SYN-437 rewrite (commit `04e1b5b`).
3. **The refresh path was only ever exercised by a non-rotating provider.** It caches access tokens but silently discards rotated refresh tokens — invisible with HubSpot, fatal for QuickBooks/Google.

This plan closes all three, in dependency order: **A) make the flow correct & robust → B) make providers declarative → C) add the central app registry** (the original requirement).

## 2. What backs this plan

- **Discovered gaps** came out of the QuickBooks investigation (`docs/quickbooks-agent-plan.md`) and were each traced to real code.
- **Workstream A** (caching/rotation) was adversarially reviewed — concurrency, regression, and security lenses — and its tenancy assumption (one process per tenant) was verified against `config.rs`/deploy manifests.
- **Workstreams B & C** were designed against the live code plus recovered prior art (the removed smo-management OAuth entities) and a cross-provider auth-quirk survey.

## 3. Gap register

Every gap below is a real, cited code fact — not a hypothetical. The right-hand column points to the workstream that closes it.

| # | Gap | Evidence | Fix / Workstream |
|---|-----|----------|------------------|
| **G1** | Rotated refresh token is **discarded**; refresh happens into an in-memory cache with **no single-flight** and **no DB write-back** | `parse_token_response` drops `body["refresh_token"]` (`token_cache.rs:207-226`); `resolve_cached_token` is lock-free (`token_cache.rs:126-139`); the module has no `ConnectionRepository` handle | **A1** — capture + single-flight + facade persist w/ optimistic-concurrency |
| **G2** | Token-endpoint auth **hardcoded to credentials-in-body**; providers that require HTTP Basic (Intuit) fail | `exchange_code` (`service/oauth.rs:223-244`) & `refresh_oauth_access_token` (`token_cache.rs:167-189`) form-encode creds in the body, no `Authorization` header | **A2 + B** — `TokenEndpointAuth {FormBody\|BasicHeader}` in the provider descriptor |
| **G3** | Provider-specific **callback params not captured** (Intuit returns `realmId`) | `OAuthCallbackQuery` parses only `code/state/error/error_description` (`handler/oauth.rs:111-117`) | **A3 + B** — descriptor lists extra callback params; threaded into `handle_callback` merge |
| **G4** | **No env/path base-URL resolution** — can't vary host by sandbox/prod nor embed a path segment like `/v3/company/{realmId}/` | `describe_connection_auth` returns a static per-arm `base_url` (HubSpot = `https://api.hubapi.com`) | **A4 + B** — base-URL rule (`Static\|EnvSelect\|Template`) in the descriptor |
| **G5** | **Only HubSpot wired**; adding a provider = new hardcoded arm; **no central app registry** (bring-your-own creds only) | single `oauth_config` declaration (`connection_types.rs:692`); creds read from `connection_parameters` | **B** (declarative descriptor) + **C** (app registry) |

**G1 is the highest-severity and highest-effort** (it needs new plumbing across three layers). **G5 is the strategic one** — it's the original requirement, and B is a prerequisite for adding any provider at scale.

## 4. Workstream A — Correctness & robustness

Make the existing flow correct for *any* provider before wiring more of them. Four sub-fixes, sequenced with the load-bearing one first:

- **A1 — Token caching & refresh-token rotation (G1)** — the largest; full design immediately below.
- **A2 — HTTP Basic auth at the token endpoint (G2)**
- **A3 — Extra callback params, e.g. `realmId` (G3)**
- **A4 — Environment & path-templated base URLs (G4)**

A1 is detailed in its own section (it spans `token_cache.rs`, `provider_auth.rs`, `facade.rs`, and the repository); A2–A4 follow it and are individually small — their real payoff is that Workstream B turns each into a **descriptor field** so the next provider inherits them for free.


### A1 · Token caching & refresh-token rotation (G1)

#### 1. How caching works today
Per proxied request the host loads the connection with decrypted params (`internal_proxy.rs:242-278` → `facade.get_with_parameters`) and calls `facade.resolve_connection_auth` (`crates/runtara-connections/src/facade.rs:290`) → `provider_auth::resolve_connection_auth` (`crates/runtara-connections/src/auth/provider_auth.rs:40`), which for the HubSpot/QBO family builds `DeferredAuth::OAuth2RefreshToken{ refresh_token, client_id, client_secret, token_url, fallback_access_token, fallback_expires_at }` (`provider_auth.rs:409-436`) and hands it to `token_cache::resolve_deferred_auth` (`crates/runtara-connections/src/auth/token_cache.rs:44`). Access tokens are cached in a process-global `OnceLock<DashMap<cache_key, CachedAccessToken>>` (`token_cache.rs:42`) keyed on `SHA256("oauth_refresh"|conn_id|integration_id)`; a lock-free fast path serves the DB-stored `access_token` when `token_is_fresh(fallback_expires_at)` (expiry > now+300s, `token_cache.rs:86-97`), otherwise `resolve_cached_token` (`token_cache.rs:126-139`) refreshes via `POST grant_type=refresh_token` and `parse_token_response` extracts `access_token` + `expires_in` **only** (`token_cache.rs:193-227`).

#### 2. Why rotation breaks it
`parse_token_response` **discards** `body["refresh_token"]`; the DB keeps the original refresh token forever. HubSpot (`hubspot_private_app`, the sole current caller — `provider_auth.rs:127`) has stable, reusable refresh tokens, so this is harmless. QuickBooks/Intuit **rotates and invalidates the old refresh token on every refresh**. After the first in-memory refresh the DB refresh token is dead; the next cold start, cache eviction, or second process reads a dead token → permanent auth break with no error at the point of failure. Persisting the rotated refresh token is mandatory before QBO can be the second caller.

#### 3. The fix — concrete change list

**`token_cache.rs` — capture + single-flight (revised per concurrency review).**
- `parse_token_response` also reads `body["refresh_token"]`; the OAuth2-refresh arm carries `Option<refresh_token>` + computed `token_expires_at` out to the persistence layer. It is **not** stored in the DashMap value (`CachedAccessToken` stays `{access_token, expires_at}`).
- **Scope the capture to the `OAuth2RefreshToken` branch only.** `parse_token_response` is shared with `exchange_client_credentials_token` (`token_cache.rs:141-165`); the client-credentials arm must structurally return no rotated credential (thread rotation through a separate return path / distinct type, not a shared nullable field) so a `RotatedCredentials` can never surface on the client-credentials return.
- Add per-`cache_key` single-flight. **The mutex is acquired *inside* `resolve_cached_token`, below the lock-free fast read**, and the post-lock double-check re-reads the *DashMap* via a `get_fresh_cached_token(cache_key)` — **not** the per-request `fallback_access_token`. The fast path at `token_cache.rs:86-97` stays lock-free but must remain a *pure read* (cache-insert + return) and must never issue a network refresh; every code path that can perform an HTTP refresh must funnel through the locked section. *(Reviewer fix — the fast-path fallback was an unlocked refresh trigger that bypassed the lock on cold start / near-expiry.)*
- Build the per-key mutex atomically: `lock_map.entry(cache_key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()`, **clone the `Arc` out and drop the DashMap shard guard before `.await`ing the mutex** (never hold a sync shard lock across an await). Lock-map growth is bounded by the tenant's connection count (per-tenant process) — document the bounded leak or evict on refcount==1.
- **Negative caching on failure.** On a failed refresh, record a short-lived `last_refresh_error` (few-second cooldown) checked at the top of the locked section, so waiters that acquire the lock right after a failure inherit the error instead of each re-hammering the provider under a persistent outage. *(Reviewer fix — serialized-but-repeated refresh storm.)*
- **Redact the token-endpoint body from surfaced errors.** `parse_token_response` currently returns `format!("Token endpoint returned {}: {}", status, body)` (`token_cache.rs:203-204`), which propagates through `facade` → `internal_proxy.rs:276` into the proxy response handed to the WASM agent. Log the full body host-side at debug; return only the status plus whitelisted OAuth `error`/`error_description`. Add a regression test asserting no `access_token`/`refresh_token` substring can appear in the `Err` returned by `resolve_deferred_auth`. *(Reviewer fix — token material leak across the WASM boundary.)*

**`provider_auth.rs` — bubble-up.**
- `resolve_connection_auth` returns an optional `RotatedCredentials{ access_token, refresh_token: Option<String>, token_expires_at }`, set **only when a refresh actually occurred** (never on the fast-path fallback), so no DB write happens on ordinary proxied requests.
- `RotatedCredentials` (and any struct carrying `access_token`/`refresh_token`) gets a **manual redacting `Debug` impl** (or a `Secret<String>` newtype) so a stray `{:?}`/`?field` in tracing can't write the token to logs → Loki. `token_cache`/`provider_auth` stay repo-free.

**`facade.rs` — persist with a new repo method + optimistic-concurrency guard.**
- `ConnectionsFacade::resolve_connection_auth` gains `tenant_id` (both real callers already have it — `internal_proxy.rs:223`, `internal_presign.rs:46`). When `RotatedCredentials` is present it **re-reads** the connection params inside the facade write, applies the optimistic-concurrency guard, merges the three token keys into the *freshly re-read* blob (preserving all other keys and never forcing status), seals, and writes via a **new** `ConnectionRepository::update_parameters(id, tenant_id, sealed_params)` that updates `connection_parameters` only — status untouched (mirrors `reencrypt_all`'s write pattern, unlike `update_parameters_and_status` at `repository/connections.rs:954` which forces status).
- **Do not blind read-modify-write the full T0 snapshot.** The concurrent writer is real: `oauth.rs:157-209` (`complete_oauth`) rewrites the entire sealed blob with a new `client_secret` + `status=ACTIVE`. Because the blob is encrypted, no server-side JSONB partial merge is possible; the guard is: re-read params in the facade write, **verify the DB refresh token still equals the one refreshed-from** (compare against a plaintext, non-secret `refresh_token` fingerprint/generation column — see §4), only then seal+replace; on mismatch, adopt the winner's now-persisted token rather than overwriting. *(Reviewer fix — last-writer-wins clobber of a concurrent re-auth.)*
- **Persist-before-serve, and fail closed for rotating providers.** The write is awaited before the Bearer is served. Add a per-integration `rotates_refresh_token` flag (`true` for QBO, `false` for HubSpot). If rotation actually occurred and the persist fails: for a **rotating** provider, return `Err` from `resolve_connection_auth` (fail the proxy request) **and do not populate/keep the DashMap entry** for the new access token, so the next attempt re-refreshes from the still-current DB token instead of silently diverging. For **non-rotating**/client-credentials, an idempotent same-value write failure is logged-and-tolerated. Give the DB persist its own tighter timeout so a stuck write can't pin the token path for the full pool timeout. *(Reviewer fix — persist-after-serve time bomb.)*
- **Presign caller is a typed no-op, not a comment.** `internal_presign.rs` (S3/AWS) never rotates; make the facade signature *force* the caller to handle the returned `RotatedCredentials` (e.g. it is `None` by construction on that path, or a `debug_assert!(rotated.is_none())`), so a future OAuth-refresh integration routed through presign can't silently drop a rotated token.
- **Proactive cache invalidation.** Expose `token_cache::invalidate(cache_key)` and call it from both `update_parameters_and_status` (re-auth via `oauth.rs`) and the new `update_parameters`, so a re-auth or out-of-band rotation takes effect immediately instead of at next expiry.

**Warm cold-start (§4 of original, retained).** Persisting `access_token` + `token_expires_at` on every refresh keeps the DB fallback fresh, so a restarted process serves the stored access token via the fast path without an extra refresh — shrinking refresh frequency and the rotation-race window. **Self-healing fast path:** on a `401`/`invalid_token` from the upstream for an `OAuth2RefreshToken` connection, invalidate the DashMap entry **and** bypass the DB fallback for one cycle, forcing a real refresh under the single-flight lock, so a stale-but-not-yet-expired fallback recovers instead of silently returning a dead Bearer. *(Reviewer fix — timestamp-only trust of the fallback.)*

#### 4. Concurrency & multi-process
**Verified cardinality (CONFIRMED):** runtara is **one server process per tenant** — `TENANT_ID` is required at startup and resolved once into a process-wide `OnceLock<Config>` (`crates/runtara-server/src/config.rs`; `docs/entitlements.md`, `docs/deployment/auth-modes.md`); `docker-compose.yml` defines a single `runtara` service with no replicas; no k8s/LB manifests exist. Under that invariant the process-global `TOKEN_CACHE` is per-tenant and **in-process single-flight is sufficient** for correctness — no Valkey/Postgres advisory lock is required in steady state.

**But the invariant is operational, not code-enforced, and a rolling/blue-green deploy transiently runs two processes against the same tenant DB.** Two processes read the same DB refresh token, both rotate, Intuit kills the first, last-writer-wins persists one and orphans the other → the exact QBO break, reintroduced at the process layer. **Recommended mitigation (ship this, do not rely on the single-process assumption for a rotating provider): optimistic concurrency at the persist step.** Store a plaintext, non-secret `refresh_token` fingerprint (hash) or a monotonic `rotation_generation` in its own column (the sealed blob can't be compared without decrypting); `update_parameters` does a conditional write (`WHERE refresh_token_hash = $expected` / generation check). On mismatch the loser re-reads the winner's freshly persisted token and serves it without a second rotation. This makes correctness self-enforcing and only engages on the rare cross-process race. Additionally emit a metric / startup guard when >1 process is observed holding the same tenant DB so deploy-overlap is observable.

#### 5. Regression & security guarantees
- **HubSpot unaffected.** Persisting a same-valued (stable) refresh token is a harmless no-op write; `rotates_refresh_token=false` means a benign write failure is tolerated, not fail-closed. No behavior change for `hubspot_private_app`.
- **Client-credentials path untouched.** Rotation capture is scoped to the `OAuth2RefreshToken` arm; the client-credentials arm (Shopify, Microsoft Entra — `provider_auth.rs:100-113`) structurally cannot produce `RotatedCredentials`.
- **Credentials never logged, evented, or crossing the WASM boundary.** Tokens are resolved host-side and injected into the *outbound* `reqwest` headers only; the `ProxyResponse` returned to the guest carries upstream status/headers/body, never the injected `Authorization` (`internal_proxy.rs:577-585`). The `ConnectionLifecycleEvent::TokenRefreshed` event carries only `{connection_id, integration, success}` (`events.rs:33-39`) — no token material, and rotation persistence goes through the facade, not the event stream. Redacting `Debug` impls + the token-endpoint-body redaction (§3) close the two remaining leak paths.
- **Sealed writes.** `update_parameters` seals via the same `CredentialCipher` (`seal()` → `cipher.encrypt`, `repository/connections.rs:39-52`) used everywhere else; the decrypt → mutate → seal → write round-trip is proven safe by the existing idempotent `reencrypt_all` (`connections.rs:1020-1061`). Connection params remain encrypted at rest and excluded from all API responses (`types.rs:~20`, `handler/connections.rs:97,192,381`).

#### 6. Residual risks / open questions
- **`rotates_refresh_token` flag placement** — decide where the per-integration flag lives (connection-type metadata vs. a hardcoded provider list) so QBO=true / HubSpot=false is declarative. **TO CONFIRM.**
- **Plaintext fingerprint column migration** — the optimistic-concurrency guard needs a new non-encrypted `refresh_token_hash`/`rotation_generation` column on the connections table (migration + backfill = hash of the current DB refresh token). Confirm the exact table/column name against `crates/runtara-connections/src/types.rs:385-386`. **TO CONFIRM.**
- **Intuit refresh-token grace window** — the optimistic guard removes reliance on it, but confirm Intuit's actual old-token grace period to size the negative-cache cooldown and the deploy-overlap risk window. **UNCERTAIN (probe did not verify Intuit's live grace behavior — RFC 6749 §6 cited, not Intuit docs).**
- **DB-persist timeout value** — pick a concrete tighter timeout for the awaited persist distinct from the pool timeout; needs a value from load testing. **TO CONFIRM.**
- **Cache-cold storm tail latency** — after a deploy, many connections may refresh near-simultaneously, each now doing a synchronous seal+write (+ re-read) on the hot path; acceptable for the rotating case (must be awaited), but consider best-effort/async persistence for the non-rotating cold-start optimization to keep it off the request path (mirror `record_credential_request_async`, `internal_proxy.rs:282`).
- **Multi-replica advisory lock (deferred)** — if a tenant is ever intentionally run multi-replica (not just deploy overlap), a short per-connection Redis/`pg_advisory_xact_lock` around the refresh (the facade already wires a `redis_manager`) is the stronger guarantee; optimistic concurrency is the chosen baseline, this is the escalation path.


### A2 · HTTP Basic auth at the token endpoint (G2)

Two call sites currently form-encode `client_id`/`client_secret` in the **body** with no `Authorization` header: `exchange_code` (`service/oauth.rs:223-244`, initial code→token) and `refresh_oauth_access_token` (`token_cache.rs:167-189`, refresh). Intuit **requires** `Authorization: Basic base64(client_id:client_secret)` with `Content-Type: application/x-www-form-urlencoded`.

**Change:** introduce `TokenEndpointAuth { FormBody, BasicHeader }` (default `FormBody`), resolved per integration from the provider descriptor (Workstream B). When `BasicHeader`: set the Basic `Authorization` header, keep the form content-type, and **drop the creds from the body** (body = `grant_type` + `code`+`redirect_uri` | `refresh_token`). The base64 machinery already exists (Mailgun arm, `provider_auth.rs:171-172`). Apply to **both** exchange and refresh; the strategy rides on `DeferredAuth::OAuth2RefreshToken` (or is derived from `integration_id`) so `token_cache` knows the mode. HubSpot stays `FormBody` → zero behavior change; QuickBooks = `BasicHeader`.

### A3 · Extra callback params — e.g. `realmId` (G3)

`OAuthCallbackQuery` (`handler/oauth.rs:111-117`) parses only `code/state/error/error_description`. Intuit appends `realmId` (the company id) directly to the callback URL, and it is **not** in the token JSON — so without capture there is no way to build a QBO request path.

**Change:** the provider descriptor declares `extra_callback_params: &[&str]` (e.g. `["realmId"]`). The callback handler reads those off the query string and passes them to `handle_callback`, which merges them into `connection_parameters` in the **same sealed write** as the tokens (`service/oauth.rs:182-209`). `realmId` arrives on the callback directly, so no `oauth_state` round-trip and no schema change. A user-editable `realm_id` field remains as a manual fallback.

### A4 · Environment & path-templated base URLs (G4)

`describe_connection_auth` returns a static `base_url` per hardcoded arm today (HubSpot = `https://api.hubapi.com`). The proxy **pins** the request URL to that `base_url` (`internal_proxy.rs:310`) and there is **no per-request path templating**, so a path segment like `/v3/company/{realmId}/` must be folded into `base_url` itself.

**Change:** the descriptor carries a base-URL rule — `Static(url)` | `EnvSelect{ param, sandbox_host, prod_host }` | `Template("{host}/v3/company/{realm_id}")` with substitution from stored/captured params. `describe_connection_auth` becomes data-driven: read the rule, substitute `environment` + captured params, return `base_url`. QBO = `EnvSelect(environment → sandbox-quickbooks.api.intuit.com | quickbooks.api.intuit.com)` then template-append `/v3/company/{realm_id}`; HubSpot = `Static(https://api.hubapi.com)`, unchanged. One-connection-one-realm is the model; multi-realm-per-connection would require proxy path templating (out of scope).

## 5. Workstream B — Provider extensibility framework

Adding an OAuth provider today means editing three places by hand: the `#[connection(...)]` macro in `crates/runtara-agents/src/agents/extractors/connection_types.rs`, the `CONNECTION_TYPES` slice in `crates/runtara-agents/src/static_registry.rs`, and — the real bottleneck — a bespoke match arm in `crates/runtara-connections/src/auth/provider_auth.rs::describe_connection_auth` (20+ arms already; only `hubspot_private_app` at line 127 is wired for interactive OAuth). Every per-provider quirk (Basic vs. body token auth, rotating refresh tokens, `realmId`-style callback params, sandbox/prod hosts, templated base URLs) currently has no home in the descriptor, so it leaks into hand-written code.

The framework replaces the arm explosion with a **declarative descriptor** carried on `ConnectionTypeMeta`, populated from macro attributes, and consumed data-driven by the three OAuth entry points.

### The descriptor (Rust shape)

Add to `crates/runtara-dsl/src/agent_meta.rs` (after the existing `OAuthConfig` at line 874, which stays for backward compat):

```rust
/// How client credentials reach the token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenEndpointAuthStyle {
    FormBodyCredentials,   // grant_type=…&client_id=…&client_secret=… in the POST body
    HttpBasicAuth,         // Authorization: Basic base64(client_id:client_secret)
    PkceOnly,              // code_verifier in body, no client_secret
}

/// How the API base URL is resolved at request time.
#[derive(Debug, Clone)]
pub enum BaseUrlResolution {
    Static(&'static str),
    EnvironmentSelect { sandbox_url: &'static str, prod_url: &'static str, env_param_name: &'static str },
    Template { pattern: &'static str, param_names: &'static [&'static str] }, // e.g. /v3/company/{realmId}/
}

/// A callback query param to capture beyond code/state/error.
#[derive(Debug, Clone)]
pub struct ExtraCallbackParam {
    pub query_name: &'static str,  // as returned by provider, e.g. "realmId"
    pub param_name: &'static str,  // key stored under, e.g. "realm_id"
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderOAuthDescriptor {
    // Core endpoints
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub default_scopes: &'static str,
    pub scope_delimiter: char,                       // ' ' default; some providers differ

    // Token endpoint + refresh behavior
    pub token_endpoint_auth_style: TokenEndpointAuthStyle,
    pub refresh_token_rotates: bool,                 // drives rotation persistence (see below)
    pub reauth_on_error_codes: &'static [&'static str], // e.g. ["invalid_grant"] -> NEEDS_REAUTH

    // Callback + base URL
    pub extra_callback_params: &'static [ExtraCallbackParam],
    pub base_url_resolution: BaseUrlResolution,

    // Revocation on disconnect
    pub revocation_endpoint: Option<&'static str>,
    pub revocation_http_method: &'static str,        // "POST" | "GET" | "DELETE"

    // PKCE
    pub pkce_required: bool,
}
```

`ConnectionTypeMeta` (agent_meta.rs:901) gains one optional field alongside the legacy `oauth_config`:

```rust
pub struct ConnectionTypeMeta {
    // … existing fields …
    pub oauth_config: Option<&'static OAuthConfig>,               // legacy, retained
    pub oauth_descriptor: Option<&'static ProviderOAuthDescriptor>, // new
}
```

### Macro + registry wiring

Extend the `#[connection(...)]` attribute set in `crates/runtara-agent-macro/src/lib.rs` (the `ConnectionContainerArgs` struct near line 758, code-gen near line 787–1005). New `#[darling(default)]` attributes map 1:1 onto descriptor fields:

- `oauth_token_endpoint_auth` — `"form_body"` | `"http_basic"` | `"pkce"`
- `oauth_refresh_token_rotates` — bool
- `oauth_reauth_on_error_codes` — comma-separated (`"invalid_grant,expired_token"`)
- `oauth_extra_callback_params` — `param_name:query_name:required` list (`"realm_id:realmId:true"`)
- `oauth_base_url_resolution` — `"static:URL"` | `"env_select:sandbox:prod:env_param"` | `"template:pattern:p1,p2"`
- `oauth_scope_delimiter`, `oauth_revocation_endpoint`, `oauth_revocation_http_method`, `oauth_pkce_required`

The macro emits a `static OAUTH_DESC: ProviderOAuthDescriptor = …` and stores `&OAUTH_DESC` into `oauth_descriptor`. Registration remains the existing `CONNECTION_TYPES` slice in `static_registry.rs` — no new registration mechanism, just a richer `ConnectionTypeMeta` per entry. **Net effect: a new provider is one attribute block, zero procedural code.**

### The three entry points become data-driven

**`generate_authorization_url`** (`crates/runtara-connections/src/service/oauth.rs:67–136`): resolve `auth_url`, scopes (joined with `descriptor.scope_delimiter`), and — when `descriptor.pkce_required` — generate a `code_challenge=S256` and persist the verifier on the `oauth_state` row. URLs come from the descriptor, no hardcoded per-provider branch.

**`exchange_code`** (`oauth.rs:223–267`): dispatch on `descriptor.token_endpoint_auth_style`. For `HttpBasicAuth`, put `base64(client_id:client_secret)` in the `Authorization` header and keep credentials *out* of the body; for `FormBodyCredentials`, keep the current body form; for `PkceOnly`, send `code_verifier` and omit the secret.

**`describe_connection_auth` / base URL** (`crates/runtara-connections/src/auth/provider_auth.rs:69–328`): replace the 20+ arms with a single `resolve_base_url(&descriptor.base_url_resolution, params)`:

```rust
fn resolve_base_url(res: &BaseUrlResolution, params: &Value) -> Result<Option<String>, String> {
    match res {
        BaseUrlResolution::Static(u) => Ok(Some(u.to_string())),
        BaseUrlResolution::EnvironmentSelect { sandbox_url, prod_url, env_param_name } => {
            let sandbox = params[env_param_name].as_bool().unwrap_or(false);
            Ok(Some(if sandbox { sandbox_url } else { prod_url }.to_string()))
        }
        BaseUrlResolution::Template { pattern, param_names } => {
            let mut out = pattern.to_string();
            for p in *param_names {
                let v = params[p].as_str().ok_or_else(|| format!("missing template param: {p}"))?;
                out = out.replace(&format!("{{{p}}}"), v);
            }
            Ok(Some(out))
        }
    }
}
```

**Callback capture** (`crates/runtara-connections/src/handler/oauth.rs:111–212`): widen `OAuthCallbackQuery` with `#[serde(flatten)] extra_params: serde_json::Map<String, Value>`, and in `handle_callback` iterate `descriptor.extra_callback_params`, storing each present value into `connection_parameters` (erroring if `required` and absent). This is how Intuit's `realmId` gets persisted alongside the tokens.

**Rotation flag → persistence** (`crates/runtara-connections/src/auth/token_cache.rs:167–191`): the descriptor's `refresh_token_rotates` flag gates the rotated-refresh-token write-back. The concrete persistence + single-flight design is covered in the rotation-persistence section — here the descriptor simply supplies the per-provider switch that tells that machinery when to run.

**Needs-reauth** : when a refresh returns a `descriptor.reauth_on_error_codes` code (typically `invalid_grant`), flip the connection to `NEEDS_REAUTH` instead of retrying forever (see the best-practices section).

**Revocation-on-disconnect** : when `descriptor.revocation_endpoint` is set, the disconnect handler calls it before deleting the local row.

### Filled-in descriptor table

| Field | HubSpot | QuickBooks / Intuit | Google | Salesforce |
|---|---|---|---|---|
| `auth_url` | `app.hubspot.com/oauth/authorize` | `appcenter.intuit.com/connect/oauth2` | `accounts.google.com/o/oauth2/v2/auth` | `login.salesforce.com/services/oauth2/authorize` |
| `token_url` | `api.hubapi.com/oauth/v1/token` | `oauth.platform.intuit.com/oauth2/v1/tokens/bearer` | `oauth2.googleapis.com/token` | `login.salesforce.com/services/oauth2/token` |
| `token_endpoint_auth_style` | `FormBodyCredentials` | `HttpBasicAuth` | `FormBodyCredentials` | `FormBodyCredentials` |
| `refresh_token_rotates` | `false` | `true` (~24h) | `false` | `false` (policy-configurable) |
| `reauth_on_error_codes` | `["invalid_grant"]` | `["invalid_grant","expired_token"]` | `["invalid_grant"]` | `["invalid_grant"]` |
| `extra_callback_params` | — | `realm_id:realmId:true` | — | `instance_url:instance_url:true` |
| `base_url_resolution` | `Static("api.hubapi.com")` | `EnvironmentSelect{sandbox,prod,"sandbox_mode"}` | `Static("www.googleapis.com")` | `Template{"{instance_url}/services/data","instance_url"}` |
| `scope_delimiter` | `' '` | `' '` | `' '` | `' '` |
| `pkce_required` | `false` | recommended | recommended | `true` (ISV apps) |
| `revocation_endpoint` | `DELETE api.hubapi.com/oauth/v1/refresh-tokens/{token}` | `developer.api.intuit.com/v2/oauth2/tokens/revoke` | `oauth2.googleapis.com/revoke` | `/services/oauth2/revoke` |

## 6. Workstream C — Central OAuth app registry (the initial requirement)

**The ask:** replace per-connection bring-your-own credentials — where every tenant pastes their own `client_id`/`client_secret` — with a central list of platform-registered OAuth apps that kickstart the interactive flow and let runtara capture the refresh token.

### Design decision: A / B / C

- **Option A — Platform-managed shared apps:** admins register one OAuth app per provider centrally; all tenants authorize through it. One registration serves N tenants.
- **Option B — Bring-your-own (today):** each tenant supplies credentials per connection. High friction, duplicate apps, N×M registrations.
- **Option C — Hybrid:** conditional logic selecting A or B per connection.

**Recommendation: Option A, with B retained as an additive fallback.** A is the enterprise pattern the requirement describes and scales as one registration per provider rather than N tenants × M providers. C's conditional benefit is marginal — and it falls out for free anyway, because A is implemented as *fallback-if-absent*: a connection with no registry reference keeps working on its own pasted credentials. So the recommendation is "A as the default path, B as the legacy escape hatch," which is C's benefit without C's branching cost.

### Where the registry lives: smo-management (not runtara admin)

**Recommendation: the authoritative registry lives in smo-management.** Rationale:

- **Scope alignment.** Provider app credentials are platform infrastructure metadata, shared 1:many. smo-management is the single shared control plane (tenants, invitations, roles via Auth0 Orgs, audit); runtara is replicated per-tenant (per-tenant VM/process + per-tenant object DB). A platform-shared secret does not belong in a per-tenant process.
- **Prior art proves the fit.** smo-management *already had* HubSpot OAuth entities — the `integration_entity` table (`id, connection_type, description, icon_url, category, enabled, details, supported_operators`), migration `20251112000000_create_integration_entity_table.sql`, and `GET /api/management/integration-entities` — removed in the SYN-437 rewrite (commit `04e1b5b`, "feat: add HubSpot integration entities (OAuth2 + access token)"). SYN-437 stripped smo-management to identity/invitations/audit and pushed integration *metadata* into runtara's static registry. The registry we're adding back is narrower and correctly scoped: **app credentials**, not integration metadata. The model demonstrably fits the codebase (Rust/axum/sqlx, JWT-gated, `audit_events`).
- **Caller auth + audit for free.** smo-management verifies callers via Auth0 JWT offline against cached JWKS and can role-gate to platform-admin; it has `audit_events` for credential lifecycle (register/rotate/revoke). runtara's per-tenant API-key model would have to reinvent both.
- **Secret management precedent.** smo-management already custodies Auth0 org_ids and per-tenant Valkey credentials.

runtara holds a **read-only, encrypted replica** synced at deployment, so a single runtara binary serves all tenants without a runtime round-trip to smo-management on every authorize.

### Data model

**smo-management (control plane, authoritative)** — new `oauth_app_registry`:

```sql
CREATE TABLE oauth_app_registry (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id       TEXT NOT NULL,               -- "hubspot_private_app", "quickbooks_oauth", …
    provider_name     TEXT NOT NULL,               -- display name
    app_id            TEXT NOT NULL,               -- platform's app name (tracking)
    client_id         TEXT NOT NULL,
    client_secret     TEXT NOT NULL,               -- DB encryption at rest
    sandbox_mode      BOOLEAN NOT NULL DEFAULT FALSE,
    auth_url_override  TEXT,                        -- NULL -> descriptor default
    token_url_override TEXT,
    token_auth_style  TEXT NOT NULL DEFAULT 'body', -- 'body' | 'basic'
    default_scopes    TEXT,
    redirect_uris     TEXT NOT NULL,               -- JSON array
    tenant_allowlist  TEXT,                         -- JSON array; NULL = all tenants
    enabled           BOOLEAN NOT NULL DEFAULT TRUE,
    created_by        TEXT, updated_by TEXT, notes TEXT,
    rotated_at        TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_id, app_id, sandbox_mode)
);
```

Endpoints (in `smo-management/src/lib.rs`, router near line 107): `POST/GET/PATCH /api/management/oauth-apps` (JWT + platform-admin role, emitting `audit_events`), and internal `GET /api/management/internal/oauth-apps/{provider_id}` (token-gated) for the deployment sync.

**runtara (per-tenant runtime, read-only replica)** — mirror table synced at deployment, with `client_secret` re-encrypted using the existing `CredentialCipher` (AES-256-GCM), plus sync metadata (`synced_at`, `synced_from_smo_version`). It is immutable at runtime; tenant processes only read + decrypt on demand.

### How a connection references a registered app

Add a nullable FK to the connection row (`crates/runtara-server/migrations/…` on `connection_data_entity`):

```sql
ALTER TABLE connection_data_entity
  ADD COLUMN registry_app_id UUID REFERENCES oauth_app_registry(id);
```

- `registry_app_id` set → platform-managed: creds sourced from the replica; `connection_parameters` holds only tokens + captured provider metadata.
- `registry_app_id` NULL → legacy bring-your-own: creds read from `connection_parameters` as today.

Post-consent `connection_parameters` for a registry-backed connection:

```json
{
  "access_token": "…",
  "refresh_token": "…",              // may rotate (persisted by rotation fix)
  "token_expires_at": "2026-07-07T14:30:00Z",
  "provider_metadata": { "realm_id": "1234567890" }
}
```

### Authorize / callback sourcing creds from the registry

In `generate_authorization_url` (`service/oauth.rs:67–136`), resolve credentials by branch:

```rust
let (client_id, client_secret, auth_url, token_url) = match conn.registry_app_id {
    Some(app_id) => {
        let app = self.oauth_repo.get_registry_app(&app_id).await?
            .ok_or(OAuthError::RegistryAppNotFound)?;
        // overrides > connection-type descriptor default
        (app.client_id, app.client_secret,
         app.auth_url_override.unwrap_or_else(|| descriptor.auth_url.into()),
         app.token_url_override.unwrap_or_else(|| descriptor.token_url.into()))
    }
    None => { /* legacy: read client_id/client_secret from connection params */ }
};
```

`exchange_code` and `token_cache::refresh_oauth_access_token` receive the same resolved credentials, so the registry and bring-your-own paths converge on one exchange/refresh code path. New repository methods (`crates/runtara-connections/src/repository/oauth.rs`): `get_registry_app(id)` and `get_registry_app_by_provider(provider_id, sandbox_mode)` returning a `RegistryAppRow`.

### The shared-app redirect_uri problem

With one shared app, the provider console holds **one** redirect URI, but runtara's callback path is per-tenant (`/api/oauth/{tenant_id}/callback`). Providers like Google forbid wildcard redirect URIs, so per-tenant paths cannot all be pre-registered.

**Recommended solution: one fixed callback path, tenant carried in `state`, not in the path.** Register a single `https://platform.example.com/api/oauth/callback` per provider. The tenant is already bound in the CSRF `state` row (`oauth_state.tenant_id` + `connection_id`), so isolation is preserved without the path segment: the callback validates and single-use-consumes the state, reads the tenant/connection from that row, and exchanges the code against that tenant's DB. This keeps exact-match at token time (the same fixed `redirect_uri` is echoed at exchange) and eliminates per-tenant console registration. Forgery is defeated by the existing atomic `get_and_delete_state`.

### Secret storage

- **smo-management:** `client_secret` protected by DB encryption at rest; never transmitted to public networks; served only over TLS to the token-gated internal sync endpoint.
- **runtara replica:** `client_secret` encrypted with `CredentialCipher` (AES-256-GCM, `crates/runtara-connections/src/crypto/aes_gcm.rs`) before INSERT, decrypted on demand during authorize/exchange/refresh. The cipher **already fails closed when encryption is required**: `cipher_from_env` hard-errors at boot on a missing/invalid key when `RUNTARA_ENV=production` or `REQUIRE_ENCRYPTION` is set (`crypto/factory.rs:57,76,109`). The NoOp plaintext fallback (`factory.rs:119`) engages only when encryption is **not** required (local/dev), and it logs a prominent error — it is not silent. Residual risk is operational, not a code bug: a production deploy that forgets `RUNTARA_ENV=production`/`REQUIRE_ENCRYPTION` would store tokens in plaintext (see best-practices).
- **Tokens never cross the WASM boundary:** creds resolve host-side in runtara-connections and are injected by the proxy; agents carry only an opaque `connection_id`.

### Multi-tenant isolation

Registry rows are platform-wide metadata, not tenant data. Isolation holds because: (1) the replica is read-only, synced at deploy, no runtime cross-tenant mutation; (2) each tenant is a separate process + DB; (3) the `state` row atomically binds a code exchange to one `tenant_id`+`connection_id`; (4) optional `tenant_allowlist` can scope a provider app to specific tenants when a provider requires it.

### End-to-end flow

1. **Admin registers app.** A platform-admin POSTs the QuickBooks app (`client_id`, `client_secret`, `sandbox_mode`, scopes, the single shared redirect URI) to `POST /api/management/oauth-apps` in smo-management; an `audit_events` row records who/when. Deployment sync pushes an encrypted replica row into each tenant's runtara DB.
2. **Tenant picks provider.** In the connection UI the tenant selects "QuickBooks"; because a registry app exists for that provider, the client_id/secret fields are skipped and the new connection is created with `registry_app_id` set — no credential entry.
3. **Interactive consent.** Tenant hits `GET /api/runtime/connections/{id}/oauth/authorize`; runtara resolves creds from the replica, builds the auth URL (PKCE challenge if `pkce_required`), stores the CSRF `state` (with `tenant_id`, `connection_id`, verifier), and redirects to Intuit. The user consents; Intuit redirects to the single fixed callback with `code`, `state`, and `realmId`.
4. **runtara captures + persists.** The callback validates/consumes `state`, exchanges the code (HTTP Basic per the QuickBooks descriptor), stores `access_token`/`refresh_token`/`token_expires_at` plus `provider_metadata.realm_id` into encrypted `connection_parameters`. On every subsequent runtime refresh, because the descriptor's `refresh_token_rotates` is true, the rotated refresh token is persisted back (rotation-persistence section), so the connection survives Intuit's ~24h rotation instead of dying on the next `invalid_grant`.

## 7. OAuth 2.0 best-practices adoption

Verified against `crates/runtara-connections/src/service/oauth.rs`, `.../auth/token_cache.rs`, `.../auth/provider_auth.rs`, `.../crypto/aes_gcm.rs`, `.../crypto/factory.rs`, and the `OAuthConfig` descriptor at `crates/runtara-dsl/src/agent_meta.rs:874`.

| Practice | Have today | Add |
|---|---|---|
| **state / CSRF** | 32-byte CSPRNG hex, server-persisted, atomically single-use via `get_and_delete_state` (oauth.rs:216, :142), tenant-scoped | — (solid) |
| **Secret encryption at rest** | AES-256-GCM, random nonce per encrypt (`crypto/aes_gcm.rs`); **already hard-errors at boot** when the key is missing/invalid and `RUNTARA_ENV=production`/`REQUIRE_ENCRYPTION` is set (`crypto/factory.rs:57,76,109`) | **Verify/enforce the enforcement flag** — the NoOp plaintext fallback (`factory.rs:119`, loudly logged) engages only when encryption isn't required; confirm prod actually sets the flag, and consider making encryption default-required so a forgotten env var can't degrade to plaintext |
| **Host-side token isolation** | Creds resolved host-side, injected by proxy; WASM boundary carries only opaque `connection_id` | — (solid) |
| **Exact redirect_uri at exchange** | Deterministic redirect_uri echoed identically at token time (`state_row.redirect_uri`, oauth.rs:178) | — for the match itself; but **redirect_uri strategy** below |
| **PKCE (RFC 7636)** | Missing — auth URL emits only client_id/redirect_uri/scope/state (oauth.rs:126–133); no verifier stored, none sent at exchange | Add `code_challenge`=`S256` + `method`, persist verifier on the `state` row, send `code_verifier` in `exchange_code`. Required by Salesforce ISV, first-class for Xero, recommended by Google/Microsoft. Gated by descriptor `pkce_required` |
| **Refresh-token rotation persistence** | Broken for rotating providers — runtime refresh (`token_cache.rs:167`) parses only `access_token`+`expires_in` into an in-memory DashMap; the module has no `ConnectionRepository` and never writes a rotated `refresh_token` back. First exchange persists correctly; recurring refresh is the defect | Persist rotated refresh tokens on every runtime refresh (detailed in the rotation-persistence section; gated by descriptor `refresh_token_rotates`) |
| **needs-reauth error taxonomy** | Missing — both exchange and refresh collapse all failures into one string (`TokenExchangeFailed`, `"Token endpoint returned {}"`); `invalid_grant` is indistinguishable from transient 5xx, so a dead connection is retried forever | Parse the standard `error` field; map `invalid_grant`/`invalid_token` to a `NEEDS_REAUTH` connection status. Driven by descriptor `reauth_on_error_codes` |
| **Token revocation on disconnect** | Missing — `delete_connection_handler` only deletes the local row; no provider-side revoke | Add `revocation_endpoint`/method to the descriptor and call it on disconnect (Intuit, Google, Salesforce, Xero, HubSpot all expose one) |
| **redirect_uri strategy** | Per-tenant path `/api/oauth/{tenant_id}/callback` forces every tenant callback to be pre-registered per provider; Google forbids wildcards | Move to one fixed callback path; carry tenant in `state`, not the path (see registry section) |
| **Refresh-token expiry / inactivity + re-consent UX** | Missing — no stored refresh-token expiry, no inactivity tracking, no re-consent trigger | Track per-connection refresh-token expiry; surface a "reconnect" state before it lapses. Windows differ sharply (Xero 60-day inactivity; Microsoft ~90-day, 24h for SPA-registered redirect URIs; Intuit 5-year absolute + ~24h rotation; HubSpot/Google effectively non-expiring unless revoked) |
| **Least-privilege / incremental scopes** | Partial — single `default_scopes` per connection type, overridable per connection; no step-up consent | Optional later: per-capability incremental consent (Google supports `include_granted_scopes`) |

**Priority order:** (1) persist rotated refresh tokens on the runtime refresh path; (2) `invalid_grant`→`NEEDS_REAUTH` taxonomy; (3) PKCE; (4) revoke-on-disconnect; (5) refresh-token expiry/inactivity tracking + reconnect UX; (6) fail-closed at-rest encryption.

## 8. Cross-provider auth-quirk matrix

Confidence HIGH on endpoints / PKCE / rotation (official docs); expiry policies are provider-stated and can drift — re-verify at integration time. This is the evidence base validating each descriptor field.

| Provider | Auth endpoint | Token endpoint | Token auth style | Refresh rotates? | Extra callback param | PKCE | Scope delimiter | Sandbox vs prod host | Refresh expiry / notes | Revoke |
|---|---|---|---|---|---|---|---|---|---|---|
| **QuickBooks / Intuit** | `appcenter.intuit.com/connect/oauth2` | `oauth.platform.intuit.com/oauth2/v1/tokens/bearer` | **HTTP Basic** | **YES** (~24–26h) | **`realmId`** (required; also needed on revoke) | Supported, not mandatory for server apps | space | sandbox `sandbox-quickbooks.api.intuit.com` / prod `quickbooks.api.intuit.com` (auth+token hosts shared) | rotates ~24h; 5-year absolute max (post-Oct-2023) | `developer.api.intuit.com/v2/oauth2/tokens/revoke` (realmId query) |
| **Salesforce** | `login.salesforce.com/services/oauth2/authorize` (sandbox `test.salesforce.com` / My Domain) | `login.salesforce.com/services/oauth2/token` | client_id+secret in **body** (secret optional under PKCE / JWT assertion) | **NO** by default (rotation is a connected-app policy) | none (but `instance_url` returned in token response → API host) | **REQUIRED** for ISV/packaged apps | space | prod `login`; sandbox `test`; API host = per-org `instance_url` | governed by connected-app refresh-token policy | `/services/oauth2/revoke` |
| **Google** | `accounts.google.com/o/oauth2/v2/auth` | `oauth2.googleapis.com/token` | client_id+secret in **body** (Basic also accepted) | **NO** (same refresh token reused; only issued on first auth with `access_type=offline`+`prompt=consent`) | none (`access_type`/`prompt` are request-side) | strongly recommended; required native/desktop/mobile | space (full-URI scopes) | no sandbox; separate OAuth clients for test/prod | non-rotating; revoked if unused ~6mo, on password change (some scopes), or 7-day if app in "testing" | `oauth2.googleapis.com/revoke` |
| **Microsoft Entra / M365** | `login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize` | `login.microsoftonline.com/{tenant}/oauth2/v2.0/token` | client_id+secret (or cert assertion) in **body** | **YES** (new refresh token every refresh) | none (must request `offline_access` to get a refresh token) | recommended; required for SPA | space (or `.default` static scope) | no sandbox; national clouds differ (`.us`, `.cn` via `login.chinacloudapi.cn`) | ~90-day typical, **24h if redirect URI registered as `spa`**; configurable lifetime policies | standard revoke |
| **Xero** | `login.xero.com/identity/connect/authorize` | `identity.xero.com/connect/token` | **HTTP Basic** (PKCE public-client sends client_id in body, no secret) | **YES** (prior access+refresh invalidated each refresh; 30-min grace to retry old token if response lost) | none on callback, but must call `GET api.xero.com/connections` post-exchange for `tenantId` (→ `xero-tenant-id` header) | recommended; first-class PKCE flow | space (must include `offline_access`) | no separate host; single `api.xero.com` + demo company | **60-day inactivity** (any use resets) | `DELETE api.xero.com/connections/{id}` |
| **HubSpot** | `app.hubspot.com/oauth/authorize` | `api.hubapi.com/oauth/v1/token` (v1; v3 introduced Jan 2026, v1 deprecated-but-operational) | client_id+secret in **body** (NOT Basic) | **NO** (long-lived, reused) | none | not required (confidential-client model) | space | single `api.hubapi.com`; "sandbox" is a separate portal, not a host; EU accounts still `api.hubapi.com` | non-rotating, effectively non-expiring unless app uninstalled or scopes change; access token ~1800s | `DELETE api.hubapi.com/oauth/v1/refresh-tokens/{token}` |

**Descriptor cross-check:** these quirks map 1:1 onto the framework fields — Basic-vs-body → `token_endpoint_auth_style` (Intuit/Xero need Basic); rotation → `refresh_token_rotates` (Intuit/Microsoft/Xero rotate, and Salesforce may by policy; Google/HubSpot don't); `realmId`/`instance_url` → `extra_callback_params`; sandbox/prod and templated hosts → `base_url_resolution`; every provider's revoke URL → `revocation_endpoint`. Xero's post-exchange `/connections` tenant fetch is the one quirk beyond the current field set — it needs a post-callback hook (a `provider_metadata`-populating step) rather than a plain callback-param capture.

**Open items to confirm at integration time:** Salesforce rotation depends on the target org's connected-app policy (confirm reuse-vs-rotate before assuming); HubSpot v3 endpoints (Jan 2026) may shift the token host/style for new integrations; Microsoft's 24h refresh expiry only bites if runtara's redirect URI is registered as `spa` rather than `web` (verify the app-registration type); and confirm production actually sets the AES-GCM key env var so `crypto/factory.rs` isn't silently storing tokens in plaintext.

## 9. Phased roadmap

Sequenced by dependency: **correct the flow → make it declarative → prove it with a rotating provider → harden → add the registry.** Rotation persistence (G1) gates everything that touches a rotating provider, so it comes first (alongside a quick check that production enforces at-rest encryption).

| Phase | Scope | Delivers | Depends on |
|-------|-------|----------|-----------|
| **P0 · Correctness foundation** | A1 rotation persistence + single-flight + optimistic-concurrency guard (`refresh_token_hash`/`rotation_generation` column); **verify/enforce at-rest encryption** (confirm prod sets `RUNTARA_ENV=production`/`REQUIRE_ENCRYPTION`; consider default-required — `crypto/factory.rs`) | The platform becomes safe for rotating providers; refresh tokens are guaranteed encrypted at rest | — |
| **P1 · Provider descriptor (B)** | `ProviderOAuthDescriptor` + macro attrs; migrate HubSpot onto it (byte-for-byte parity, no behavior change); make `generate_authorization_url`/`exchange_code`/`describe_connection_auth`/callback **data-driven**; fold in A2 (Basic auth), A3 (extra callback params), A4 (env/template base URL) | Adding a provider becomes one attribute block, zero procedural code | P0 (the `refresh_token_rotates` flag it exposes drives P0's persistence switch) |
| **P2 · QuickBooks (first rotating provider)** | `quickbooks_online` connection type + `runtara-agent-quickbooks` (per `docs/quickbooks-agent-plan.md`), riding the P1 descriptor | End-to-end proof: Basic-auth token endpoint, `realmId` capture, sandbox/prod host, rotation survival — all through the framework, against an Intuit sandbox | P0, P1 |
| **P3 · Best-practices hardening** | `invalid_grant`→`NEEDS_REAUTH` taxonomy; PKCE (descriptor-gated); revoke-on-disconnect; refresh-token expiry tracking + reconnect UX | Dead connections stop retrying forever; PKCE-required providers (Salesforce ISV, Xero) become addable; clean disconnect | P1 |
| **P4 · Central app registry (C)** | smo-management `oauth_app_registry` + admin endpoints + audit; runtara read-only **encrypted replica** + deploy sync; `connection_data_entity.registry_app_id` FK; authorize/exchange source creds from the registry; **single fixed callback + tenant-in-`state`** redirect_uri migration | The original requirement: platform-registered apps kickstart the flow; tenants consent without pasting client creds; bring-your-own retained as fallback | P1 (descriptor), P3 (redirect_uri strategy) |

P0+P1 are the critical path; P2 validates them; P3 and P4 are independent hardening/strategic tracks that both build on P1.

## 10. Testing & rollout

- **Per-phase gate:** the `e2e-verify` skill (full stack + embedded WASM runner + separate DBs) — unit tests are insufficient for auth/runtime changes (repo policy).
- **P0 rotation regression (highest-value test):** create a connection, force an access-token refresh (advance past the 55-min margin or evict the cache), re-run a call, and assert (a) success **and** (b) the **stored `refresh_token` changed** and (c) a concurrent second refresh does not double-rotate (optimistic-concurrency guard holds). Add a test asserting no `access_token`/`refresh_token` substring can appear in any `Err` surfaced to the proxy/WASM boundary.
- **P1 parity:** HubSpot must behave identically before/after descriptor migration (same authorize URL, same body-form token exchange, same non-rotating persistence).
- **P2 QBO:** Intuit **developer sandbox** company + sandbox app; register the callback exactly; drive authorize→consent→callback; assert `access_token`/`refresh_token`/`token_expires_at`/`realm_id` land encrypted; run `SELECT * FROM Customer`; exercise the read→sparse-update `SyncToken` chain.
- **P4 registry:** test the deploy sync (encrypted replica populated, immutable at runtime), a registry-backed connection consenting with **no** credential entry, and the fixed-callback/tenant-in-`state` path across two tenants sharing one app.
- **Rollout ordering caveat:** the P4 redirect_uri migration (per-tenant path → single fixed path) is a coordinated change with each provider console; ship the runtara callback change and the console registration together, and keep the legacy per-tenant callback route alive until all providers are migrated.

## 11. Risks & open questions

**Carried from the rotation review (P0):**
- **`refresh_token_hash`/`rotation_generation` column** — new plaintext, non-secret column on the connections table for the optimistic-concurrency guard; needs a migration + backfill. Confirm exact table/column against `crates/runtara-connections/src/types.rs:385-386`. **TO CONFIRM.**
- **DB-persist timeout** — pick a tight timeout for the awaited persist (distinct from pool timeout); needs a load-tested value. **TO CONFIRM.**
- **Cache-cold storm tail latency** — post-deploy, many connections refresh near-simultaneously (each now a synchronous seal+write); acceptable for rotating (must be awaited), but keep the non-rotating cold-start persist best-effort/async (mirror `record_credential_request_async`).
- **Deploy overlap** — one-process-per-tenant is verified but *operational, not code-enforced*; rolling/blue-green transiently runs two processes → the optimistic-concurrency guard (not the single-process assumption) is what keeps rotation correct. Emit a metric when >1 process holds a tenant DB.
- **Multi-replica advisory lock (deferred)** — if a tenant is ever intentionally multi-replica, escalate to a per-connection Redis/`pg_advisory_xact_lock` (the facade already wires `redis_manager`); optimistic concurrency is the baseline.

**Carried from the extension design (P1–P4):**
- **At-rest encryption enforcement (verify in P0)** — the cipher **already hard-errors at boot** when the key is missing/invalid and `RUNTARA_ENV=production`/`REQUIRE_ENCRYPTION` is set (`crypto/factory.rs:57,76,109`); it degrades to a loudly-logged NoOp plaintext cipher (`factory.rs:119`) only when encryption isn't required. So this is **not** a code bug — the action is operational: confirm every production deployment sets the enforcement flag + the AES-GCM key, and consider making encryption default-required so a forgotten env var can't silently drop to plaintext.
- **Xero's post-exchange `/connections` fetch** is the one provider quirk *beyond* the current descriptor fields (tenantId isn't a callback param) — needs a generic **post-callback `provider_metadata` hook**, not just callback-param capture. Fold into the descriptor design in P1 so it isn't bolted on later.
- **Recovered smo-management schema needs re-verification** — the `oauth_app_registry`/`integration_entity` prior art was recovered from removed commit `04e1b5b`; re-verify the exact columns/migration when implementing P4.
- **Provider policy drift** — Salesforce rotation depends on the target org's connected-app policy (confirm reuse-vs-rotate per org); HubSpot's v3 token endpoints (Jan 2026) may shift host/style; Microsoft's 24h refresh expiry bites only if runtara's redirect URI is registered as `spa` vs `web`. Re-verify each at integration time.
- **`rotates_refresh_token` flag placement** — resolved by the P1 descriptor (`refresh_token_rotates`); until P1 lands, P0 may need a minimal hardcoded map (HubSpot=false, QBO=true).

