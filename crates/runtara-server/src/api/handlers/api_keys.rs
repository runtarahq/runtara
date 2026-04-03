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

use crate::middleware::tenant_auth::OrgId;

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
    State(pool): State<PgPool>,
    Json(request): Json<CreateApiKeyRequest>,
) -> (StatusCode, Json<Value>) {
    let created_by = Some("jwt-user".to_string());

    let random_bytes: [u8; 24] = rand::random();
    let random_hex = hex::encode(random_bytes);
    let plaintext_key = format!("rt_{}", random_hex);
    let key_prefix = &plaintext_key[..12];
    let key_hash = sha256_hex(&plaintext_key);

    match sqlx::query_as::<_, ApiKey>(
        r#"
        INSERT INTO public.api_keys (org_id, name, key_prefix, key_hash, created_by, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, org_id, name, key_prefix, key_hash, created_by, created_at, expires_at, last_used_at, is_revoked
        "#,
    )
    .bind(&tenant_id)
    .bind(&request.name)
    .bind(key_prefix)
    .bind(&key_hash)
    .bind(created_by.as_deref())
    .bind(request.expires_at)
    .fetch_one(&pool)
    .await
    {
        Ok(api_key) => {
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
    State(pool): State<PgPool>,
) -> (StatusCode, Json<Value>) {
    match sqlx::query_as::<_, ApiKey>(
        r#"
        SELECT id, org_id, name, key_prefix, key_hash, created_by, created_at, expires_at, last_used_at, is_revoked
        FROM public.api_keys
        WHERE org_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(&tenant_id)
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
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match sqlx::query(
        r#"
        UPDATE public.api_keys
        SET is_revoked = TRUE
        WHERE id = $1 AND org_id = $2
        "#,
    )
    .bind(id)
    .bind(&tenant_id)
    .execute(&pool)
    .await
    {
        Ok(result) => {
            if result.rows_affected() == 0 {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "API key not found"})),
                )
            } else {
                (StatusCode::NO_CONTENT, Json(json!(null)))
            }
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
        RETURNING id, org_id, name, key_prefix, key_hash, created_by, created_at, expires_at, last_used_at, is_revoked
        "#,
    )
    .bind(key_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Database error: {e}"))?
    .ok_or_else(|| "Invalid or expired API key".to_string())
}
