use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;

pub(crate) const TOKEN_REFRESH_MARGIN_SECS: i64 = 300;
pub(crate) const DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone)]
pub(crate) struct CachedAccessToken {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
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
) -> Result<(String, String), String> {
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
            Ok((header_name, header_value))
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
                return Ok((header_name, format!("Bearer {}", access_token)));
            }

            let token = resolve_cached_token(&cache_key, || async {
                refresh_oauth_access_token(
                    client,
                    &token_url,
                    &client_id,
                    &client_secret,
                    &refresh_token,
                )
                .await
            })
            .await?;
            Ok((header_name, format!("Bearer {}", token)))
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

    parse_token_response(response, default_ttl_seconds).await
}

async fn refresh_oauth_access_token(
    client: &Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<CachedAccessToken, String> {
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
) -> Result<CachedAccessToken, String> {
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    if !status.is_success() {
        return Err(format!("Token endpoint returned {}: {}", status, body));
    }

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| format!("Token response missing access_token field: {}", body))?
        .to_string();

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

    Ok(CachedAccessToken {
        access_token,
        expires_at,
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

        let resolved = resolve_deferred_auth(&client, auth).await.unwrap();
        assert_eq!(resolved.0, "Authorization");
        assert_eq!(resolved.1, "Bearer existing-token");
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
}
