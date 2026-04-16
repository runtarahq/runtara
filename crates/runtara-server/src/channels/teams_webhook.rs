use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::session::{ChannelRouter, InboundMessage};

/// Cached JWKS keys for Bot Framework JWT validation.
struct JwksCache {
    keys: Vec<JwkKey>,
    fetched_at: std::time::Instant,
}

#[derive(Debug, Clone, Deserialize)]
struct JwkKey {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Deserialize)]
struct OpenIdConfig {
    jwks_uri: String,
}

/// Global JWKS cache (refreshed every 6 hours).
static JWKS_CACHE: std::sync::LazyLock<RwLock<Option<JwksCache>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

const JWKS_REFRESH_SECS: u64 = 6 * 3600;
const OPENID_CONFIG_URL: &str =
    "https://login.botframework.com/v1/.well-known/openid-configuration";

/// Microsoft Teams Bot Framework webhook handler.
///
/// Receives Activity objects from the Bot Framework. Validates the JWT
/// bearer token in the Authorization header against Microsoft's JWKS.
///
/// POST /api/runtime/events/webhook/teams/{connection_id}
pub async fn teams_webhook(
    State(router): State<Arc<ChannelRouter>>,
    Path(connection_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let activity_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

    // Ignore non-message activities (conversationUpdate, typing, etc.)
    if activity_type != "message" {
        return StatusCode::OK.into_response();
    }

    // Load connection to get app_id for JWT validation.
    let app_id = match load_app_id(&router, &connection_id).await {
        Ok(id) => id,
        Err(e) => {
            warn!(error = %e, "Failed to load Teams connection");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    // Validate JWT bearer token.
    if let Err(e) = validate_teams_jwt(&headers, &app_id).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Teams JWT validation failed"
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Extract message fields.
    let conversation_id = payload
        .get("conversation")
        .and_then(|c| c.get("id"))
        .and_then(|id| id.as_str());
    let text = payload.get("text").and_then(|t| t.as_str());
    let service_url = payload.get("serviceUrl").and_then(|s| s.as_str());

    let (Some(conversation_id), Some(text)) = (conversation_id, text) else {
        return StatusCode::OK.into_response();
    };

    // Store the service URL so the channel adapter can send replies.
    if let Some(svc_url) = service_url {
        router.set_teams_service_url(conversation_id, svc_url);
    }

    // Strip bot mentions from text (Teams includes <at>BotName</at> in text).
    let clean_text = strip_teams_mentions(text);
    let clean_text = clean_text.trim();
    if clean_text.is_empty() {
        return StatusCode::OK.into_response();
    }

    let sender_id = payload
        .get("from")
        .and_then(|f| f.get("id"))
        .and_then(|id| id.as_str())
        .unwrap_or(conversation_id)
        .to_string();

    let msg = InboundMessage {
        text: clean_text.to_string(),
        sender_id,
        conv_id: conversation_id.to_string(),
        channel: "teams".into(),
        attachments: vec![],
        original_message: payload.clone(),
    };

    debug!(
        connection_id = %connection_id,
        conversation = %conversation_id,
        "Teams message received"
    );

    if let Err(e) = router.handle_message(&connection_id, &msg).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Failed to handle Teams message"
        );
    }

    StatusCode::OK.into_response()
}

/// Validate the JWT bearer token from the Authorization header.
async fn validate_teams_jwt(headers: &HeaderMap, app_id: &str) -> anyhow::Result<()> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("Missing Authorization header"))?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow::anyhow!("Invalid Authorization header format"))?;

    // Decode the JWT header to get the key ID.
    let header = decode_header(token).map_err(|e| anyhow::anyhow!("Invalid JWT header: {}", e))?;
    let kid = header
        .kid
        .ok_or_else(|| anyhow::anyhow!("JWT has no kid"))?;

    // Get the matching JWK.
    let jwk = get_jwk(&kid).await?;
    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|e| anyhow::anyhow!("Invalid RSA key: {}", e))?;

    // Validate the JWT.
    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[app_id]);
    // Bot Framework tokens can have multiple issuers.
    validation.set_issuer(&[
        "https://api.botframework.com",
        "https://sts.windows.net/d6d49420-f39b-4df7-a1dc-d59a935871db/",
        "https://login.microsoftonline.com/d6d49420-f39b-4df7-a1dc-d59a935871db/v2.0",
    ]);

    decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| anyhow::anyhow!("JWT validation failed: {}", e))?;

    Ok(())
}

/// Get a JWK by key ID, fetching/refreshing the JWKS cache as needed.
async fn get_jwk(kid: &str) -> anyhow::Result<JwkKey> {
    // Try cache first.
    {
        let cache = JWKS_CACHE.read().await;
        if let Some(ref c) = *cache
            && c.fetched_at.elapsed().as_secs() < JWKS_REFRESH_SECS
            && let Some(key) = c.keys.iter().find(|k| k.kid == kid)
        {
            return Ok(key.clone());
        }
    }

    // Refresh cache.
    let client = reqwest::Client::new();

    let openid: OpenIdConfig = client.get(OPENID_CONFIG_URL).send().await?.json().await?;

    let jwks: JwksResponse = client.get(&openid.jwks_uri).send().await?.json().await?;

    let key = jwks
        .keys
        .iter()
        .find(|k| k.kid == kid)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("JWK not found for kid: {}", kid))?;

    {
        let mut cache = JWKS_CACHE.write().await;
        *cache = Some(JwksCache {
            keys: jwks.keys,
            fetched_at: std::time::Instant::now(),
        });
    }

    Ok(key)
}

/// Load the app_id from the Teams connection.
async fn load_app_id(router: &ChannelRouter, connection_id: &str) -> anyhow::Result<String> {
    let expected_tenant = crate::config::tenant_id();
    let conn = router
        .connections()
        .get_with_parameters(connection_id, expected_tenant)
        .await
        .map_err(|e| anyhow::anyhow!("DB error: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Connection not found"))?;

    let params = conn
        .connection_parameters
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Connection has no parameters"))?;

    params["app_id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing app_id"))
}

/// Strip Teams @mention markup (`<at>BotName</at>`) from message text.
fn strip_teams_mentions(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' && chars.peek() == Some(&'a') {
            // Check for <at> tag
            let mut tag = String::from('<');
            let mut found_at = false;
            for inner in chars.by_ref() {
                tag.push(inner);
                if inner == '>' {
                    if tag.starts_with("<at>") || tag.starts_with("</at>") {
                        found_at = true;
                    }
                    break;
                }
            }
            if found_at {
                // Skip content between <at> and </at>
                if tag.starts_with("<at>") {
                    // Consume until </at>
                    let mut depth = String::new();
                    for inner in chars.by_ref() {
                        depth.push(inner);
                        if depth.ends_with("</at>") {
                            break;
                        }
                    }
                }
                // </at> by itself — already consumed
            } else {
                // Not an <at> tag, keep the text
                result.push_str(&tag);
            }
        } else {
            result.push(c);
        }
    }

    result
}
