//! Native SQL boundary. Identity is supplied by the component host, never JSON.
use std::sync::Arc;

use crate::api::repositories::object_model::ObjectStoreManager;
use runtara_component_host::DatabaseHost;
use runtara_connections::ConnectionsFacade;
use runtara_database_contract::*;
use runtara_object_store::{ObjectStore, database::DatabaseLimits};

pub struct NativeDatabase {
    pub manager: Arc<ObjectStoreManager>,
    pub connections: Arc<ConnectionsFacade>,
}

impl NativeDatabase {
    async fn store(
        &self,
        tenant: &str,
        connection: &str,
    ) -> Result<Arc<ObjectStore>, DatabaseError> {
        authorize(crate::config::entitlements(), tenant, connection)?;
        let unavailable = || DatabaseError {
            code: "DATABASE_CONNECTION_UNAVAILABLE".into(),
            message: "Database connection is unavailable".into(),
            outcome: Outcome::NotStarted,
            retryable: false,
            sqlstate: None,
            statement_index: None,
        };
        // Recheck ownership/type on every call, then share the pool. Raw SQL
        // must not create Object Model metadata as a side effect of connecting.
        let url = super::object_model::resolve_database_url_uncached(
            Some(&self.connections),
            Some(connection),
            tenant,
        )
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
        self.manager
            .get_database_by_url(&url)
            .await
            .map_err(|_| unavailable())
    }

    fn limits() -> DatabaseLimits {
        let sql = crate::config::raw_sql_guardrails();
        DatabaseLimits {
            statement_timeout: std::time::Duration::from_millis(sql.statement_timeout_ms),
            operation_timeout: std::time::Duration::from_secs(u64::from(
                crate::config::execution_timeout_policy()
                    .default_timeout()
                    .as_secs(),
            )),
            max_rows: usize::try_from(sql.max_rows).unwrap_or(usize::MAX),
            max_response_bytes: usize::try_from(sql.max_response_bytes)
                .unwrap_or(MAX_RESPONSE_BYTES)
                .min(MAX_RESPONSE_BYTES),
            max_batch_statements: MAX_BATCH_STATEMENTS,
        }
    }
}

fn authorize(
    snapshot: &crate::entitlements::EntitlementSnapshot,
    tenant: &str,
    connection: &str,
) -> Result<(), DatabaseError> {
    if tenant.trim().is_empty() || connection.trim().is_empty() {
        return Err(DatabaseError::invalid("Tenant and connection are required"));
    }
    crate::middleware::entitlement::gate_decision(
        snapshot,
        crate::entitlements::FeatureKey::Database,
    )
    .map_err(|_| DatabaseError {
        code: "ENTITLEMENT_REQUIRED".into(),
        message: "Database entitlement is required".into(),
        outcome: Outcome::NotStarted,
        retryable: false,
        sqlstate: None,
        statement_index: None,
    })?;
    Ok(())
}

fn audit<T>(
    tenant: &str,
    connection: &str,
    operation: &str,
    started: std::time::Instant,
    result: &Result<T, DatabaseError>,
) {
    // No SQL text or parameters: literals and driver diagnostics can contain secrets.
    tracing::info!(target: "runtara::raw_sql_audit", tenant_id = tenant, connection_id = connection,
        operation, duration_ms = started.elapsed().as_millis() as u64,
        outcome = if result.is_ok() { "ok" } else { "error" },
        error_code = result.as_ref().err().map(|e| e.code.as_str()).unwrap_or(""),
        "native workflow SQL");
}

#[async_trait::async_trait]
impl DatabaseHost for NativeDatabase {
    async fn query(
        &self,
        tenant: &str,
        connection: &str,
        request: QueryRequest,
    ) -> Result<RowSet, DatabaseError> {
        let started = std::time::Instant::now();
        let result = async {
            self.store(tenant, connection)
                .await?
                .database_query(request, Self::limits())
                .await
        }
        .await;
        audit(tenant, connection, "query", started, &result);
        result
    }
    async fn execute(
        &self,
        tenant: &str,
        connection: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError> {
        let started = std::time::Instant::now();
        let result = async {
            self.store(tenant, connection)
                .await?
                .database_execute(request, Self::limits())
                .await
        }
        .await;
        audit(tenant, connection, "execute", started, &result);
        result
    }
    async fn execute_batch(
        &self,
        tenant: &str,
        connection: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError> {
        let started = std::time::Instant::now();
        let result = async {
            self.store(tenant, connection)
                .await?
                .database_execute_batch(request, Self::limits())
                .await
        }
        .await;
        audit(tenant, connection, "execute-batch", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn database_authority_and_entitlement_are_required_before_connection_lookup() {
        let snapshot = crate::entitlements::EntitlementSnapshot::parse_entitlements(
            "tenant",
            None,
            Some(r#"{"features":{"database":false}}"#),
            None,
            &std::collections::BTreeSet::new(),
        )
        .unwrap();
        let error = authorize(&snapshot, "tenant", "connection").unwrap_err();
        assert_eq!(error.code, "ENTITLEMENT_REQUIRED");
        assert_eq!(error.outcome, Outcome::NotStarted);
        assert!(!error.retryable);
        for (tenant, connection) in [("", "connection"), ("tenant", "")] {
            assert_eq!(
                authorize(&snapshot, tenant, connection).unwrap_err().code,
                "DATABASE_INVALID_REQUEST"
            );
        }
    }
}
