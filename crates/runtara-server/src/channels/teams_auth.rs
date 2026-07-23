// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bot Framework inbound JWT validation for the Teams webhook.
//!
//! Implements the channel→bot validation rules from the Bot Connector
//! authentication spec, for both trust domains a Teams bot can receive
//! tokens from:
//!
//! * **Bot Framework issuer** (`https://api.botframework.com`): signature
//!   against the login.botframework.com JWKS, whose keys additionally carry
//!   channel `endorsements` — the signing key must be endorsed for `msteams`.
//!   These tokens carry a `serviceurl` claim that MUST equal the activity's
//!   root-level `serviceUrl` (the anti-token-exfiltration check).
//! * **Connection tenant issuers** (single-tenant bots):
//!   `https://sts.windows.net/{tenant}/` and
//!   `https://login.microsoftonline.com/{tenant}/v2.0`, validated against the
//!   tenant's Microsoft Entra JWKS. Only the tenant configured on the
//!   connection is trusted. The `serviceurl` claim is enforced when present.
//!
//! The signing algorithm is pinned to RS256 (the only algorithm the Bot
//! Framework metadata advertises) — the attacker-controlled JWT header `alg`
//! is never consulted. Rejections map to HTTP 403 per the spec.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dashmap::DashMap;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

/// Teams' Bot Framework channel id, as endorsed on JWKS signing keys.
pub const TEAMS_CHANNEL_ID: &str = "msteams";

const BOTFRAMEWORK_ISSUER: &str = "https://api.botframework.com";

const DEFAULT_BOTFRAMEWORK_OPENID_URL: &str =
    "https://login.botframework.com/v1/.well-known/openid-configuration";
/// Microsoft Entra v2 tenant metadata; `{tenant}` is substituted.
const DEFAULT_ENTRA_OPENID_URL_TEMPLATE: &str =
    "https://login.microsoftonline.com/{tenant}/v2.0/.well-known/openid-configuration";

/// Spec: "Industry-standard clock-skew is 5 minutes."
const CLOCK_SKEW_SECS: u64 = 300;
/// Keys are stable and cacheable; the spec requires refreshing at least once
/// every 24 hours. A miss on an unknown `kid` still triggers a refetch (so key
/// additions propagate immediately), but at most once per `JWKS_NEG_CACHE_SECS`
/// per kid — see the negative cache below.
const JWKS_REFRESH_SECS: u64 = 6 * 3600;
/// Hard ceiling on a single OpenID-metadata or JWKS fetch. Without it a hung
/// Microsoft endpoint would pin an inbound-validation task indefinitely.
const JWKS_FETCH_TIMEOUT_SECS: u64 = 5;
/// After a fresh fetch that still lacks a requested `kid`, suppress refetches
/// for that kid this long. Legitimate key rotation propagates on the first
/// unknown-kid request; a flood of bogus-kid tokens can no longer hammer the
/// upstream JWKS endpoint (once per minute per distinct kid at most).
const JWKS_NEG_CACHE_SECS: u64 = 60;
/// Cap on the negative-cache map so a flood of random kids cannot grow it
/// without bound; on overflow the whole map is dropped (correctness-neutral —
/// entries only rate-limit refetches).
const JWKS_NEG_CACHE_MAX: usize = 10_000;

/// Where to fetch OpenID metadata from. Injectable so tests (and mock-based
/// e2e) can point validation at a local mock authority instead of Microsoft.
pub struct TeamsAuthEndpoints {
    pub botframework_openid_url: String,
    pub entra_openid_url_template: String,
}

impl TeamsAuthEndpoints {
    /// Production endpoints, overridable via env for mock-based testing.
    pub fn from_env() -> &'static Self {
        static ENDPOINTS: std::sync::OnceLock<TeamsAuthEndpoints> = std::sync::OnceLock::new();
        ENDPOINTS.get_or_init(|| TeamsAuthEndpoints {
            botframework_openid_url: std::env::var("RUNTARA_TEAMS_OPENID_CONFIG_URL")
                .unwrap_or_else(|_| DEFAULT_BOTFRAMEWORK_OPENID_URL.to_string()),
            entra_openid_url_template: std::env::var("RUNTARA_TEAMS_ENTRA_OPENID_URL_TEMPLATE")
                .unwrap_or_else(|_| DEFAULT_ENTRA_OPENID_URL_TEMPLATE.to_string()),
        })
    }
}

