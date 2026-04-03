use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use reqwest::Client;
use runtara_dsl::agent_meta::find_connection_type;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::OnceLock;

pub struct ResolvedConnectionAuth {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
}

pub struct AwsSigningParams {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub service: String,
    pub session_token: Option<String>,
}

#[derive(Clone)]
struct CachedAccessToken {
    access_token: String,
    expires_at: Option<DateTime<Utc>>,
}

enum TokenRequestBody {
    Json(Value),
}

enum DeferredAuth {
    OAuth2ClientCredentials {
        cache_key: String,
        token_url: String,
        header_name: String,
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

struct ConnectionAuthDescriptor {
    base_url: Option<String>,
    aws_signing: Option<AwsSigningParams>,
    deferred_auth: Option<DeferredAuth>,
}

const TOKEN_REFRESH_MARGIN_SECS: i64 = 300;
const DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS: i64 = 24 * 60 * 60;
const SHOPIFY_SCOPE_FIELDS: &[(&str, &str)] = &[
    ("scope_read_products", "read_products"),
    ("scope_write_products", "write_products"),
    ("scope_read_orders", "read_orders"),
    ("scope_write_orders", "write_orders"),
    ("scope_read_inventory", "read_inventory"),
    ("scope_write_inventory", "write_inventory"),
    ("scope_read_locations", "read_locations"),
    ("scope_read_customers", "read_customers"),
    ("scope_write_customers", "write_customers"),
    ("scope_read_fulfillments", "read_fulfillments"),
    ("scope_write_fulfillments", "write_fulfillments"),
];

static TOKEN_CACHE: OnceLock<DashMap<String, CachedAccessToken>> = OnceLock::new();

pub async fn resolve_connection_auth(
    client: &Client,
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
) -> Result<ResolvedConnectionAuth, String> {
    let descriptor = describe_connection_auth(connection_id, integration_id, params, headers);

    if let Some(deferred_auth) = descriptor.deferred_auth {
        let resolved = resolve_deferred_auth(client, deferred_auth).await?;
        headers.insert(resolved.0, resolved.1);
    }

    Ok(ResolvedConnectionAuth {
        base_url: descriptor.base_url,
        aws_signing: descriptor.aws_signing,
    })
}

fn describe_connection_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
) -> ConnectionAuthDescriptor {
    match integration_id {
        "openai_api_key" => {
            if let Some(key) = params["api_key"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.openai.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "shopify_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("X-Shopify-Access-Token".into(), token.to_string());
            }
            ConnectionAuthDescriptor {
                base_url: params["shop_domain"]
                    .as_str()
                    .map(|domain| format!("https://{}", domain)),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "shopify_client_credentials" => ConnectionAuthDescriptor {
            base_url: params["shop_domain"]
                .as_str()
                .map(|domain| format!("https://{}", domain)),
            aws_signing: None,
            deferred_auth: describe_shopify_client_credentials_auth(connection_id, params),
        },
        "hubspot_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.hubapi.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "hubspot_private_app" => {
            if let Some(auth) =
                describe_oauth_refresh_auth(connection_id, integration_id, params, "Authorization")
            {
                ConnectionAuthDescriptor {
                    base_url: Some("https://api.hubapi.com".into()),
                    aws_signing: None,
                    deferred_auth: Some(auth),
                }
            } else {
                if let Some(token) = params["access_token"].as_str() {
                    headers.insert("Authorization".into(), format!("Bearer {}", token));
                }
                ConnectionAuthDescriptor {
                    base_url: Some("https://api.hubapi.com".into()),
                    aws_signing: None,
                    deferred_auth: None,
                }
            }
        }
        "stripe_api_key" => {
            if let Some(key) = params["secret_key"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.stripe.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "slack_bot" => {
            if let Some(token) = params["bot_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://slack.com".into()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "mailgun" => {
            if let Some(key) = params["api_key"].as_str() {
                let encoded = BASE64.encode(format!("api:{}", key));
                headers.insert("Authorization".into(), format!("Basic {}", encoded));
            }
            let region = params["region"].as_str().unwrap_or("us");
            let base = match region {
                "eu" => "https://api.eu.mailgun.net".to_string(),
                _ => "https://api.mailgun.net".to_string(),
            };
            ConnectionAuthDescriptor {
                base_url: Some(base),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        // ── HTTP Bearer token ────────────────────────────────────
        "http_bearer" => {
            if let Some(token) = params["token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        // ── HTTP API Key ────────────────────────────────────────
        "http_api_key" => {
            if let Some(key) = params["api_key"].as_str() {
                let header_name = params["header_name"]
                    .as_str()
                    .unwrap_or("X-API-Key")
                    .to_string();
                headers.insert(header_name, key.to_string());
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
        "aws_credentials" | "s3_compatible" => {
            let access_key_id = params["access_key_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let secret_access_key = params["secret_access_key"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let region = params["region"].as_str().unwrap_or("us-east-1").to_string();
            let session_token = params["session_token"].as_str().map(|s| s.to_string());

            let (base_url, service) = if integration_id == "s3_compatible" {
                let endpoint = params["endpoint"]
                    .as_str()
                    .map(normalize_endpoint)
                    .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", region));
                (Some(endpoint), "s3".to_string())
            } else {
                let svc = params["service"].as_str().unwrap_or("s3").to_string();
                let base = params["endpoint"].as_str().map(normalize_endpoint);
                (base, svc)
            };

            ConnectionAuthDescriptor {
                base_url,
                aws_signing: Some(AwsSigningParams {
                    access_key_id,
                    secret_access_key,
                    region,
                    service,
                    session_token,
                }),
                deferred_auth: None,
            }
        }
        _ => {
            if let Some(key) = params["api_key"].as_str() {
                headers
                    .entry("Authorization".into())
                    .or_insert_with(|| format!("Bearer {}", key));
            } else if let Some(token) = params["access_token"].as_str() {
                headers
                    .entry("Authorization".into())
                    .or_insert_with(|| format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                deferred_auth: None,
            }
        }
    }
}

fn describe_shopify_client_credentials_auth(
    connection_id: &str,
    params: &Value,
) -> Option<DeferredAuth> {
    let shop_domain = params["shop_domain"].as_str()?.trim_end_matches('/');
    let client_id = params["client_id"].as_str()?;
    let client_secret = params["client_secret"].as_str()?;

    let scopes = collect_shopify_scopes(params);
    let mut body = serde_json::Map::new();
    body.insert(
        "client_id".to_string(),
        Value::String(client_id.to_string()),
    );
    body.insert(
        "client_secret".to_string(),
        Value::String(client_secret.to_string()),
    );
    body.insert(
        "grant_type".to_string(),
        Value::String("client_credentials".to_string()),
    );
    if !scopes.is_empty() {
        body.insert("scope".to_string(), Value::String(scopes.clone()));
    }

    Some(DeferredAuth::OAuth2ClientCredentials {
        cache_key: build_token_cache_key(&[
            "shopify_client_credentials",
            connection_id,
            shop_domain,
            &scopes,
        ]),
        token_url: format!("https://{shop_domain}/admin/oauth/access_token"),
        header_name: "X-Shopify-Access-Token".to_string(),
        request_body: TokenRequestBody::Json(Value::Object(body)),
        default_ttl_seconds: DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
    })
}

fn describe_oauth_refresh_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    header_name: &str,
) -> Option<DeferredAuth> {
    let refresh_token = params["refresh_token"].as_str()?.to_string();
    let client_id = params["client_id"].as_str()?.to_string();
    let client_secret = params["client_secret"].as_str()?.to_string();
    let token_url = find_connection_type(integration_id)?
        .oauth_config
        .map(|config| config.token_url.to_string())?;

    Some(DeferredAuth::OAuth2RefreshToken {
        cache_key: build_token_cache_key(&["oauth_refresh", connection_id, integration_id]),
        token_url,
        header_name: header_name.to_string(),
        client_id,
        client_secret,
        refresh_token,
        fallback_access_token: params["access_token"].as_str().map(|s| s.to_string()),
        fallback_expires_at: parse_expiry(params["token_expires_at"].as_str()),
    })
}

async fn resolve_deferred_auth(
    client: &Client,
    auth: DeferredAuth,
) -> Result<(String, String), String> {
    match auth {
        DeferredAuth::OAuth2ClientCredentials {
            cache_key,
            token_url,
            header_name,
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
            Ok((header_name, token))
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

fn collect_shopify_scopes(params: &Value) -> String {
    SHOPIFY_SCOPE_FIELDS
        .iter()
        .filter(|(field, _)| params[*field].as_bool().unwrap_or(false))
        .map(|(_, scope)| *scope)
        .collect::<Vec<_>>()
        .join(",")
}

fn build_token_cache_key(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

fn parse_expiry(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn token_is_fresh(expires_at: Option<DateTime<Utc>>) -> bool {
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

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("https://{}", endpoint)
    }
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
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn collect_shopify_scopes_keeps_enabled_scopes_only() {
        let params = json!({
            "scope_read_products": true,
            "scope_write_products": false,
            "scope_read_inventory": true
        });

        assert_eq!(
            collect_shopify_scopes(&params),
            "read_products,read_inventory"
        );
    }

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
}
