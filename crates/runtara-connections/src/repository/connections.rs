//! Connection Repository
//!
//! Handles all database operations for connection management
//! SECURITY: Provides methods to explicitly exclude connection_parameters from queries

use crate::types::*;
use sqlx::PgPool;

/// Convert JSONB value from DB to RateLimitConfigDto
fn parse_rate_limit_config(value: Option<serde_json::Value>) -> Option<RateLimitConfigDto> {
    value.and_then(|v| serde_json::from_value(v).ok())
}

pub struct ConnectionRepository {
    pool: PgPool,
}

impl ConnectionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
        .bind(&request.connection_parameters)
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

        let connections = rows
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
                    }
                },
            )
            .collect();

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

        Ok(result.map(
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
                }
            },
        ))
    }

    /// Update a connection with dynamic fields
    pub async fn update(
        &self,
        id: &str,
        tenant_id: &str,
        request: &UpdateConnectionRequest,
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
        }

        let query_str = format!(
            "UPDATE connection_data_entity SET {} WHERE id = $1 AND tenant_id = $2",
            updates.join(", ")
        );

        // Execute update with dynamic bindings
        let mut query = sqlx::query(&query_str).bind(id).bind(tenant_id);

        if let Some(ref title) = request.title {
            query = query.bind(title);
        }
        if let Some(ref connection_subtype) = request.connection_subtype {
            query = query.bind(connection_subtype);
        }
        if let Some(ref connection_parameters) = request.connection_parameters {
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
        let rows = if !integration_ids.is_empty() && status.is_some() {
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
            .bind(status.unwrap())
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

        let connections = rows
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
                    }
                },
            )
            .collect();

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

        Ok(result.map(
            |(
                id,
                tid,
                integration_id,
                connection_subtype,
                connection_parameters,
                rate_limit_config,
            )| {
                ConnectionWithParameters {
                    id,
                    tenant_id: Some(tid),
                    integration_id,
                    connection_subtype,
                    connection_parameters,
                    rate_limit_config,
                }
            },
        ))
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

        Ok(result.map(
            |(
                id,
                tid,
                integration_id,
                connection_subtype,
                connection_parameters,
                rate_limit_config,
            )| {
                ConnectionWithParameters {
                    id,
                    tenant_id: Some(tid),
                    integration_id,
                    connection_subtype,
                    connection_parameters,
                    rate_limit_config,
                }
            },
        ))
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

        Ok(result.map(
            |(
                id,
                tid,
                integration_id,
                connection_subtype,
                connection_parameters,
                rate_limit_config,
            )| {
                ConnectionWithParameters {
                    id,
                    tenant_id: Some(tid),
                    integration_id,
                    connection_subtype,
                    connection_parameters,
                    rate_limit_config,
                }
            },
        ))
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
        .bind(parameters)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
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
