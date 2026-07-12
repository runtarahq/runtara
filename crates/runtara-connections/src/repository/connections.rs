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

use crate::crypto::CredentialCipher;
use crate::types::*;
use sqlx::PgPool;

/// Convert JSONB value from DB to RateLimitConfigDto
fn parse_rate_limit_config(value: Option<serde_json::Value>) -> Option<RateLimitConfigDto> {
    value.and_then(|v| serde_json::from_value(v).ok())
}

pub struct ConnectionRepository {
    pool: PgPool,
    cipher: Arc<dyn CredentialCipher>,
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
        plaintext: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, sqlx::Error> {
        let Some(value) = plaintext else {
            return Ok(None);
        };
        match self.cipher.encrypt(value) {
            Ok(envelope) => Ok(Some(envelope)),
            Err(e) => {
                tracing::error!(error = %e, "Failed to encrypt connection parameters");
                Err(sqlx::Error::Encode(format!("cipher encrypt: {}", e).into()))
            }
        }
    }

    /// Decrypt connection parameters after retrieval. `None` in → `None` out.
    ///
    /// Plaintext values (e.g., rows written before encryption was enabled)
    /// pass through unchanged — the cipher handles the envelope check.
    fn unseal(
        &self,
        stored: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, sqlx::Error> {
        let Some(value) = stored else {
            return Ok(None);
        };
        match self.cipher.decrypt(&value) {
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
    ) -> Result<(), sqlx::Error> {
        let status = request.status.as_ref().unwrap_or(&ConnectionStatus::Active);
        let rate_limit_json = request
            .rate_limit_config
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok());

        // Encrypt connection_parameters at rest (no-op if cipher is NoOp).
        let sealed_parameters = self.seal(request.connection_parameters.as_ref())?;

        sqlx::query(
            r#"
            INSERT INTO connection_data_entity
            (id, tenant_id, title, connection_subtype, connection_parameters, integration_id, valid_until, status, rate_limit_config, is_default_file_storage)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
        .execute(&self.pool)
        .await?;

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
                        integration_id,
                        status: ConnectionStatus::parse(&status),
                        rate_limit_config: parse_rate_limit_config(rate_limit_config),
                        rate_limit_stats: None,
                        is_default_file_storage,
                        default_for: Vec::new(),
                        edit_projection: None,
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
                    integration_id,
                    status: ConnectionStatus::parse(&status),
                    rate_limit_config: parse_rate_limit_config(rate_limit_config),
                    rate_limit_stats: None,
                    is_default_file_storage,
                    default_for: Vec::new(),
                    edit_projection: None,
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
        expected_updated_at: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        // Build dynamic UPDATE query based on provided fields
        let mut updates = vec!["updated_at = NOW()".to_string()];
        let mut param_idx = 3;

        if request.title.is_some() {
            updates.push(format!("title = ${}", param_idx));
            param_idx += 1;
        }
        if request.connection_subtype.is_some() {
            updates.push(format!("connection_subtype = ${}", param_idx));
            param_idx += 1;
        }
        if request.connection_parameters.is_some() {
            updates.push(format!("connection_parameters = ${}", param_idx));
            param_idx += 1;
        }
        if request.integration_id.is_some() {
            updates.push(format!("integration_id = ${}", param_idx));
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
        if request.status.is_some() {
            updates.push(format!("status = ${}", param_idx));
            param_idx += 1;
        }
        if request.is_default_file_storage.is_some() {
            updates.push(format!("is_default_file_storage = ${}", param_idx));
            param_idx += 1;
        }

        let version_parameter = expected_updated_at.map(|_| param_idx);
        let version_clause = version_parameter
            .map(|parameter| format!(" AND updated_at = ${parameter}"))
            .unwrap_or_default();
        let query_str = format!(
            "UPDATE connection_data_entity SET {} WHERE id = $1 AND tenant_id = $2{}",
            updates.join(", "),
            version_clause
        );

        // Pre-encrypt parameters (if any) before binding — must live long
        // enough for the bind to reference it.
        let sealed_parameters = match request.connection_parameters.as_ref() {
            Some(plain) => Some(self.seal(Some(plain))?.unwrap_or(serde_json::Value::Null)),
            None => None,
        };

        // Execute update with dynamic bindings
        let mut query = sqlx::query(&query_str).bind(id).bind(tenant_id);

        if let Some(ref title) = request.title {
            query = query.bind(title);
        }
        if let Some(ref connection_subtype) = request.connection_subtype {
            query = query.bind(connection_subtype);
        }
        if let Some(ref connection_parameters) = sealed_parameters {
            query = query.bind(connection_parameters);
        }
        if let Some(ref integration_id) = request.integration_id {
            query = query.bind(integration_id);
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
        if let Some(ref status) = request.status {
            query = query.bind(status.as_str());
        }
        if let Some(is_default) = request.is_default_file_storage {
            query = query.bind(is_default);
        }
        if let Some(expected_updated_at) = expected_updated_at {
            let parsed = chrono::DateTime::parse_from_rfc3339(expected_updated_at)
                .map_err(|error| sqlx::Error::Encode(error.into()))?
                .with_timezone(&chrono::Utc);
            query = query.bind(parsed);
        }

        let result = query.execute(&self.pool).await?;
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
    pub async fn list_by_operator(
        &self,
        tenant_id: &str,
        integration_ids: &[String],
        status: Option<&str>,
    ) -> Result<Vec<ConnectionDto>, sqlx::Error> {
        // Build query with integration_id filter
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

        // Add integration_id filter if operator specifies integration_ids
        if !integration_ids.is_empty() {
            query.push_str(" AND integration_id = ANY($2)");
        }

        // Add optional status filter
        if status.is_some() {
            let param_idx = if integration_ids.is_empty() { 2 } else { 3 };
            query.push_str(&format!(" AND status = ${}", param_idx));
        }

        query.push_str(" ORDER BY created_at DESC");

        // Execute query with dynamic parameter binding
        let rows = if let (false, Some(status_val)) = (integration_ids.is_empty(), status) {
            // Has integration_ids AND status filter
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
            .bind(integration_ids)
            .bind(status_val)
            .fetch_all(&self.pool)
            .await?
        } else if !integration_ids.is_empty() {
            // Has integration_ids, no status filter
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
            .bind(integration_ids)
            .fetch_all(&self.pool)
            .await?
        } else if let Some(status_val) = status {
            // No integration_ids, has status filter
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
            // No integration_ids, no status filter - return empty vec
            return Ok(vec![]);
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
                        integration_id,
                        status: ConnectionStatus::parse(&status),
                        rate_limit_config: parse_rate_limit_config(rate_limit_config),
                        rate_limit_stats: None,
                        is_default_file_storage,
                        default_for: Vec::new(),
                        edit_projection: None,
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
        let result = sqlx::query_as::<
            _,
            (
                String,                    // id
                String,                    // tenant_id
                Option<String>,            // integration_id
                Option<String>,            // connection_subtype
                Option<serde_json::Value>, // connection_parameters
                Option<serde_json::Value>, // rate_limit_config
            ),
        >(
            r#"
            SELECT id, tenant_id, integration_id, connection_subtype, connection_parameters, rate_limit_config
            FROM connection_data_entity
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(id)
        .bind(tenant_id)
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
                )| {
                    Ok::<_, sqlx::Error>(ConnectionWithParameters {
                        id,
                        tenant_id: Some(tid),
                        integration_id,
                        connection_subtype,
                        connection_parameters: self.unseal(connection_parameters)?,
                        rate_limit_config,
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
            ),
        >(
            r#"
            SELECT id, tenant_id, integration_id, connection_subtype, connection_parameters, rate_limit_config
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
                )| {
                    Ok::<_, sqlx::Error>(ConnectionWithParameters {
                        id,
                        tenant_id: Some(tid),
                        integration_id,
                        connection_subtype,
                        connection_parameters: self.unseal(connection_parameters)?,
                        rate_limit_config,
                    })
                },
            )
            .transpose()
    }

    /// Get the default file storage connection for a tenant.
    /// Returns the single connection where is_default_file_storage = TRUE,
    /// including connection_parameters for internal use.
    ///
    /// SECURITY WARNING: Returns sensitive credentials. Internal use only.
    pub async fn get_default_file_storage(
        &self,
        tenant_id: &str,
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
            ),
        >(
            r#"
            SELECT id, tenant_id, integration_id, connection_subtype, connection_parameters, rate_limit_config
            FROM connection_data_entity
            WHERE tenant_id = $1 AND is_default_file_storage = TRUE
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
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
                )| {
                    Ok::<_, sqlx::Error>(ConnectionWithParameters {
                        id,
                        tenant_id: Some(tid),
                        integration_id,
                        connection_subtype,
                        connection_parameters: self.unseal(connection_parameters)?,
                        rate_limit_config,
                    })
                },
            )
            .transpose()
    }

    /// Clear any existing default file storage for the tenant.
    /// Used before setting a new default to enforce the one-default-per-tenant invariant.
    pub async fn clear_default_file_storage(&self, tenant_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE connection_data_entity
            SET is_default_file_storage = FALSE, updated_at = NOW()
            WHERE tenant_id = $1 AND is_default_file_storage = TRUE
            "#,
        )
        .bind(tenant_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Update connection parameters and status atomically.
    /// Used by OAuth callback to store tokens and set status to ACTIVE.
    pub async fn update_parameters_and_status(
        &self,
        id: &str,
        tenant_id: &str,
        parameters: &serde_json::Value,
        status: &str,
    ) -> Result<u64, sqlx::Error> {
        let sealed = self
            .seal(Some(parameters))?
            .unwrap_or(serde_json::Value::Null);
        let result = sqlx::query(
            r#"
            UPDATE connection_data_entity
            SET connection_parameters = $3,
                status = $4,
                updated_at = NOW()
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(&sealed)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
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
            .seal(Some(parameters))?
            .unwrap_or(serde_json::Value::Null);
        let result = sqlx::query(
            r#"
            UPDATE connection_data_entity
            SET connection_parameters = $3,
                refresh_token_hash = $4,
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
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
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
        // Stream all rows with non-null parameters. We process row-by-row to
        // keep memory flat on large deployments.
        let rows: Vec<(String, String, Option<serde_json::Value>)> = if let Some(tid) = tenant_id {
            sqlx::query_as(
                "SELECT id, tenant_id, connection_parameters FROM connection_data_entity \
                 WHERE tenant_id = $1 AND connection_parameters IS NOT NULL",
            )
            .bind(tid)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as(
                "SELECT id, tenant_id, connection_parameters FROM connection_data_entity \
                 WHERE connection_parameters IS NOT NULL",
            )
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
            // Decrypt (or passthrough if plaintext), then encrypt with the current cipher.
            let plaintext = match self.cipher.decrypt(&stored_value) {
                Ok(v) => v,
                Err(e) => {
                    stats.failed += 1;
                    tracing::error!(
                        connection_id = %id,
                        tenant_id = %tid,
                        error = %e,
                        "reencrypt_all: decrypt failed, skipping row"
                    );
                    continue;
                }
            };
            let new_envelope = match self.cipher.encrypt(&plaintext) {
                Ok(v) => v,
                Err(e) => {
                    stats.failed += 1;
                    tracing::error!(
                        connection_id = %id,
                        tenant_id = %tid,
                        error = %e,
                        "reencrypt_all: encrypt failed, skipping row"
                    );
                    continue;
                }
            };
            sqlx::query(
                "UPDATE connection_data_entity SET connection_parameters = $3 \
                 WHERE id = $1 AND tenant_id = $2",
            )
            .bind(&id)
            .bind(&tid)
            .bind(&new_envelope)
            .execute(&self.pool)
            .await?;
            stats.reencrypted += 1;
        }

        Ok(stats)
    }
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
}

impl ConnectionWithParameters {
    /// Alias for connection_parameters (used by OAuth service)
    #[allow(dead_code)]
    pub fn parameters(&self) -> Option<&serde_json::Value> {
        self.connection_parameters.as_ref()
    }
}
