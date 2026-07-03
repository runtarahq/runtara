use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::middleware::tenant_auth::{CallerId, OrgId, Source};
use crate::product_events::{EventType, ProductEvent, ProductEventSink};

/// API key record (key_hash is never exposed via serde skip)
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct ApiKey {
    pub id: Uuid,
    pub org_id: String,
    pub name: String,
    pub key_prefix: String,
    #[serde(skip_serializing)]
    #[sqlx(default)]
    #[allow(dead_code)]
    #[schema(read_only)]
    pub key_hash: String,
    pub created_by: Option<String>,
    /// Auth0 `sub` of the user who owns the key. The key acts as this user: it inherits their
    /// current role from the tenant Valkey at validation time, and only they may read/revoke it.
    /// Required — every key has an owner (enforced by the NOT NULL column).
    pub issuing_user_id: String,
    /// Token identity — the `token:revoked:{jti}` revocation-denylist key. `None` for
    /// legacy rows.
    pub jti: Option<String>,
    #[schema(value_type = String)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = Option<String>)]
    pub expires_at: Option<DateTime<Utc>>,
    #[schema(value_type = Option<String>)]
    pub last_used_at: Option<DateTime<Utc>>,
    pub is_revoked: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateApiKeyRequest {
    /// Human-readable name for the key
    pub name: String,
    /// Optional expiration time
    #[schema(value_type = Option<String>)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateApiKeyResponse {
    #[serde(flatten)]
    pub api_key: ApiKey,
    /// The plaintext API key — shown only once, store it securely
    pub key: String,
}

fn sha256_hex(input: &str) -> String {
    hex::encode(sha2::Sha256::digest(input.as_bytes()))
}

/// `true` when an org id is safe to embed in a structured API key
/// (`rt_<org_id>_<random>`, SYN-524). Auth0-style ids (`org_xxx`) and any
/// `[A-Za-z0-9_-]+` id qualify; anything else (exotic OSS tenant ids) keeps
/// the legacy `rt_<random>` format so the key stays a single opaque token.
fn org_id_embeddable(org_id: &str) -> bool {
    !org_id.is_empty()
        && org_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Recover the embedded org id from a structured API key (SYN-524).
///
/// New-format keys are `rt_<org_id>_<random>` where `<random>` is strictly
/// alphanumeric (`[A-Za-z0-9]`, no underscores) — so even though org ids
/// themselves contain underscores (`org_xxx`), the org segment is exactly
/// recoverable via strip-prefix + `rsplit_once('_')`. Legacy keys
/// (`rt_<hex>`) contain no `_` after the prefix and yield `None`.
///
/// This is the reference implementation for edge-side routing (the shared
/// MCP hostname resolves a key to its tenant without a DB hit). Validation
/// does NOT use it — keys are still matched by full-string sha256 hash.
pub fn parse_org_segment(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("rt_")?;
    let (org, random) = rest.rsplit_once('_')?;
    if org.is_empty() || random.is_empty() || !random.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(org)
}

/// Create a new API key for the authenticated tenant.
/// The plaintext key is returned ONCE in the response — store it securely.
#[utoipa::path(
    post,
    path = "/api/runtime/api-keys",
    request_body = CreateApiKeyRequest,
    responses(
        (status = 201, description = "API key created", body = CreateApiKeyResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "api-keys-controller",
    security(("bearer_auth" = []))
)]
#[allow(clippy::too_many_arguments)]
pub async fn create_api_key(
    OrgId(tenant_id): OrgId,
    CallerId(user_id): CallerId,
    State(pool): State<PgPool>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Json(request): Json<CreateApiKeyRequest>,
) -> (StatusCode, Json<Value>) {
    let snapshot = crate::config::entitlements();
    match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM public.api_keys WHERE org_id = $1 AND is_revoked = false",
    )
    .bind(&tenant_id)
    .fetch_one(&pool)
    .await
    {
        Ok(current) => {
            if let Err(denial) = crate::middleware::entitlement::limit_decision(
                current as u64,
                snapshot.limits.max_api_keys,
                "maxApiKeys",
            ) {
                crate::product_events::emit_quota_exceeded(
                    &events,
                    ProductEvent::from_auth(EventType::QuotaExceeded, &ctx).source(source),
                    &denial,
                );
                return (StatusCode::FORBIDDEN, Json(denial.json_body()));
            }
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to enforce API key limit",
                    "message": e.to_string()
                })),
            );
        }
    }

    // The key inherits the creating user's identity: `issuing_user_id` drives the Valkey
    // role lookup at validation time, and `jti` is its revocation-denylist key. `created_by`
    // moves off the legacy hard-coded "jwt-user" to the real caller.
    let issuing_user_id = user_id;
    let created_by = issuing_user_id.clone();
    let jti = Uuid::new_v4().to_string();

    // Structured key format (SYN-524): embed the tenant org id so the edge
    // can route a key to its tenant without a DB hit. The random segment is
    // hex — strictly alphanumeric, no underscores — keeping the org segment
    // recoverable via `parse_org_segment` and entropy at 24 bytes (192 bits),
    // same as the legacy format. Validation is untouched: full-string sha256
    // lookup, and `rt_` still prefixes every key.
    let random_bytes: [u8; 24] = rand::random();
    let random_hex = hex::encode(random_bytes);
    let plaintext_key = if org_id_embeddable(&tenant_id) {
        format!("rt_{tenant_id}_{random_hex}")
    } else {
        // Exotic org id (can't round-trip through the structured format):
        // keep the legacy opaque shape.
        format!("rt_{random_hex}")
    };
    // Display prefix (VARCHAR(12) column). For structured keys the first 12
    // chars would be `rt_` + org id — identical for every key of the tenant —
    // so show the start of the random segment instead, which is what actually
    // disambiguates keys in a list. Legacy-format keys keep the old behavior.
    let key_prefix = if parse_org_segment(&plaintext_key).is_some() {
        format!("rt_{}", &random_hex[..9])
    } else {
        plaintext_key[..12].to_string()
    };
    let key_hash = sha256_hex(&plaintext_key);

    match sqlx::query_as::<_, ApiKey>(
        r#"
        INSERT INTO public.api_keys (org_id, name, key_prefix, key_hash, created_by, issuing_user_id, jti, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, org_id, name, key_prefix, key_hash, created_by, issuing_user_id, jti, created_at, expires_at, last_used_at, is_revoked
        "#,
    )
    .bind(&tenant_id)
    .bind(&request.name)
    .bind(key_prefix)
    .bind(&key_hash)
    .bind(&created_by)
    .bind(&issuing_user_id)
    .bind(&jti)
    .bind(request.expires_at)
    .fetch_one(&pool)
    .await
    {
        Ok(api_key) => {
            crate::audit::emit(
                &pool,
                &tenant_id,
                Some(&issuing_user_id),
                crate::audit::AuditEvent::new("token.create")
                    .resource("api_key", api_key.id.to_string()),
            )
            .await;
            events.emit(
                ProductEvent::from_auth(EventType::ApiKeyCreated, &ctx)
                    .resource(api_key.id.to_string(), "api_key")
                    .source(source),
            );
            let response = CreateApiKeyResponse {
                api_key,
                key: plaintext_key,
            };
            (StatusCode::CREATED, Json(serde_json::to_value(response).unwrap()))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to create API key", "message": e.to_string()})),
        ),
    }
}

