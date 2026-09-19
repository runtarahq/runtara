use super::*;
use runtara_database_contract::*;
use test_support::{Ticker, bounded, spec};

#[derive(Default)]
struct Database {
    calls: std::sync::Mutex<Vec<(String, String, &'static str)>>,
}

impl Database {
    fn record(&self, tenant: &str, connection: &str, operation: &'static str) {
        self.calls
            .lock()
            .unwrap()
            .push((tenant.into(), connection.into(), operation));
    }
}
#[async_trait::async_trait]
impl crate::DatabaseHost for Database {
    async fn query(
        &self,
        tenant: &str,
        connection: &str,
        request: QueryRequest,
    ) -> Result<RowSet, DatabaseError> {
        assert_eq!(request.sql, "SELECT 1");
        self.record(tenant, connection, "query");
        Ok(RowSet::default())
    }
    async fn execute(
        &self,
        tenant: &str,
        connection: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError> {
        assert_eq!(request.sql, "SELECT 1");
        self.record(tenant, connection, "execute");
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
        self.record(tenant, connection, "execute-batch");
        Ok(BatchResult {
            mode: request.mode,
            results: vec![],
        })
    }
}

#[tokio::test]
async fn database_imports_use_host_identity_and_do_not_cache_mutations() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let database = Arc::new(Database::default());
    executor.set_database(database.clone())?;
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, include_str!("database_test.wat"))?)
        .await?;
    for tenant in ["tenant-a", "tenant-b", "tenant-a"] {
        let mut run = spec();
        run.trusted_tenant = Some(tenant.into());
        run.env.insert("RUNTARA_TENANT_ID".into(), "spoofed".into());
        let result =
            bounded(executor.execute_invoke(prepared.instance_pre(), run, b"{}".to_vec())).await;
        match result.exit {
            InvokeExit::Completed(bytes) => assert_eq!(
                serde_json::from_slice::<BatchResult>(&bytes)?.mode,
                BatchMode::Atomic
            ),
            other => panic!("unexpected exit: {other:?}"),
        }
    }
    let calls = database.calls.lock().unwrap();
    assert_eq!(calls.len(), 9);
    for (index, (tenant, connection, operation)) in calls.iter().enumerate() {
        assert_eq!(
            tenant,
            if index / 3 == 1 {
                "tenant-b"
            } else {
                "tenant-a"
            }
        );
        assert_eq!(connection, "conn");
        assert_eq!(*operation, ["query", "execute", "execute-batch"][index % 3]);
    }
    Ok(())
}

