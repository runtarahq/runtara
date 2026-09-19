//! Real built WASM -> native host imports -> isolated PostgreSQL. No HTTP listener.
use runtara_component_host::{CallContext, ConnectionResolverHost, DatabaseHost, HostState};
use runtara_database_contract::*;
use runtara_object_store::{
    ColumnDefinition, ColumnType, CreateSchemaRequest, ObjectStore, StoreConfig,
    database::DatabaseLimits,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use wasmtime::{
    Store,
    component::{Component, Linker},
};

struct Backend(ObjectStore);
impl Backend {
    fn authority(&self, tenant: &str, connection: &str) {
        assert_eq!(tenant, "wasm-tenant");
        assert_eq!(connection, "wasm-connection");
    }
}
#[async_trait::async_trait]
impl DatabaseHost for Backend {
    async fn query(
        &self,
        tenant: &str,
        connection: &str,
        request: QueryRequest,
    ) -> Result<RowSet, DatabaseError> {
        self.authority(tenant, connection);
        self.0
            .database_query(request, DatabaseLimits::default())
            .await
    }
    async fn execute(
        &self,
        tenant: &str,
        connection: &str,
        request: Statement,
    ) -> Result<ExecutionResult, DatabaseError> {
        self.authority(tenant, connection);
        self.0
            .database_execute(request, DatabaseLimits::default())
            .await
    }
    async fn execute_batch(
        &self,
        tenant: &str,
        connection: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError> {
        self.authority(tenant, connection);
        self.0
            .database_execute_batch(request, DatabaseLimits::default())
            .await
    }
}
#[async_trait::async_trait]
impl ConnectionResolverHost for Backend {
    async fn describe(&self, tenant: &str, connection: String) -> Result<Vec<u8>, String> {
        self.authority(tenant, &connection);
        Ok(serde_json::to_vec(&json!({"connectionId":connection,"integrationId":"postgres","metadata":{"object_model":runtara_object_store::config::ObjectModelLayout::from(self.0.config())}})).unwrap())
    }
    async fn resolve_resource(&self, _: &str, _: String, _: Vec<u8>) -> Result<Vec<u8>, String> {
        panic!("unexpected resource lookup")
    }
}
struct Agent {
    engine: Arc<wasmtime::Engine>,
    component: Component,
    linker: Linker<HostState>,
    backend: Arc<Backend>,
}
impl Agent {
    async fn invoke(&self, capability: &str, mut input: Value) -> anyhow::Result<Value> {
        input["_connection"] =
            json!({"connection_id":"wasm-connection","integration_id":"postgres","parameters":{}});
        let state = HostState::new(Arc::new(CallContext::for_test("wasm-tenant", "")))
            .with_database(self.backend.clone())
            .with_connection_resolver(self.backend.clone());
        let mut store = Store::new(&self.engine, state);
        let instance = self
            .linker
            .instantiate_async(&mut store, &self.component)
            .await?;
        let interface = instance
            .get_export_index(
                &mut store,
                None,
                "runtara:agent-object-model/capabilities@0.4.0",
            )
            .unwrap();
        let export = instance
            .get_export_index(&mut store, Some(&interface), "invoke")
            .unwrap();
        type Output = (Result<Vec<u8>, runtara_component_host::ErrorInfo>,);
        let invoke = instance.get_typed_func::<(String, Vec<u8>), Output>(&mut store, export)?;
        let (output,) = invoke
            .call_async(&mut store, (capability.into(), serde_json::to_vec(&input)?))
            .await?;
        let output = output.map_err(|e| anyhow::anyhow!("{capability}: {e:?}"))?;
        let value: Value = serde_json::from_slice(&output)?;
        anyhow::ensure!(value["success"] != false, "{capability}: {value}");
        Ok(value)
    }
}

#[tokio::test]
async fn built_object_model_crud_bulk_aggregate_and_memory_use_native_postgres()
-> anyhow::Result<()> {
    let url = std::env::var("TEST_DATABASE_URL")?;
    let namespace = format!("wasm_objects_{}", uuid::Uuid::new_v4().simple());
    let parent = ObjectStore::connect(
        StoreConfig::builder(&url)
            .metadata_table(format!("{namespace}_meta"))
            .build(),
    )
    .await?;
    parent
        .execute(&format!("CREATE SCHEMA {namespace}"), &[])
        .await?;
    let mut scoped = url::Url::parse(&url)?;
    scoped
        .query_pairs_mut()
        .append_pair("options", &format!("-c search_path={namespace},public"));
    let store = ObjectStore::connect(StoreConfig::builder(scoped.to_string()).build()).await?;
    let directory = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release")
        });
    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let component =
        Component::from_file(&engine, directory.join("runtara_agent_object_model.wasm"))?;
    let linker = runtara_component_host::build_linker(&engine)?;
    let agent = Agent {
        engine,
        component,
        linker,
        backend: Arc::new(Backend(store)),
    };
    let fresh = agent
        .invoke(
            "query-sql",
            json!({"sql":"SELECT to_regclass('__schema')::text AS metadata"}),
        )
        .await?;
    assert_eq!(
        fresh["rows"][0]["metadata"],
        Value::Null,
        "raw SQL must not initialize Object Model metadata"
    );
    let empty = agent
        .invoke("load-memory", json!({"conversation_id":"fresh-session"}))
        .await?;
    assert_eq!(
        empty["messages"],
        json!([]),
        "the agent bootstraps metadata and memory on a fresh database"
    );
    agent
        .backend
        .0
        .create_schema(CreateSchemaRequest::new(
            "records",
            "records",
            vec![
                ColumnDefinition::new("name", ColumnType::String).default("'default'"),
                ColumnDefinition::new("data", ColumnType::Json),
                ColumnDefinition::new("count", ColumnType::Integer),
            ],
        ))
        .await?;
    let created = agent
        .invoke(
            "create-instance",
            json!({"schema_name":"records","data":{"data":null,"count":1}}),
        )
        .await?;
    let id = created["instance_id"].as_str().unwrap();
    let native = agent.backend.0.get_instance("records", id).await?.unwrap();
    assert_eq!(native.properties["name"], "default");
    assert_eq!(native.properties["data"], Value::Null);
    agent
        .invoke(
            "update-instance",
            json!({"schema_name":"records","instance_id":id,"data":{"count":2}}),
        )
        .await?;
    let queried = agent
        .invoke(
            "query-instances",
            json!({"schema_name":"records","filters":{"name":"default"}}),
        )
        .await?;
    assert_eq!(queried["instances"][0]["count"], 2);
    let exists = agent
        .invoke(
            "check-instance-exists",
            json!({"schema_name":"records","filters":{"name":"default"}}),
        )
        .await?;
    assert_eq!(exists["instance_id"], id);
    let unchanged=agent.invoke("create-if-not-exists",json!({"schema_name":"records","match_filters":{"name":"default"},"data":{"name":"should-not-create"}})).await?;
    assert_eq!(unchanged["already_existed"], true);
    let bulk = agent
        .invoke(
            "bulk-create-instances",
            json!({"schema_name":"records","columns":["name","count"],"rows":[["a",3],["b",4]]}),
        )
        .await?;
    assert_eq!(bulk["created_count"], 2);
    let aggregate=agent.invoke("query-aggregate",json!({"schema_name":"records","aggregates":[{"fn":"SUM","column":"count","alias":"total"}]})).await?;
    assert_eq!(aggregate["rows"][0][0].as_f64(), Some(9.0));
    let records = agent
        .invoke("query-instances", json!({"schema_name":"records"}))
        .await?;
    let a = records["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "a")
        .unwrap();
    let a_id = a["id"].as_str().unwrap();
    let changed = agent
        .invoke(
            "bulk-update-instances",
            json!({"schema_name":"records","updates":[{"id":a_id,"properties":{"count":null}}]}),
        )
        .await?;
    assert_eq!(changed["updated_count"], 1);
    let updated = agent
        .invoke(
            "query-instances",
            json!({"schema_name":"records","filters":{"name":"a"}}),
        )
        .await?;
    assert!(
        updated["instances"][0].get("count").is_none(),
        "SQL NULL object properties remain omitted"
    );
    let deleted = agent
        .invoke(
            "bulk-delete-instances",
            json!({"schema_name":"records","ids":[a_id]}),
        )
        .await?;
    assert_eq!(deleted["deleted_count"], 1);
    assert!(
        agent
            .backend
            .0
            .get_instance("records", a_id)
            .await?
            .is_none()
    );
    let written = agent.invoke("execute-sql", json!({"sql":"UPDATE records SET count = $1 WHERE name = $2","params":[{"type":"integer","value":5},{"type":"string","value":"b"}]})).await?;
    assert_eq!(written["rows_affected"], 1);
    let raw_row = agent.invoke("query-sql", json!({"sql":"SELECT count FROM records WHERE name = $1","params":[{"type":"string","value":"b"}]})).await?;
    assert_eq!(raw_row["rows"][0]["count"], 5);
    agent
        .invoke(
            "delete-instance",
            json!({"schema_name":"records","instance_id":id}),
        )
        .await?;
    assert!(agent.backend.0.get_instance("records", id).await?.is_none());
    let raw=agent.invoke("query-sql",json!({"sql":"SELECT $1 AS value","params":[{"type":"integer","value":"9223372036854775807"}]})).await?;
    assert_eq!(raw["rows"][0]["value"], json!(i64::MAX));
    let empty = agent
        .invoke("load-memory", json!({"conversation_id":"session"}))
        .await?;
    assert_eq!(empty["messages"], json!([]));
    for messages in [
        json!([{"role":"user","content":"first"}]),
        json!([{"role":"user","content":"replacement"}]),
    ] {
        agent
            .invoke(
                "save-memory",
                json!({"conversation_id":"session","messages":messages}),
            )
            .await?;
        let loaded = agent
            .invoke("load-memory", json!({"conversation_id":"session"}))
            .await?;
        assert_eq!(loaded["messages"], messages);
        assert_eq!(loaded["message_count"], 1);
    }
    Ok(())
}