/// List all API keys for the authenticated tenant.
/// Key hashes are never exposed.
#[utoipa::path(
    get,
    path = "/api/runtime/api-keys",
    responses(
        (status = 200, description = "List of API keys", body = [ApiKey]),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "api-keys-controller",
    security(("bearer_auth" = []))
)]
pub async fn list_api_keys(
    OrgId(tenant_id): OrgId,
    CallerId(user_id): CallerId,
    State(pool): State<PgPool>,
) -> (StatusCode, Json<Value>) {
    // An API key is a personal credential: a caller sees only the keys they issued, regardless
    // of role. Ownership — not a role permission — is the gate, so this scoping is unconditional.
    match sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT id, org_id, name, key_prefix, key_hash, created_by, issuing_user_id, jti, created_at, expires_at, last_used_at, is_revoked
        FROM public.api_keys
        WHERE org_id = $1 AND issuing_user_id = $2
        ORDER BY created_at DESC
        "#,
    )
    .bind(&tenant_id)
    .bind(&user_id)
    .fetch_all(&pool)
    .await
    {
        Ok(keys) => (StatusCode::OK, Json(serde_json::to_value(keys).unwrap())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to list API keys", "message": e.to_string()})),
        ),
    }
}

/// Revoke an API key. The key can no longer be used for authentication.
#[utoipa::path(
    delete,
    path = "/api/runtime/api-keys/{id}",
    params(
        ("id" = Uuid, Path, description = "API key ID")
    ),
    responses(
        (status = 204, description = "API key revoked"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "API key not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "api-keys-controller",
    security(("bearer_auth" = []))
)]
#[allow(clippy::too_many_arguments)]
pub async fn revoke_api_key(
    OrgId(tenant_id): OrgId,
    CallerId(user_id): CallerId,
    State(pool): State<PgPool>,
    State(valkey): State<Option<redis::aio::ConnectionManager>>,
    State(events): State<ProductEventSink>,
    Extension(ctx): Extension<AuthContext>,
    Source(source): Source,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    // A caller may revoke only a key they issued. Scoping the mutation by `issuing_user_id`
    // makes ownership the gate atomically: another user's key simply isn't matched, so it reads
    // as 404 (not found) rather than leaking its existence.
    let revoked = sqlx::query_as::<_, (Option<String>, Option<DateTime<Utc>>)>(
        r#"
        UPDATE public.api_keys
        SET is_revoked = TRUE
        WHERE id = $1 AND org_id = $2 AND issuing_user_id = $3
        RETURNING jti, expires_at
        "#,
    )
    .bind(id)
    .bind(&tenant_id)
    .bind(&user_id)
    .fetch_optional(&pool)
    .await;

    match revoked {
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "API key not found"})),
        ),
        Ok(Some((jti, expires_at))) => {
            // Propagate the revocation to the tenant's Valkey denylist so it takes effect
            // immediately across the contract. The DB `is_revoked` flag is the authoritative
            // block for `rt_*` keys, so a Valkey failure here is logged, not fatal.
            if let (Some(jti), Some(manager)) = (jti.as_deref(), valkey.as_ref()) {
                let ttl = expires_at.map(|exp| (exp - Utc::now()).num_seconds().max(0) as u64);
                if let Err(e) = crate::valkey::auth::revoke_token(manager, jti, ttl).await {
                    tracing::warn!(error = %e, "failed to write token revocation to Valkey");
                }
            }
            crate::audit::emit(
                &pool,
                &tenant_id,
                Some(&user_id),
                crate::audit::AuditEvent::new("token.revoke").resource("api_key", id.to_string()),
            )
            .await;
            events.emit(
                ProductEvent::from_auth(EventType::ApiKeyRevoked, &ctx)
                    .resource(id.to_string(), "api_key")
                    .source(source),
            );
            (StatusCode::NO_CONTENT, Json(json!(null)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to revoke API key", "message": e.to_string()})),
        ),
    }
}

