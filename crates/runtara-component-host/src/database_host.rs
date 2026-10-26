//! Native SQL service behind the workflow API connection SQL routes.
use runtara_database_contract::*;
use std::sync::Arc;

#[async_trait::async_trait]
pub trait DatabaseHost: Send + Sync {
    async fn query(
        &self,
        tenant: &str,
        connection_id: &str,
        request: QueryRequest,
    ) -> Result<RowSet, DatabaseError>;
    async fn execute(
        &self,
        tenant: &str,
        connection_id: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError>;
    async fn execute_batch(
        &self,
        tenant: &str,
        connection_id: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError>;
}

/// One run's database access: the service and the run's tenant.
pub struct RunDatabase {
    backend: Arc<dyn DatabaseHost>,
    tenant: String,
}

/// Database access for a run of `tenant`, both host-supplied.
pub fn database_for_run(
    backend: Option<&Arc<dyn DatabaseHost>>,
    tenant: Option<&str>,
) -> Result<Arc<RunDatabase>, String> {
    Ok(Arc::new(RunDatabase {
        backend: Arc::clone(backend.ok_or("native database service is not configured")?),
        tenant: tenant
            .filter(|t| !t.trim().is_empty())
            .ok_or("authoritative tenant is not configured")?
            .to_owned(),
    }))
}

fn error_wire(error: DatabaseError) -> String {
    serde_json::to_string(&error).expect("DatabaseError contains only serializable fields")
}

fn decode<T: serde::de::DeserializeOwned>(request: &[u8]) -> Result<T, String> {
    serde_json::from_slice(request)
        .map_err(|_| error_wire(DatabaseError::invalid("Invalid database request")))
}

fn encode<T: serde::Serialize>(result: Result<T, DatabaseError>) -> Result<Vec<u8>, String> {
    let result = result.map_err(error_wire)?;
    let bytes = serde_json::to_vec(&result)
        .map_err(|_| error_wire(DatabaseError::invalid("Cannot encode database result")))?;
    // Native adapters must enforce this before a mutation commits. This check
    // also bounds injected third-party adapters; it cannot undo their commits.
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(error_wire(DatabaseError {
            code: "DATABASE_RESULT_TOO_LARGE".into(),
            message: "Database result exceeds the byte limit".into(),
            outcome: Outcome::Unknown,
            retryable: false,
            sqlstate: None,
            statement_index: None,
        }));
    }
    Ok(bytes)
}

/// A SQL call of `POST /v1/connections/{id}/sql/{op}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Query,
    Execute,
    Batch,
}

impl RunDatabase {
    /// Run `operation` on `connection` with the JSON `request`, returning the
    /// JSON result, or the serialized `DatabaseError` as the error.
    pub async fn call(
        &self,
        operation: Operation,
        connection: String,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        if connection.trim().is_empty()
            || connection.len().saturating_add(request.len()) > MAX_REQUEST_BYTES
        {
            return Err(error_wire(DatabaseError::invalid(
                "Missing connection ID or oversized database request",
            )));
        }
        match operation {
            Operation::Query => encode(
                self.backend
                    .query(&self.tenant, &connection, decode(&request)?)
                    .await,
            ),
            Operation::Execute => encode(
                self.backend
                    .execute(&self.tenant, &connection, decode(&request)?)
                    .await,
            ),
            Operation::Batch => encode(
                self.backend
                    .execute_batch(&self.tenant, &connection, decode(&request)?)
                    .await,
            ),
        }
    }
}
