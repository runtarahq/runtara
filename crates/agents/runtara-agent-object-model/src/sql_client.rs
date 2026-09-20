//! Typed agent client for native SQL. There is no native HTTP fallback.
use runtara_database_contract::*;
use runtara_object_model_core::config::ObjectModelLayout;

#[cfg(target_family = "wasm")]
mod sql_bindings {
    wit_bindgen::generate!({
        path: "../../runtara-workflow-wit/wit/database",
        world: "database-client",
        async: true,
    });
}
#[cfg(target_family = "wasm")]
mod connection_bindings {
    wit_bindgen::generate!({
        path: "../../runtara-workflow-wit/wit/connection-resolver",
        world: "connection-client",
        async: true,
    });
}

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

#[cfg(target_family = "wasm")]
fn response<T: serde::de::DeserializeOwned>(
    result: Result<Vec<u8>, String>,
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
    let bytes =
        result.map_err(|error| serde_json::from_str(&error).unwrap_or_else(|_| invalid()))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(invalid());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

impl SqlClient for HostSqlClient {
    async fn layout(&self, connection: &str) -> Result<ObjectModelLayout, DatabaseError> {
        #[cfg(target_family = "wasm")]
        {
            let bytes = connection_bindings::runtara::connection_resolver::resolver::describe(
                connection.to_owned(),
            )
            .await
            .map_err(|_| {
                DatabaseError::invalid("Cannot resolve Object Model connection metadata")
            })?;
            let descriptor: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| DatabaseError::invalid("Invalid connection descriptor"))?;
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
                "Native SQL imports require a WASM execution host",
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
                sql_bindings::runtara::database::sql::query(
                    connection.to_owned(),
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
                "Native SQL imports require a WASM execution host",
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
                sql_bindings::runtara::database::sql::execute(
                    connection.to_owned(),
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
                "Native SQL imports require a WASM execution host",
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
                sql_bindings::runtara::database::sql::execute_batch(
                    connection.to_owned(),
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
                "Native SQL imports require a WASM execution host",
            ))
        }
    }
}
