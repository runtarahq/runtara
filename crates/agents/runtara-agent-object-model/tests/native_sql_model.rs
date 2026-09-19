//! Agent orchestration exercises the same three SQL operations as its WIT client.
use runtara_agent_object_model::{model::ObjectModel, sql_client::SqlClient};
use runtara_database_contract::*;
use runtara_object_model_core::config::ObjectModelLayout;
use runtara_object_model_core::{ColumnDefinition, ColumnType, CreateSchemaRequest, FilterRequest};
use runtara_object_store::{ObjectStore, StoreConfig, database::DatabaseLimits};
use serde_json::{Value, json};

struct Native(ObjectStore);
impl SqlClient for Native {
    async fn layout(&self, connection: &str) -> Result<ObjectModelLayout, DatabaseError> {
        assert_eq!(connection, "test-connection");
        Ok(ObjectModelLayout::from(self.0.config()))
    }
    async fn query(&self, _: &str, request: QueryRequest) -> Result<RowSet, DatabaseError> {
        self.0
            .database_query(request, DatabaseLimits::default())
            .await
    }
    async fn execute(&self, _: &str, request: Statement) -> Result<ExecutionResult, DatabaseError> {
        self.0
            .database_execute(request, DatabaseLimits::default())
            .await
    }
    async fn execute_batch(
        &self,
        _: &str,
        request: BatchRequest,
    ) -> Result<BatchResult, DatabaseError> {
        self.0
            .database_execute_batch(request, DatabaseLimits::default())
            .await
    }
}
async fn setup() -> Native {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL required");
    let metadata = format!("agent_meta_{}", uuid::Uuid::new_v4().simple());
    Native(
        ObjectStore::connect(StoreConfig::builder(url).metadata_table(metadata).build())
            .await
            .unwrap(),
    )
}
fn schema() -> CreateSchemaRequest {
    CreateSchemaRequest::new(
        "records",
        format!("agent_data_{}", uuid::Uuid::new_v4().simple()),
        vec![
            ColumnDefinition::new("name", ColumnType::String).default("'unnamed'"),
            ColumnDefinition::new("data", ColumnType::Json),
            ColumnDefinition::new("count", ColumnType::Integer),
        ],
    )
}
fn filter() -> FilterRequest {
    serde_json::from_value(json!({"limit":100,"offset":0})).unwrap()
}

#[tokio::test]
async fn agent_schema_crud_defaults_nulls_and_native_reads_agree() {
    let native = setup().await;
    let model = ObjectModel::new(&native, "test-connection").await.unwrap();
    let request = schema();
    let created = model.create_schema(request.clone()).await.unwrap();
    assert_eq!(created["success"], true);
    let bootstrapped = model.create_schema(request).await.unwrap();
    assert_eq!(bootstrapped["success"], true);
    let created = model
        .create("records", json!({"data":null,"count":"42"}))
        .await
        .unwrap();
    let id = created["instance_id"].as_str().unwrap();
    let rows = model.query("records", filter()).await.unwrap();
    assert_eq!(rows["total_count"], 1);
    assert_eq!(rows["instances"][0]["name"], "unnamed");
    assert_eq!(rows["instances"][0]["count"], 42);
    assert_eq!(rows["instances"][0]["data"], Value::Null);
    let (stored, count) = native
        .0
        .filter_instances("records", filter())
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(stored[0].properties["name"], rows["instances"][0]["name"]);
    assert_eq!(stored[0].id, id);
    model
        .update("records", id, json!({"count":43}))
        .await
        .unwrap();
    let rows = model.query("records", filter()).await.unwrap();
    assert_eq!(rows["instances"][0]["name"], "unnamed");
    assert_eq!(rows["instances"][0]["count"], 43);
    model
        .update("records", id, json!({"count":null}))
        .await
        .unwrap();
    let rows = model.query("records", filter()).await.unwrap();
    assert!(
        rows["instances"][0].get("count").is_none(),
        "SQL NULL instance properties must remain omitted"
    );
    assert_eq!(
        rows["instances"][0].get("data"),
        Some(&Value::Null),
        "JSON null must remain present"
    );
    model.delete("records", id).await.unwrap();
    assert_eq!(
        model.query("records", filter()).await.unwrap()["total_count"],
        0
    );
}

