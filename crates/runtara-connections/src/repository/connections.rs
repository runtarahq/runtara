//! Connection Repository
//!
//! Handles all database operations for connection management.
//!
//! SECURITY: This is the only layer that reads or writes `connection_parameters`
//! directly. Encryption at rest is applied here — writes encrypt via
//! [`CredentialCipher::encrypt`], reads decrypt via [`CredentialCipher::decrypt`].
//! Consumers of the facade always see plaintext.
//!
//! SECURITY: Provides methods to explicitly exclude connection_parameters from queries

use std::collections::HashMap;
use std::sync::Arc;

use crate::crypto::{CipherContext, CredentialCipher, is_envelope};
use crate::types::*;
use sqlx::PgPool;

/// SHA-256 (hex) of a refresh token — a non-reversible fingerprint stored in the
/// `refresh_token_hash` column for the rotation optimistic-concurrency guard. The
/// token value itself is never persisted in plaintext.
/// The rotation guard of a stored refresh token: the hash of the token, or
/// of its envelope when it is sealed. Each rotation seals a new envelope, so
/// the guard tells one stored token from the next either way.
pub fn refresh_token_guard(stored: &serde_json::Value) -> Option<String> {
    match stored {
        serde_json::Value::String(token) => Some(refresh_token_hash(token)),
        sealed if crate::sealed::is_sealed(sealed) => Some(refresh_token_hash(
            &sealed[crate::sealed::SEALED_MARKER]["envelope"].to_string(),
        )),
        _ => None,
    }
}

pub fn refresh_token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// The parameter of a `postgres` connection that stands for the platform's
/// own database (the deployment's `OBJECT_MODEL_DATABASE_URL`): the
/// connection holds no URL and no secret, and whoever uses it uses the
/// platform's database, which is the operator's to configure.
pub const PLATFORM_DATABASE_PARAM: &str = "platform_database";

/// How a connection's secrets are held, as `credential_format` records it.
pub const FORMAT_CIPHER: &str = "cipher";
/// See [`FORMAT_CIPHER`]: each secret inside the parameters is sealed by seal.
pub const FORMAT_SEAL: &str = "seal";

/// The format of the parameters about to be stored: sealed if any secret in
/// them is a sealed value's marker (see [`crate::sealed`]).
fn credential_format(parameters: Option<&serde_json::Value>) -> &'static str {
    if parameters.is_some_and(crate::sealed::has_sealed_values) {
        FORMAT_SEAL
    } else {
        FORMAT_CIPHER
    }
}

/// Convert JSONB value from DB to RateLimitConfigDto
fn parse_rate_limit_config(value: Option<serde_json::Value>) -> Option<RateLimitConfigDto> {
    value.and_then(|v| serde_json::from_value(v).ok())
}

pub struct ConnectionRepository {
    pool: PgPool,
    cipher: Arc<dyn CredentialCipher>,
}

/// Server-derived values that may accompany a public versioned update but are
/// never accepted as unrestricted public request fields.
pub struct ConnectionUpdateValues<'a> {
    pub parameters: Option<&'a serde_json::Value>,
    pub status: Option<&'a ConnectionStatus>,
    pub default_for: Option<&'a [String]>,
}

impl ConnectionRepository {
    /// Construct a repository with a cipher for at-rest encryption.
    pub fn new(pool: PgPool, cipher: Arc<dyn CredentialCipher>) -> Self {
        Self { pool, cipher }
    }

