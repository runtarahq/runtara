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

/// True when the connection type is an *interactive* OAuth authorization-code
/// type — one that requires the user to complete a consent popup before it is
/// usable. Only authorization-code types declare an `oauth_config` (via the
/// `oauth_auth_url`/`oauth_token_url` descriptor attrs); the static `auth_url` is
/// empty for the params-driven generic type, so `oauth_config` *presence* is the
/// discriminator, matching the frontend's `isOAuthAuthCode = !!oauthConfig`.
/// Client-credentials OAuth types declare no `oauth_config` — they mint their own
/// token on demand and are usable immediately — so they are excluded.
fn requires_interactive_oauth(integration_id: &str) -> bool {
    runtara_agents::registry::find_connection_type(integration_id)
        .is_some_and(|m| m.oauth_config.is_some())
}

/// True when the connection parameters already carry an OAuth token — e.g. a
/// connection seeded with credentials rather than created for the interactive
/// popup flow. Such a connection is usable straight away.
fn params_have_oauth_tokens(params: Option<&serde_json::Value>) -> bool {
    params.is_some_and(|p| {
        ["access_token", "refresh_token"].iter().any(|k| {
            p.get(k)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
        })
    })
}

/// PATCH-merge submitted connection parameters over the stored set: a non-empty
/// submitted value overrides the existing one; an absent, null, or blank-string
/// value keeps the existing one. This preserves server-captured OAuth params
/// (`access_token` / `refresh_token` / `token_expires_at` / provider callback
/// params like `realm_id`) and untouched secrets — none of which the UI ever
/// receives (the connection DTO omits `connection_parameters`), so it cannot
/// resend them — and honors the edit form's "leaving fields empty keeps existing
/// values" contract. Without it, editing any field (e.g. `environment`) on an
/// authorized OAuth connection would wipe its tokens.
fn merge_connection_parameters(
    existing: &serde_json::Value,
    submitted: &serde_json::Value,
) -> serde_json::Value {
    let mut merged = existing.as_object().cloned().unwrap_or_default();
    if let Some(obj) = submitted.as_object() {
        for (k, v) in obj {
            let blank = v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty());
            if blank {
                continue; // keep the existing value
            }
            merged.insert(k.clone(), v.clone());
        }
    }
    serde_json::Value::Object(merged)
}

fn build_edit_projection(
    integration_id: &str,
    params: Option<&serde_json::Value>,
    version: String,
) -> ConnectionEditProjection {
    let mut values = serde_json::Map::new();
    let mut secret_state = std::collections::HashMap::new();
    let params = params.and_then(serde_json::Value::as_object);

    if let Some(meta) = runtara_agents::registry::find_connection_type(integration_id) {
        for field in meta.fields {
            let current = params.and_then(|params| params.get(field.name));
            if field.is_secret {
                let configured = current.is_some_and(|value| {
                    !value.is_null() && !value.as_str().is_some_and(|value| value.is_empty())
                });
                secret_state.insert(
                    field.name.to_string(),
                    ConnectionSecretState {
                        configured,
                        clearable: false,
                    },
                );
            } else if field.access != runtara_dsl::form::FieldAccessMode::Write
                && let Some(value) = current
            {
                values.insert(field.name.to_string(), value.clone());
            }
        }
    }

    // `auth_mode` was added after SFTP connections already existed. Preserve
    // old key-only records by projecting the mode their stored credentials
    // imply; new saves persist the explicit field normally.
    if integration_id == "sftp" && params.is_none_or(|params| !params.contains_key("auth_mode")) {
        let inferred = if params
            .and_then(|params| params.get("private_key"))
            .is_some_and(|value| !value.is_null() && value.as_str().is_none_or(|s| !s.is_empty()))
        {
            "private_key"
        } else {
            "password"
        };
        values.insert(
            "auth_mode".to_string(),
            serde_json::Value::String(inferred.to_string()),
        );
    }

    ConnectionEditProjection {
        values: serde_json::Value::Object(values),
        secret_state,
        version,
    }
}

