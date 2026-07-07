//! Service facade for `runtara-connections`.
//!
//! Provides a single entry point for all connection operations,
//! usable without going through HTTP. This is the primary integration
//! surface for host applications and other crates.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock};

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::Counter;
use serde_json::Value;

use crate::auth::provider_auth::{self, ResolvedConnectionAuth, RotatedCredentials};
use crate::auth::token_cache;
use crate::config::ConnectionsState;
use crate::error::ConnectionsError;
use crate::repository::connections::{ConnectionRepository, ConnectionWithParameters};
use crate::service::rate_limits::RateLimitService;
use crate::types::{ConnectionDto, ConnectionStatus, CreateConnectionRequest, RateLimitEventType};

/// Facade over all connection-domain operations.
///
/// Construct once during application startup and share via `Arc`.
/// Internally delegates to repository/service layer per call.
#[derive(Clone)]
pub struct ConnectionsFacade {
    state: ConnectionsState,
}

// ── Rate-limit fail-open observability ───────────────────────────────────────
// `check_rate_limit` deliberately *fails open* (allows the request) when
// rate-limit tracking is unavailable, so a Redis/Valkey outage never blocks
// egress. But a *silent* fail-open means a tenant's configured limits can be off
// indefinitely with nothing to alert on — discovered only after a provider
// retaliates. Count every fail-open and warn (throttled) when it happens.

/// Total requests allowed without rate-limit enforcement, by `reason`.
static FAIL_OPEN_COUNTER: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("runtara-connections")
        .u64_counter("runtara.rate_limit.fail_open.total")
        .with_description(
            "Requests allowed without rate-limit enforcement because rate-limit tracking was unavailable",
        )
        .build()
});

