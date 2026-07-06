use super::provider_auth::RotatedCredentials;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

pub(crate) const TOKEN_REFRESH_MARGIN_SECS: i64 = 300;
pub(crate) const DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS: i64 = 24 * 60 * 60;
/// How long a failed refresh is remembered so concurrent waiters that acquire the
/// single-flight lock right after a failure inherit the error instead of each
/// re-hammering the provider under a persistent outage.
const REFRESH_ERROR_COOLDOWN_SECS: i64 = 5;

#[derive(Clone)]
pub(crate) struct CachedAccessToken {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Parsed token-endpoint response. Unlike [`CachedAccessToken`] this also carries
/// the (possibly rotated) `refresh_token` so the refresh path can surface it for
/// persistence. It is never stored in the in-memory access-token cache.
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub refresh_token: Option<String>,
}

/// Result of resolving a [`DeferredAuth`]: the header to inject, plus — only when an
/// actual refresh occurred on the refresh-token grant — the credentials that must be
/// persisted back to the connection. `rotated` is `None` on every cache hit / fast-path
/// and structurally always `None` for the client-credentials grant.
pub(crate) struct ResolvedDeferredAuth {
    pub header_name: String,
    pub header_value: String,
    pub rotated: Option<RotatedCredentials>,
}

impl ResolvedDeferredAuth {
    fn header_only(header_name: String, header_value: String) -> Self {
        Self {
            header_name,
            header_value,
            rotated: None,
        }
    }
}

pub(crate) enum TokenRequestBody {
    Json(Value),
    FormUrlEncoded(Vec<(String, String)>),
}

pub(crate) enum DeferredAuth {
    OAuth2ClientCredentials {
        cache_key: String,
        token_url: String,
        header_name: String,
        header_value_prefix: Option<String>,
        request_body: TokenRequestBody,
        default_ttl_seconds: i64,
    },
    OAuth2RefreshToken {
        cache_key: String,
        token_url: String,
        header_name: String,
        client_id: String,
        client_secret: String,
        refresh_token: String,
        fallback_access_token: Option<String>,
        fallback_expires_at: Option<DateTime<Utc>>,
    },
}

static TOKEN_CACHE: OnceLock<DashMap<String, CachedAccessToken>> = OnceLock::new();

pub(crate) async fn resolve_deferred_auth(
    client: &Client,
    auth: DeferredAuth,
    events: &crate::events::ConnectionEvents,
    connection_id: &str,
    integration: &str,
) -> Result<ResolvedDeferredAuth, String> {
    match auth {
        DeferredAuth::OAuth2ClientCredentials {
            cache_key,
            token_url,
            header_name,
            header_value_prefix,
            request_body,
            default_ttl_seconds,
        } => {
            let token = resolve_cached_token(&cache_key, || async {
                exchange_client_credentials_token(
                    client,
                    &token_url,
                    request_body,
                    default_ttl_seconds,
                )
                .await
            })
            .await?;
            let header_value = match header_value_prefix {
                Some(prefix) => format!("{}{}", prefix, token),
                None => token,
            };
            // The client-credentials grant has no refresh token to rotate.
            Ok(ResolvedDeferredAuth::header_only(header_name, header_value))
        }
        DeferredAuth::OAuth2RefreshToken {
            cache_key,
            token_url,
            header_name,
            client_id,
            client_secret,
            refresh_token,
            fallback_access_token,
            fallback_expires_at,
        } => {
            // Fast path 1: a still-fresh DB-stored access token — pure read, no refresh,
            // no lock. Must never trigger a network refresh (that would bypass single-flight).
            if let Some(access_token) = fallback_access_token
                && token_is_fresh(fallback_expires_at)
            {
                cache_token(
                    &cache_key,
                    CachedAccessToken {
                        access_token: access_token.clone(),
                        expires_at: fallback_expires_at,
                    },
                );
                return Ok(ResolvedDeferredAuth::header_only(
                    header_name,
                    format!("Bearer {access_token}"),
                ));
            }

            // Fast path 2: a fresh in-memory cached access token — pure read, no lock.
            if let Some(token) = get_fresh_cached_token(&cache_key) {
                return Ok(ResolvedDeferredAuth::header_only(
                    header_name,
                    format!("Bearer {token}"),
                ));
            }

            // Slow path: single-flight per connection. A rotating provider (e.g. Intuit)
            // invalidates the old refresh token on every refresh, so concurrent refreshes
            // with the same one-time-use token would self-invalidate. Serialize them.
            let lock = refresh_lock(&cache_key);
            let _guard = lock.lock().await;

            // Double-check the cache under the lock — a peer may have just refreshed.
            if let Some(token) = get_fresh_cached_token(&cache_key) {
                return Ok(ResolvedDeferredAuth::header_only(
                    header_name,
                    format!("Bearer {token}"),
                ));
            }

            // Negative cache: inherit a very recent failure rather than re-hammering the
            // provider under a persistent outage.
            if let Some(err) = recent_refresh_error(&cache_key) {
                return Err(err);
            }

            let outcome = refresh_oauth_access_token(
                client,
                &token_url,
                &client_id,
                &client_secret,
                &refresh_token,
            )
            .await;
            // Emitted on every actual refresh (connection-health signal), regardless of outcome.
            crate::events::emit(
                events,
                crate::events::ConnectionLifecycleEvent::TokenRefreshed {
                    connection_id: connection_id.to_string(),
                    integration: integration.to_string(),
                    success: outcome.is_ok(),
                },
            );

            match outcome {
                Ok(tr) => {
                    clear_refresh_error(&cache_key);
                    cache_token(
                        &cache_key,
                        CachedAccessToken {
                            access_token: tr.access_token.clone(),
                            expires_at: tr.expires_at,
                        },
                    );
                    let header_value = format!("Bearer {}", tr.access_token);
                    Ok(ResolvedDeferredAuth {
                        header_name,
                        header_value,
                        // Surface the (possibly rotated) refresh token + fresh expiry so the
                        // facade can persist them back into connection_parameters.
                        rotated: Some(RotatedCredentials {
                            access_token: tr.access_token,
                            refresh_token: tr.refresh_token,
                            token_expires_at: tr.expires_at,
                        }),
                    })
                }
                Err(e) => {
                    record_refresh_error(&cache_key, &e);
                    Err(e)
                }
            }
        }
    }
}