fn apply_connection_parameter_patch(
    integration_id: &str,
    existing: &serde_json::Value,
    patch: &ConnectionParameterPatch,
) -> Result<serde_json::Value, ServiceError> {
    let meta = runtara_agents::registry::find_connection_type(integration_id).ok_or_else(|| {
        ServiceError::ValidationError(format!(
            "Unknown connection type '{integration_id}' cannot accept a parameter patch"
        ))
    })?;
    let fields: std::collections::HashMap<_, _> = meta
        .fields
        .iter()
        .map(|field| (field.name, field))
        .collect();
    let mut values = existing.as_object().cloned().unwrap_or_default();
    let mut touched = std::collections::HashSet::new();

    for (name, value) in &patch.set {
        let field = fields.get(name.as_str()).ok_or_else(|| {
            ServiceError::ValidationError(format!("Unknown connection field '{name}'"))
        })?;
        if !touched.insert(name.as_str()) {
            return Err(ServiceError::ValidationError(format!(
                "Connection field '{name}' appears in multiple patch operations"
            )));
        }
        if field.is_secret || field.access != runtara_dsl::form::FieldAccessMode::ReadWrite {
            return Err(ServiceError::ValidationError(format!(
                "Connection field '{name}' cannot be set"
            )));
        }
        values.insert(name.clone(), value.clone());
    }

    for (name, value) in &patch.replace_secrets {
        let field = fields.get(name.as_str()).ok_or_else(|| {
            ServiceError::ValidationError(format!("Unknown connection field '{name}'"))
        })?;
        if !touched.insert(name.as_str()) {
            return Err(ServiceError::ValidationError(format!(
                "Connection field '{name}' appears in multiple patch operations"
            )));
        }
        if !field.is_secret || field.access != runtara_dsl::form::FieldAccessMode::Write {
            return Err(ServiceError::ValidationError(format!(
                "Connection field '{name}' is not a replaceable secret"
            )));
        }
        if value.is_empty() {
            return Err(ServiceError::ValidationError(format!(
                "Replacement secret '{name}' cannot be blank"
            )));
        }
        values.insert(name.clone(), serde_json::Value::String(value.clone()));
    }

    for name in &patch.clear {
        let field = fields.get(name.as_str()).ok_or_else(|| {
            ServiceError::ValidationError(format!("Unknown connection field '{name}'"))
        })?;
        if !touched.insert(name.as_str()) {
            return Err(ServiceError::ValidationError(format!(
                "Connection field '{name}' appears in multiple patch operations"
            )));
        }
        if field.is_secret || field.access != runtara_dsl::form::FieldAccessMode::ReadWrite {
            return Err(ServiceError::ValidationError(format!(
                "Connection field '{name}' cannot be cleared"
            )));
        }
        values.remove(name);
    }

    let merged = serde_json::Value::Object(values);
    let definition = runtara_dsl::form::connection_form_definition(meta);
    let form_values = serde_json::Value::Object(
        meta.fields
            .iter()
            .filter_map(|field| {
                merged
                    .get(field.name)
                    .cloned()
                    .map(|value| (field.name.to_string(), value))
            })
            .collect(),
    );
    let analysis = runtara_dsl::form::analyze_form(&definition, &form_values);
    if !analysis.valid {
        return Err(ServiceError::ValidationError(
            analysis
                .issues
                .into_iter()
                .map(|issue| format!("{}: {}", issue.path, issue.message))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(merged)
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

        // An interactive OAuth (authorization-code) connection is not usable until
        // the consent popup completes and the callback stores tokens + flips it to
        // ACTIVE. Created without tokens it must start REQUIRES_RECONNECTION, so the
        // UI surfaces the reconnect affordance (and an honest status) instead of a
        // misleading "Connected". Callers that pre-seed tokens or an explicit status
        // are respected; client-credentials OAuth types (no auth_url) stay ACTIVE.
        if request.status.is_none()
            && requires_interactive_oauth(&integration_id)
            && !params_have_oauth_tokens(request.connection_parameters.as_ref())
        {
            request.status = Some(ConnectionStatus::RequiresReconnection);
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
        let mut connection = self
            .repository
            .get_by_id(id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Connection not found".to_string()))?;
        let parameters = self
            .repository
            .get_with_parameters(id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .and_then(|connection| connection.connection_parameters);
        if let Some(integration_id) = connection.integration_id.as_deref() {
            connection.edit_projection = Some(build_edit_projection(
                integration_id,
                parameters.as_ref(),
                connection.updated_at.clone(),
            ));
        }
        Ok(connection)
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
            || request.connection_parameter_patch.is_some()
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

        let mut request = request;
        if request.connection_parameters.is_some() && request.connection_parameter_patch.is_some() {
            return Err(ServiceError::ValidationError(
                "Use connectionParameterPatch or legacy connectionParameters, not both".to_string(),
            ));
        }
        let expected_version = request
            .connection_parameter_patch
            .as_ref()
            .map(|patch| patch.version.clone());
        if let (Some(expected), Some(current)) = (&expected_version, &current_connection)
            && expected != &current.updated_at
        {
            return Err(ServiceError::Conflict(
                "Connection changed since it was opened; reload before saving".to_string(),
            ));
        }
        if let Some(ref default_for) = default_for {
            request.is_default_file_storage = Some(
                default_for
                    .iter()
                    .any(|value| value == OBJECT_STORAGE_DEFAULT_FOR),
            );
        }

        // When this PATCH touches connection_parameters, merge the submitted fields
        // over the stored set instead of replacing wholesale. `old_params` is read
        // once and reused for the merge and for SSRF rule E below.
        let old_params = if request.connection_parameters.is_some()
            || request.connection_parameter_patch.is_some()
        {
            self.repository
                .get_with_parameters(id, tenant_id)
                .await
                .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
                .and_then(|c| c.connection_parameters)
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        };
        if let Some(submitted) = request.connection_parameters.take() {
            request.connection_parameters =
                Some(merge_connection_parameters(&old_params, &submitted));
        }
        if let Some(patch) = request.connection_parameter_patch.take() {
            request.connection_parameters = Some(apply_connection_parameter_patch(
                &integration_id,
                &old_params,
                &patch,
            )?);
        }

        // Validate the EFFECTIVE (merged) parameters against the type schema — a
        // partial edit (e.g. only `environment`) must still satisfy required fields
        // that live in the preserved existing params.
        if request.connection_parameters.is_some() {
            validate_connection_parameters(
                &integration_id,
                request.connection_parameters.as_ref(),
            )?;
        }

        if let Some(ref default_for) = default_for {
            self.validate_default_for(&integration_id, default_for)?;
        }

        // SSRF rule E: on a params-driven OAuth type, changing an endpoint param
        // must invalidate the captured tokens (DB + in-memory cache) and force a
        // reconnect. Otherwise a captured refresh token could be replayed against
        // a swapped-in token endpoint, or a still-cached Bearer would flow to the
        // newly-edited base_url host. Compared against the merged (effective) params,
        // so a blank submission that kept the old endpoint is correctly a no-change.
        let endpoint_edit_requires_reauth =
            if let Some(ref merged_params) = request.connection_parameters {
                let params_driven = runtara_agents::registry::find_connection_type(&integration_id)
                    .and_then(|meta| meta.oauth_config)
                    .map(|cfg| cfg.params_driven)
                    .unwrap_or(false);
                const ENDPOINT_KEYS: [&str; 5] = [
                    "auth_url",
                    "token_url",
                    "base_url",
                    "revocation_url",
                    "token_auth",
                ];
                let changed = ENDPOINT_KEYS
                    .iter()
                    .any(|k| old_params.get(k) != merged_params.get(k));
                if params_driven && changed {
                    // Strip the merge-preserved captured tokens from what's written.
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
            .update(id, tenant_id, &request, expected_version.as_deref())
            .await
            .map_err(|e| {
                // Check for unique constraint violation on title
                if e.to_string().contains("uc_connection_data_entity_title") {
                    ServiceError::Conflict("Connection with this title already exists".to_string())
                } else {
                    ServiceError::DatabaseError(e.to_string())
                }
            })?;

        if rows_affected == 0 && expected_version.is_some() {
            return Err(ServiceError::Conflict(
                "Connection changed since it was opened; reload before saving".to_string(),
            ));
        }
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
        ServiceError, apply_connection_parameter_patch, build_edit_projection,
        params_have_oauth_tokens, requires_interactive_oauth, validate_connection_parameters,
        validate_rate_limit_config, validate_url_field,
    };
    use crate::types::{ConnectionParameterPatch, RateLimitConfigDto};
    use serde_json::json;
    use std::collections::HashMap;

    // ── base-URL validation (F1 creation side) ───────────────────────────────

    fn is_validation_err(r: Result<(), ServiceError>) -> bool {
        matches!(r, Err(ServiceError::ValidationError(_)))
    }

    #[test]
    fn edit_projection_returns_readable_values_without_secrets() {
        let projection = build_edit_projection(
            "quickbooks_online",
            Some(&json!({
                "client_id": "client-id",
                "client_secret": "never-return-me",
                "environment": "production",
                "realm_id": "12345",
                "access_token": "server-only-token"
            })),
            "2026-07-12T08:00:00Z".to_string(),
        );

        assert_eq!(projection.values["client_id"], "client-id");
        assert_eq!(projection.values["environment"], "production");
        assert_eq!(projection.values["realm_id"], "12345");
        assert!(projection.values.get("client_secret").is_none());
        assert!(projection.values.get("access_token").is_none());
        assert!(projection.secret_state["client_secret"].configured);
        assert_eq!(projection.version, "2026-07-12T08:00:00Z");
    }

    #[test]
    fn sftp_legacy_projection_infers_auth_mode() {
        let private_key = build_edit_projection(
            "sftp",
            Some(&json!({
                "host": "files.example.com",
                "private_key": "-----BEGIN PRIVATE KEY-----"
            })),
            "v1".to_string(),
        );
        assert_eq!(private_key.values["auth_mode"], "private_key");

        let password = build_edit_projection(
            "sftp",
            Some(&json!({
                "host": "files.example.com",
                "password": "stored-secret"
            })),
            "v1".to_string(),
        );
        assert_eq!(password.values["auth_mode"], "password");
    }

    #[test]
    fn every_registered_connection_descriptor_produces_a_valid_form() {
        for meta in runtara_agents::registry::get_all_connection_types() {
            let definition = runtara_dsl::form::connection_form_definition(meta);
            let issues = runtara_dsl::form::validate_form_definition(&definition);
            assert!(issues.is_empty(), "{}: {issues:?}", meta.integration_id);
        }
    }

    #[test]
    fn explicit_patch_preserves_untouched_secrets_and_rejects_read_fields() {
        let existing = json!({
            "client_id": "client",
            "client_secret": "old-secret",
            "environment": "production",
            "realm_id": "managed",
            "scopes": "com.intuit.quickbooks.accounting",
            "access_token": "server-captured"
        });
        let patch = ConnectionParameterPatch {
            version: "v1".to_string(),
            set: HashMap::from([("environment".to_string(), json!("sandbox"))]),
            replace_secrets: HashMap::new(),
            clear: Vec::new(),
        };
        let merged =
            apply_connection_parameter_patch("quickbooks_online", &existing, &patch).unwrap();
        assert_eq!(merged["environment"], "sandbox");
        assert_eq!(merged["client_secret"], "old-secret");
        assert_eq!(merged["access_token"], "server-captured");

        let read_patch = ConnectionParameterPatch {
            version: "v1".to_string(),
            set: HashMap::from([("realm_id".to_string(), json!("hijack"))]),
            replace_secrets: HashMap::new(),
            clear: Vec::new(),
        };
        assert!(matches!(
            apply_connection_parameter_patch("quickbooks_online", &existing, &read_patch),
            Err(ServiceError::ValidationError(_))
        ));
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
    fn interactive_oauth_detection_gates_on_oauth_config() {
        // Authorization-code types (interactive consent) → true.
        assert!(requires_interactive_oauth("quickbooks_online"));
        assert!(requires_interactive_oauth("http_oauth2_authorization_code"));
        // Client-credentials OAuth (empty auth_url, mints its own token) → false.
        assert!(!requires_interactive_oauth(
            "http_oauth2_client_credentials"
        ));
        // Non-OAuth types (no oauth_config) → false.
        assert!(!requires_interactive_oauth("http_bearer"));
        assert!(!requires_interactive_oauth("totally_unknown"));
    }

    #[test]
    fn oauth_token_presence_detection() {
        assert!(params_have_oauth_tokens(Some(
            &json!({"access_token": "a"})
        )));
        assert!(params_have_oauth_tokens(Some(
            &json!({"refresh_token": "r"})
        )));
        // Empty / whitespace tokens don't count as present.
        assert!(!params_have_oauth_tokens(Some(
            &json!({"access_token": ""})
        )));
        assert!(!params_have_oauth_tokens(Some(
            &json!({"client_id": "c", "client_secret": "s"})
        )));
        assert!(!params_have_oauth_tokens(None));
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