/// Validate an API key by hash. Returns the ApiKey if valid.
/// Called by the auth middleware — not an HTTP handler.
pub async fn validate_api_key_by_hash(pool: &PgPool, key_hash: &str) -> Result<ApiKey, String> {
    sqlx::query_as::<_, ApiKey>(
        r#"
        UPDATE public.api_keys
        SET last_used_at = NOW()
        WHERE key_hash = $1
          AND is_revoked = FALSE
          AND (expires_at IS NULL OR expires_at > NOW())
        RETURNING id, org_id, name, key_prefix, key_hash, created_by, issuing_user_id, jti, created_at, expires_at, last_used_at, is_revoked
        "#,
    )
    .bind(key_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Database error: {e}"))?
    .ok_or_else(|| "Invalid or expired API key".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_org_segment: reference implementation for edge routing ────

    #[test]
    fn new_format_recovers_org_id() {
        assert_eq!(
            parse_org_segment("rt_org_abc123XYZ_9f8e7d6c5b4a39281706f5e4d3c2b1a0"),
            Some("org_abc123XYZ")
        );
    }

    #[test]
    fn org_id_with_underscores_round_trips() {
        // Org ids contain underscores (`org_xxx`, or even multiple). Because
        // the random segment is strictly alphanumeric, rsplit_once('_')
        // always cuts at the org/random boundary.
        let org = "org_multi_part_id";
        let key = format!("rt_{org}_{}", "a".repeat(48));
        assert_eq!(parse_org_segment(&key), Some(org));
    }

    #[test]
    fn legacy_format_yields_none() {
        // Legacy keys are rt_ + 48 hex chars: no '_' after the prefix.
        let key = format!("rt_{}", "0123456789abcdef".repeat(3));
        assert_eq!(parse_org_segment(&key), None);
    }

    #[test]
    fn non_rt_prefixes_yield_none() {
        assert_eq!(parse_org_segment("smo_org_abc_random123"), None);
        assert_eq!(parse_org_segment("Bearer whatever"), None);
        assert_eq!(parse_org_segment(""), None);
    }

    #[test]
    fn degenerate_shapes_yield_none() {
        // Empty org / empty random / non-alphanumeric random must not parse.
        assert_eq!(parse_org_segment("rt__random123"), None);
        assert_eq!(parse_org_segment("rt_org_abc_"), None);
        // Random segment containing a non-alphanumeric char: the whole tail
        // fails the charset check.
        assert_eq!(parse_org_segment("rt_org_abc_rand-om"), None);
    }

    #[test]
    fn generated_shape_round_trips_end_to_end() {
        // Mirror of the generation logic in create_api_key.
        let tenant_id = "org_pDq7kM2xL9aBcDeF";
        assert!(org_id_embeddable(tenant_id));
        let random_bytes: [u8; 24] = rand::random();
        let random_hex = hex::encode(random_bytes);
        let key = format!("rt_{tenant_id}_{random_hex}");

        assert_eq!(parse_org_segment(&key), Some(tenant_id));
        // Middleware routing still recognizes it as an rt_ key.
        assert!(key.starts_with("rt_"));
    }

    // ── org_id_embeddable: legacy fallback for exotic tenant ids ────────

    #[test]
    fn exotic_org_ids_are_not_embedded() {
        assert!(!org_id_embeddable(""));
        assert!(!org_id_embeddable("org id with spaces"));
        assert!(!org_id_embeddable("org/slash"));
        assert!(!org_id_embeddable("org.dot"));
        assert!(!org_id_embeddable("orgé"));
    }

    #[test]
    fn typical_org_ids_are_embedded() {
        assert!(org_id_embeddable("org_pDq7kM2xL9aBcDeF"));
        assert!(org_id_embeddable("local"));
        assert!(org_id_embeddable("tenant-42"));
    }
}
