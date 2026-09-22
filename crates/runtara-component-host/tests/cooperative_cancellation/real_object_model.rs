//! Real Object Model WASM imports, with a native SQL fixture and no HTTP server.
use super::real_agent::{
    cancel_and_reuse_with_state, compose_agent, invoke_named_agent_with_state,
};
use super::*;
use runtara_component_host::{CallContext, ConnectionResolverHost, DatabaseHost, HostState};
use runtara_database_contract::*;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug)]
enum Operation {
    Query,
    Execute,
    Load,
    LoadMissingSchema,
    SaveNew,
    SaveExisting,
}
impl Operation {
    fn capability(self) -> &'static str {
        match self {
            Self::Query => "query-sql",
            Self::Execute => "execute-sql",
            Self::Load | Self::LoadMissingSchema => "load-memory",
            Self::SaveNew | Self::SaveExisting => "save-memory",
        }
    }
    fn stages(self) -> usize {
        // Includes native connection metadata lookup (cached once per instance),
        // schema reads, count/select, bootstrap batch, and the final mutation.
        match self {
            Self::Query | Self::Execute => 1,
            Self::Load => 5,
            Self::LoadMissingSchema => 8,
            Self::SaveNew | Self::SaveExisting => 7,
        }
    }
    fn input(self) -> Value {
        json!({"_connection":{"connection_id":"fixture-connection","integration_id":"postgres","parameters":{}},"sql":if matches!(self,Self::Execute) {"UPDATE fixture SET value=1"} else {"SELECT 1"},"params":[],"conversation_id":"conversation","messages":[{"role":"user","content":"before"}]})
    }
}

#[path = "../support/object_model.rs"]
mod sql_fixture;
use sql_fixture::rows;

