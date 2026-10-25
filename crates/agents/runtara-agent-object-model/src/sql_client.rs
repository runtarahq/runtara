//! Typed agent client for native SQL, over the workflow API's connection
//! endpoints. There is no native HTTP fallback.
use runtara_database_contract::*;
use runtara_object_model_core::config::ObjectModelLayout;

/// Separates agent orchestration from the component ABI for native parity tests.
/// Futures need not be Send: guest execution uses the component async runtime.
#[allow(async_fn_in_trait)]
pub trait SqlClient {
    async fn layout(&self, connection: &str) -> Result<ObjectModelLayout, DatabaseError>;
    async fn query(&self, connection: &str, request: QueryRequest)
    -> Result<RowSet, DatabaseError>;
    async fn execute(
        &self,
        connection: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError>;
    async fn execute_batch(
        &self,
        connection: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError>;
}

pub struct HostSqlClient;

#[cfg(target_family = "wasm")]
fn request_bytes<T: serde::Serialize>(request: &T) -> Result<Vec<u8>, DatabaseError> {
    let bytes = serde_json::to_vec(request)
        .map_err(|_| DatabaseError::invalid("Cannot encode database request"))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(DatabaseError::invalid(
            "Database request exceeds the byte limit",
        ));
    }
    Ok(bytes)
}

/// A database failure arrives as `database-error` carrying the service's
/// `DatabaseError`; any other failure is an invalid response.
#[cfg(target_family = "wasm")]
fn response<T: serde::de::DeserializeOwned>(
    result: Result<Vec<u8>, runtara_workflow_sdk::Error>,
    read_only: bool,
) -> Result<T, DatabaseError> {
    let invalid = || DatabaseError {
        code: "DATABASE_INVALID_RESPONSE".into(),
        message: "Invalid response from native database service".into(),
        outcome: if read_only {
            Outcome::RolledBack
        } else {
            Outcome::Unknown
        },
        retryable: read_only,
        sqlstate: None,
        statement_index: None,
    };
    let bytes = result.map_err(|error| match error {
        runtara_workflow_sdk::Error::Api(error) if error.code.as_str() == "database-error" => error
            .details
            .and_then(|details| serde_json::from_value(details).ok())
            .unwrap_or_else(invalid),
        _ => invalid(),
    })?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(invalid());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

impl SqlClient for HostSqlClient {
    async fn layout(&self, connection: &str) -> Result<ObjectModelLayout, DatabaseError> {
        #[cfg(target_family = "wasm")]
        {
            let descriptor = runtara_workflow_sdk::describe_connection(connection)
                .await
                .map_err(|_| {
                    DatabaseError::invalid("Cannot resolve Object Model connection metadata")
                })?;
            if descriptor["integrationId"] != "postgres" {
                return Err(DatabaseError::invalid(
                    "Object Model requires a PostgreSQL connection",
                ));
            }
            serde_json::from_value(descriptor["metadata"]["object_model"].clone()).map_err(|_| {
                DatabaseError::invalid("Missing or incompatible Object Model layout metadata")
            })
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = connection;
            Err(DatabaseError::invalid(
                "Native SQL requires the workflow API",
            ))
        }
    }
    async fn query(
        &self,
        connection: &str,
        request: QueryRequest,
    ) -> Result<RowSet, DatabaseError> {
        #[cfg(target_family = "wasm")]
        {
            response(
                runtara_workflow_sdk::sql_bytes(
                    connection,
                    runtara_workflow_api::SqlOperation::Query,
                    request_bytes(&request)?,
                )
                .await,
                true,
            )
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = (connection, request);
            Err(DatabaseError::invalid(
                "Native SQL requires the workflow API",
            ))
        }
    }
    async fn execute(
        &self,
        connection: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError> {
        #[cfg(target_family = "wasm")]
        {
            response(
                runtara_workflow_sdk::sql_bytes(
                    connection,
                    runtara_workflow_api::SqlOperation::Execute,
                    request_bytes(&request)?,
                )
                .await,
                false,
            )
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = (connection, request);
            Err(DatabaseError::invalid(
                "Native SQL requires the workflow API",
            ))
        }
    }
    async fn execute_batch(
        &self,
        connection: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError> {
        #[cfg(target_family = "wasm")]
        {
            response(
                runtara_workflow_sdk::sql_bytes(
                    connection,
                    runtara_workflow_api::SqlOperation::Batch,
                    request_bytes(&request)?,
                )
                .await,
                false,
            )
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = (connection, request);
            Err(DatabaseError::invalid(
                "Native SQL requires the workflow API",
            ))
        }
    }
}
