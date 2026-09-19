//! Native SQL imports shared by workflow and standalone agent execution.
use runtara_database_contract::*;
use std::sync::Arc;
use wasmtime::{
    AsContext,
    component::{Linker, WasmList},
};

pub const DATABASE_INTERFACE_NAME: &str = runtara_workflow_wit::DATABASE_INTERFACE_NAME;

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

pub(crate) struct RunDatabase {
    backend: Arc<dyn DatabaseHost>,
    tenant: String,
}

pub(crate) fn database_for_run(
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

pub(crate) trait DatabaseContext {
    fn database(&self) -> Result<Arc<RunDatabase>, String>;
    fn database_deadline(&self) -> Option<tokio::time::Instant>;
}

impl DatabaseContext for crate::workflow::WorkflowState {
    fn database(&self) -> Result<Arc<RunDatabase>, String> {
        self.database.clone()
    }
    fn database_deadline(&self) -> Option<tokio::time::Instant> {
        Some(self.database_deadline())
    }
}

impl DatabaseContext for crate::host_state::HostState {
    fn database(&self) -> Result<Arc<RunDatabase>, String> {
        if self.restricted {
            return Err("database access is denied in trusted instances".into());
        }
        self.database.clone()
    }
    fn database_deadline(&self) -> Option<tokio::time::Instant> {
        self.http_deadline
    }
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

#[derive(Clone, Copy)]
enum Operation {
    Query,
    Execute,
    Batch,
}

impl RunDatabase {
    async fn call(
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

pub(crate) fn add_database_to_linker<T: DatabaseContext + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut interface = linker.instance(DATABASE_INTERFACE_NAME)?;
    for (name, operation) in [
        ("query", Operation::Query),
        ("execute", Operation::Execute),
        ("execute-batch", Operation::Batch),
    ] {
        interface.func_wrap_concurrent(
            name,
            move |accessor, (connection, request): (String, WasmList<u8>)| {
                let (host, deadline) = accessor.with(|mut access| {
                    let state = access.get();
                    let deadline = state.database_deadline();
                    let host = state
                        .database()
                        .map_err(|message| error_wire(DatabaseError::invalid(message)));
                    // Lift only the guest slice descriptor. Check authority and size
                    // before copying potentially large guest data into native memory.
                    let host = host.and_then(|host| {
                        if connection.trim().is_empty()
                            || connection.len().saturating_add(request.len()) > MAX_REQUEST_BYTES
                        {
                            return Err(error_wire(DatabaseError::invalid(
                                "Missing connection ID or oversized database request",
                            )));
                        }
                        Ok((host, request.as_le_slice(access.as_context()).to_vec()))
                    });
                    (host, deadline)
                });
                Box::pin(async move {
                    let result = match host {
                        Err(error) => Err(error),
                        Ok((host, request)) => {
                            let deadline = deadline.unwrap_or_else(|| {
                                tokio::time::Instant::now() + std::time::Duration::from_secs(60)
                            });
                            match tokio::time::timeout_at(
                                deadline,
                                host.call(operation, connection, request),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => Err(error_wire(DatabaseError {
                                    code: "DATABASE_DEADLINE_EXCEEDED".into(),
                                    message: "Database execution deadline exceeded".into(),
                                    outcome: if matches!(operation, Operation::Query) {
                                        Outcome::RolledBack
                                    } else {
                                        Outcome::Unknown
                                    },
                                    retryable: matches!(operation, Operation::Query),
                                    sqlstate: None,
                                    statement_index: None,
                                })),
                            }
                        }
                    };
                    Ok((result,))
                })
            },
        )?;
    }
    Ok(())
}