/// Per-request context the token is validated against.
pub struct TeamsTokenContext<'a> {
    /// The connection's Microsoft App ID — the expected `aud`.
    pub app_id: &'a str,
    /// The connection's configured Microsoft tenant. `None` for legacy
    /// multi-tenant connections; when set, tenant-issued tokens for exactly
    /// this tenant are also accepted (single-tenant bots).
    pub azure_tenant_id: Option<&'a str>,
    /// `serviceUrl` from the root of the incoming activity.
    pub activity_service_url: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
struct JwkKey {
    kid: String,
    n: String,
    e: String,
    /// Bot Framework JWKS only: channel ids this key is endorsed for.
    #[serde(default)]
    endorsements: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Deserialize)]
struct OpenIdConfig {
    jwks_uri: String,
}

struct JwksCacheEntry {
    keys: Vec<JwkKey>,
    fetched_at: Instant,
}

/// JWKS caches keyed by OpenID metadata URL (one entry per trust domain /
/// tenant). Process-global; entries refresh after [`JWKS_REFRESH_SECS`] or on
/// an unknown-`kid` miss.
static JWKS_CACHES: std::sync::LazyLock<DashMap<String, JwksCacheEntry>> =
    std::sync::LazyLock::new(DashMap::new);

/// Recently-confirmed-absent `(metadata_url, kid)` pairs → the instant we
/// confirmed absence. Rate-limits refetches for a kid the upstream keyset does
/// not contain (see [`JWKS_NEG_CACHE_SECS`]).
static JWKS_NEG_CACHE: std::sync::LazyLock<DashMap<(String, String), Instant>> =
    std::sync::LazyLock::new(DashMap::new);

/// Per-metadata-URL fetch locks: a cold cache or unknown kid must trigger at
/// most ONE upstream fetch even under a burst of concurrent inbound requests
/// (single-flight). Losers re-check the cache after the winner populates it.
static JWKS_FETCH_LOCKS: std::sync::LazyLock<DashMap<String, Arc<Mutex<()>>>> =
    std::sync::LazyLock::new(DashMap::new);

/// Which issuer domain a token claims to come from, decided by the token's
/// (unverified) `iss` and then *proven* by full validation against that
/// domain's JWKS. An issuer outside both domains is rejected outright.
enum TrustDomain {
    BotFramework,
    ConnectionTenant,
}

