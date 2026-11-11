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

use crate::auth::provider_auth::{self, ResolvedConnectionAuth};
use crate::config::ConnectionsState;
use crate::error::ConnectionsError;
use crate::repository::connections::{
    ConnectionRepository, ConnectionWithParameters, refresh_token_guard,
};
use crate::resolution::{ConnectionDescriptor, ConnectionResourcePage, ConnectionResourceRequest};
use crate::resource_resolver::resources_for_integration;
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

/// How a webhook's signature checks against a connection's signing secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureCheck {
    /// The connection has no such secret.
    NoSecret,
    /// The signature was made with the secret.
    Valid,
    /// It was not, or is not a signature at all.
    Invalid,
}

impl ConnectionsFacade {
    pub fn new(state: ConnectionsState) -> Self {
        Self { state }
    }

    /// Checks a webhook's HMAC-SHA256 signature, given in hex, against the
    /// connection's signing secret in `field`. `message` is what the provider
    /// signs; the caller builds it, and checks any timestamp itself.
    ///
    /// A sealed secret is checked by seal, which says only yes or no; any
    /// other is compared here, in constant time. An error means the check
    /// could not be made, and the webhook must then be refused.
    pub async fn check_webhook_signature(
        &self,
        connection: &ConnectionWithParameters,
        tenant_id: &str,
        field: &str,
        message: &[u8],
        signature_hex: &str,
    ) -> Result<SignatureCheck, ConnectionsError> {
        use hmac::{Hmac, Mac};
        let empty = Value::Null;
        let parameters = connection.connection_parameters.as_ref().unwrap_or(&empty);
        let secret = &parameters[field];
        let sealed = crate::sealed::is_sealed(secret);
        if !sealed && secret.as_str().is_none_or(str::is_empty) {
            return Ok(SignatureCheck::NoSecret);
        }
        let Ok(signature) = hex::decode(signature_hex) else {
            return Ok(SignatureCheck::Invalid);
        };
        let valid = if sealed {
            self.state
                .sealed
                .verify(
                    tenant_id,
                    &connection.id,
                    parameters,
                    field,
                    message.to_vec(),
                    signature,
                )
                .await
                .map_err(|error| {
                    // seal's detail has identifiers and static text only.
                    ConnectionsError::AuthResolution(
                        format!(
                            "the signature could not be checked: {}: {}",
                            error.code.as_str(),
                            error.detail
                        )
                        .into(),
                    )
                })?
        } else {
            let mut mac = Hmac::<sha2::Sha256>::new_from_slice(
                secret.as_str().unwrap_or_default().as_bytes(),
            )
            .map_err(|_| ConnectionsError::AuthResolution("Invalid signing key".into()))?;
            mac.update(message);
            mac.verify_slice(&signature).is_ok()
        };
        Ok(if valid {
            SignatureCheck::Valid
        } else {
            SignatureCheck::Invalid
        })
    }

    // ── Repository helpers (cheap to construct per call) ─────────────────

    fn repo(&self) -> ConnectionRepository {
        ConnectionRepository::new(self.state.db_pool.clone(), self.state.cipher.clone())
    }

