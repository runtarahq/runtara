use axum::{
    extract::{Path, State},
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

use crate::middleware::tenant_auth::{CallerId, OrgId};

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
pub async fn create_api_key(
    OrgId(tenant_id): OrgId,
    CallerId(user_id): CallerId,
    State(pool): State<PgPool>,
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

    let random_bytes: [u8; 24] = rand::random();
    let random_hex = hex::encode(random_bytes);
    let plaintext_key = format!("rt_{}", random_hex);
    let key_prefix = &plaintext_key[..12];
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
pub async fn revoke_api_key(
    OrgId(tenant_id): OrgId,
    CallerId(user_id): CallerId,
    State(pool): State<PgPool>,
    State(valkey): State<Option<redis::aio::ConnectionManager>>,
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