struct NativeFixture {
    operation: Operation,
    blocked: usize,
    calls: AtomicUsize,
    schema_exists: AtomicBool,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
    failure: Option<DatabaseError>,
}
impl NativeFixture {
    fn new(operation: Operation, blocked: usize) -> Arc<Self> {
        Arc::new(Self {
            operation,
            blocked,
            calls: AtomicUsize::new(0),
            schema_exists: AtomicBool::new(!matches!(operation, Operation::LoadMissingSchema)),
            started: Arc::new(Notify::new()),
            cleaned: Arc::new(Notify::new()),
            failure: None,
        })
    }
    fn state(self: &Arc<Self>) -> HostState {
        HostState::new(Arc::new(CallContext::for_test("fixture-tenant", "")))
            .with_connection_resolver(self.clone())
            .with_database(self.clone())
    }
    async fn step(&self, tenant: &str, connection: &str) {
        assert_eq!(tenant, "fixture-tenant");
        assert_eq!(connection, "fixture-connection");
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.blocked > 0 {
            assert!(call <= self.blocked, "agent continued after cancellation");
        }
        if call == self.blocked {
            struct Cleanup(Arc<Notify>);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    self.0.notify_one();
                }
            }
            let _cleanup = Cleanup(self.cleaned.clone());
            self.started.notify_one();
            std::future::pending::<()>().await;
        }
    }
    fn schema(&self) -> RowSet {
        if !self.schema_exists.load(Ordering::SeqCst) {
            return RowSet::default();
        }
        sql_fixture::memory_schema()
    }
}
#[async_trait::async_trait]
impl ConnectionResolverHost for NativeFixture {
    async fn describe(&self, tenant: &str, connection: String) -> Result<Vec<u8>, String> {
        self.step(tenant, &connection).await;
        Ok(sql_fixture::descriptor(&connection))
    }
    async fn resolve_resource(&self, _: &str, _: String, _: Vec<u8>) -> Result<Vec<u8>, String> {
        panic!("unexpected resource lookup")
    }
}
#[async_trait::async_trait]
impl DatabaseHost for NativeFixture {
    async fn query(
        &self,
        tenant: &str,
        connection: &str,
        request: QueryRequest,
    ) -> Result<RowSet, DatabaseError> {
        if request.sql == "SELECT 42" {
            assert_eq!(tenant, "fixture-tenant");
            assert_eq!(connection, "fixture-connection");
            assert_eq!(self.calls.load(Ordering::SeqCst), self.blocked);
            return Ok(rows(vec![json!({"value":42})]));
        }
        self.step(tenant, connection).await;
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if request.sql.contains("deleted=TRUE") {
            return Ok(RowSet::default());
        }
        if request.sql.contains("FROM \"__schema\"") {
            return Ok(self.schema());
        }
        if request.sql.contains("COUNT(*)") {
            return Ok(rows(vec![
                json!({"count":if matches!(self.operation,Operation::SaveExisting) {1} else {0}}),
            ]));
        }
        if request.sql.contains("FROM \"ai_conversation_memory\"") {
            return Ok(if matches!(self.operation, Operation::SaveExisting) {
                rows(vec![
                    json!({"id":"existing","messages":[],"message_count":0,"conversation_id":"conversation"}),
                ])
            } else {
                RowSet::default()
            });
        }
        assert_eq!(request.sql, "SELECT 1");
        Ok(rows(vec![json!({"one":1})]))
    }
    async fn execute(
        &self,
        tenant: &str,
        connection: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError> {
        self.step(tenant, connection).await;
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        assert!(request.sql.starts_with("INSERT") || request.sql.starts_with("UPDATE"));
        Ok(ExecutionResult {
            rows_affected: 1,
            returned: None,
        })
    }
    async fn execute_batch(
        &self,
        tenant: &str,
        connection: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError> {
        self.step(tenant, connection).await;
        assert_eq!(request.mode, BatchMode::Atomic);
        assert!(
            request
                .statements
                .iter()
                .any(|s| s.sql.starts_with("CREATE TABLE"))
        );
        self.schema_exists.store(true, Ordering::SeqCst);
        Ok(BatchResult {
            mode: request.mode,
            results: request
                .statements
                .iter()
                .enumerate()
                .map(|(index, _)| StatementResult {
                    index,
                    result: Ok(ExecutionResult {
                        rows_affected: 1,
                        returned: None,
                    }),
                })
                .collect(),
        })
    }
}

async fn cancellation(operation: Operation, blocked: usize) -> anyhow::Result<()> {
    let fixture = NativeFixture::new(operation, blocked);
    let mut next = Operation::Query.input();
    next["sql"] = "SELECT 42".into();
    let bytes = compose_agent(
        "object-model",
        operation.capability(),
        &serde_json::to_vec(&operation.input())?,
        "query-sql",
        &serde_json::to_vec(&next)?,
    )?;
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        cancel_and_reuse_with_state(
            bytes,
            fixture.state(),
            fixture.started.clone(),
            fixture.cleaned.clone(),
        ),
    )
    .await
    .map_err(|error| anyhow::anyhow!("{operation:?} stage {blocked}: {error}"))??;
    assert_eq!(
        output,
        json!({"success":true,"rows":[{"value":42}],"row_count":1,"error":null})
    );
    Ok(())
}
#[tokio::test]
async fn sql_query_and_execute_cancel_native_calls_and_reuse_instance() -> anyhow::Result<()> {
    for operation in [Operation::Query, Operation::Execute] {
        cancellation(operation, 1).await?;
    }
    Ok(())
}
#[tokio::test]
async fn memory_load_cancels_metadata_schema_creation_count_and_query() -> anyhow::Result<()> {
    for operation in [Operation::Load, Operation::LoadMissingSchema] {
        for stage in 1..=operation.stages() {
            cancellation(operation, stage).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn memory_save_cancels_before_or_during_create_and_update() -> anyhow::Result<()> {
    for operation in [Operation::SaveNew, Operation::SaveExisting] {
        for stage in 1..=operation.stages() {
            cancellation(operation, stage).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn memory_load_and_save_complete_via_native_sql() -> anyhow::Result<()> {
    for operation in [
        Operation::Load,
        Operation::LoadMissingSchema,
        Operation::SaveNew,
        Operation::SaveExisting,
    ] {
        let fixture = NativeFixture::new(operation, 0);
        let output = invoke_named_agent_with_state(
            "object-model",
            fixture.state(),
            operation.capability(),
            serde_json::to_vec(&operation.input())?,
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let output: Value = serde_json::from_slice(&output)?;
        assert_eq!(output["success"], true, "{operation:?}: {output}");
        assert_eq!(
            fixture.calls.load(Ordering::SeqCst),
            operation.stages(),
            "{operation:?}"
        );
    }
    Ok(())
}
#[tokio::test]
async fn sql_read_and_write_keep_distinct_retry_contracts() -> anyhow::Result<()> {
    for operation in [Operation::Query, Operation::Execute] {
        for outcome in [Outcome::RolledBack, Outcome::Unknown] {
            let mut fixture = NativeFixture::new(operation, 0);
            Arc::get_mut(&mut fixture).unwrap().failure = Some(DatabaseError {
                code: "DATABASE_CONNECTION_UNAVAILABLE".into(),
                message: "Connection unavailable".into(),
                outcome,
                retryable: true,
                sqlstate: None,
                statement_index: None,
            });
            let result = invoke_named_agent_with_state(
                "object-model",
                fixture.state(),
                operation.capability(),
                serde_json::to_vec(&operation.input())?,
            )
            .await?;
            let error =
                result.expect_err("SQL failure must not become a successful capability result");
            assert_eq!(
                error.retryable,
                matches!(operation, Operation::Query) || outcome == Outcome::RolledBack
            );
            assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        }
    }
    Ok(())
}