/// Validate the Bearer token of an inbound Teams webhook request.
///
/// Returns `Err` on any failure; callers must map that to HTTP 403 and must
/// not process or persist anything from the activity — including its
/// `serviceUrl` — beforehand.
pub async fn validate_teams_request(
    headers: &HeaderMap,
    ctx: &TeamsTokenContext<'_>,
    endpoints: &TeamsAuthEndpoints,
) -> anyhow::Result<()> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("Missing Authorization header"))?;
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow::anyhow!("Invalid Authorization header format"))?;

    // Header is attacker-controlled: take only the kid from it. The
    // algorithm is pinned to RS256 below, never read from the header.
    let header = decode_header(token).map_err(|e| anyhow::anyhow!("Invalid JWT header: {e}"))?;
    let kid = header
        .kid
        .ok_or_else(|| anyhow::anyhow!("JWT has no kid"))?;

    // The issuer decides which JWKS proves the signature, so it must be read
    // before verification; it is then enforced by the verified decode.
    let issuer = unverified_issuer(token)?;
    let (domain, metadata_url) = match issuer.as_str() {
        BOTFRAMEWORK_ISSUER => (
            TrustDomain::BotFramework,
            endpoints.botframework_openid_url.clone(),
        ),
        _ => {
            let tenant = ctx
                .azure_tenant_id
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Untrusted token issuer: {issuer}"))?;
            let accepted = [
                format!("https://sts.windows.net/{tenant}/"),
                format!("https://login.microsoftonline.com/{tenant}/v2.0"),
            ];
            if !accepted.iter().any(|a| a == &issuer) {
                anyhow::bail!("Untrusted token issuer: {issuer}");
            }
            (
                TrustDomain::ConnectionTenant,
                endpoints
                    .entra_openid_url_template
                    .replace("{tenant}", tenant),
            )
        }
    };

    let jwk = get_jwk(&metadata_url, &kid).await?;

    // Bot Framework keys carry channel endorsements: the signing key must be
    // endorsed for the Teams channel, or a key minted for another channel
    // could sign Teams-shaped traffic.
    if matches!(domain, TrustDomain::BotFramework) {
        let endorsed = jwk
            .endorsements
            .as_ref()
            .is_some_and(|e| e.iter().any(|c| c == TEAMS_CHANNEL_ID));
        if !endorsed {
            anyhow::bail!("Signing key is not endorsed for the {TEAMS_CHANNEL_ID} channel");
        }
    }

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|e| anyhow::anyhow!("Invalid RSA key: {e}"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.leeway = CLOCK_SKEW_SECS;
    // Enforce `nbf` in addition to the default `exp`: a token minted for a
    // future window must not be accepted early (replay-ahead defense).
    validation.validate_nbf = true;
    validation.set_audience(&[ctx.app_id]);
    validation.set_issuer(&[issuer.as_str()]);

    let data = decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| anyhow::anyhow!("JWT validation failed: {e}"))?;

    // serviceUrl claim vs activity serviceUrl — the check that stops a
    // validly-signed token from binding a reply target the channel never
    // vouched for. Bot Framework tokens always carry it (claim name is
    // lowercase); tenant-issued tokens are enforced when the claim is present.
    let claim_service_url = data.claims.get("serviceurl").and_then(|v| v.as_str());
    match (claim_service_url, &domain) {
        (Some(claim), _) => {
            let activity = ctx.activity_service_url.ok_or_else(|| {
                anyhow::anyhow!("Token has a serviceurl claim but the activity has no serviceUrl")
            })?;
            if normalize_service_url(claim) != normalize_service_url(activity) {
                anyhow::bail!("Activity serviceUrl does not match the token's serviceurl claim");
            }
        }
        (None, TrustDomain::BotFramework) => {
            anyhow::bail!("Bot Framework token is missing the serviceurl claim");
        }
        (None, TrustDomain::ConnectionTenant) => {}
    }

    // Success path: leave a breadcrumb of WHICH trust domain and issuer proved
    // the token, so a misrouted/misconfigured tenant is diagnosable without
    // turning on trace for the whole crate. serviceUrl is logged only as
    // present/absent (it is not a secret, but keep logs terse).
    tracing::debug!(
        target: "teams_auth",
        issuer = %issuer,
        trust_domain = match domain {
            TrustDomain::BotFramework => "botframework",
            TrustDomain::ConnectionTenant => "connection_tenant",
        },
        has_serviceurl_claim = claim_service_url.is_some(),
        "Teams inbound JWT validated"
    );

    Ok(())
}

/// serviceUrls are compared with the trailing slash trimmed; Microsoft's own
/// examples interchange `https://smba.trafficmanager.net/teams` and `.../teams/`.
pub fn normalize_service_url(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// Read the `iss` claim without verifying the signature. Used ONLY to select
/// the trust domain; the value is re-enforced by the verified decode.
fn unverified_issuer(token: &str) -> anyhow::Result<String> {
    let payload_b64 = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Malformed JWT"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|e| anyhow::anyhow!("Malformed JWT payload: {e}"))?;
    let claims: Value =
        serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("Malformed JWT claims: {e}"))?;
    claims
        .get("iss")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("JWT has no issuer"))
}