#[tokio::test]
async fn guest_environment_cannot_supply_missing_database_authority() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let database = Arc::new(Database::default());
    executor.set_database(database.clone())?;
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, include_str!("database_test.wat"))?)
        .await?;
    let mut run = spec();
    run.trusted_tenant = None;
    run.env.insert("RUNTARA_TENANT_ID".into(), "spoofed".into());
    let result =
        bounded(executor.execute_invoke(prepared.instance_pre(), run, b"{}".to_vec())).await;
    assert!(!matches!(result.exit, InvokeExit::Completed(_)));
    assert!(database.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn restricted_instances_deny_all_database_imports_even_with_a_backend() -> anyhow::Result<()>
{
    use crate::lifecycle::{WorkflowErrorInfo, WorkflowOutcome};
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    // This fixture invokes all three imports and expects each to return an error.
    let source = include_str!("database_test.wat").replace(
        "i32.const 256 i32.load if unreachable end",
        "i32.const 256 i32.load i32.eqz if unreachable end",
    );
    let component = Component::new(&engine, source)?;
    let database = Arc::new(Database::default());
    let mut state =
        crate::HostState::new(Arc::new(crate::CallContext::for_test("tenant-a", "", "")))
            .with_database(database.clone());
    state.restricted = true;
    let mut store = wasmtime::Store::new(&engine, state);
    let linker = crate::build_linker(&engine)?;
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let interface = instance
        .get_export_index(&mut store, None, crate::lifecycle::LIFECYCLE_INTERFACE_NAME)
        .unwrap();
    let export = instance
        .get_export_index(&mut store, Some(&interface), "invoke")
        .unwrap();
    let invoke = instance
        .get_typed_func::<(Vec<u8>,), (Result<WorkflowOutcome, WorkflowErrorInfo>,)>(
            &mut store, export,
        )?;
    let (result,) = invoke.call_async(&mut store, (b"{}".to_vec(),)).await?;
    let Ok(WorkflowOutcome::Completed(bytes)) = result else {
        panic!("expected a denied database result")
    };
    let error: DatabaseError = serde_json::from_slice(&bytes)?;
    assert!(error.message.contains("denied in trusted instances"));
    assert_eq!(error.outcome, Outcome::NotStarted);
    assert!(database.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn oversized_guest_sql_requests_are_rejected_before_native_dispatch() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let database = Arc::new(Database::default());
    executor.set_database(database.clone())?;
    // A valid guest slice larger than the request ceiling: no out-of-bounds
    // trap may stand in for the host's check. Exercise all three imports.
    let source = include_str!("database_test.wat")
        .replace(
            "(memory (export \"memory\") 1)",
            "(memory (export \"memory\") 1025)",
        )
        .replace(
            "(i32.const 18) (i32.const 256)",
            "(i32.const 67108864) (i32.const 256)",
        )
        .replace(
            "(i32.const 17) (i32.const 256)",
            "(i32.const 67108864) (i32.const 256)",
        )
        .replace(
            "i32.const 256 i32.load if unreachable end",
            "i32.const 256 i32.load i32.eqz if unreachable end",
        );
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, source)?)
        .await?;
    let mut run = spec();
    run.trusted_tenant = Some("tenant".into());
    let result =
        bounded(executor.execute_invoke(prepared.instance_pre(), run, b"{}".to_vec())).await;
    match result.exit {
        InvokeExit::Completed(bytes) => {
            let error: DatabaseError = serde_json::from_slice(&bytes)?;
            assert_eq!(error.code, "DATABASE_INVALID_REQUEST");
            assert!(error.message.contains("oversized"));
        }
        other => panic!("unexpected exit: {other:?}"),
    }
    assert!(database.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn scoped_workflow_tasks_keep_database_authority_and_release_results() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = Arc::new(WorkflowExecutor::new(engine.clone())?);
    let database = Arc::new(Database::default());
    executor.set_database(database.clone())?;
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, include_str!("database_test.wat"))?)
        .await?;
    let tasks = crate::isolated_tasks::IsolatedTasks::new(engine, 4, MAX_RESPONSE_BYTES)?;
    let mut children = Vec::new();
    for tenant in ["child-tenant-a", "child-tenant-b"] {
        let executor = executor.clone();
        let prepared = prepared.clone();
        let mut run = spec();
        run.trusted_tenant = Some(tenant.into());
        run.env.insert("RUNTARA_TENANT_ID".into(), "forged".into());
        children.push(tasks.spawn(move |token| async move {
            executor
                .execute_isolated_workflow(
                    prepared.instance_pre(),
                    run,
                    b"{}".to_vec(),
                    token,
                    None,
                )
                .await
                .exit
        })?);
    }
    for child in children {
        let result = bounded(tasks.join(child)).await?;
        assert!(
            matches!(result.outcome(), InvokeExit::Completed(_)),
            "{:?}",
            result.outcome()
        );
        tasks.release(child).await?;
    }
    tasks.shutdown().await?;
    let calls = database.calls.lock().unwrap();
    for tenant in ["child-tenant-a", "child-tenant-b"] {
        for operation in ["query", "execute", "execute-batch"] {
            assert_eq!(
                calls
                    .iter()
                    .filter(|(t, c, o)| t == tenant && c == "conn" && *o == operation)
                    .count(),
                1
            );
        }
    }
    assert_eq!(calls.len(), 6);
    Ok(())
}