/// Warn at most once per process per this interval — `check_rate_limit` runs on
/// every proxied request, so an unthrottled warn would flood the logs during a
/// sustained outage. The process is per-tenant, so a process-global throttle is
/// the correct (tenant-wide) granularity.
const FAIL_OPEN_WARN_INTERVAL_MS: i64 = 60_000;
static LAST_FAIL_OPEN_WARN_MS: AtomicI64 = AtomicI64::new(0);

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Claim the warn slot at most once per `interval_ms`. Pure + testable; the CAS
/// makes concurrent callers race to a single winner.
fn claim_warn_slot(last_warn_ms: &AtomicI64, now_ms: i64, interval_ms: i64) -> bool {
    let last = last_warn_ms.load(Ordering::Relaxed);
    now_ms.saturating_sub(last) >= interval_ms
        && last_warn_ms
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// Increment the fail-open counter (always — counters are cheap and accurate).
fn count_fail_open(reason: &'static str) {
    FAIL_OPEN_COUNTER.add(1, &[KeyValue::new("reason", reason)]);
}

/// Count *and* emit a throttled warning that the limiter is failing open. Used
/// on the otherwise-silent "no tracking backend" path.
fn record_rate_limit_fail_open(reason: &'static str) {
    count_fail_open(reason);
    if claim_warn_slot(
        &LAST_FAIL_OPEN_WARN_MS,
        epoch_ms(),
        FAIL_OPEN_WARN_INTERVAL_MS,
    ) {
        tracing::warn!(
            target: "runtara_connections::rate_limit",
            reason,
            "Rate limiting is NOT being enforced — rate-limit tracking is unavailable, so all \
             configured connection rate limits are failing open for this tenant. Requests pass \
             through unthrottled; a misbehaving workflow can now trigger provider 429s or bans."
        );
    }
}

impl ConnectionsFacade {
    pub fn new(state: ConnectionsState) -> Self {
        Self { state }
    }

    // ── Repository helpers (cheap to construct per call) ─────────────────

    fn repo(&self) -> ConnectionRepository {
        ConnectionRepository::new(self.state.db_pool.clone(), self.state.cipher.clone())
    }

    /// Build a rate-limit analytics service using the facade's configured
    /// PostgreSQL pool and shared Redis connection manager.
    pub fn rate_limit_service(&self) -> RateLimitService {
        let repo = Arc::new(self.repo());
        RateLimitService::with_redis_manager_and_db_pool(
            repo,
            self.state.redis_manager.clone(),
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

    /// Get the default connection for an agent/operator, including secret parameters.
    ///
    /// SECURITY: Only use for internal runtime credential resolution.
    pub async fn get_default_with_parameters(
        &self,
        tenant_id: &str,
        default_for: &str,
    ) -> Result<Option<ConnectionWithParameters>, ConnectionsError> {
        self.repo()
            .get_default_connection_with_parameters(tenant_id, default_for)
            .await
            .map_err(ConnectionsError::Database)
    }

    /// Ensure a tenant has a default connection for an agent/operator.
    ///
    /// If the default already exists, returns that connection id. Otherwise a
    /// new active connection is created and mapped as the default.
    pub async fn ensure_default_connection(
        &self,
        tenant_id: &str,
        default_for: &str,
        title: String,
        integration_id: String,
        connection_parameters: Value,
    ) -> Result<String, ConnectionsError> {
        let repo = self.repo();
        if let Some(connection_id) = repo
            .get_default_connection_id(tenant_id, default_for)
            .await
            .map_err(ConnectionsError::Database)?
        {
            return Ok(connection_id);
        }

        let connection_id = uuid::Uuid::new_v4().to_string();
        let request = CreateConnectionRequest {
            title,
            connection_subtype: None,
            connection_parameters: Some(connection_parameters),
            integration_id: Some(integration_id),
            rate_limit_config: None,
            valid_until: None,
            status: Some(ConnectionStatus::Active),
            is_default_file_storage: None,
            default_for: None,
        };
        repo.create(&request, tenant_id, &connection_id)
            .await
            .map_err(ConnectionsError::Database)?;
        repo.replace_defaults_for_connection(tenant_id, &connection_id, &[default_for.to_string()])
            .await
            .map_err(ConnectionsError::Database)?;

        Ok(connection_id)
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
    /// client credentials flow, and token caching. When an OAuth refresh actually
    /// fires, the (possibly rotated) tokens are persisted back to the connection
    /// so a rotating provider survives process restarts / cache eviction.
    pub async fn resolve_connection_auth(
        &self,
        connection_id: &str,
        tenant_id: &str,
        integration_id: &str,
        params: &Value,
        headers: &mut HashMap<String, String>,
    ) -> Result<ResolvedConnectionAuth, ConnectionsError> {
        let resolved = provider_auth::resolve_connection_auth(
            &self.state.http_client,
            connection_id,
            integration_id,
            params,
            headers,
            &self.state.connection_events,
        )
        .await
        .map_err(ConnectionsError::AuthResolution)?;

        // Only set when an actual refresh occurred (once per ~55-min cycle, not per
        // request), so this DB write is off the common hot path.
        if let Some(rotated) = &resolved.rotated_credentials {
            self.persist_rotated_credentials(
                connection_id,
                tenant_id,
                integration_id,
                params,
                rotated,
            )
            .await?;
        }

        Ok(resolved)
    }

    /// Seal the refreshed tokens back into `connection_parameters` with an
    /// optimistic-concurrency guard on the refresh-token hash (see
    /// [`ConnectionRepository::persist_refreshed_oauth`]).
    async fn persist_rotated_credentials(
        &self,
        connection_id: &str,
        tenant_id: &str,
        integration_id: &str,
        params: &Value,
        rotated: &RotatedCredentials,
    ) -> Result<(), ConnectionsError> {
        let old_refresh = params.get("refresh_token").and_then(|v| v.as_str());
        // A provider may omit refresh_token on refresh (non-rotating) — keep the old one.
        let new_refresh = rotated.refresh_token.as_deref().or(old_refresh);

        let mut merged = params.clone();
        if let Some(obj) = merged.as_object_mut() {
            obj.insert(
                "access_token".to_string(),
                Value::String(rotated.access_token.clone()),
            );
            if let Some(rt) = new_refresh {
                obj.insert("refresh_token".to_string(), Value::String(rt.to_string()));
            }
            if let Some(exp) = rotated.token_expires_at {
                obj.insert(
                    "token_expires_at".to_string(),
                    Value::String(exp.to_rfc3339()),
                );
            }
        }

        let expected_hash = old_refresh.map(refresh_token_hash);
        let new_hash = new_refresh.map(refresh_token_hash);

        match self
            .repo()
            .persist_refreshed_oauth(
                connection_id,
                tenant_id,
                &merged,
                expected_hash.as_deref(),
                new_hash.as_deref(),
            )
            .await
        {
            // Lost the optimistic race: another process rotated concurrently and already
            // persisted a valid token. This process's in-memory access token is still good
            // for the cycle; the DB holds the winner's token for cold starts.
            Ok(0) => {
                tracing::warn!(
                    connection_id,
                    "oauth rotation persist skipped — concurrent rotation won the optimistic guard"
                );
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(e) => {
                if rotates_refresh_token(integration_id) {
                    // Fail closed: the DB now holds a dead (rotated-away) token and we could
                    // not replace it. Drop the just-cached access token so the next attempt
                    // re-refreshes from the persisted state rather than diverging silently.
                    token_cache::invalidate_oauth_refresh_cache(connection_id, integration_id);
                    tracing::error!(connection_id, error = %e, "failed to persist rotated oauth tokens — failing closed");
                    Err(ConnectionsError::Database(e))
                } else {
                    // Non-rotating (e.g. HubSpot): the DB token stays valid, so a failed
                    // same-value write is harmless — log and continue.
                    tracing::error!(connection_id, error = %e, "failed to persist refreshed oauth tokens (tolerated: non-rotating provider)");
                    Ok(())
                }
            }
        }
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

        let mut conn = match self.state.redis_manager.clone() {
            Some(m) => m,
            None => {
                // No tracking backend — fail open, but make the lost enforcement
                // observable (counter + throttled warn) so it can be alerted on.
                record_rate_limit_fail_open("tracking_unavailable");
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
                // The Lua check errored — fail open. This path already warns per
                // error; add the fail-open counter so it shows up in the same
                // metric as the no-backend path.
                count_fail_open("tracking_error");
                tracing::warn!(
                    connection_id = connection_id,
                    error = %e,
                    "Token bucket Lua script failed — allowing request (rate limit not enforced)"
                );
                Ok(())
            }
        }
    }
}

/// SHA-256 (hex) of a refresh token — a non-reversible fingerprint stored in the
/// `refresh_token_hash` column for the rotation optimistic-concurrency guard. The
/// token value itself is never persisted in plaintext.
fn refresh_token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Whether a provider rotates (and invalidates) its refresh token on every refresh.
/// Drives fail-closed handling when a rotated token can't be persisted. Sourced from
/// the connection type's OAuth descriptor; non-rotating providers (e.g. HubSpot)
/// default to `false`.
fn rotates_refresh_token(integration_id: &str) -> bool {
    runtara_agents::registry::find_connection_type(integration_id)
        .and_then(|meta| meta.oauth_config)
        .map(|cfg| cfg.refresh_token_rotates)
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::{FAIL_OPEN_WARN_INTERVAL_MS, claim_warn_slot};
    use std::sync::atomic::AtomicI64;

    #[test]
    fn warn_slot_fires_once_then_throttles_until_interval_elapses() {
        let last = AtomicI64::new(0);
        let iv = FAIL_OPEN_WARN_INTERVAL_MS;
        // Realistic epoch-ms base so `now - 0 >= interval` on the first call.
        let base: i64 = 1_700_000_000_000;

        // First occurrence claims the slot (warns).
        assert!(claim_warn_slot(&last, base, iv));
        // Subsequent calls within the interval are throttled.
        assert!(!claim_warn_slot(&last, base + 1, iv));
        assert!(!claim_warn_slot(&last, base + iv - 1, iv));
        // Once the interval elapses it warns again, then throttles again.
        assert!(claim_warn_slot(&last, base + iv, iv));
        assert!(!claim_warn_slot(&last, base + iv + 1, iv));
    }
}
