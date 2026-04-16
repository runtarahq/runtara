//! Service facade for `runtara-connections`.
//!
//! Provides a single entry point for all connection operations,
//! usable without going through HTTP. This is the primary integration
//! surface for host applications and other crates.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::auth::provider_auth::{self, ResolvedConnectionAuth};
use crate::config::ConnectionsState;
use crate::error::ConnectionsError;
use crate::repository::connections::{ConnectionRepository, ConnectionWithParameters};
use crate::service::rate_limits::RateLimitService;
use crate::types::{ConnectionDto, RateLimitEventType};

/// Facade over all connection-domain operations.
///
/// Construct once during application startup and share via `Arc`.
/// Internally delegates to repository/service layer per call.
#[derive(Clone)]
pub struct ConnectionsFacade {
    state: ConnectionsState,
}

impl ConnectionsFacade {
    pub fn new(state: ConnectionsState) -> Self {
        Self { state }
    }

    // ── Repository helpers (cheap to construct per call) ─────────────────

    fn repo(&self) -> ConnectionRepository {
        ConnectionRepository::new(self.state.db_pool.clone(), self.state.cipher.clone())
    }

    fn rate_limit_service(&self) -> RateLimitService {
        let repo = Arc::new(self.repo());
        RateLimitService::with_redis_url_and_db_pool(
            repo,
            self.state.redis_url.clone(),
            self.state.db_pool.clone(),
        )
    }

    // ── Metadata (no secrets) ───────────────────────────────────────────