async fn resolve_cached_token<F, Fut>(cache_key: &str, fetch: F) -> Result<String, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<CachedAccessToken, String>>,
{
    if let Some(token) = get_fresh_cached_token(cache_key) {
        return Ok(token);
    }

    let cached = fetch().await?;
    let access_token = cached.access_token.clone();
    cache_token(cache_key, cached);
    Ok(access_token)
}

async fn exchange_client_credentials_token(
    client: &Client,
    token_url: &str,
    request_body: TokenRequestBody,
    default_ttl_seconds: i64,
) -> Result<CachedAccessToken, String> {
    let request = match request_body {
        TokenRequestBody::Json(body) => client
            .post(token_url)
            .header("Content-Type", "application/json")
            .json(&body),
        TokenRequestBody::FormUrlEncoded(fields) => client
            .post(token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_urlencoded(&fields)),
    };

    let response = request
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Token exchange request failed: {}", e))?;

    // Client credentials has no refresh token to carry forward — drop it.
    let tr = parse_token_response(response, default_ttl_seconds).await?;
    Ok(CachedAccessToken {
        access_token: tr.access_token,
        expires_at: tr.expires_at,
    })
}

async fn refresh_oauth_access_token(
    client: &Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    let body = form_urlencoded(&[
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("client_id".to_string(), client_id.to_string()),
        ("client_secret".to_string(), client_secret.to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ]);

    let response = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("OAuth token refresh request failed: {}", e))?;

    parse_token_response(response, DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS).await
}

async fn parse_token_response(
    response: reqwest::Response,
    default_ttl_seconds: i64,
) -> Result<TokenResponse, String> {
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    if !status.is_success() {
        // NEVER echo the full response body upward — it flows through the proxy to the
        // (untrusted) guest agent and could carry sensitive material. Log the full body
        // host-side; surface only the status + the standard OAuth error fields.
        tracing::debug!(status = %status, body = %body, "token endpoint returned non-success");
        let code = body["error"].as_str().unwrap_or("unknown_error");
        let desc = body["error_description"].as_str().unwrap_or("");
        return Err(format!("Token endpoint returned {status}: {code} {desc}")
            .trim()
            .to_string());
    }

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| {
            tracing::debug!(body = %body, "token response missing access_token");
            "Token response missing access_token field".to_string()
        })?
        .to_string();

    // The (possibly rotated) refresh token, when the provider returns one.
    let refresh_token = body["refresh_token"].as_str().map(|s| s.to_string());

    let expires_at = body["expires_in"]
        .as_i64()
        .map(|ttl| Utc::now() + Duration::seconds(ttl))
        .or_else(|| {
            if default_ttl_seconds > 0 {
                Some(Utc::now() + Duration::seconds(default_ttl_seconds))
            } else {
                None
            }
        });

    Ok(TokenResponse {
        access_token,
        expires_at,
        refresh_token,
    })
}

pub(crate) fn token_is_fresh(expires_at: Option<DateTime<Utc>>) -> bool {
    expires_at
        .is_some_and(|expiry| expiry > Utc::now() + Duration::seconds(TOKEN_REFRESH_MARGIN_SECS))
}