#[tokio::test]
async fn concurrent_agent_schema_bootstrap_returns_success() {
    let native = setup().await;
    let model = ObjectModel::new(&native, "test-connection").await.unwrap();
    let request = schema();
    let (a, b) = tokio::join!(
        model.create_schema(request.clone()),
        model.create_schema(request)
    );
    assert_eq!(a.unwrap()["success"], true);
    assert_eq!(b.unwrap()["success"], true);
    assert!(model.schema("records").await.unwrap().is_some());
}

#[tokio::test]
async fn validation_failure_does_not_mutate_database() {
    let native = setup().await;
    let model = ObjectModel::new(&native, "test-connection").await.unwrap();
    model.create_schema(schema()).await.unwrap();
    assert!(
        model
            .create("records", json!({"count":"not-an-integer"}))
            .await
            .is_err()
    );
    assert_eq!(
        model.query("records", filter()).await.unwrap()["total_count"],
        0
    );
}

#[tokio::test]
async fn agent_bulk_modes_and_partial_updates_match_native_semantics() {
    use runtara_agent_object_model::operations::{Operation, invoke};
    let native = setup().await;
    let model = ObjectModel::new(&native, "test-connection").await.unwrap();
    let mut request = schema();
    request.columns[0].unique = true;
    model.create_schema(request).await.unwrap();
    let result = invoke(&native, Operation::BulkCreate, json!({"schema_name":"records", "columns":["name","count"], "rows":[["a","1"],["b","2"],["bad","invalid"]], "on_error":"skip"}), "test-connection").await.unwrap();
    assert_eq!(result["created_count"], 2);
    assert_eq!(result["skipped_count"], 1);
    assert_eq!(result["errors"][0]["index"], 2);
    let result = invoke(&native, Operation::BulkCreate, json!({"schema_name":"records", "instances":[{"name":"a","count":3},{"name":"c","count":4}], "on_conflict":"skip", "conflict_columns":["name"]}), "test-connection").await.unwrap();
    assert_eq!(result["created_count"], 1);
    assert_eq!(result["skipped_count"], 1);
    let result = invoke(&native, Operation::BulkCreate, json!({"schema_name":"records", "instances":[{"name":"a","data":{"preserved":true}},{"name":"b","count":5}], "on_conflict":"upsert", "conflict_columns":["name"]}), "test-connection").await.unwrap();
    assert_eq!(result["created_count"], 2);
    let result = invoke(
        &native,
        Operation::Query,
        json!({"schema_name":"records", "filters":{"name":"a"}}),
        "test-connection",
    )
    .await
    .unwrap();
    assert_eq!(result["instances"][0]["count"], 1);
    assert_eq!(result["instances"][0]["data"], json!({"preserved":true}));
    let id = result["instances"][0]["id"].as_str().unwrap();
    let result = invoke(&native, Operation::BulkUpdate, json!({"schema_name":"records","mode":"byIds","updates":[{"id":id,"properties":{"count":7}},{"id":id,"properties":{"count":8}}]}), "test-connection").await.unwrap();
    assert_eq!(result["updated_count"], 2);
    let result = invoke(&native, Operation::BulkUpdate, json!({"schema_name":"records","mode":"byCondition","properties":{"count":9},"condition":{"op":"EQ","arguments":["name","a"]}}), "test-connection").await.unwrap();
    assert_eq!(result["updated_count"], 1);
    let result = invoke(
        &native,
        Operation::BulkDelete,
        json!({"schema_name":"records","ids":[id]}),
        "test-connection",
    )
    .await
    .unwrap();
    assert_eq!(result["deleted_count"], 1);
    assert_eq!(
        model.query("records", filter()).await.unwrap()["total_count"],
        2
    );
}

#[tokio::test]
async fn sql_capability_adapters_preserve_inputs_and_output_shapes() {
    use runtara_agent_object_model::operations::{Operation, invoke};
    let native = setup().await;
    let result = invoke(&native, Operation::QuerySql, json!({"sql":"SELECT $1 AS n, $2 AS j", "params":[{"type":"integer","value":"9223372036854775807"},{"type":"json","value":null}],"resultSchema":[{"name":"n","type":"integer"},{"name":"j","type":"json"}]}), "test-connection").await.unwrap();
    assert_eq!(result["rowCount"], 1);
    assert_eq!(result["rows"][0]["n"], json!(i64::MAX));
    assert!(result["rows"][0]["j"].is_null());
}

