//! Connection Service
//!
//! Business logic for connection management
//! Handles validation and error mapping

use crate::integration_compatibility::{IntegrationCompatibility, OBJECT_STORAGE_DEFAULT_FOR};
use crate::repository::connections::ConnectionRepository;
use crate::service::rate_limits::RateLimitService;
use crate::types::*;
use crate::util::rate_limit_defaults::get_default_rate_limit_config;
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

/// Validate a rate-limit config before it is persisted.
///
/// The token-bucket limiter treats `requests_per_second == 0` as "no limit"
/// (a silent bypass), so a config with 0 looks enabled in the UI but enforces
/// nothing — harder to diagnose than having no config at all. Reject the
/// degenerate values here (the authoritative gate, since the HTTP API and MCP
/// bypass the frontend) so the only ways to be unlimited are an explicit "no
/// config" or an opt-out integration type.
fn validate_rate_limit_config(cfg: &RateLimitConfigDto) -> Result<(), ServiceError> {
    if cfg.requests_per_second < 1 {
        return Err(ServiceError::ValidationError(
            "rate_limit_config.requests_per_second must be at least 1 (0 would silently \
             disable enforcement — clear the rate limit instead to leave the connection \
             unlimited)"
                .to_string(),
        ));
    }
    if cfg.burst_size < 1 {
        return Err(ServiceError::ValidationError(
            "rate_limit_config.burst_size must be at least 1".to_string(),
        ));
    }
    if cfg.burst_size < cfg.requests_per_second {
        return Err(ServiceError::ValidationError(format!(
            "rate_limit_config.burst_size ({}) must be >= requests_per_second ({})",
            cfg.burst_size, cfg.requests_per_second
        )));
    }
    const MAX_RETRIES: u32 = 100;
    if cfg.max_retries > MAX_RETRIES {
        return Err(ServiceError::ValidationError(format!(
            "rate_limit_config.max_retries ({}) must be <= {MAX_RETRIES}",
            cfg.max_retries
        )));
    }
    const MAX_WAIT_MS: u64 = 3_600_000; // 1 hour
    if cfg.max_wait_ms > MAX_WAIT_MS {
        return Err(ServiceError::ValidationError(format!(
            "rate_limit_config.max_wait_ms ({}) must be <= {MAX_WAIT_MS} (1 hour)",
            cfg.max_wait_ms
        )));
    }
    Ok(())
}

/// Validate connection parameters against the connection type's field schema.
///
/// For every field flagged `is_required` / `is_url`, enforce presence and https
/// URL format. This is the authoritative creation-time gate — the HTTP API and
/// MCP both flow through here — and pairs with the proxy's runtime base-URL pin:
/// a credential-bearing generic HTTP connection cannot be persisted without a
/// valid https base URL to pin its egress to. Unknown types (no meta) and types
/// with no flagged fields (openai/stripe/etc., whose base URL is derived) are
/// unaffected.
fn validate_connection_parameters(
    integration_id: &str,
    params: Option<&serde_json::Value>,
) -> Result<(), ServiceError> {
    let Some(meta) = runtara_agents::registry::find_connection_type(integration_id) else {
        return Ok(());
    };
    for field in meta.fields {
        if !(field.is_required || field.is_url) {
            continue;
        }
        let raw = params
            .and_then(|p| p.get(field.name))
            .and_then(|v| v.as_str());
        let display = field.display_name.unwrap_or(field.name);
        validate_url_field(display, raw, field.is_required, field.is_url)?;
    }
    Ok(())
}