fn token_cache() -> &'static DashMap<String, CachedAccessToken> {
    TOKEN_CACHE.get_or_init(DashMap::new)
}

fn get_fresh_cached_token(cache_key: &str) -> Option<String> {
    token_cache().get(cache_key).and_then(|entry| {
        if token_is_fresh(entry.expires_at) {
            Some(entry.access_token.clone())
        } else {
            None
        }
    })
}

fn cache_token(cache_key: &str, token: CachedAccessToken) {
    token_cache().insert(cache_key.to_string(), token);
}

/// Drop the cached access token for an OAuth refresh-token connection so the next
/// resolution re-refreshes from the persisted connection state. Used on the
/// fail-closed path when a rotated token could not be persisted. The key must
/// match how `describe_oauth_refresh_auth` builds it.
pub(crate) fn invalidate_oauth_refresh_cache(connection_id: &str, integration_id: &str) {
    let key = build_token_cache_key(&["oauth_refresh", connection_id, integration_id]);
    token_cache().remove(&key);
}

// ── Single-flight refresh lock ───────────────────────────────────────────────
// One async mutex per cache key so a rotating provider can't self-invalidate by
// refreshing concurrently with the same one-time-use refresh token. Bounded by
// the tenant's connection count (one process per tenant).
static REFRESH_LOCKS: OnceLock<DashMap<String, Arc<AsyncMutex<()>>>> = OnceLock::new();