#[tokio::test]
async fn legacy_result_schemas_keep_type_families_and_validation() {
    use runtara_agent_object_model::operations::{Operation, invoke};
    let native = setup().await;
    let result = invoke(&native, Operation::QuerySql, json!({
        "sql":"SELECT '00000000-0000-0000-0000-000000000001'::uuid AS id, 9223372036854775807::bigint AS n, '2026-01-02'::date AS d, '2026-01-02 03:04:05'::timestamp AS t, 'ready'::text AS status, ARRAY[1,2] AS ignored",
        "resultSchema":[{"name":"id","type":"string"},{"name":"n","type":"decimal"},{"name":"d","type":"timestamp"},{"name":"t","type":"timestamp"},{"name":"status","type":"enum","values":["ready"]}]
    }), "test-connection").await.unwrap();
    assert_eq!(
        result["rows"][0],
        json!({"id":"00000000-0000-0000-0000-000000000001","n":i64::MAX,"d":"2026-01-02","t":"2026-01-02 03:04:05","status":"ready"})
    );
    for (sql, column) in [
        (
            "SELECT 'other'::text AS value",
            json!({"name":"value","type":"enum","values":["ready"]}),
        ),
        (
            "SELECT NULL::text AS value",
            json!({"name":"value","type":"string","nullable":false}),
        ),
        (
            "SELECT 1 AS value",
            json!({"name":"value","type":"boolean"}),
        ),
    ] {
        assert!(
            invoke(
                &native,
                Operation::QuerySql,
                json!({"sql":sql,"resultSchema":[column]}),
                "test-connection"
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn recreating_a_deleted_schema_preserves_tombstoned_data() {
    let native = setup().await;
    let model = ObjectModel::new(&native, "test-connection").await.unwrap();
    let request = schema();
    model.create_schema(request.clone()).await.unwrap();
    model
        .create("records", json!({"name":"old"}))
        .await
        .unwrap();
    native.0.delete_schema("records").await.unwrap();
    let (a, b) = tokio::join!(
        model.create_schema(request.clone()),
        model.create_schema(request)
    );
    assert_eq!(a.unwrap()["success"], true);
    assert_eq!(b.unwrap()["success"], true);
    assert_eq!(
        model.query("records", filter()).await.unwrap()["total_count"],
        0
    );
    model
        .create("records", json!({"name":"new"}))
        .await
        .unwrap();
    let tombstone = model
        .rows(
            format!(
                "SELECT table_name FROM {} WHERE deleted=TRUE",
                runtara_object_model_core::sql::quote_identifier(&native.0.config().metadata_table)
            ),
            vec![],
        )
        .await
        .unwrap();
    let old_table = tombstone[0]["table_name"].as_str().unwrap();
    let old = model
        .rows(
            format!(
                "SELECT name FROM {}",
                runtara_object_model_core::sql::quote_identifier(old_table)
            ),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(old[0]["name"], "old");
}

#[tokio::test]
async fn aggregate_and_condition_filters_match_native_results() {
    let native = setup().await;
    let model = ObjectModel::new(&native, "test-connection").await.unwrap();
    model.create_schema(schema()).await.unwrap();
    for (name, count) in [("a", 1), ("a", 2), ("b", 3)] {
        model
            .create("records", json!({"name":name,"count":count}))
            .await
            .unwrap();
    }
    let request:runtara_object_model_core::sql::AggregateRequest=serde_json::from_value(json!({"groupBy":["name"],"aggregates":[{"fn":"SUM","column":"count","alias":"total"}],"orderBy":[{"column":"name","direction":"ASC"}],"condition":{"op":"GT","arguments":["count",1]}})).unwrap();
    let expected = native
        .0
        .aggregate_instances("records", request.clone())
        .await
        .unwrap();
    let actual = model.aggregate("records", request).await.unwrap();
    assert_eq!(actual["rows"], serde_json::to_value(expected.rows).unwrap());
    assert_eq!(
        actual["columns"],
        serde_json::to_value(expected.columns).unwrap()
    );
    assert_eq!(actual["group_count"], expected.group_count);
}