    /// Get a single connection by ID (no secrets).
    pub async fn get_connection(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<Option<ConnectionDto>, ConnectionsError> {
        self.repo()
            .get_by_id(id, tenant_id)
            .await
            .map_err(ConnectionsError::Database)
    }

    /// List connections for a tenant (no secrets).
    pub async fn list_connections(
        &self,
        tenant_id: &str,
        integration_id: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<ConnectionDto>, ConnectionsError> {
        self.repo()
            .list(tenant_id, integration_id, status)
            .await
            .map_err(ConnectionsError::Database)
    }

    // ── With secrets (internal use only) ────────────────────────────────

    /// Get a connection including secret parameters.
    ///
    /// SECURITY: Only use for internal runtime credential resolution.
    pub async fn get_with_parameters(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<Option<ConnectionWithParameters>, ConnectionsError> {
        self.repo()
            .get_with_parameters(id, tenant_id)
            .await
            .map_err(ConnectionsError::Database)
    }

    /// Get a connection by ID without tenant filter (for webhook routing).
    ///
    /// SECURITY: Used only by channel webhook routing where the connection_id
    /// in the URL acts as the authentication token.
    pub async fn get_channel_connection(
        &self,
        id: &str,
    ) -> Result<Option<ConnectionWithParameters>, ConnectionsError> {
        self.repo()
            .get_channel_connection(id)
            .await
            .map_err(ConnectionsError::Database)
    }

    /// Get the default file storage connection for a tenant.
    ///
    /// SECURITY: Returns sensitive credentials. Internal use only.
    pub async fn get_default_file_storage(
        &self,
        tenant_id: &str,
    ) -> Result<Option<ConnectionWithParameters>, ConnectionsError> {
        self.repo()
            .get_default_file_storage(tenant_id)
            .await
            .map_err(ConnectionsError::Database)
    }

    // ── Validation ──────────────────────────────────────────────────────

    /// Check which connection IDs exist for a tenant.
    pub async fn get_existing_ids(
        &self,
        tenant_id: &str,
        connection_ids: &[String],
    ) -> Result<HashSet<String>, ConnectionsError> {
        self.repo()
            .get_existing_ids(tenant_id, connection_ids)
            .await
            .map_err(ConnectionsError::Database)
    }

    // ── Migration ──────────────────────────────────────────────────────

    /// Whether the configured cipher actually encrypts data at rest.
    ///
    /// Returns `false` when the crate is running with [`crate::crypto::noop::NoOpCipher`].
    pub fn is_encryption_enabled(&self) -> bool {
        self.state.cipher.is_encrypting()
    }

    /// Re-encrypt every connection row (optionally scoped to one tenant) with
    /// the current cipher. Idempotent; safe to call repeatedly.
    ///
    /// Use this after enabling encryption for the first time, or after
    /// rotating the encryption key.
    pub async fn reencrypt_all(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<crate::repository::connections::ReencryptionStats, ConnectionsError> {
        self.repo()
            .reencrypt_all(tenant_id)
            .await
            .map_err(ConnectionsError::Database)
    }

    // ── Auth resolution ─────────────────────────────────────────────────

    /// Resolve connection credentials and inject auth into headers.
    ///
    /// Handles per-integration auth logic, OAuth token refresh,
    /// client credentials flow, and token caching.
    pub async fn resolve_connection_auth(
        &self,
        connection_id: &str,
        integration_id: &str,
        params: &Value,
        headers: &mut HashMap<String, String>,
    ) -> Result<ResolvedConnectionAuth, ConnectionsError> {
        provider_auth::resolve_connection_auth(
            &self.state.http_client,
            connection_id,
            integration_id,
            params,
            headers,
        )
        .await
        .map_err(ConnectionsError::AuthResolution)
    }

    // ── Rate limiting ───────────────────────────────────────────────────

    /// Record a credential request event for analytics tracking.
    pub async fn record_credential_request(
        &self,
        connection_id: &str,
        tenant_id: &str,
        event_type: &RateLimitEventType,
        metadata: Option<Value>,
    ) -> Result<(), ConnectionsError> {
        self.rate_limit_service()
            .record_credential_request(connection_id, tenant_id, event_type, metadata)
            .await
            .map_err(|e| ConnectionsError::Internal(format!("{:?}", e)))
    }

    /// Check the token bucket for a connection and consume a token.
    ///
    /// Returns `Ok(())` if the request is allowed.
    /// Returns `Err(retry_after_ms)` if the caller should wait.
    pub async fn check_rate_limit(
        &self,
        connection_id: &str,
        rate_limit_config: &Option<Value>,
    ) -> Result<(), u64> {
        let config: crate::types::RateLimitConfigDto = match rate_limit_config {
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(c) => c,
                Err(_) => return Ok(()),
            },
            None => return Ok(()),
        };

        if config.requests_per_second == 0 {
            return Ok(());
        }

        let redis_url = match &self.state.redis_url {
            Some(url) => url,
            None => return Ok(()), // No Redis — fail open
        };

        let client = match redis::Client::open(redis_url.as_str()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    connection_id = connection_id,
                    error = %e,
                    "Redis connect failed for rate limit check — allowing request"
                );
                return Ok(());
            }
        };

        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    connection_id = connection_id,
                    error = %e,
                    "Redis connection failed for rate limit check — allowing request"
                );
                return Ok(());
            }
        };

        let key = format!("rate_limit:{}", connection_id);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let result: Result<i64, redis::RedisError> = redis::Script::new(TOKEN_BUCKET_LUA)
            .key(&key)
            .arg(config.requests_per_second)
            .arg(config.burst_size)
            .arg(now_ms)
            .invoke_async(&mut conn)
            .await;

        match result {
            Ok(v) if v > 0 => Ok(()),
            Ok(v) => {
                let retry_after_ms = (-v) as u64;
                Err(retry_after_ms)
            }
            Err(e) => {
                tracing::warn!(
                    connection_id = connection_id,
                    error = %e,
                    "Token bucket Lua script failed — allowing request"
                );
                Ok(())
            }
        }
    }
}

/// Lua script for atomic token bucket check-and-decrement.
const TOKEN_BUCKET_LUA: &str = r#"
local key = KEYS[1]
local rps = tonumber(ARGV[1])
local burst = tonumber(ARGV[2])
local now_ms = tonumber(ARGV[3])

local tokens = tonumber(redis.call('hget', key, 'tokens') or burst)
local last_refill = tonumber(redis.call('hget', key, 'last_refill') or now_ms)

-- Refill tokens based on elapsed time
local elapsed_ms = now_ms - last_refill
if elapsed_ms > 0 then
    local refill = (elapsed_ms / 1000.0) * rps
    tokens = math.min(burst, tokens + refill)
end

if tokens >= 1 then
    tokens = tokens - 1
    redis.call('hset', key, 'tokens', tostring(tokens))
    redis.call('hset', key, 'last_refill', tostring(now_ms))
    return 1
else
    redis.call('hset', key, 'tokens', tostring(tokens))
    redis.call('hset', key, 'last_refill', tostring(now_ms))
    -- Compute wait time until one token is available
    local wait_ms = math.ceil((1 - tokens) / rps * 1000)
    return -wait_ms
end
"#;