    /// Encrypt connection parameters for storage. `None` in → `None` out.
    ///
    /// Treats cipher failure as a database error so callers can propagate it
    /// without introducing a new error variant. Logs the underlying cause.
    fn seal(
        &self,
        tenant_id: &str,
        connection_id: &str,
        plaintext: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, sqlx::Error> {
        let Some(value) = plaintext else {
            return Ok(None);
        };
        let context = CipherContext {
            tenant_id,
            connection_id,
        };
        match self.cipher.encrypt(value, &context) {
            Ok(envelope) => Ok(Some(envelope)),
            Err(e) => {
                tracing::error!(error = %e, "Failed to encrypt connection parameters");
                Err(sqlx::Error::Encode(format!("cipher encrypt: {}", e).into()))
            }
        }
    }

    /// Decrypt connection parameters after retrieval. `None` in → `None` out.
    ///
    /// A plaintext value is an error: see [`Self::encrypt_plaintext`].
    fn unseal(
        &self,
        tenant_id: &str,
        connection_id: &str,
        stored: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, sqlx::Error> {
        let Some(value) = stored else {
            return Ok(None);
        };
        let context = CipherContext {
            tenant_id,
            connection_id,
        };
        match self.cipher.decrypt(&value, &context) {
            Ok(plaintext) => Ok(Some(plaintext)),
            Err(e) => {
                tracing::error!(error = %e, "Failed to decrypt connection parameters");
                Err(sqlx::Error::Decode(format!("cipher decrypt: {}", e).into()))
            }
        }
    }

    async fn default_for_map(
        &self,
        tenant_id: &str,
        connection_ids: &[String],
    ) -> Result<HashMap<String, Vec<String>>, sqlx::Error> {
        if connection_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query_as::<_, (String, String)>(
            r#"
            SELECT connection_id, default_for
            FROM connection_defaults
            WHERE tenant_id = $1 AND connection_id = ANY($2)
            ORDER BY default_for
            "#,
        )
        .bind(tenant_id)
        .bind(connection_ids)
        .fetch_all(&self.pool)
        .await?;

        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for (connection_id, default_for) in rows {
            map.entry(connection_id).or_default().push(default_for);
        }
        Ok(map)
    }

    async fn attach_default_for(
        &self,
        tenant_id: &str,
        connections: &mut [ConnectionDto],
    ) -> Result<(), sqlx::Error> {
        let ids: Vec<String> = connections.iter().map(|c| c.id.clone()).collect();
        let defaults = self.default_for_map(tenant_id, &ids).await?;
        for connection in connections {
            connection.default_for = defaults.get(&connection.id).cloned().unwrap_or_default();
        }
        Ok(())
    }

    /// Create a new connection
    pub async fn create(
        &self,
        request: &CreateConnectionRequest,
        tenant_id: &str,
        connection_id: &str,
        status: &ConnectionStatus,
        default_for: &[String],
    ) -> Result<(), sqlx::Error> {
        let rate_limit_json = request
            .rate_limit_config
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok());

        // Encrypt connection_parameters at rest (no-op if cipher is NoOp).
        let sealed_parameters = self.seal(
            tenant_id,
            connection_id,
            request.connection_parameters.as_ref(),
        )?;

        let mut tx = self.pool.begin().await?;
        if request.is_default_file_storage == Some(true) {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
                .bind(tenant_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                r#"
                UPDATE connection_data_entity
                SET is_default_file_storage = FALSE, updated_at = NOW()
                WHERE tenant_id = $1 AND is_default_file_storage = TRUE
                "#,
            )
            .bind(tenant_id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            r#"
            INSERT INTO connection_data_entity
            (id, tenant_id, title, connection_subtype, connection_parameters, integration_id, valid_until, status, rate_limit_config, is_default_file_storage, credential_format)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(connection_id)
        .bind(tenant_id)
        .bind(&request.title)
        .bind(&request.connection_subtype)
        .bind(&sealed_parameters)
        .bind(&request.integration_id)
        .bind(request.valid_until.as_ref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&chrono::Utc))))
        .bind(status.as_str())
        .bind(rate_limit_json.as_ref())
        .bind(request.is_default_file_storage.unwrap_or(false))
        .bind(credential_format(request.connection_parameters.as_ref()))
        .execute(&mut *tx)
        .await?;

        for operator_id in default_for {
            sqlx::query(
                r#"
                INSERT INTO connection_defaults (tenant_id, default_for, connection_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (tenant_id, default_for)
                DO UPDATE SET connection_id = EXCLUDED.connection_id, updated_at = NOW()
                "#,
            )
            .bind(tenant_id)
            .bind(operator_id)
            .bind(connection_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// List all connections for a tenant with optional filters
    /// SECURITY: Explicitly excludes connection_parameters from SELECT
    pub async fn list(
        &self,
        tenant_id: &str,
        integration_id: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<ConnectionDto>, sqlx::Error> {
        // Build query with optional filters
        // SECURITY: Explicitly exclude connection_parameters from SELECT
        let mut query = String::from(
            r#"
            SELECT id, tenant_id, created_at, valid_until, updated_at, title,
                   connection_subtype, integration_id, status, rate_limit_config,
                   is_default_file_storage
            FROM connection_data_entity
            WHERE tenant_id = $1
            "#,
        );

        let mut condition_strings = vec![];
        if integration_id.is_some() {
            condition_strings.push("integration_id = $2".to_string());
        }
        if status.is_some() {
            let param_idx = if integration_id.is_some() { 3 } else { 2 };
            condition_strings.push(format!("status = ${}", param_idx));
        }

        if !condition_strings.is_empty() {
            query.push_str(" AND ");
            query.push_str(&condition_strings.join(" AND "));
        }

        query.push_str(" ORDER BY created_at DESC");

        // Execute query
        let rows = if let Some(int_id) = integration_id {
            if let Some(status_val) = status {
                sqlx::query_as::<
                    _,
                    (
                        String,
                        String,
                        chrono::DateTime<chrono::Utc>,
                        Option<chrono::DateTime<chrono::Utc>>,
                        chrono::DateTime<chrono::Utc>,
                        String,
                        Option<String>,
                        Option<String>,
                        String,
                        Option<serde_json::Value>,
                        bool,
                    ),
                >(&query)
                .bind(tenant_id)
                .bind(int_id)
                .bind(status_val)
                .fetch_all(&self.pool)
                .await?
            } else {
                sqlx::query_as::<
                    _,
                    (
                        String,
                        String,
                        chrono::DateTime<chrono::Utc>,
                        Option<chrono::DateTime<chrono::Utc>>,
                        chrono::DateTime<chrono::Utc>,
                        String,
                        Option<String>,
                        Option<String>,
                        String,
                        Option<serde_json::Value>,
                        bool,
                    ),
                >(&query)
                .bind(tenant_id)
                .bind(int_id)
                .fetch_all(&self.pool)
                .await?
            }
        } else if let Some(status_val) = status {
            sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    chrono::DateTime<chrono::Utc>,
                    Option<chrono::DateTime<chrono::Utc>>,
                    chrono::DateTime<chrono::Utc>,
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                    Option<serde_json::Value>,
                    bool,
                ),
            >(&query)
            .bind(tenant_id)
            .bind(status_val)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    chrono::DateTime<chrono::Utc>,
                    Option<chrono::DateTime<chrono::Utc>>,
                    chrono::DateTime<chrono::Utc>,
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                    Option<serde_json::Value>,
                    bool,
                ),
            >(&query)
            .bind(tenant_id)
            .fetch_all(&self.pool)
            .await?
        };

        let mut connections: Vec<ConnectionDto> = rows
            .into_iter()
            .map(
                |(
                    id,
                    tenant_id,
                    created_at,
                    valid_until,
                    updated_at,
                    title,
                    connection_subtype,
                    integration_id,
                    status,
                    rate_limit_config,
                    is_default_file_storage,
                )| {
                    ConnectionDto {
                        id,
                        tenant_id,
                        created_at: created_at.to_rfc3339(),
                        valid_until: valid_until.map(|dt| dt.to_rfc3339()),
                        updated_at: updated_at.to_rfc3339(),
                        title,
                        connection_subtype,
                        resources: crate::resource_resolver::resources_for_integration(
                            integration_id.as_deref().unwrap_or_default(),
                        ),
                        integration_id,
                        status: ConnectionStatus::parse(&status),
                        rate_limit_config: parse_rate_limit_config(rate_limit_config),
                        rate_limit_stats: None,
                        is_default_file_storage,
                        default_for: Vec::new(),
                        edit_projection: None,
                        grant_state: None,
                    }
                },
            )
            .collect();

        self.attach_default_for(tenant_id, &mut connections).await?;

        Ok(connections)
    }

    /// Get a connection by ID
    /// SECURITY: Explicitly excludes connection_parameters from SELECT
    pub async fn get_by_id(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<Option<ConnectionDto>, sqlx::Error> {
        let result = sqlx::query_as::<
            _,
            (
                String,
                String,
                chrono::DateTime<chrono::Utc>,
                Option<chrono::DateTime<chrono::Utc>>,
                chrono::DateTime<chrono::Utc>,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<serde_json::Value>,
                bool,
            ),
        >(
            r#"
            SELECT id, tenant_id, created_at, valid_until, updated_at, title,
                   connection_subtype, integration_id, status, rate_limit_config,
                   is_default_file_storage
            FROM connection_data_entity
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;

        let mut connection = result.map(
            |(
                id,
                tenant_id,
                created_at,
                valid_until,
                updated_at,
                title,
                connection_subtype,
                integration_id,
                status,
                rate_limit_config,
                is_default_file_storage,
            )| {
                ConnectionDto {
                    id,
                    tenant_id,
                    created_at: created_at.to_rfc3339(),
                    valid_until: valid_until.map(|dt| dt.to_rfc3339()),
                    updated_at: updated_at.to_rfc3339(),
                    title,
                    connection_subtype,
                    resources: crate::resource_resolver::resources_for_integration(
                        integration_id.as_deref().unwrap_or_default(),
                    ),
                    integration_id,
                    status: ConnectionStatus::parse(&status),
                    rate_limit_config: parse_rate_limit_config(rate_limit_config),
                    rate_limit_stats: None,
                    is_default_file_storage,
                    default_for: Vec::new(),
                    edit_projection: None,
                    grant_state: None,
                }
            },
        );

        if let Some(ref mut connection) = connection {
            self.attach_default_for(tenant_id, std::slice::from_mut(connection))
                .await?;
        }

        Ok(connection)
    }

    /// Update a connection with dynamic fields
    pub async fn update(
        &self,
        id: &str,
        tenant_id: &str,
        request: &UpdateConnectionRequest,
        values: ConnectionUpdateValues<'_>,
        expected_updated_at: &str,
    ) -> Result<u64, sqlx::Error> {
        // Build dynamic UPDATE query based on provided fields
        let mut updates = vec!["updated_at = NOW()".to_string()];
        let mut param_idx = 3;

        if request.title.is_some() {
            updates.push(format!("title = ${}", param_idx));
            param_idx += 1;
        }
        if values.parameters.is_some() {
            updates.push(format!("connection_parameters = ${}", param_idx));
            param_idx += 1;
            updates.push(format!("credential_format = ${}", param_idx));
            param_idx += 1;
        }
        if request.rate_limit_config.is_some() {
            updates.push(format!("rate_limit_config = ${}", param_idx));
            param_idx += 1;
        }
        if request.valid_until.is_some() {
            updates.push(format!("valid_until = ${}", param_idx));
            param_idx += 1;
        }
        if values.status.is_some() {
            updates.push(format!("status = ${}", param_idx));
            param_idx += 1;
        }
        if request.is_default_file_storage.is_some() {
            updates.push(format!("is_default_file_storage = ${}", param_idx));
            param_idx += 1;
        }

        let version_parameter = param_idx;
        let version_clause = format!(" AND updated_at = ${version_parameter}");
        let query_str = format!(
            "UPDATE connection_data_entity SET {} WHERE id = $1 AND tenant_id = $2{}",
            updates.join(", "),
            version_clause
        );

        // Pre-encrypt parameters (if any) before binding — must live long
        // enough for the bind to reference it.
        let sealed_parameters = match values.parameters {
            Some(plain) => Some(
                self.seal(tenant_id, id, Some(plain))?
                    .unwrap_or(serde_json::Value::Null),
            ),
            None => None,
        };

        // Execute update with dynamic bindings
        let mut query = sqlx::query(&query_str).bind(id).bind(tenant_id);

        if let Some(ref title) = request.title {
            query = query.bind(title);
        }
        if let Some(ref connection_parameters) = sealed_parameters {
            query = query
                .bind(connection_parameters)
                .bind(credential_format(values.parameters));
        }
        if let Some(ref rate_limit_config) = request.rate_limit_config {
            let json_value =
                serde_json::to_value(rate_limit_config).unwrap_or(serde_json::Value::Null);
            query = query.bind(json_value);
        }
        if let Some(ref valid_until) = request.valid_until {
            let parsed_dt = chrono::DateTime::parse_from_rfc3339(valid_until)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc));
            query = query.bind(parsed_dt);
        }
        if let Some(status) = values.status {
            query = query.bind(status.as_str());
        }
        if let Some(is_default) = request.is_default_file_storage {
            query = query.bind(is_default);
        }
        let parsed = chrono::DateTime::parse_from_rfc3339(expected_updated_at)
            .map_err(|error| sqlx::Error::Encode(error.into()))?
            .with_timezone(&chrono::Utc);
        query = query.bind(parsed);

        let mut tx = self.pool.begin().await?;
        if request.is_default_file_storage == Some(true) {
            // Serialize the tenant-scoped singleton transition and keep it in
            // the same transaction as the optimistic row update.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
                .bind(tenant_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                r#"
                UPDATE connection_data_entity
                SET is_default_file_storage = FALSE, updated_at = NOW()
                WHERE tenant_id = $1 AND id <> $2 AND is_default_file_storage = TRUE
                "#,
            )
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }

        let result = query.execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(0);
        }

        if let Some(default_for) = values.default_for {
            sqlx::query(
                r#"
                DELETE FROM connection_defaults
                WHERE tenant_id = $1 AND connection_id = $2
                "#,
            )
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            for operator_id in default_for {
                sqlx::query(
                    r#"
                    INSERT INTO connection_defaults (tenant_id, default_for, connection_id)
                    VALUES ($1, $2, $3)
                    ON CONFLICT (tenant_id, default_for)
                    DO UPDATE SET connection_id = EXCLUDED.connection_id, updated_at = NOW()
                    "#,
                )
                .bind(tenant_id)
                .bind(operator_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(result.rows_affected())
    }

    /// Delete a connection
    pub async fn delete(&self, id: &str, tenant_id: &str) -> Result<u64, sqlx::Error> {
        let result =
            sqlx::query("DELETE FROM connection_data_entity WHERE id = $1 AND tenant_id = $2")
                .bind(id)
                .bind(tenant_id)
                .execute(&self.pool)
                .await?;

        Ok(result.rows_affected())
    }

    /// Get connections by operator name
    /// Searches by integration_id (matching operator's supported integration_ids)
    /// SECURITY: Explicitly excludes connection_parameters from SELECT
    ///
    /// An empty `integration_ids` means "this agent accepts no integration" and
    /// always yields an empty list — never the tenant's whole connection set.
    /// The filter is the only thing scoping the result to the caller's agent,
    /// so dropping it (for any combination of the other filters) would leak
    /// every integration's connections into an unrelated agent's picker.
    pub async fn list_by_operator(
        &self,
        tenant_id: &str,
        integration_ids: &[String],
        status: Option<&str>,
    ) -> Result<Vec<ConnectionDto>, sqlx::Error> {
        // Fail closed before touching the DB: no ids, no rows.
        if integration_ids.is_empty() {
            return Ok(vec![]);
        }

        type Row = (
            String,
            String,
            chrono::DateTime<chrono::Utc>,
            Option<chrono::DateTime<chrono::Utc>>,
            chrono::DateTime<chrono::Utc>,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<serde_json::Value>,
            bool,
        );

        // SECURITY: Explicitly exclude connection_parameters from SELECT
        let mut query = String::from(
            r#"
            SELECT id, tenant_id, created_at, valid_until, updated_at, title,
                   connection_subtype, integration_id, status, rate_limit_config,
                   is_default_file_storage
            FROM connection_data_entity
            WHERE tenant_id = $1 AND integration_id = ANY($2)
            "#,
        );

        // Add optional status filter
        if status.is_some() {
            query.push_str(" AND status = $3");
        }

        query.push_str(" ORDER BY created_at DESC");

        let rows = if let Some(status_val) = status {
            sqlx::query_as::<_, Row>(&query)
                .bind(tenant_id)
                .bind(integration_ids)
                .bind(status_val)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query_as::<_, Row>(&query)
                .bind(tenant_id)
                .bind(integration_ids)
                .fetch_all(&self.pool)
                .await?
        };

        let mut connections: Vec<ConnectionDto> = rows
            .into_iter()
            .map(
                |(
                    id,
                    tenant_id,
                    created_at,
                    valid_until,
                    updated_at,
                    title,
                    connection_subtype,
                    integration_id,
                    status,
                    rate_limit_config,
                    is_default_file_storage,
                )| {
                    ConnectionDto {
                        id,
                        tenant_id,
                        created_at: created_at.to_rfc3339(),
                        valid_until: valid_until.map(|dt| dt.to_rfc3339()),
                        updated_at: updated_at.to_rfc3339(),
                        title,
                        connection_subtype,
                        resources: crate::resource_resolver::resources_for_integration(
                            integration_id.as_deref().unwrap_or_default(),
                        ),
                        integration_id,
                        status: ConnectionStatus::parse(&status),
                        rate_limit_config: parse_rate_limit_config(rate_limit_config),
                        rate_limit_stats: None,
                        is_default_file_storage,
                        default_for: Vec::new(),
                        edit_projection: None,
                        grant_state: None,
                    }
                },
            )
            .collect();

        self.attach_default_for(tenant_id, &mut connections).await?;

        Ok(connections)
    }

    /// Check which connection IDs exist for a tenant
    /// Returns a set of IDs that exist in the database
    pub async fn get_existing_ids(
        &self,
        tenant_id: &str,
        connection_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, sqlx::Error> {
        if connection_ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }

        let rows = sqlx::query_scalar::<_, String>(
            r#"
            SELECT id FROM connection_data_entity
            WHERE tenant_id = $1 AND id = ANY($2)
            "#,
        )
        .bind(tenant_id)
        .bind(connection_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().collect())
    }

    /// Get a connection by ID INCLUDING connection_parameters
    ///
    /// SECURITY WARNING: This method returns sensitive credentials.
    /// Only use for internal runtime connection resolution (runtara-workflows).
    /// Never expose this data to external API consumers.
    pub async fn get_with_parameters(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<Option<ConnectionWithParameters>, sqlx::Error> {
        self.get_with_parameters_matching(id, tenant_id, None).await
    }

    /// Filter the authoritative row before unsealing its parameters. Ownership,
    /// type and credentials therefore come from the same database snapshot.
    pub async fn get_with_parameters_for_types(
        &self,
        id: &str,
        tenant_id: &str,
        allowed: &[String],
    ) -> Result<Option<ConnectionWithParameters>, sqlx::Error> {
        self.get_with_parameters_matching(id, tenant_id, Some(allowed))
            .await
    }

    async fn get_with_parameters_matching(
        &self,
        id: &str,
        tenant_id: &str,
        allowed: Option<&[String]>,
    ) -> Result<Option<ConnectionWithParameters>, sqlx::Error> {
        let result = sqlx::query_as::<
            _,
            (
                String,                    // id
                String,                    // tenant_id
                Option<String>,            // integration_id
                Option<String>,            // connection_subtype
                Option<serde_json::Value>, // connection_parameters
                Option<serde_json::Value>, // rate_limit_config
                String,                    // credential_format
            ),
        >(
            r#"
            SELECT id, tenant_id, integration_id, connection_subtype, connection_parameters, rate_limit_config, credential_format
            FROM connection_data_entity
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;

        result
            .filter(|row| {
                allowed.is_none_or(|types| row.2.as_ref().is_some_and(|kind| types.contains(kind)))
            })
            .map(
                |(
                    id,
                    tid,
                    integration_id,
                    connection_subtype,
                    connection_parameters,
                    rate_limit_config,
                    credential_format,
                )| {
                    let connection_parameters = self.unseal(&tid, &id, connection_parameters)?;
                    Ok::<_, sqlx::Error>(ConnectionWithParameters {
                        id,
                        tenant_id: Some(tid),
                        integration_id,
                        connection_subtype,
                        connection_parameters,
                        rate_limit_config,
                        sealed: credential_format == FORMAT_SEAL,
                    })
                },
            )
            .transpose()
    }

    /// Get the connection id configured as the default for an agent/operator.
    pub async fn get_default_connection_id(
        &self,
        tenant_id: &str,
        default_for: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT connection_id
            FROM connection_defaults
            WHERE tenant_id = $1 AND default_for = $2
            "#,
        )
        .bind(tenant_id)
        .bind(default_for)
        .fetch_optional(&self.pool)
        .await
    }

    /// Get the default connection for an agent/operator, including parameters.
    ///
    /// SECURITY WARNING: Returns sensitive credentials. Internal use only.
    pub async fn get_default_connection_with_parameters(
        &self,
        tenant_id: &str,
        default_for: &str,
    ) -> Result<Option<ConnectionWithParameters>, sqlx::Error> {
        let Some(connection_id) = self
            .get_default_connection_id(tenant_id, default_for)
            .await?
        else {
            return Ok(None);
        };

        self.get_with_parameters(&connection_id, tenant_id).await
    }

    /// Return all agent/operator ids this connection is default for.
    pub async fn list_defaults_for_connection(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT default_for
            FROM connection_defaults
            WHERE tenant_id = $1 AND connection_id = $2
            ORDER BY default_for
            "#,
        )
        .bind(tenant_id)
        .bind(connection_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Replace all defaults assigned to a connection.
    ///
    /// Existing defaults for the requested operator ids are moved to this
    /// connection via the primary-key upsert.
    pub async fn replace_defaults_for_connection(
        &self,
        tenant_id: &str,
        connection_id: &str,
        default_for: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            DELETE FROM connection_defaults
            WHERE tenant_id = $1 AND connection_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;

        for operator_id in default_for {
            sqlx::query(
                r#"
                INSERT INTO connection_defaults (tenant_id, default_for, connection_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (tenant_id, default_for)
                DO UPDATE SET
                    connection_id = EXCLUDED.connection_id,
                    updated_at = NOW()
                "#,
            )
            .bind(tenant_id)
            .bind(operator_id)
            .bind(connection_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }

    /// Look up a connection by ID (without tenant filter) including credentials.
    ///
    /// SECURITY: Used only by channel webhook routing where the connection_id
    /// in the URL acts as the authentication token. The webhook URL is only
    /// known to the connection owner.
    pub async fn get_channel_connection(
        &self,
        id: &str,
    ) -> Result<Option<ConnectionWithParameters>, sqlx::Error> {
        let result = sqlx::query_as::<
            _,
            (
                String,                    // id
                String,                    // tenant_id
                Option<String>,            // integration_id
                Option<String>,            // connection_subtype
                Option<serde_json::Value>, // connection_parameters
                Option<serde_json::Value>, // rate_limit_config
                String,                    // credential_format
            ),
        >(
            r#"
            SELECT id, tenant_id, integration_id, connection_subtype, connection_parameters, rate_limit_config, credential_format
            FROM connection_data_entity
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        result
            .map(
                |(
                    id,
                    tid,
                    integration_id,
                    connection_subtype,
                    connection_parameters,
                    rate_limit_config,
                    credential_format,
                )| {
                    let connection_parameters = self.unseal(&tid, &id, connection_parameters)?;
                    Ok::<_, sqlx::Error>(ConnectionWithParameters {
                        id,
                        tenant_id: Some(tid),
                        integration_id,
                        connection_subtype,
                        connection_parameters,
                        rate_limit_config,
                        sealed: credential_format == FORMAT_SEAL,
                    })
                },
            )
            .transpose()
    }

    /// Update connection parameters and status atomically.
    /// Used by OAuth callback to store tokens and set status to ACTIVE.
    ///
    /// The rotation guard's `refresh_token_hash` is set to the hash of the
    /// refresh token written here. A new authorization starts a new chain: left
    /// at the old chain's hash, the guard would match no refresh token of the new
    /// one, and every rotation after a reconnect would be dropped by
    /// [`Self::persist_refreshed_oauth`]. Reset to `NULL` instead, it would let a
    /// refresh still running on the old chain overwrite the new tokens.
    pub async fn update_parameters_and_status(
        &self,
        id: &str,
        tenant_id: &str,
        parameters: &serde_json::Value,
        status: &str,
    ) -> Result<u64, sqlx::Error> {
        let sealed = self
            .seal(tenant_id, id, Some(parameters))?
            .unwrap_or(serde_json::Value::Null);
        let result = sqlx::query(
            r#"
            UPDATE connection_data_entity
            SET connection_parameters = $3,
                status = $4,
                refresh_token_hash = $5,
                credential_format = $6,
                updated_at = NOW()
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(&sealed)
        .bind(status)
        .bind(
            parameters
                .get("refresh_token")
                .and_then(refresh_token_guard),
        )
        .bind(credential_format(Some(parameters)))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Every connection of `tenants` with `status`, as `(id, tenant_id)`.
    pub async fn list_with_status(
        &self,
        status: &str,
        tenants: &[String],
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, tenant_id FROM connection_data_entity WHERE status = $1 AND tenant_id = ANY($2)",
        )
        .bind(status)
        .bind(tenants)
        .fetch_all(&self.pool)
        .await
    }

    /// Update only the connection status (parameters untouched). Used to flip a
    /// connection to `REQUIRES_RECONNECTION` when its OAuth grant is dead.
    pub async fn update_status(
        &self,
        id: &str,
        tenant_id: &str,
        status: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE connection_data_entity
            SET status = $3, updated_at = NOW()
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Persist tokens produced by an OAuth *refresh* (rotating providers), sealing
    /// the merged parameters. Unlike [`Self::update_parameters_and_status`] this
    /// leaves `status` untouched — a refresh is not a re-authorization.
    ///
    /// Optimistic-concurrency guard: the write only lands when the stored
    /// `refresh_token_hash` is still `NULL` (never rotated / legacy row) or equals
    /// `expected_hash` (the hash of the refresh token we refreshed from). If another
    /// process rotated concurrently the guard fails and `0` rows are affected, so the
    /// caller can adopt the winner's freshly-persisted token instead of clobbering it.
    ///
    /// Returns the number of rows updated (`0` == lost the optimistic race).
    pub async fn persist_refreshed_oauth(
        &self,
        id: &str,
        tenant_id: &str,
        parameters: &serde_json::Value,
        expected_hash: Option<&str>,
        new_hash: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        let sealed = self
            .seal(tenant_id, id, Some(parameters))?
            .unwrap_or(serde_json::Value::Null);
        let result = sqlx::query(
            r#"
            UPDATE connection_data_entity
            SET connection_parameters = $3,
                refresh_token_hash = $4,
                credential_format = $6,
                updated_at = NOW()
            WHERE id = $1 AND tenant_id = $2
              AND (refresh_token_hash IS NULL OR refresh_token_hash = $5)
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(&sealed)
        .bind(new_hash)
        .bind(expected_hash)
        .bind(credential_format(Some(parameters)))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Seal the secrets of connections an earlier release stored readable,
    /// for one tenant or for all: each is sealed as it would be on its next
    /// save.
    ///
    /// A connection whose secret seal cannot hold (its type is not one this
    /// build knows, or its secret goes somewhere seal cannot express) is
    /// counted as unsealable and left as it is: it cannot be used until it is
    /// entered again. One written meanwhile is left to that write. With
    /// `dry_run`, nothing is written, and the count is of what would be
    /// sealed. Sealing is one-way: a sealed secret is not stored readable
    /// again.
    pub async fn seal_existing(
        &self,
        sealed: &crate::sealed::SealedCredentials,
        tenant_id: Option<&str>,
        dry_run: bool,
    ) -> Result<SealingStats, sqlx::Error> {
        const UNSEALED: &str = "SELECT id, tenant_id, integration_id, connection_parameters \
             FROM connection_data_entity \
             WHERE credential_format = 'cipher' AND connection_parameters IS NOT NULL \
               AND integration_id IS NOT NULL";
        let rows: Vec<(String, String, String, serde_json::Value)> = match tenant_id {
            Some(tenant_id) => {
                sqlx::query_as(&format!("{UNSEALED} AND tenant_id = $1 ORDER BY id"))
                    .bind(tenant_id)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query_as(&format!("{UNSEALED} ORDER BY tenant_id, id"))
                    .fetch_all(&self.pool)
                    .await?
            }
        };

        let mut stats = SealingStats {
            scanned: rows.len(),
            ..SealingStats::default()
        };
        for (id, tenant, integration_id, stored) in rows {
            let context = CipherContext {
                tenant_id: &tenant,
                connection_id: &id,
            };
            let parameters = if is_envelope(&stored) {
                match self.cipher.decrypt(&stored, &context) {
                    Ok(parameters) => parameters,
                    Err(error) => {
                        stats.failed += 1;
                        tracing::error!(
                            connection_id = %id, tenant_id = %tenant, %error,
                            "seal: the connection could not be read, skipping it"
                        );
                        continue;
                    }
                }
            } else {
                stored.clone()
            };
            let sealed_parameters = match sealed
                .seal_parameters(&tenant, &id, &integration_id, &parameters, None)
                .await
            {
                Ok(Some(sealed_parameters))
                    if crate::sealed::has_sealed_values(&sealed_parameters) =>
                {
                    sealed_parameters
                }
                // Nothing entered to seal.
                Ok(_) => {
                    stats.unchanged += 1;
                    continue;
                }
                // A secret seal cannot hold: the connection is left as it
                // is, and cannot be used until it is entered again in a form
                // seal can hold.
                Err(crate::sealed::SealingError::CannotSeal { reason }) => {
                    stats.unsealable += 1;
                    tracing::warn!(
                        connection_id = %id, tenant_id = %tenant, %integration_id, %reason,
                        "seal: the connection's credentials cannot be sealed"
                    );
                    continue;
                }
                Err(error) => {
                    stats.failed += 1;
                    tracing::error!(
                        connection_id = %id, tenant_id = %tenant, %error,
                        "seal: the connection could not be sealed, skipping it"
                    );
                    continue;
                }
            };
            if dry_run {
                stats.sealed += 1;
                continue;
            }
            let encrypted = match self.cipher.encrypt(&sealed_parameters, &context) {
                Ok(encrypted) => encrypted,
                Err(error) => {
                    stats.failed += 1;
                    tracing::error!(
                        connection_id = %id, tenant_id = %tenant, %error,
                        "seal: the sealed connection could not be stored, skipping it"
                    );
                    continue;
                }
            };
            // Only the value that was read is replaced: a save or a token
            // refresh meanwhile wins. A refresh token that is now sealed
            // takes the rotation guard of its sealed form.
            let updated = sqlx::query(
                "UPDATE connection_data_entity \
                 SET connection_parameters = $3, credential_format = $5, \
                     refresh_token_hash = COALESCE($6, refresh_token_hash) \
                 WHERE id = $1 AND tenant_id = $2 AND connection_parameters = $4",
            )
            .bind(&id)
            .bind(&tenant)
            .bind(&encrypted)
            .bind(&stored)
            .bind(FORMAT_SEAL)
            .bind(
                sealed_parameters
                    .get("refresh_token")
                    .filter(|token| crate::sealed::is_sealed(token))
                    .and_then(refresh_token_guard),
            )
            .execute(&self.pool)
            .await?;
            if updated.rows_affected() == 0 {
                stats.unchanged += 1;
            } else {
                stats.sealed += 1;
            }
        }
        Ok(stats)
    }

    /// Turn the copies of the platform's own database URL that earlier
    /// releases stored in tenants' `postgres` connections into what stands
    /// for it now: [`PLATFORM_DATABASE_PARAM`], which holds no secret. Only a
    /// connection whose stored URL is exactly `platform_url` is changed, and
    /// one written meanwhile is left to that write. Returns how many were.
    pub async fn adopt_platform_database(&self, platform_url: &str) -> Result<usize, sqlx::Error> {
        let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
            "SELECT id, tenant_id, connection_parameters FROM connection_data_entity \
             WHERE credential_format = 'cipher' AND connection_parameters IS NOT NULL \
               AND integration_id = 'postgres' ORDER BY tenant_id, id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut adopted = 0;
        for (id, tenant, stored) in rows {
            let context = CipherContext {
                tenant_id: &tenant,
                connection_id: &id,
            };
            let parameters = if is_envelope(&stored) {
                match self.cipher.decrypt(&stored, &context) {
                    Ok(parameters) => parameters,
                    // Left to the sealing that follows, which reports it.
                    Err(_) => continue,
                }
            } else {
                stored.clone()
            };
            if parameters["database_url"].as_str() != Some(platform_url) {
                continue;
            }
            let mut standing = parameters;
            if let Some(fields) = standing.as_object_mut() {
                fields.remove("database_url");
                fields.insert(PLATFORM_DATABASE_PARAM.to_owned(), true.into());
            }
            let Ok(encrypted) = self.cipher.encrypt(&standing, &context) else {
                continue;
            };
            let updated = sqlx::query(
                "UPDATE connection_data_entity SET connection_parameters = $3 \
                 WHERE id = $1 AND tenant_id = $2 AND connection_parameters = $4",
            )
            .bind(&id)
            .bind(&tenant)
            .bind(&encrypted)
            .bind(&stored)
            .execute(&self.pool)
            .await?;
            adopted += usize::try_from(updated.rows_affected()).unwrap_or(0);
        }
        Ok(adopted)
    }

    /// Re-encrypt all rows for a given tenant (or all tenants if `tenant_id`
    /// is `None`) using the current cipher.
    ///
    /// Idempotent: plaintext rows get encrypted; already-encrypted rows are
    /// decrypted and re-encrypted (possibly with a new key ID, if the cipher
    /// has been rotated). Rows with `NULL` parameters are skipped.
    ///
    /// Returns statistics about the migration.
    pub async fn reencrypt_all(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<ReencryptionStats, sqlx::Error> {
        self.reencrypt_rows(tenant_id, false).await
    }

    /// Encrypt every row still stored as plaintext, across all tenants,
    /// leaving envelopes untouched. Run at server start: releases before
    /// encryption was mandatory could store credentials as plaintext, and
    /// [`CredentialCipher::decrypt`] refuses to read them. Cheap when there
    /// is nothing to do, because the database picks out the plaintext rows.
    pub async fn encrypt_plaintext(&self) -> Result<ReencryptionStats, sqlx::Error> {
        self.reencrypt_rows(None, true).await
    }

    async fn reencrypt_rows(
        &self,
        tenant_id: Option<&str>,
        plaintext_only: bool,
    ) -> Result<ReencryptionStats, sqlx::Error> {
        // The same shape test as `is_envelope`, so only plaintext rows are
        // loaded. COALESCE because a missing key makes the test NULL, not false.
        const PLAINTEXT_ONLY: &str = " AND NOT COALESCE(\
             jsonb_typeof(connection_parameters) = 'object' \
             AND jsonb_typeof(connection_parameters->'v') = 'number' \
             AND jsonb_typeof(connection_parameters->'alg') = 'string' \
             AND jsonb_typeof(connection_parameters->'nonce') = 'string' \
             AND jsonb_typeof(connection_parameters->'ct') = 'string', false)";
        let filter = if plaintext_only { PLAINTEXT_ONLY } else { "" };
        let rows: Vec<(String, String, Option<serde_json::Value>)> = if let Some(tid) = tenant_id {
            sqlx::query_as(&format!(
                "SELECT id, tenant_id, connection_parameters FROM connection_data_entity \
                 WHERE tenant_id = $1 AND connection_parameters IS NOT NULL{filter}"
            ))
            .bind(tid)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as(&format!(
                "SELECT id, tenant_id, connection_parameters FROM connection_data_entity \
                 WHERE connection_parameters IS NOT NULL{filter}"
            ))
            .fetch_all(&self.pool)
            .await?
        };

        let mut stats = ReencryptionStats {
            scanned: rows.len(),
            reencrypted: 0,
            unchanged: 0,
            failed: 0,
        };

        for (id, tid, stored) in rows {
            let Some(stored_value) = stored else {
                stats.unchanged += 1;
                continue;
            };
            let stored_plaintext = !is_envelope(&stored_value);
            if plaintext_only && !stored_plaintext {
                stats.unchanged += 1;
                continue;
            }
            let context = CipherContext {
                tenant_id: &tid,
                connection_id: &id,
            };
            // Plaintext is the one thing `decrypt` refuses; this is where it
            // gets converted.
            let plaintext = if stored_plaintext {
                stored_value.clone()
            } else {
                match self.cipher.decrypt(&stored_value, &context) {
                    Ok(v) => v,
                    Err(e) => {
                        stats.failed += 1;
                        tracing::error!(
                            connection_id = %id,
                            tenant_id = %tid,
                            error = %e,
                            "reencrypt: decrypt failed, skipping row"
                        );
                        continue;
                    }
                }
            };
            let new_envelope = match self.cipher.encrypt(&plaintext, &context) {
                Ok(v) => v,
                Err(e) => {
                    stats.failed += 1;
                    tracing::error!(
                        connection_id = %id,
                        tenant_id = %tid,
                        error = %e,
                        "reencrypt: encrypt failed, skipping row"
                    );
                    continue;
                }
            };
            // Only overwrite the value that was read: a concurrent writer (or
            // another replica doing the same at boot) wins, and its row is
            // already sealed.
            let updated = sqlx::query(
                "UPDATE connection_data_entity SET connection_parameters = $3 \
                 WHERE id = $1 AND tenant_id = $2 AND connection_parameters = $4",
            )
            .bind(&id)
            .bind(&tid)
            .bind(&new_envelope)
            .bind(&stored_value)
            .execute(&self.pool)
            .await?;
            if updated.rows_affected() == 0 {
                stats.unchanged += 1;
            } else {
                stats.reencrypted += 1;
            }
        }

        Ok(stats)
    }
}

/// Summary returned by [`ConnectionRepository::seal_existing`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SealingStats {
    /// Connections not sealed yet that were looked at.
    pub scanned: usize,
    /// Connections whose secrets were sealed (or would be, in a dry run).
    pub sealed: usize,
    /// Connections left as they are: they hold no secret.
    pub unchanged: usize,
    /// Connections left as they are though they hold a secret: seal cannot
    /// hold it (the type is not one it handles, or the destination is one it
    /// cannot express). They cannot be used until entered again.
    pub unsealable: usize,
    /// Connections that could not be read or sealed, and were left alone.
    pub failed: usize,
}

/// Summary returned by [`ConnectionRepository::reencrypt_all`].
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReencryptionStats {
    /// Number of rows inspected.
    pub scanned: usize,
    /// Rows successfully re-encrypted (decrypt + encrypt + write).
    pub reencrypted: usize,
    /// Rows that had `NULL` parameters and were skipped.
    pub unchanged: usize,
    /// Rows that failed to decrypt or encrypt and were skipped.
    pub failed: usize,
}

/// Internal struct for connection data including parameters
/// SECURITY: Only used for internal runtime connection resolution
#[derive(Debug)]
#[allow(dead_code)]
pub struct ConnectionWithParameters {
    pub id: String,
    pub tenant_id: Option<String>,
    pub integration_id: Option<String>,
    pub connection_subtype: Option<String>,
    pub connection_parameters: Option<serde_json::Value>,
    pub rate_limit_config: Option<serde_json::Value>,
    /// Whether the secrets among the parameters are sealed by seal: each is
    /// then a marker (see [`crate::sealed`]), not a value, and a request that
    /// uses them goes through seal.
    pub sealed: bool,
}

impl ConnectionWithParameters {
    /// Alias for connection_parameters (used by OAuth service)
    #[allow(dead_code)]
    pub fn parameters(&self) -> Option<&serde_json::Value> {
        self.connection_parameters.as_ref()
    }
}