/// Validate a single `is_url` / `is_required` field value.
fn validate_url_field(
    field_display: &str,
    raw: Option<&str>,
    is_required: bool,
    is_url: bool,
) -> Result<(), ServiceError> {
    let trimmed = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(value) = trimmed else {
        if is_required {
            return Err(ServiceError::ValidationError(format!(
                "{field_display} is required and must be a non-empty https URL \
                 (e.g. https://api.example.com)"
            )));
        }
        return Ok(());
    };
    if !is_url {
        return Ok(());
    }
    let parsed = url::Url::parse(value).map_err(|_| {
        ServiceError::ValidationError(format!(
            "{field_display} must be a valid absolute URL (got '{value}')"
        ))
    })?;
    let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or_else(|| {
        ServiceError::ValidationError(format!("{field_display} must include a host"))
    })?;
    match parsed.scheme() {
        "https" => {}
        "http" if connection_http_allowed(host) => {}
        _ => {
            return Err(ServiceError::ValidationError(format!(
                "{field_display} must use https:// ({value})"
            )));
        }
    }
    // SSRF rule B: a literal private/internal IP host is rejected outright even
    // over https (hostnames are enforced at connect time by the guarded DNS
    // resolver — see crate::net). The dev http-allowlist doubles as the escape
    // hatch for loopback test endpoints.
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>()
        && crate::net::is_private_ip(&ip)
        && !connection_http_allowed(host)
    {
        return Err(ServiceError::ValidationError(format!(
            "{field_display} host {host} is a private/internal address"
        )));
    }
    Ok(())
}