    /// seal, which holds every connection's secrets and uses them.
    pub fn sealed(&self) -> &Arc<crate::sealed::SealedCredentials> {
        &self.state.sealed
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

    /// Resolve an opaque connection id to its authoritative, non-secret
    /// descriptor. This is the runtime/editor metadata boundary: callers get
    /// integration routing and extractor-owned resources, never credential material.
    pub async fn describe_connection(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<Option<ConnectionDescriptor>, ConnectionsError> {
        let Some(connection) = self.get_connection(id, tenant_id).await? else {
            return Ok(None);
        };
        let integration_id = connection.integration_id.unwrap_or_default();
        // MCP tool configuration shares the encrypted parameter blob. Project
        // only the approved fields; endpoint URLs and headers remain native.
        let metadata = if integration_id == "mcp" {
            let full = self
                .get_with_parameters_for_types(id, tenant_id, &["mcp".to_owned()])
                .await?
                .ok_or_else(|| ConnectionsError::NotFound("Connection not found".into()))?;
            mcp_guest_metadata(full.connection_parameters.as_ref().unwrap_or(&Value::Null))
        } else {
            Value::Null
        };
        Ok(Some(ConnectionDescriptor {
            connection_id: connection.id,
            integration_id: integration_id.clone(),
            connection_subtype: connection.connection_subtype,
            status: connection.status,
            resources: resources_for_integration(&integration_id),
            metadata,
        }))
    }

    /// Resolve a read-only resource advertised by a tenant-owned connection.
    /// The connection-specific extractor validates the connection-local name,
    /// parses typed parameters, and performs provider-specific discovery.
    pub async fn resolve_connection_resource(
        &self,
        id: &str,
        tenant_id: &str,
        request: &ConnectionResourceRequest,
    ) -> Result<ConnectionResourcePage, ConnectionsError> {
        let connection = self
            .get_with_parameters(id, tenant_id)
            .await?
            .ok_or_else(|| ConnectionsError::NotFound(format!("Connection '{id}' not found")))?;
        let integration_id = connection.integration_id.as_deref().ok_or_else(|| {
            ConnectionsError::Validation(format!(
                "Connection '{id}' has no integrationId and cannot resolve resources"
            ))
        })?;
        let empty_parameters = Value::Null;
        let parameters = connection
            .connection_parameters
            .as_ref()
            .unwrap_or(&empty_parameters);
        // A sealed connection's discovery request goes through seal, which
        // attaches the secret the extractor can no longer read.
        let presents_sealed = connection.sealed
            && !crate::sealed::request_fields(integration_id, parameters).is_empty();
        let fetcher = if presents_sealed {
            crate::resource_resolver::Fetcher::sealed(
                &self.state.http_client,
                crate::resource_resolver::SealedRoute {
                    sealed: &self.state.sealed,
                    tenant_id,
                    connection_id: id,
                },
            )
        } else {
            crate::resource_resolver::Fetcher::plain(&self.state.http_client)
        };
        crate::resource_resolver::resolve_resource(&fetcher, integration_id, parameters, request)
            .await
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

    /// Get a connection with its parameters, its secrets sealed. One that
    /// still holds a readable secret is not given out: nothing here uses a
    /// secret but through seal.
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
            .and_then(sealed_only)
    }

    /// Trusted execution: verify the integration on the stored row before
    /// decrypting credentials, without a metadata/decrypt check-use race.
    pub async fn get_with_parameters_for_types(
        &self,
        id: &str,
        tenant_id: &str,
        allowed: &[String],
    ) -> Result<Option<ConnectionWithParameters>, ConnectionsError> {
        self.repo()
            .get_with_parameters_for_types(id, tenant_id, allowed)
            .await
            .map_err(ConnectionsError::Database)
            .and_then(sealed_only)
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
            .and_then(sealed_only)
    }

    /// Ensure a tenant has a default connection for an agent/operator.
    ///
    /// If the default already exists, returns that connection id. Otherwise a
    /// new active connection is created and mapped as the default, with its
    /// secrets sealed. Parameters with a secret seal cannot hold are refused,
    /// and no connection is made.
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
        // As any save: a secret is stored sealed, or the connection is not
        // made.
        let connection_parameters = self
            .state
            .sealed
            .seal_parameters(
                tenant_id,
                &connection_id,
                &integration_id,
                &connection_parameters,
                None,
            )
            .await
            .map_err(|error| ConnectionsError::Validation(error.to_string()))?
            .unwrap_or(connection_parameters);
        let request = CreateConnectionRequest {
            title,
            connection_subtype: None,
            connection_parameters: Some(connection_parameters),
            integration_id: Some(integration_id),
            rate_limit_config: None,
            valid_until: None,
            is_default_file_storage: None,
            default_for: None,
        };
        repo.create(
            &request,
            tenant_id,
            &connection_id,
            &ConnectionStatus::Active,
            &[default_for.to_string()],
        )
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
            .and_then(sealed_only)
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
        // A platform app's credentials go to the provider only: `params`, not
        // `effective`, is what a refresh writes back.
        let app = crate::oauth_apps::app_for(
            self.state.oauth.apps.as_deref(),
            tenant_id,
            integration_id,
            params,
        )
        .map_err(|error| {
            ConnectionsError::AuthResolution(
                crate::auth::token_cache::AuthResolutionError::permanent(error),
            )
        })?;
        let effective = app
            .as_ref()
            .map(|app| crate::oauth_apps::with_app_credentials(params, app));
        let resolved = match provider_auth::resolve_connection_auth(
            tenant_id,
            connection_id,
            integration_id,
            effective.as_ref().unwrap_or(params),
            headers,
            &self.state.connection_events,
            &self.state.sealed,
        )
        .await
        {
            Ok(resolved) => resolved,
            Err(e) => {
                if let Some(app) = &app {
                    crate::oauth_apps::record_provider_error(&app.app_id, "refresh", &e.message);
                }
                // A dead grant (e.g. `invalid_grant` on refresh) means retrying forever
                // is pointless — flip the connection to REQUIRES_RECONNECTION so the UI
                // can prompt a reconnect.
                self.maybe_flip_needs_reauth(connection_id, tenant_id, integration_id, &e)
                    .await;
                return Err(ConnectionsError::AuthResolution(e));
            }
        };

        // The pair a refresh through seal brought back, already sealed. Only
        // set when a refresh was made (once per token lifetime, not per
        // request), so this write is off the common path.
        if let Some(rotated) = &resolved.rotated_sealed {
            let mut merged = params.clone();
            if let Some(obj) = merged.as_object_mut() {
                obj.insert("access_token".to_string(), rotated.access_token.clone());
                // A provider may omit the refresh token: the one held stays.
                if let Some(refresh_token) = &rotated.refresh_token {
                    obj.insert("refresh_token".to_string(), refresh_token.clone());
                }
                if let Some(expires_at) = rotated.token_expires_at {
                    obj.insert(
                        "token_expires_at".to_string(),
                        Value::String(expires_at.to_rfc3339()),
                    );
                }
            }
            let expected_hash = refresh_token_guard(&params["refresh_token"]);
            let new_hash = refresh_token_guard(&merged["refresh_token"]);
            self.persist_rotation(
                connection_id,
                tenant_id,
                integration_id,
                params,
                &merged,
                expected_hash.as_deref(),
                new_hash.as_deref(),
            )
            .await?;
        }

        Ok(resolved)
    }

    /// If the auth-resolution error is terminal — an OAuth error code the
    /// provider's descriptor flags as reauth-worthy (e.g. `invalid_grant`), or a
    /// PERMANENT credential failure on a client-credentials connection (wrong
    /// secret → token-endpoint 4xx) — flip the connection to
    /// `REQUIRES_RECONNECTION`. Best-effort: a failed status write is logged, not
    /// propagated (the original auth error is what the caller sees).
    async fn maybe_flip_needs_reauth(
        &self,
        connection_id: &str,
        tenant_id: &str,
        integration_id: &str,
        error: &crate::auth::token_cache::AuthResolutionError,
    ) {
        let meta = runtara_agents::registry::find_connection_type(integration_id);
        let codes = meta
            .and_then(|meta| meta.oauth_config)
            .map(|cfg| cfg.reauth_on_error_codes)
            .unwrap_or(&[]);
        // Client-credentials types declare no oauth_config/reauth codes; for
        // them, permanence of the mint failure IS the terminal signal.
        let client_credentials_permanent = error.permanent
            && meta.is_some_and(|m| {
                matches!(
                    m.auth_type,
                    Some(runtara_dsl::agent_meta::ConnectionAuthType::Oauth2ClientCredentials)
                )
            });
        if !client_credentials_permanent && !is_reauth_error(&error.message, codes) {
            return;
        }
        match self
            .repo()
            .update_status(
                connection_id,
                tenant_id,
                ConnectionStatus::RequiresReconnection.as_str(),
            )
            .await
        {
            // The message holds the token endpoint's status and OAuth error fields
            // (`invalid_scope`, `invalid_client`, …), never its body or a credential;
            // without it the reason is only in a debug log.
            Ok(_) => tracing::warn!(
                connection_id,
                reason = %error.message,
                "oauth grant is dead (reauth error) — connection flipped to REQUIRES_RECONNECTION"
            ),
            Err(e) => tracing::error!(
                connection_id,
                error = %e,
                "failed to flip connection to REQUIRES_RECONNECTION after a reauth error"
            ),
        }
    }

    /// Store a connection's refreshed tokens (`merged`), unless another
    /// process stored a later rotation first.
    #[allow(clippy::too_many_arguments)]
    async fn persist_rotation(
        &self,
        connection_id: &str,
        tenant_id: &str,
        integration_id: &str,
        params: &Value,
        merged: &Value,
        expected_hash: Option<&str>,
        new_hash: Option<&str>,
    ) -> Result<(), ConnectionsError> {
        match self
            .repo()
            .persist_refreshed_oauth(connection_id, tenant_id, merged, expected_hash, new_hash)
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
                if rotates_refresh_token(integration_id, params) {
                    // Fail closed: the DB now holds a dead (rotated-away) token and we could
                    // not replace it. Drop the just-cached access token so the next attempt
                    // re-refreshes from the persisted state rather than diverging silently.
                    provider_auth::invalidate_connection_token_caches(
                        connection_id,
                        integration_id,
                        params,
                    );
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
        tenant_id: &str,
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

        let key = crate::service::rate_limits::rate_limit_key(tenant_id, connection_id);
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

/// Whether an auth-resolution error message carries one of the provider's terminal
/// OAuth error codes (matched as a substring — `parse_token_response` places the
/// provider's `error` code in the message). Empty `codes` never matches.
fn is_reauth_error(error: &str, codes: &[&str]) -> bool {
    !codes.is_empty() && codes.iter().any(|code| error.contains(code))
}

/// Whether a provider rotates (and invalidates) its refresh token on every refresh.
/// Drives fail-closed handling when a rotated token can't be persisted. Sourced from
/// the EFFECTIVE OAuth config: the static descriptor for curated providers, the
/// connection's own `refresh_rotates` param (default true — fail closed) for the
/// params-driven generic types. A fail-open default there would recreate the
/// silent rotation-loss bug class the QuickBooks P0 work fixed.
fn rotates_refresh_token(integration_id: &str, params: &serde_json::Value) -> bool {
    runtara_agents::registry::find_connection_type(integration_id)
        .and_then(|meta| meta.oauth_config)
        .map(|cfg| {
            crate::auth::provider_auth::resolve_effective_oauth_config(cfg, params)
                .refresh_token_rotates
        })
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

/// A connection to use, only if seal holds every secret it has. One stored
/// readable by an earlier release is sealed when the server starts, or by
/// `runtara-server seal-connections`; what seal cannot hold must be entered
/// again.
fn sealed_only(
    connection: Option<ConnectionWithParameters>,
) -> Result<Option<ConnectionWithParameters>, ConnectionsError> {
    let Some(connection) = connection else {
        return Ok(None);
    };
    let readable = connection
        .connection_parameters
        .as_ref()
        .map(|parameters| {
            crate::sealed::policy::readable_secrets(
                connection.integration_id.as_deref().unwrap_or_default(),
                parameters,
            )
        })
        .unwrap_or_default();
    if readable.is_empty() {
        return Ok(Some(connection));
    }
    tracing::warn!(
        connection_id = %connection.id,
        tenant_id = connection.tenant_id.as_deref().unwrap_or_default(),
        "refusing a connection whose credentials are not sealed"
    );
    Err(ConnectionsError::Validation(format!(
        "Connection '{}' holds credentials that are not sealed, and cannot be used: \
         enter them again",
        connection.id
    )))
}

#[cfg(test)]
mod tests {
    use super::{FAIL_OPEN_WARN_INTERVAL_MS, claim_warn_slot, is_reauth_error};
    use std::sync::atomic::AtomicI64;

    #[test]
    fn reauth_error_matches_only_declared_codes() {
        let codes = &["invalid_grant"];
        assert!(is_reauth_error(
            "Token endpoint returned 400: invalid_grant Token expired",
            codes
        ));
        // A transient network error is not a reauth condition.
        assert!(!is_reauth_error(
            "OAuth token refresh request failed: connection refused",
            codes
        ));
        // No declared codes → never flips (e.g. HubSpot).
        assert!(!is_reauth_error(
            "Token endpoint returned 400: invalid_grant",
            &[]
        ));
    }

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

/// Explicit allowlist: URL, auth material and extra headers never cross into WASM.
fn mcp_guest_metadata(parameters: &Value) -> Value {
    let hints: serde_json::Map<String, Value> = parameters["tool_hints"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| value.is_string())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let scope: Vec<Value> = parameters["tool_scope"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|value| value.is_string())
        .cloned()
        .collect();
    serde_json::json!({"tool_hints": hints, "tool_scope": scope})
}

#[cfg(test)]
mod guest_metadata_tests {
    use super::*;
    #[test]
    fn mcp_metadata_only_contains_tool_configuration() {
        let parameters = serde_json::json!({
            "url": "https://example.test/mcp?key=synthetic-secret",
            "bearer_token": "synthetic-secret",
            "extra_headers": {"Authorization": "synthetic-secret"},
            "tool_hints": {"search": "Find documents", "invalid": {}},
            "tool_scope": ["search", 7], "future_field": "synthetic-secret"
        });
        assert_eq!(
            mcp_guest_metadata(&parameters),
            serde_json::json!({
                "tool_hints": {"search": "Find documents"}, "tool_scope": ["search"]
            })
        );
    }
}

#[cfg(test)]
mod signature_tests {
    use super::*;
    use crate::sealed::{Allowances, MemoryKeySource, SealedCredentials};
    use hmac::{Hmac, Mac};

    const SECRET: &str = "signing-CANARY";
    const MESSAGE: &[u8] = b"v0:1700000000:{\"type\":\"event_callback\"}";

    /// A facade over a database it never reaches: a signature check reads
    /// only the connection it is given.
    fn facade(sealed: Option<Arc<SealedCredentials>>) -> ConnectionsFacade {
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![]));
        ConnectionsFacade::new(ConnectionsState {
            db_pool: sqlx::PgPool::connect_lazy("postgres://unused@127.0.0.1:1/unused").unwrap(),
            redis_manager: None,
            public_base_url: String::new(),
            oauth_complete_url: None,
            http_client: reqwest::Client::new(),
            cipher: Arc::new(crate::crypto::noop::NoOpCipher),
            compatibility: Arc::new(
                crate::integration_compatibility::IntegrationCompatibility::from_catalog(&catalog),
            ),
            agent_catalog: catalog,
            connection_events: None,
            oauth: Default::default(),
            agent_access: Arc::new(crate::config::EveryAgent),
            sealed: sealed.unwrap_or_else(SealedCredentials::for_tests),
        })
    }

    fn connection(parameters: Value, sealed: bool) -> ConnectionWithParameters {
        ConnectionWithParameters {
            id: "c-1".into(),
            tenant_id: Some("acme".into()),
            integration_id: Some("slack_bot".into()),
            connection_subtype: None,
            connection_parameters: Some(parameters),
            rate_limit_config: None,
            sealed,
        }
    }

    fn signature(secret: &str, message: &[u8]) -> String {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(message);
        hex::encode(mac.finalize().into_bytes())
    }

    async fn check(
        facade: &ConnectionsFacade,
        connection: &ConnectionWithParameters,
        message: &[u8],
        signature: &str,
    ) -> Result<SignatureCheck, ConnectionsError> {
        facade
            .check_webhook_signature(connection, "acme", "signing_secret", message, signature)
            .await
    }

    /// The same answers whether the secret is readable or sealed.
    async fn assert_checks(facade: &ConnectionsFacade, connection: &ConnectionWithParameters) {
        let good = signature(SECRET, MESSAGE);
        assert_eq!(
            check(facade, connection, MESSAGE, &good).await.unwrap(),
            SignatureCheck::Valid
        );
        // Hex in either case is the same code.
        assert_eq!(
            check(facade, connection, MESSAGE, &good.to_uppercase())
                .await
                .unwrap(),
            SignatureCheck::Valid
        );
        for bad in [
            signature("another secret", MESSAGE),
            signature(SECRET, b"another message"),
            good[..32].to_owned(),
            "not hex".to_owned(),
            String::new(),
        ] {
            assert_eq!(
                check(facade, connection, MESSAGE, &bad).await.unwrap(),
                SignatureCheck::Invalid,
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn a_readable_signing_secret_checks_the_signature_here() {
        let connection = connection(serde_json::json!({"signing_secret": SECRET}), false);
        assert_checks(&facade(None), &connection).await;
    }

    #[tokio::test]
    async fn a_sealed_signing_secret_is_checked_by_seal() {
        let sealed = Arc::new(
            SealedCredentials::in_process(
                Arc::new(MemoryKeySource::default()),
                &[7_u8; 32],
                Allowances::default(),
            )
            .unwrap(),
        );
        let stored = sealed
            .seal_parameters(
                "acme",
                "c-1",
                "slack_bot",
                &serde_json::json!({"bot_token": "xoxb", "signing_secret": SECRET}),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.to_string().contains(SECRET));
        let connection = connection(stored, true);

        assert_checks(&facade(Some(sealed)), &connection).await;

        // Without seal the check cannot be made, and that is an error, never
        // a pass.
        let good = signature(SECRET, MESSAGE);
        assert!(
            check(&facade(None), &connection, MESSAGE, &good)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_connection_with_no_signing_secret_says_so() {
        for parameters in [
            serde_json::json!({}),
            serde_json::json!({"signing_secret": ""}),
            serde_json::json!({"signing_secret": null}),
        ] {
            let connection = connection(parameters, false);
            assert_eq!(
                check(&facade(None), &connection, MESSAGE, "00")
                    .await
                    .unwrap(),
                SignatureCheck::NoSecret
            );
        }
    }
}
