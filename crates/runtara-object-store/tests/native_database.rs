//! Native SQL contract integration tests against an explicitly configured test DB.
use runtara_database_contract::*;
use runtara_object_store::{ObjectStore, StoreConfig, database::DatabaseLimits};
use std::time::Duration;

async fn store() -> ObjectStore {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL required");
    ObjectStore::connect(
        StoreConfig::builder(url)
            .metadata_table(format!("sql_host_{}", uuid::Uuid::new_v4().simple()))
            .pool(runtara_object_store::PoolConfig {
                max_connections: 1,
                ..Default::default()
            })
            .build(),
    )
    .await
    .expect("test store must initialize")
}
fn command(sql: impl Into<String>) -> Statement {
    Statement {
        sql: sql.into(),
        params: vec![],
        returning: None,
    }
}
fn read(sql: impl Into<String>) -> QueryRequest {
    QueryRequest {
        sql: sql.into(),
        params: vec![],
        result_schema: ResultSpec::Raw,
    }
}
fn table() -> String {
    format!("sql_host_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn postgres_handles_sql_syntax_without_an_application_allowlist() {
    let store = store().await;
    let limits = DatabaseLimits::default();
    let table = table();
    store
        .database_execute_batch(
            BatchRequest {
                mode: BatchMode::Atomic,
                statements: vec![
                    command(format!(
                        "CREATE TABLE {table} (id bigint, active boolean, value text)"
                    )),
                    command(format!(
                        "CREATE UNIQUE INDEX {table}_active ON {table} (id) WHERE active"
                    )),
                    command(format!("COMMENT ON TABLE {table} IS 'caller-owned SQL'")),
                ],
            },
            limits,
        )
        .await
        .unwrap();
    store
        .database_execute(
            command(format!(
                "DO $$ BEGIN INSERT INTO {table} VALUES (1, true, 'initial'); END $$"
            )),
            limits,
        )
        .await
        .unwrap();
    let result = store
        .database_execute(
            Statement {
                sql: format!(
                    "INSERT INTO {table} VALUES (1, true, $1) \
                     ON CONFLICT (id) WHERE active DO UPDATE SET value = EXCLUDED.value \
                     RETURNING value"
                ),
                params: vec![SqlValue::Text("updated".into())],
                returning: Some(ResultSpec::Raw),
            },
            limits,
        )
        .await
        .unwrap();
    assert_eq!(result.rows_affected, 1);
    assert_eq!(
        result.returned.unwrap().rows,
        vec![vec![SqlValue::Text("updated".into())]]
    );
    let result = store
        .database_query(
            read("SELECT set_config('application_name', 'caller-owned SQL', true)"),
            limits,
        )
        .await
        .unwrap();
    assert_eq!(
        result.rows,
        vec![vec![SqlValue::Text("caller-owned SQL".into())]]
    );
}

#[tokio::test]
async fn exact_codec_round_trip_and_null_distinction() {
    let store = store().await;
    let values = vec![
        SqlValue::Integer(i64::MAX.to_string()),
        SqlValue::Decimal("12345678901234567890.12345678".into()),
        SqlValue::Null(SqlType::Json),
        SqlValue::Json(serde_json::Value::Null),
        SqlValue::Uuid(uuid::Uuid::new_v4().to_string()),
        SqlValue::TimestampTz("2025-01-02T03:04:05+00:00".into()),
        SqlValue::Timestamp("2025-01-02 03:04:05".into()),
        SqlValue::Date("2025-01-02".into()),
        SqlValue::Time("03:04:05".into()),
        SqlValue::Boolean(true),
    ];
    let result = store.database_query(QueryRequest { sql: "SELECT $1 AS n, $2 AS d, $3 AS sql_null, $4 AS json_null, $5 AS id, $6 AS tz, $7 AS ts, $8 AS date, $9 AS time, $10 AS b".into(), params: values.clone(), result_schema: ResultSpec::Raw }, DatabaseLimits::default()).await.unwrap();
    assert_eq!(result.rows, vec![values]);
}

#[tokio::test]
async fn column_metadata_survives_empty_rows_and_duplicate_names() {
    let store = store().await;
    let empty = store
        .database_query(
            read("SELECT 1::bigint AS n, 'x'::text AS n WHERE false"),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.columns.len(), 2);
    assert_eq!(empty.columns[0].name, empty.columns[1].name);
    assert_ne!(empty.columns[0].value_type, empty.columns[1].value_type);
    let rows = store
        .database_query(
            read("SELECT 1::bigint AS n, 'x'::text AS n"),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.rows[0],
        vec![SqlValue::Integer("1".into()), SqlValue::Text("x".into())]
    );
}

#[tokio::test]
async fn returning_is_validated_before_commit() {
    let store = store().await;
    let table = table();
    store
        .database_execute(
            command(format!(
                "CREATE TABLE {table} (id bigint PRIMARY KEY, value text)"
            )),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    let mut insert = command(format!(
        "INSERT INTO {table} VALUES (1, 'first') RETURNING id"
    ));
    insert.returning = Some(ResultSpec::Raw);
    let result = store
        .database_execute(insert, DatabaseLimits::default())
        .await
        .unwrap();
    assert_eq!(result.rows_affected, 1);
    assert_eq!(
        result.returned.unwrap().rows[0],
        vec![SqlValue::Integer("1".into())]
    );
    let mut insert = command(format!(
        "INSERT INTO {table} VALUES (2, 'second') RETURNING value"
    ));
    insert.returning = Some(ResultSpec::Columns(vec![ResultColumn {
        name: "value".into(),
        value_type: SqlType::Integer,
        nullable: false,
    }]));
    let error = store
        .database_execute(insert, DatabaseLimits::default())
        .await
        .unwrap_err();
    assert_eq!(error.outcome, Outcome::RolledBack);
    assert_eq!(
        store
            .database_query(
                read(format!("SELECT count(*) FROM {table}")),
                DatabaseLimits::default()
            )
            .await
            .unwrap()
            .rows[0],
        vec![SqlValue::Integer("1".into())]
    );
}

#[tokio::test]
async fn atomic_batch_rolls_back_and_independent_batch_reports_partial_commits() {
    let store = store().await;
    let table = table();
    store
        .database_execute(
            command(format!("CREATE TABLE {table} (id bigint PRIMARY KEY)")),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    let statements = vec![
        command(format!("INSERT INTO {table} VALUES (1)")),
        command(format!("INSERT INTO {table} VALUES (1)")),
        command(format!("INSERT INTO {table} VALUES (2)")),
    ];
    let error = store
        .database_execute_batch(
            BatchRequest {
                mode: BatchMode::Atomic,
                statements: statements.clone(),
            },
            DatabaseLimits::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome, Outcome::RolledBack);
    assert_eq!(error.statement_index, Some(1));
    assert_eq!(
        store
            .database_query(
                read(format!("SELECT count(*) FROM {table}")),
                DatabaseLimits::default()
            )
            .await
            .unwrap()
            .rows[0],
        vec![SqlValue::Integer("0".into())]
    );
    let batch = store
        .database_execute_batch(
            BatchRequest {
                mode: BatchMode::Independent,
                statements,
            },
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert!(batch.results[0].result.is_ok());
    assert_eq!(
        batch.results[1]
            .result
            .as_ref()
            .unwrap_err()
            .sqlstate
            .as_deref(),
        Some("23505")
    );
    assert!(batch.results[2].result.is_ok());
    assert_eq!(
        store
            .database_query(
                read(format!("SELECT count(*) FROM {table}")),
                DatabaseLimits::default()
            )
            .await
            .unwrap()
            .rows[0],
        vec![SqlValue::Integer("2".into())]
    );
}

#[tokio::test]
async fn read_only_enforcement_catches_writes_in_ctes() {
    let store = store().await;
    let table = table();
    store
        .database_execute(
            command(format!("CREATE TABLE {table} (id bigint)")),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    let error = store
        .database_query(
            read(format!(
                "WITH write AS (INSERT INTO {table} VALUES (1) RETURNING id) SELECT * FROM write"
            )),
            DatabaseLimits::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.sqlstate.as_deref(), Some("25006"));
}

#[tokio::test]
async fn deadlines_limits_and_pool_reuse() {
    let store = store().await;
    let deadline = DatabaseLimits {
        statement_timeout: Duration::from_millis(30),
        ..DatabaseLimits::default()
    };
    let error = store
        .database_query(read("SELECT pg_sleep(5)::text"), deadline)
        .await
        .unwrap_err();
    assert_eq!(error.sqlstate.as_deref(), Some("57014"));
    let error = store
        .database_query(
            read("SELECT generate_series(1, 4)"),
            DatabaseLimits {
                max_rows: 3,
                ..DatabaseLimits::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome, Outcome::RolledBack);
    let error = store
        .database_query(
            read("SELECT repeat('x', 10000)"),
            DatabaseLimits {
                max_response_bytes: 1_000,
                ..DatabaseLimits::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome, Outcome::RolledBack);
    assert!(
        store
            .database_query(read("SELECT 1"), DatabaseLimits::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn ddl_is_atomic_with_metadata_writes() {
    let store = store().await;
    let table = table();
    let error = store
        .database_execute_batch(
            BatchRequest {
                mode: BatchMode::Atomic,
                statements: vec![
                    command(format!("CREATE TABLE {table} (id bigint PRIMARY KEY)")),
                    command(format!("INSERT INTO {table} VALUES (1), (1)")),
                ],
            },
            DatabaseLimits::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome, Outcome::RolledBack);
    let result = store
        .database_query(
            QueryRequest {
                sql: "SELECT to_regclass($1)::text AS name".into(),
                params: vec![SqlValue::Text(table)],
                result_schema: ResultSpec::Raw,
            },
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert_eq!(result.rows[0], vec![SqlValue::Null(SqlType::Text)]);
}

#[tokio::test]
async fn high_precision_numeric_is_never_rounded_by_the_driver() {
    let store = store().await;
    let decimal = "1234567890123456789012345678901234567890.123456789012345678901234567890";
    let result = store
        .database_query(
            QueryRequest {
                sql: "SELECT $1 AS decimal".into(),
                params: vec![SqlValue::Decimal(decimal.into())],
                result_schema: ResultSpec::Raw,
            },
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    let SqlValue::Decimal(actual) = &result.rows[0][0] else {
        panic!("expected exact decimal")
    };
    // PostgreSQL's base-10000 numeric representation may add trailing zeros.
    assert_eq!(actual.trim_end_matches('0'), decimal.trim_end_matches('0'));
    let result = store
        .database_query(
            read(format!("SELECT {decimal}::numeric AS decimal")),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    let SqlValue::Decimal(actual) = &result.rows[0][0] else {
        panic!("expected exact decimal")
    };
    // PostgreSQL's base-10000 numeric representation may add trailing zeros.
    assert_eq!(actual.trim_end_matches('0'), decimal.trim_end_matches('0'));
}

#[tokio::test]
async fn batch_row_budget_is_shared_and_checked_before_commit() {
    let store = store().await;
    for mode in [BatchMode::Atomic, BatchMode::Independent] {
        let table = table();
        store
            .database_execute(
                command(format!("CREATE TABLE {table} (id bigint)")),
                DatabaseLimits::default(),
            )
            .await
            .unwrap();
        let statements = (0..3)
            .map(|index| Statement {
                sql: format!(
                    "INSERT INTO {table} VALUES ({}), ({}) RETURNING id",
                    index * 2,
                    index * 2 + 1
                ),
                params: vec![],
                returning: Some(ResultSpec::Raw),
            })
            .collect();
        let result = store
            .database_execute_batch(
                BatchRequest { mode, statements },
                DatabaseLimits {
                    max_rows: 3,
                    ..DatabaseLimits::default()
                },
            )
            .await;
        if mode == BatchMode::Atomic {
            let error = result.unwrap_err();
            assert_eq!(error.outcome, Outcome::RolledBack);
            assert_eq!(error.statement_index, Some(1));
        } else {
            let result = result.unwrap();
            assert!(result.results[0].result.is_ok());
            assert_eq!(
                result.results[1].result.as_ref().unwrap_err().outcome,
                Outcome::RolledBack
            );
            assert_eq!(
                result.results[2].result.as_ref().unwrap_err().outcome,
                Outcome::NotStarted
            );
        }
        let count = store
            .database_query(
                read(format!("SELECT count(*) FROM {table}")),
                DatabaseLimits::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            count.rows[0][0],
            SqlValue::Integer(if mode == BatchMode::Atomic { "0" } else { "2" }.into())
        );
    }
}

#[tokio::test]
async fn independent_batches_continue_after_rejected_sql_and_result_decoding() {
    let store = store().await;
    let table = table();
    store
        .database_execute(
            command(format!("CREATE TABLE {table} (n integer)")),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    let batch = store
        .database_execute_batch(
            BatchRequest {
                mode: BatchMode::Independent,
                statements: vec![
                    command("INVALID SQL"),
                    Statement {
                        sql: format!("INSERT INTO {table} VALUES (1) RETURNING ARRAY[n]"),
                        params: vec![],
                        returning: Some(ResultSpec::Raw),
                    },
                    command(format!("INSERT INTO {table} VALUES (2)")),
                ],
            },
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        batch.results[0].result.as_ref().unwrap_err().outcome,
        Outcome::RolledBack
    );
    assert_eq!(
        batch.results[1].result.as_ref().unwrap_err().outcome,
        Outcome::RolledBack
    );
    assert_eq!(batch.results[2].result.as_ref().unwrap().rows_affected, 1);
    let rows = store
        .database_query(
            read(format!("SELECT n::bigint FROM {table}")),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert_eq!(rows.rows, vec![vec![SqlValue::Integer("2".into())]]);

    let error = store
        .database_execute_batch(
            BatchRequest {
                mode: BatchMode::Atomic,
                statements: vec![
                    command(format!("INSERT INTO {table} VALUES (3)")),
                    command(format!("INSERT INTO {table} VALUES (4)")),
                ],
            },
            DatabaseLimits {
                max_batch_statements: 1,
                ..DatabaseLimits::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.outcome, Outcome::NotStarted);
    let count = store
        .database_query(
            read(format!("SELECT COUNT(*) FROM {table}")),
            DatabaseLimits::default(),
        )
        .await
        .unwrap();
    assert_eq!(count.rows, vec![vec![SqlValue::Integer("1".into())]]);
}