/// Get a JWK by key id from the JWKS advertised by `metadata_url`, using the
/// process cache. Fetches go through the hardened egress client (no
/// redirects, DNS-guarded).
async fn get_jwk(metadata_url: &str, kid: &str) -> anyhow::Result<JwkKey> {
    // 1. Fresh positive cache with the kid present — the common path, lock-free.
    if let Some(key) = cached_fresh_key(metadata_url, kid) {
        return Ok(key);
    }
    // 2. Recently confirmed absent — refuse without hammering the upstream.
    if neg_cached(metadata_url, kid) {
        anyhow::bail!("JWK not found for kid: {kid} (negative-cached)");
    }

    // 3. Single-flight: only one task per metadata URL performs the fetch.
    let lock = JWKS_FETCH_LOCKS
        .entry(metadata_url.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // 4. Re-check after acquiring — the winner may have just populated things.
    if let Some(key) = cached_fresh_key(metadata_url, kid) {
        return Ok(key);
    }
    if neg_cached(metadata_url, kid) {
        anyhow::bail!("JWK not found for kid: {kid} (negative-cached)");
    }

    // 5. Fetch OpenID metadata + JWKS, each bounded by a hard timeout.
    let client = runtara_connections::net::shared_hardened_client();
    let timeout = Duration::from_secs(JWKS_FETCH_TIMEOUT_SECS);
    let openid: OpenIdConfig = tokio::time::timeout(timeout, async {
        client
            .get(metadata_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timed out fetching OpenID metadata"))??;
    let jwks: JwksResponse = tokio::time::timeout(timeout, async {
        client
            .get(&openid.jwks_uri)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timed out fetching JWKS"))??;

    let key = jwks.keys.iter().find(|k| k.kid == kid).cloned();
    JWKS_CACHES.insert(
        metadata_url.to_string(),
        JwksCacheEntry {
            keys: jwks.keys,
            fetched_at: Instant::now(),
        },
    );

    match key {
        Some(k) => {
            // A rotation may have re-introduced a kid we'd negatively cached.
            JWKS_NEG_CACHE.remove(&(metadata_url.to_string(), kid.to_string()));
            Ok(k)
        }
        None => {
            record_neg_cache(metadata_url, kid);
            anyhow::bail!("JWK not found for kid: {kid}")
        }
    }
}

/// Return the key for `kid` iff the positive cache for `metadata_url` is fresh
/// and contains it.
fn cached_fresh_key(metadata_url: &str, kid: &str) -> Option<JwkKey> {
    let entry = JWKS_CACHES.get(metadata_url)?;
    if entry.fetched_at.elapsed().as_secs() >= JWKS_REFRESH_SECS {
        return None;
    }
    entry.keys.iter().find(|k| k.kid == kid).cloned()
}

/// Whether `(metadata_url, kid)` was confirmed absent within the negative TTL.
fn neg_cached(metadata_url: &str, kid: &str) -> bool {
    JWKS_NEG_CACHE
        .get(&(metadata_url.to_string(), kid.to_string()))
        .map(|at| at.elapsed().as_secs() < JWKS_NEG_CACHE_SECS)
        .unwrap_or(false)
}

/// Record `(metadata_url, kid)` as absent, bounding the map size.
fn record_neg_cache(metadata_url: &str, kid: &str) {
    if JWKS_NEG_CACHE.len() >= JWKS_NEG_CACHE_MAX {
        JWKS_NEG_CACHE.clear();
    }
    JWKS_NEG_CACHE.insert((metadata_url.to_string(), kid.to_string()), Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    /// Fixed 2048-bit RSA test keypair (test-only, never used outside tests).
    const TEST_RSA_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDGcMsIA4zdJXmH
S2Bpy1GX3t5KKa7KlOsa7CFO/yVYVMQnQzhdSuNxSZSixzqcMfEQqcEW6ftW4W5Z
TNipquNTpa9ELOCHUdd8r1nN6tw9RjJ4r4a6K7jqJET5HBZbWFj9tSPL9uVkweOC
J4uk8vrhxxehaC98TfJAKKOFO9NL5UArHzdWg8xyB+EluDfUUxXlzoSVKjGg1OE7
klZbgrB71iNwdme9NxeKrz1YS5dAoWknmJExWYuqXQsCdU2c/rtUtrWCpuWtxVZG
DJAdO8+JUGvl/Dapu5tyy4GQFSlQiqwRbx4cOrEdVZJT8iEh8ykgSjr8+ctcUUQt
cqykSBW3AgMBAAECggEAIQ3ju+N/gMy/uAoVtrmfzzjX9SmJTIROvy7LA5IbgeGo
xNN9HYkeZp33jL+74w2slnZ4S91QuPGXBHf49RYahLHqBmSlR9UZnFLHFjZDVk+N
k63FNtiWliXReV802CVYuXYFTvHC1yw2vdThfWnd4WLc7E1i74U6T3aVellzQkZT
hl5hSbIJCe/Ss+3ryBbE8mkhv678irPsTAXok3dpOyDbwjEVt7Xf6Pfe6qihL4Fd
MPyF/nfOtSQR23ypUoDullkBZ+5dgdyynz7dyv+zHPKjxiQ8QK75jAvMB5Rn6YzJ
jVpedW8i1o1JzjvFMxiJ9bvoWgotYA5Mx6b+aAAxzQKBgQD7gmDcXFKDe4S1GLXp
eAYu//jBy390yuAONR6BqB4xlarAGCp+PeDJj5U5U751lDtdiy4bpx4pOhXnjc6V
art6X1K9sU0OWjcRSSPC62BwHIxjIzglFl99JURPH9JjZbF8ZxTbvBLyInliN6c6
Zgz+EDW+Bb9g4wdGZjCuvO1/UwKBgQDJ+9fvvD1kTqG8oWQCNzw9Rj04yNQ0ehzC
zG42Sz+FXt5L4ZqybZswNr3zZvkUNaSVeDzxFCOB7yPPXCgRCRzH2jV7k8ZQX6/K
7Zuo0GhVIGR6nojGpAQpNPqVqVdo8q0erw5piv9egsoTWQE4fISTn44FBDUeN958
OdHoWkWXjQKBgC3DooZWUjlUf2hIb8lkqpNgxk3VDoMc6zoKllt3UM8q8Z/0hb7k
2YMzmi6NO2m/qDG0QpaLiSRtSlEQ75cmjaiNscuMeH31EnIVwekU1T5xI2ZioTO2
Z3epEU3od2rYtTvyscvt4/ClLzsc71Pj/9c28eB6wUEK7mbz70XMYNa7AoGASVRe
XBH6M918SJBLT6af/xruBRycNgUTRgGUDbAZ+qCrkd7xG9BBJCrroV+EFDs5am6B
qYCHN5gLZy/s9+pYAZKOEjRfLjTfDIxhE9O93RHqiL3fqEZJoHA0fXtCWb6o7Vfe
oqCs/7H6DTYmBEzokPO/SsDxS+w6oN0ZAQMs+s0CgYAInh9i3y5U4EzGV5IJtiL0
ub5Px952cG03BmZUDr2yyP8JmcWsG/I7rWetu+KRr6gOD6+IgEi3N4ja3SgGp5lq
SzPGw0VwiPkNiu9FBu4mfbT9ouJT+4ux6xN/lSP2gkJfYWpBkKODlnRSMOoa6WIG
sup34c5zDvmwEupkUwyybA==
-----END PRIVATE KEY-----";
    const TEST_RSA_N: &str = "xnDLCAOM3SV5h0tgactRl97eSimuypTrGuwhTv8lWFTEJ0M4XUrjcUmUosc6nDHxEKnBFun7VuFuWUzYqarjU6WvRCzgh1HXfK9ZzercPUYyeK-Guiu46iRE-RwWW1hY_bUjy_blZMHjgieLpPL64ccXoWgvfE3yQCijhTvTS-VAKx83VoPMcgfhJbg31FMV5c6ElSoxoNThO5JWW4Kwe9YjcHZnvTcXiq89WEuXQKFpJ5iRMVmLql0LAnVNnP67VLa1gqblrcVWRgyQHTvPiVBr5fw2qbubcsuBkBUpUIqsEW8eHDqxHVWSU_IhIfMpIEo6_PnLXFFELXKspEgVtw";
    const TEST_RSA_E: &str = "AQAB";

    const APP_ID: &str = "11111111-2222-3333-4444-555555555555";
    const TENANT: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const SERVICE_URL: &str = "https://smba.trafficmanager.net/amer/";

    /// The hardened egress client refuses loopback unless allowlisted; the
    /// allowlist env is read once per process, so set it before first use.
    fn allow_loopback_egress() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost");
        });
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    struct MockAuthority {
        endpoints: TeamsAuthEndpoints,
        #[allow(dead_code)]
        handle: tokio::task::JoinHandle<()>,
    }

    /// Serve OpenID metadata + JWKS for both trust domains from one mock
    /// server. `kid_suffix` keeps kids unique per test so the process-global
    /// JWKS cache can't leak state across tests.
    async fn mock_authority(kid: &str, endorsements: Option<Vec<&str>>) -> MockAuthority {
        allow_loopback_egress();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let keys = json!({
            "keys": [{
                "kid": kid,
                "kty": "RSA",
                "use": "sig",
                "n": TEST_RSA_N,
                "e": TEST_RSA_E,
                "endorsements": endorsements,
            }]
        });
        let openid = json!({ "issuer": "mock", "jwks_uri": format!("{base}/keys") });
        let app = axum::Router::new()
            .route(
                "/bf/openid",
                axum::routing::get({
                    let openid = openid.clone();
                    move || {
                        let openid = openid.clone();
                        async move { axum::Json(openid) }
                    }
                }),
            )
            .route(
                "/entra/{tenant}/openid",
                axum::routing::get({
                    let openid = openid.clone();
                    move || {
                        let openid = openid.clone();
                        async move { axum::Json(openid) }
                    }
                }),
            )
            .route(
                "/keys",
                axum::routing::get(move || {
                    let keys = keys.clone();
                    async move { axum::Json(keys) }
                }),
            );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockAuthority {
            endpoints: TeamsAuthEndpoints {
                botframework_openid_url: format!("{base}/bf/openid"),
                entra_openid_url_template: format!("{base}/entra/{{tenant}}/openid"),
            },
            handle,
        }
    }

    fn sign_token(kid: &str, claims: Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn bearer_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    fn botframework_claims() -> Value {
        json!({
            "iss": BOTFRAMEWORK_ISSUER,
            "aud": APP_ID,
            "exp": now() + 3600,
            "nbf": now() - 60,
            "serviceurl": SERVICE_URL,
        })
    }

    fn ctx<'a>(tenant: Option<&'a str>, service_url: Option<&'a str>) -> TeamsTokenContext<'a> {
        TeamsTokenContext {
            app_id: APP_ID,
            azure_tenant_id: tenant,
            activity_service_url: service_url,
        }
    }

    #[tokio::test]
    async fn accepts_valid_botframework_token() {
        let auth = mock_authority("bf-ok", Some(vec!["msteams", "webchat"])).await;
        let token = sign_token("bf-ok", botframework_claims());
        // Trailing-slash difference between claim and activity must not matter.
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some("https://smba.trafficmanager.net/amer")),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test]
    async fn rejects_untrusted_issuer() {
        let auth = mock_authority("bf-iss", Some(vec!["msteams"])).await;
        let mut claims = botframework_claims();
        claims["iss"] = json!("https://evil.example.com");
        let token = sign_token("bf-iss", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let auth = mock_authority("bf-aud", Some(vec!["msteams"])).await;
        let mut claims = botframework_claims();
        claims["aud"] = json!("99999999-0000-0000-0000-000000000000");
        let token = sign_token("bf-aud", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_expired_token_beyond_leeway() {
        let auth = mock_authority("bf-exp", Some(vec!["msteams"])).await;
        let mut claims = botframework_claims();
        claims["exp"] = json!(now() - CLOCK_SKEW_SECS - 120);
        let token = sign_token("bf-exp", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_hs256_alg_confusion() {
        // A token HMAC-signed with the public modulus as the secret and
        // alg=HS256 in the header must fail: the algorithm is pinned RS256.
        let auth = mock_authority("bf-alg", Some(vec!["msteams"])).await;
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("bf-alg".to_string());
        let token = encode(
            &header,
            &botframework_claims(),
            &EncodingKey::from_secret(TEST_RSA_N.as_bytes()),
        )
        .unwrap();
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_serviceurl_claim_mismatch() {
        let auth = mock_authority("bf-svc", Some(vec!["msteams"])).await;
        let token = sign_token("bf-svc", botframework_claims());
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some("https://attacker.example.com/")),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_botframework_token_without_serviceurl_claim() {
        let auth = mock_authority("bf-nosvc", Some(vec!["msteams"])).await;
        let mut claims = botframework_claims();
        claims.as_object_mut().unwrap().remove("serviceurl");
        let token = sign_token("bf-nosvc", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_key_not_endorsed_for_teams() {
        let auth = mock_authority("bf-endorse", Some(vec!["webchat", "directline"])).await;
        let token = sign_token("bf-endorse", botframework_claims());
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_key_with_no_endorsements_for_botframework_issuer() {
        let auth = mock_authority("bf-noendorse", None).await;
        let token = sign_token("bf-noendorse", botframework_claims());
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn accepts_single_tenant_issuer_v1() {
        let auth = mock_authority("st-v1", None).await;
        let claims = json!({
            "iss": format!("https://sts.windows.net/{TENANT}/"),
            "aud": APP_ID,
            "exp": now() + 3600,
            // Tenant-issued tokens: serviceurl enforced only when present.
        });
        let token = sign_token("st-v1", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(Some(TENANT), Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test]
    async fn accepts_single_tenant_issuer_v2_and_enforces_serviceurl_when_present() {
        let auth = mock_authority("st-v2", None).await;
        let claims = json!({
            "iss": format!("https://login.microsoftonline.com/{TENANT}/v2.0"),
            "aud": APP_ID,
            "exp": now() + 3600,
            "serviceurl": SERVICE_URL,
        });
        let token = sign_token("st-v2", claims.clone());
        let ok = validate_teams_request(
            &bearer_headers(&token),
            &ctx(Some(TENANT), Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(ok.is_ok(), "expected Ok, got {ok:?}");

        let mismatch = validate_teams_request(
            &bearer_headers(&token),
            &ctx(Some(TENANT), Some("https://attacker.example.com")),
            &auth.endpoints,
        )
        .await;
        assert!(mismatch.is_err());
    }

    #[tokio::test]
    async fn rejects_tenant_issuer_for_foreign_tenant() {
        let auth = mock_authority("st-foreign", None).await;
        let claims = json!({
            "iss": "https://sts.windows.net/99999999-8888-7777-6666-555555555555/",
            "aud": APP_ID,
            "exp": now() + 3600,
        });
        let token = sign_token("st-foreign", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(Some(TENANT), Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_tenant_issuer_when_connection_has_no_tenant() {
        let auth = mock_authority("st-notenant", None).await;
        let claims = json!({
            "iss": format!("https://sts.windows.net/{TENANT}/"),
            "aud": APP_ID,
            "exp": now() + 3600,
        });
        let token = sign_token("st-notenant", claims);
        let result = validate_teams_request(
            &bearer_headers(&token),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_missing_bearer() {
        let auth = mock_authority("bf-missing", Some(vec!["msteams"])).await;
        let result = validate_teams_request(
            &HeaderMap::new(),
            &ctx(None, Some(SERVICE_URL)),
            &auth.endpoints,
        )
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn neg_cache_rate_limits_unknown_kid() {
        // Unique url/kid so the process-global maps can't collide with siblings.
        let url = "https://neg-cache-test.example/openid";
        let kid = "kid-does-not-exist-xyz";
        assert!(!neg_cached(url, kid), "fresh pair must not be neg-cached");
        record_neg_cache(url, kid);
        assert!(neg_cached(url, kid), "recorded pair must be neg-cached");
        // A different kid on the same url is independent.
        assert!(!neg_cached(url, "some-other-kid"));
    }

    #[test]
    fn cached_fresh_key_respects_refresh_window() {
        let url = "https://fresh-key-test.example/openid";
        let kid = "fresh-kid";
        // Insert a stale entry: it must NOT be treated as a hit.
        JWKS_CACHES.insert(
            url.to_string(),
            JwksCacheEntry {
                keys: vec![JwkKey {
                    kid: kid.to_string(),
                    n: TEST_RSA_N.to_string(),
                    e: TEST_RSA_E.to_string(),
                    endorsements: None,
                }],
                fetched_at: Instant::now() - Duration::from_secs(JWKS_REFRESH_SECS + 1),
            },
        );
        assert!(
            cached_fresh_key(url, kid).is_none(),
            "stale cache is a miss"
        );

        // Fresh entry with the kid present is a hit; an absent kid is a miss.
        JWKS_CACHES.insert(
            url.to_string(),
            JwksCacheEntry {
                keys: vec![JwkKey {
                    kid: kid.to_string(),
                    n: TEST_RSA_N.to_string(),
                    e: TEST_RSA_E.to_string(),
                    endorsements: None,
                }],
                fetched_at: Instant::now(),
            },
        );
        assert!(cached_fresh_key(url, kid).is_some());
        assert!(cached_fresh_key(url, "absent-kid").is_none());
    }
}