/// Hosts allowed to use an `http://` base URL (`RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS`).
/// Host-scoped so a single dev/socat sidecar can be allowed without disabling
/// TLS enforcement globally. Empty = https-only (fail-closed default).
fn connection_http_allowed(host: &str) -> bool {
    static HOSTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let list = HOSTS.get_or_init(|| {
        std::env::var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let h = host.to_ascii_lowercase();
    list.iter().any(|entry| entry == &h)
}

pub struct ConnectionService {
    repository: Arc<ConnectionRepository>,
    compatibility: Arc<IntegrationCompatibility>,
    rate_limit_service: Option<Arc<RateLimitService>>,
}

impl ConnectionService {
    pub fn new(
        repository: Arc<ConnectionRepository>,
        compatibility: Arc<IntegrationCompatibility>,
    ) -> Self {
        Self {
            repository,
            compatibility,
            rate_limit_service: None,
        }
    }

    /// Create a new connection service with rate limit support for runtime API
    pub fn with_rate_limit_service(
        repository: Arc<ConnectionRepository>,
        compatibility: Arc<IntegrationCompatibility>,
        rate_limit_service: Arc<RateLimitService>,
    ) -> Self {
        Self {
            repository,
            compatibility,
            rate_limit_service: Some(rate_limit_service),
        }
    }

    fn normalize_default_for(
        default_for: Option<Vec<String>>,
        is_default_file_storage: Option<bool>,
    ) -> Vec<String> {
        let mut values = BTreeSet::new();
        for value in default_for.unwrap_or_default() {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                values.insert(trimmed.to_string());
            }
        }
        if is_default_file_storage == Some(true) {
            values.insert(OBJECT_STORAGE_DEFAULT_FOR.to_string());
        }
        values.into_iter().collect()
    }

    fn validate_default_for(
        &self,
        integration_id: &str,
        default_for: &[String],
    ) -> Result<(), ServiceError> {
        for operator_id in default_for {
            if !self
                .compatibility
                .is_compatible(integration_id, operator_id)
            {
                return Err(ServiceError::ValidationError(format!(
                    "Connection type '{}' is not compatible with default '{}'",
                    integration_id, operator_id
                )));
            }
        }
        Ok(())
    }

    /// Create a new connection
    pub async fn create_connection(
        &self,
        mut request: CreateConnectionRequest,
        tenant_id: &str,
    ) -> Result<String, ServiceError> {
        // Validation: title should not be empty
        if request.title.trim().is_empty() {
            return Err(ServiceError::ValidationError(
                "Connection title cannot be empty".to_string(),
            ));
        }

        // Validation: title length
        if request.title.len() > 255 {
            return Err(ServiceError::ValidationError(
                "Connection title cannot exceed 255 characters".to_string(),
            ));
        }

        // Validation: integration_id is required
        if request.integration_id.is_none()
            || request
                .integration_id
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(ServiceError::ValidationError(
                "integration_id (connection type) is required".to_string(),
            ));
        }

        // Validation: valid_until should be a valid RFC3339 datetime if provided
        if let Some(ref valid_until) = request.valid_until
            && chrono::DateTime::parse_from_rfc3339(valid_until).is_err()
        {
            return Err(ServiceError::ValidationError(
                "valid_until must be a valid RFC3339 datetime".to_string(),
            ));
        }

        // Apply default rate limit config if none provided
        if request.rate_limit_config.is_none()
            && let Some(ref integration_id) = request.integration_id
        {
            request.rate_limit_config = get_default_rate_limit_config(integration_id);
        }

        // Validate the effective rate-limit config (user-supplied or default).
        if let Some(ref cfg) = request.rate_limit_config {
            validate_rate_limit_config(cfg)?;
        }

        // Validate connection parameters (e.g. a required https base URL) against
        // the connection type schema — closes the F1 creation side.
        if let Some(ref integration_id) = request.integration_id {
            validate_connection_parameters(integration_id, request.connection_parameters.as_ref())?;
        }

        let default_for = Self::normalize_default_for(
            request.default_for.clone(),
            request.is_default_file_storage,
        );
        let integration_id = request.integration_id.clone().unwrap_or_default();
        self.validate_default_for(&integration_id, &default_for)?;

        if default_for
            .iter()
            .any(|value| value == OBJECT_STORAGE_DEFAULT_FOR)
        {
            request.is_default_file_storage = Some(true);
        }

        // If marking as default file storage, clear any existing default first
        if request.is_default_file_storage == Some(true) {
            self.repository
                .clear_default_file_storage(tenant_id)
                .await
                .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;
        }

        // Generate new connection ID
        let connection_id = Uuid::new_v4().to_string();

        // Delegate to repository
        self.repository
            .create(&request, tenant_id, &connection_id)
            .await
            .map_err(|e| {
                // Check for unique constraint violation on title
                if e.to_string().contains("uc_connection_data_entity_title") {
                    ServiceError::Conflict("Connection with this title already exists".to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })?;

        if !default_for.is_empty() {
            self.repository
                .replace_defaults_for_connection(tenant_id, &connection_id, &default_for)
                .await
                .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;
        }

        Ok(connection_id)
    }

    /// List connections with optional filters
    pub async fn list_connections(
        &self,
        tenant_id: &str,
        integration_id: Option<String>,
        status: Option<String>,
    ) -> Result<Vec<ConnectionDto>, ServiceError> {
        self.repository
            .list(tenant_id, integration_id.as_deref(), status.as_deref())
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// Get a connection by ID
    pub async fn get_connection(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<ConnectionDto, ServiceError> {
        self.repository
            .get_by_id(id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Connection not found".to_string()))
    }

    /// Update a connection
    pub async fn update_connection(
        &self,
        id: &str,
        tenant_id: &str,
        request: UpdateConnectionRequest,
    ) -> Result<ConnectionDto, ServiceError> {
        // Validation: if title is provided, it should not be empty
        if let Some(ref title) = request.title {
            if title.trim().is_empty() {
                return Err(ServiceError::ValidationError(
                    "Connection title cannot be empty".to_string(),
                ));
            }
            if title.len() > 255 {
                return Err(ServiceError::ValidationError(
                    "Connection title cannot exceed 255 characters".to_string(),
                ));
            }
        }

        // Validation: valid_until should be a valid RFC3339 datetime if provided
        if let Some(ref valid_until) = request.valid_until
            && chrono::DateTime::parse_from_rfc3339(valid_until).is_err()
        {
            return Err(ServiceError::ValidationError(
                "valid_until must be a valid RFC3339 datetime".to_string(),
            ));
        }

        // Validation: rate-limit config if one is being set.
        if let Some(ref cfg) = request.rate_limit_config {
            validate_rate_limit_config(cfg)?;
        }

        let default_for = request.default_for.clone().map(|default_for| {
            Self::normalize_default_for(Some(default_for), request.is_default_file_storage)
        });

        let current_connection = if request.integration_id.is_some()
            || default_for.is_some()
            || request.connection_parameters.is_some()
        {
            Some(self.get_connection(id, tenant_id).await?)
        } else {
            None
        };

        let integration_id = request
            .integration_id
            .clone()
            .or_else(|| {
                current_connection
                    .as_ref()
                    .and_then(|connection| connection.integration_id.clone())
            })
            .unwrap_or_default();

        // Validate submitted connection parameters against the type schema.
        // Only when params are part of this PATCH, so unrelated edits (title,
        // rate limit) to a legacy row aren't retroactively blocked.
        if request.connection_parameters.is_some() {
            validate_connection_parameters(
                &integration_id,
                request.connection_parameters.as_ref(),
            )?;
        }

        if let Some(ref default_for) = default_for {
            self.validate_default_for(&integration_id, default_for)?;
        }

        let mut request = request;
        if let Some(ref default_for) = default_for {
            request.is_default_file_storage = Some(
                default_for
                    .iter()
                    .any(|value| value == OBJECT_STORAGE_DEFAULT_FOR),
            );
        }

        // SSRF rule E: on a params-driven OAuth type, changing an endpoint param
        // must invalidate the captured tokens (DB + in-memory cache) and force a
        // reconnect. Otherwise a captured refresh token could be replayed against
        // a swapped-in token endpoint, or a still-cached Bearer would flow to the
        // newly-edited base_url host.
        let endpoint_edit_requires_reauth =
            if let Some(ref new_params) = request.connection_parameters {
                let params_driven = runtara_agents::registry::find_connection_type(&integration_id)
                    .and_then(|meta| meta.oauth_config)
                    .map(|cfg| cfg.params_driven)
                    .unwrap_or(false);
                if params_driven {
                    let old = self
                        .repository
                        .get_with_parameters(id, tenant_id)
                        .await
                        .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;
                    let old_params = old
                        .and_then(|c| c.connection_parameters)
                        .unwrap_or(serde_json::Value::Null);
                    const ENDPOINT_KEYS: [&str; 5] = [
                        "auth_url",
                        "token_url",
                        "base_url",
                        "revocation_url",
                        "token_auth",
                    ];
                    let changed = ENDPOINT_KEYS
                        .iter()
                        .any(|k| old_params.get(k) != new_params.get(k));
                    if changed {
                        // Strip captured tokens from the params being written.
                        if let Some(obj) = request
                            .connection_parameters
                            .as_mut()
                            .and_then(|p| p.as_object_mut())
                        {
                            obj.remove("access_token");
                            obj.remove("refresh_token");
                            obj.remove("token_expires_at");
                        }
                        // Evict the in-memory cache under the OLD params' keys.
                        crate::auth::provider_auth::invalidate_connection_token_caches(
                            id,
                            &integration_id,
                            &old_params,
                        );
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

        // If marking as default file storage, clear any existing default first
        if request.is_default_file_storage == Some(true) {
            self.repository
                .clear_default_file_storage(tenant_id)
                .await
                .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;
        }

        // Execute update
        let rows_affected = self
            .repository
            .update(id, tenant_id, &request)
            .await
            .map_err(|e| {
                // Check for unique constraint violation on title
                if e.to_string().contains("uc_connection_data_entity_title") {
                    ServiceError::Conflict("Connection with this title already exists".to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })?;

        if rows_affected == 0 {
            return Err(ServiceError::NotFound("Connection not found".to_string()));
        }

        // SSRF rule E (continued): the tokens were stripped above; also force a
        // fresh interactive consent against the new endpoints.
        if endpoint_edit_requires_reauth {
            self.repository
                .update_status(
                    id,
                    tenant_id,
                    ConnectionStatus::RequiresReconnection.as_str(),
                )
                .await
                .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;
        }

        if let Some(default_for) = default_for {
            self.repository
                .replace_defaults_for_connection(tenant_id, id, &default_for)
                .await
                .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;
        }

        // Fetch and return updated connection
        self.get_connection(id, tenant_id).await
    }

    /// Delete a connection
    pub async fn delete_connection(&self, id: &str, tenant_id: &str) -> Result<(), ServiceError> {
        // Best-effort: revoke the provider-side token before dropping the row so a
        // deleted connection's grant is invalidated upstream, not just forgotten.
        self.try_revoke_oauth(id, tenant_id).await;

        let rows_affected = self
            .repository
            .delete(id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        if rows_affected == 0 {
            return Err(ServiceError::NotFound("Connection not found".to_string()));
        }

        Ok(())
    }

    /// Revoke the connection's OAuth token at the provider, if the descriptor declares
    /// a revocation endpoint. Best-effort — failures are logged and never block the
    /// delete (the row is removed regardless).
    async fn try_revoke_oauth(&self, id: &str, tenant_id: &str) {
        let Ok(Some(conn)) = self.repository.get_with_parameters(id, tenant_id).await else {
            return;
        };
        let Some(integration_id) = conn.integration_id.as_deref() else {
            return;
        };
        let Some(oauth_config) = runtara_agents::registry::find_connection_type(integration_id)
            .and_then(|meta| meta.oauth_config)
        else {
            return;
        };
        // No static-descriptor pre-check here: for params-driven types the
        // revocation endpoint lives in the connection params, and
        // build_revoke_request (inside revoke_oauth_token) resolves the
        // EFFECTIVE endpoint — returning None (a no-op) when there is none.
        let params = conn
            .connection_parameters
            .unwrap_or(serde_json::Value::Null);
        let client = crate::net::shared_hardened_client();
        if let Err(e) =
            crate::auth::provider_auth::revoke_oauth_token(client, oauth_config, &params).await
        {
            tracing::warn!(connection_id = id, error = %e, "provider token revocation failed on disconnect (continuing)");
        }
    }

    /// List connections whose `integration_id` falls within the given set.
    ///
    /// Callers (typically API handlers) translate an agent id into the
    /// allowed integration ids using the runtime [`AgentCatalog`]; this
    /// service stays agent-agnostic and only filters by integration ids.
    pub async fn list_connections_by_integration_ids(
        &self,
        tenant_id: &str,
        integration_ids: &[String],
        status: Option<String>,
    ) -> Result<Vec<ConnectionDto>, ServiceError> {
        self.repository
            .list_by_operator(tenant_id, integration_ids, status.as_deref())
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))
    }

    /// Get connection for runtara-workflows runtime
    ///
    /// Returns connection with decrypted parameters and rate limit state.
    /// This is an internal endpoint used by runtara-workflows at runtime.
    /// Also tracks the credential request for rate limit analytics.
    pub async fn get_for_runtime(
        &self,
        connection_id: &str,
        tenant_id: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<RuntimeConnectionResponse, ServiceError> {
        // Fetch connection with parameters from repository
        let connection = self
            .repository
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Connection not found".to_string()))?;

        // Get rate limit state if service is available
        let rate_limit = if let Some(ref rate_limit_service) = self.rate_limit_service {
            match rate_limit_service
                .get_connection_rate_limit_status(connection_id, tenant_id)
                .await
            {
                Ok(status) => {
                    // Convert to RuntimeRateLimitState format
                    let is_limited = status.metrics.is_rate_limited;
                    let remaining = status.state.current_tokens.map(|t| t.floor() as u32);
                    let retry_after_ms = status.metrics.retry_after_ms;

                    // Compute reset_at from retry_after_ms
                    let reset_at = retry_after_ms.map(|ms| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        now + (ms / 1000) as i64
                    });

                    // Track the credential request for analytics
                    // Use different event type based on rate limit status
                    let event_type = if is_limited {
                        RateLimitEventType::RateLimited
                    } else {
                        RateLimitEventType::Request
                    };

                    // Record asynchronously, don't block on tracking
                    let _ = rate_limit_service
                        .record_credential_request(
                            connection_id,
                            tenant_id,
                            &event_type,
                            metadata.clone(),
                        )
                        .await;

                    Some(RuntimeRateLimitState {
                        is_limited,
                        remaining,
                        reset_at,
                        retry_after_ms,
                    })
                }
                Err(_) => None,
            }
        } else {
            None
        };

        Ok(RuntimeConnectionResponse {
            parameters: connection
                .connection_parameters
                .unwrap_or(serde_json::json!({})),
            integration_id: connection.integration_id.unwrap_or_default(),
            connection_subtype: connection.connection_subtype,
            rate_limit,
        })
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    ValidationError(String),
    NotFound(String),
    Conflict(String),
    DatabaseError(String),
}

#[cfg(test)]
mod tests {
    use super::{
        ServiceError, validate_connection_parameters, validate_rate_limit_config,
        validate_url_field,
    };
    use crate::types::RateLimitConfigDto;
    use serde_json::json;

    // ── base-URL validation (F1 creation side) ───────────────────────────────

    fn is_validation_err(r: Result<(), ServiceError>) -> bool {
        matches!(r, Err(ServiceError::ValidationError(_)))
    }

    #[test]
    fn url_field_rejects_private_literal_ip_hosts() {
        // SSRF rule B: private/internal IP literals rejected even over https.
        for bad in [
            "https://169.254.169.254/latest",
            "https://10.0.0.5/token",
            "https://127.0.0.1/token",
            "https://[::1]/token",
        ] {
            assert!(
                validate_url_field("Token URL", Some(bad), true, true).is_err(),
                "{bad} must be rejected"
            );
        }
        // Public literals + hostnames pass (hostname privacy enforced at connect time).
        assert!(
            validate_url_field("Token URL", Some("https://93.184.216.34/t"), true, true).is_ok()
        );
        assert!(
            validate_url_field("Token URL", Some("https://auth.example.com/t"), true, true).is_ok()
        );
    }

    #[test]
    fn url_field_requires_present_https() {
        assert!(is_validation_err(validate_url_field(
            "Base URL", None, true, true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some("   "),
            true,
            true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some(""),
            true,
            true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some("http://api.example.com"),
            true,
            true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some("ftp://x"),
            true,
            true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some("not a url"),
            true,
            true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some("/v2/foo"),
            true,
            true
        )));
        assert!(is_validation_err(validate_url_field(
            "Base URL",
            Some("https://"),
            true,
            true
        )));
        // Valid https passes.
        assert!(
            validate_url_field("Base URL", Some("https://api.example.com/v2"), true, true).is_ok()
        );
        // Optional + absent is fine; is_url only validates format when present.
        assert!(validate_url_field("Base URL", None, false, true).is_ok());
    }

    #[test]
    fn connection_params_enforce_http_type_base_url() {
        // Real meta drives this: proves the schema flag is emitted for http_bearer.
        assert!(is_validation_err(validate_connection_parameters(
            "http_bearer",
            Some(&json!({"token": "secret"}))
        )));
        assert!(
            validate_connection_parameters(
                "http_bearer",
                Some(&json!({"token": "secret", "base_url": "https://api.example.com"}))
            )
            .is_ok()
        );
        // http_api_key too.
        assert!(is_validation_err(validate_connection_parameters(
            "http_api_key",
            Some(&json!({"api_key": "k", "base_url": ""}))
        )));
        assert!(
            validate_connection_parameters(
                "http_api_key",
                Some(&json!({"api_key": "k", "base_url": "https://api.example.com"}))
            )
            .is_ok()
        );
    }

    #[test]
    fn connection_params_enforce_microsoft_entra_urls() {
        // Regression: microsoft_entra_client_credentials.base_url / authority_host
        // must be is_url-validated at save time (https + rule-B private-literal
        // rejection), matching the generic http_oauth2 types — an IP-literal host
        // slips past the connect-time DNS guard, so it must be caught here.
        let ok = json!({
            "client_id": "c", "client_secret": "s",
            "scope": "https://graph.microsoft.com/.default",
            "base_url": "https://graph.microsoft.com/v1.0",
            "authority_host": "https://login.microsoftonline.com"
        });
        assert!(
            validate_connection_parameters("microsoft_entra_client_credentials", Some(&ok)).is_ok()
        );

        // base_url required + must be https.
        let mut bad = ok.clone();
        bad["base_url"] = json!("");
        assert!(is_validation_err(validate_connection_parameters(
            "microsoft_entra_client_credentials",
            Some(&bad)
        )));

        // authority_host pointed at a private/loopback IP literal → SSRF vector, rejected.
        let mut ssrf = ok.clone();
        ssrf["authority_host"] = json!("http://127.0.0.1:9999");
        assert!(is_validation_err(validate_connection_parameters(
            "microsoft_entra_client_credentials",
            Some(&ssrf)
        )));

        // link-local (IMDS) authority_host → rejected.
        let mut imds = ok.clone();
        imds["authority_host"] = json!("http://169.254.169.254");
        assert!(is_validation_err(validate_connection_parameters(
            "microsoft_entra_client_credentials",
            Some(&imds)
        )));
    }

    #[test]
    fn connection_params_noop_for_unflagged_and_unknown_types() {
        // Types with a derived base URL (not flagged) are unaffected even w/o base_url.
        assert!(
            validate_connection_parameters("openai_api_key", Some(&json!({"api_key": "sk-x"})))
                .is_ok()
        );
        // Unknown integration_id → no meta → no-op.
        assert!(validate_connection_parameters("totally_unknown", Some(&json!({}))).is_ok());
    }

    fn valid() -> RateLimitConfigDto {
        RateLimitConfigDto {
            requests_per_second: 5,
            burst_size: 10,
            retry_on_limit: true,
            max_retries: 3,
            max_wait_ms: 60_000,
        }
    }

    fn assert_rejected(cfg: &RateLimitConfigDto) {
        assert!(
            matches!(
                validate_rate_limit_config(cfg),
                Err(ServiceError::ValidationError(_))
            ),
            "expected a ValidationError"
        );
    }

    #[test]
    fn accepts_a_sane_config() {
        assert!(validate_rate_limit_config(&valid()).is_ok());
    }

    #[test]
    fn accepts_burst_equal_to_rate() {
        let mut cfg = valid();
        cfg.requests_per_second = 10;
        cfg.burst_size = 10;
        assert!(validate_rate_limit_config(&cfg).is_ok());
    }

    #[test]
    fn rejects_zero_requests_per_second() {
        let mut cfg = valid();
        cfg.requests_per_second = 0;
        assert_rejected(&cfg);
    }

    #[test]
    fn rejects_zero_burst_size() {
        let mut cfg = valid();
        cfg.burst_size = 0;
        assert_rejected(&cfg);
    }

    #[test]
    fn rejects_burst_smaller_than_rate() {
        let mut cfg = valid();
        cfg.requests_per_second = 10;
        cfg.burst_size = 5;
        assert_rejected(&cfg);
    }

    #[test]
    fn rejects_absurd_max_retries() {
        let mut cfg = valid();
        cfg.max_retries = 1_000;
        assert_rejected(&cfg);
    }

    #[test]
    fn rejects_absurd_max_wait_ms() {
        let mut cfg = valid();
        cfg.max_wait_ms = 7_200_000;
        assert_rejected(&cfg);
    }
}