fn refresh_lock(cache_key: &str) -> Arc<AsyncMutex<()>> {
    // `.clone()` copies the Arc out; the DashMap shard guard is a temporary dropped
    // when this function returns, so it is never held across the caller's `.await`.
    REFRESH_LOCKS
        .get_or_init(DashMap::new)
        .entry(cache_key.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

// ── Negative cache for recent refresh failures ───────────────────────────────
static REFRESH_ERRORS: OnceLock<DashMap<String, (DateTime<Utc>, String)>> = OnceLock::new();

fn refresh_errors() -> &'static DashMap<String, (DateTime<Utc>, String)> {
    REFRESH_ERRORS.get_or_init(DashMap::new)
}

fn recent_refresh_error(cache_key: &str) -> Option<String> {
    refresh_errors().get(cache_key).and_then(|entry| {
        let (at, msg) = entry.value();
        if *at + Duration::seconds(REFRESH_ERROR_COOLDOWN_SECS) > Utc::now() {
            Some(msg.clone())
        } else {
            None
        }
    })
}

fn record_refresh_error(cache_key: &str, msg: &str) {
    refresh_errors().insert(cache_key.to_string(), (Utc::now(), msg.to_string()));
}

fn clear_refresh_error(cache_key: &str) {
    refresh_errors().remove(cache_key);
}

pub(crate) fn parse_expiry(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn build_token_cache_key(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

fn form_urlencoded(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
fn clear_token_cache() {
    token_cache().clear();
    refresh_errors().clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn resolve_cached_token_uses_cache() {
        clear_token_cache();
        let call_count = Arc::new(AtomicUsize::new(0));

        let first_counter = Arc::clone(&call_count);
        let first = resolve_cached_token("cache-key", move || async move {
            first_counter.fetch_add(1, Ordering::SeqCst);
            Ok(CachedAccessToken {
                access_token: "token-123".to_string(),
                expires_at: Some(Utc::now() + Duration::minutes(30)),
            })
        })
        .await
        .unwrap();
        assert_eq!(first, "token-123");

        let second_counter = Arc::clone(&call_count);
        let second = resolve_cached_token("cache-key", move || async move {
            second_counter.fetch_add(1, Ordering::SeqCst);
            Err("should not be called when cache is fresh".to_string())
        })
        .await
        .unwrap();
        assert_eq!(second, "token-123");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_auth_uses_fallback_access_token_when_still_fresh() {
        clear_token_cache();
        let client = Client::new();
        let auth = DeferredAuth::OAuth2RefreshToken {
            cache_key: "hubspot-cache".to_string(),
            token_url: "https://example.com/token".to_string(),
            header_name: "Authorization".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            fallback_access_token: Some("existing-token".to_string()),
            fallback_expires_at: Some(Utc::now() + Duration::minutes(30)),
        };

        let resolved = resolve_deferred_auth(&client, auth, &None, "conn-1", "hubspot")
            .await
            .unwrap();
        assert_eq!(resolved.header_name, "Authorization");
        assert_eq!(resolved.header_value, "Bearer existing-token");
        // Fresh fallback → no refresh happened → nothing to persist.
        assert!(resolved.rotated.is_none());
    }

    #[test]
    fn form_urlencoded_encodes_keys_and_values() {
        let encoded = form_urlencoded(&[
            ("grant_type".to_string(), "client_credentials".to_string()),
            (
                "scope".to_string(),
                "https://api.businesscentral.dynamics.com/.default".to_string(),
            ),
            ("client secret".to_string(), "value+with space".to_string()),
        ]);

        assert_eq!(
            encoded,
            "grant_type=client_credentials&scope=https%3A%2F%2Fapi.businesscentral.dynamics.com%2F.default&client%20secret=value%2Bwith%20space"
        );
    }

    /// Records every connection lifecycle event it receives, for assertions.
    #[derive(Default)]
    struct RecordingSink {
        events: std::sync::Mutex<Vec<crate::events::ConnectionLifecycleEvent>>,
    }

    impl crate::events::ConnectionEventSink for RecordingSink {
        fn emit(&self, event: crate::events::ConnectionLifecycleEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn refresh_token_grant_emits_token_refreshed_on_actual_refresh() {
        clear_token_cache();
        let recorder = Arc::new(RecordingSink::default());
        let events: crate::events::ConnectionEvents = Some(recorder.clone());
        let client = Client::new();

        // No fresh fallback → the fallback fast-path is skipped and the cache misses, so an
        // actual refresh is attempted. `token_url` is unreachable, so the refresh *fails* — but
        // the event must still fire (it's emitted regardless of outcome), with success=false.
        let auth = DeferredAuth::OAuth2RefreshToken {
            cache_key: "rec-refresh-cache".to_string(),
            token_url: "http://127.0.0.1:1/token".to_string(),
            header_name: "Authorization".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            fallback_access_token: None,
            fallback_expires_at: None,
        };

        let result = resolve_deferred_auth(&client, auth, &events, "conn-42", "hubspot").await;
        assert!(
            result.is_err(),
            "refresh against an unreachable endpoint should fail"
        );

        let recorded = recorder.events.lock().unwrap();
        assert_eq!(recorded.len(), 1, "exactly one token_refreshed event");
        match &recorded[0] {
            crate::events::ConnectionLifecycleEvent::TokenRefreshed {
                connection_id,
                integration,
                success,
            } => {
                assert_eq!(connection_id, "conn-42");
                assert_eq!(integration, "hubspot");
                assert!(!success, "a failed refresh must report success=false");
            }
            other => panic!("expected TokenRefreshed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fresh_fallback_does_not_emit_token_refreshed() {
        // No refresh happens when the fallback token is still fresh, so we must NOT emit —
        // `token_refreshed` fires only on an actual refresh, not on every connection use.
        clear_token_cache();
        let recorder = Arc::new(RecordingSink::default());
        let events: crate::events::ConnectionEvents = Some(recorder.clone());
        let client = Client::new();

        let auth = DeferredAuth::OAuth2RefreshToken {
            cache_key: "rec-fallback-cache".to_string(),
            token_url: "https://example.com/token".to_string(),
            header_name: "Authorization".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            fallback_access_token: Some("existing".to_string()),
            fallback_expires_at: Some(Utc::now() + Duration::minutes(30)),
        };

        let resolved = resolve_deferred_auth(&client, auth, &events, "conn-1", "hubspot")
            .await
            .unwrap();
        assert_eq!(resolved.header_value, "Bearer existing");
        assert!(
            recorder.events.lock().unwrap().is_empty(),
            "a fresh fallback must not emit a token_refreshed event"
        );
    }

    #[tokio::test]
    async fn negative_cache_suppresses_immediate_second_refresh() {
        // Two back-to-back refreshes against an unreachable endpoint: the first performs
        // an actual (failing) refresh and emits one event; the second, within the cooldown,
        // inherits the cached error via the negative cache and does NOT re-hit the provider
        // (so no second event fires).
        clear_token_cache();
        let recorder = Arc::new(RecordingSink::default());
        let events: crate::events::ConnectionEvents = Some(recorder.clone());
        let client = Client::new();

        let make_auth = || DeferredAuth::OAuth2RefreshToken {
            cache_key: "neg-cache-test".to_string(),
            token_url: "http://127.0.0.1:1/token".to_string(),
            header_name: "Authorization".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            fallback_access_token: None,
            fallback_expires_at: None,
        };

        let first =
            resolve_deferred_auth(&client, make_auth(), &events, "conn-neg", "hubspot").await;
        assert!(first.is_err());
        let second =
            resolve_deferred_auth(&client, make_auth(), &events, "conn-neg", "hubspot").await;
        assert!(second.is_err());

        assert_eq!(
            recorder.events.lock().unwrap().len(),
            1,
            "the second refresh must be short-circuited by the negative cache"
        );
    }
}
