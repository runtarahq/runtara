//! Integration tests for runtara-object-store
//!
//! These tests require a running PostgreSQL database.
//! Set the `TEST_DATABASE_URL` environment variable to run these tests.
//!
//! Example:
//! ```bash
//! TEST_DATABASE_URL="postgres://user:pass@localhost:5432/test_db" cargo test -p runtara-object-store --test integration
//! ```

use runtara_object_store::instance::{BulkCreateOptions, Condition, ConflictMode, ValidationMode};
use runtara_object_store::types::{ColumnDefinition, ColumnType, IndexDefinition};
use runtara_object_store::{
    AggregateFn, AggregateOrderBy, AggregateRequest, AggregateSpec, CreateSchemaRequest,
    FilterRequest, ObjectStore, SimpleFilter, SortDirection, StoreConfig,
};

/// Get a unique test prefix for this test run
fn test_prefix() -> String {
    format!(
        "test_{}",
        uuid::Uuid::new_v4().to_string().replace("-", "_")[..8].to_lowercase()
    )
}

/// Get the database URL from environment
fn get_database_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL").ok()
}

/// Create a test store with a unique metadata table
async fn create_test_store() -> Option<(ObjectStore, String)> {
    let db_url = get_database_url()?;
    let prefix = test_prefix();
    let metadata_table = format!("{}__schema", prefix);

    let config = StoreConfig::builder(&db_url)
        .metadata_table(&metadata_table)
        .build();

    let store = ObjectStore::new(config).await.ok()?;
    Some((store, prefix))
}

/// Clean up test tables
async fn cleanup_test(store: &ObjectStore, prefix: &str) {
    // Get all schemas
    if let Ok(schemas) = store.list_schemas().await {
        for schema in schemas {
            // Drop instance tables
            let drop_table = format!("DROP TABLE IF EXISTS \"{}\" CASCADE", schema.table_name);
            let _ = sqlx::query(&drop_table).execute(store.pool()).await;
        }
    }

    // Drop metadata table
    let drop_metadata = format!("DROP TABLE IF EXISTS \"{}__schema\" CASCADE", prefix);
    let _ = sqlx::query(&drop_metadata).execute(store.pool()).await;
}

// ==================== Schema Tests ====================

#[tokio::test]
async fn test_create_schema() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_products", prefix);
    let request = CreateSchemaRequest {
        name: "products".to_string(),
        description: Some("Product catalog".to_string()),
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("sku", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
            ColumnDefinition::new("active", ColumnType::Boolean).default("TRUE"),
        ],
        indexes: Some(vec![IndexDefinition::new(
            "name_idx",
            vec!["name".to_string()],
        )]),
    };

    let schema = store
        .create_schema(request)
        .await
        .expect("Should create schema");

    assert_eq!(schema.name, "products");
    assert_eq!(schema.table_name, table_name);
    assert_eq!(schema.columns.len(), 4);
    assert!(schema.description.is_some());

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_get_schema_by_name() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_items", prefix);
    let request = CreateSchemaRequest {
        name: "items".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![ColumnDefinition::new("name", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Get by name
    let schema = store
        .get_schema("items")
        .await
        .expect("Should not error")
        .expect("Schema should exist");

    assert_eq!(schema.name, "items");

    // Non-existent schema
    let not_found = store
        .get_schema("nonexistent")
        .await
        .expect("Should not error");

    assert!(not_found.is_none());

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_get_schema_by_id() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_widgets", prefix);
    let request = CreateSchemaRequest {
        name: "widgets".to_string(),
        description: None,
        table_name,
        columns: vec![ColumnDefinition::new("code", ColumnType::String)],
        indexes: None,
    };

    let schema = store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Get by ID
    let found = store
        .get_schema_by_id(&schema.id)
        .await
        .expect("Should not error")
        .expect("Schema should exist");

    assert_eq!(found.id, schema.id);
    assert_eq!(found.name, "widgets");

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_list_schemas() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create multiple schemas
    for i in 1..=3 {
        let request = CreateSchemaRequest {
            name: format!("schema_{}", i),
            description: None,
            table_name: format!("{}_{}", prefix, i),
            columns: vec![ColumnDefinition::new("data", ColumnType::Json)],
            indexes: None,
        };
        store
            .create_schema(request)
            .await
            .expect("Should create schema");
    }

    let schemas = store.list_schemas().await.expect("Should list schemas");

    assert_eq!(schemas.len(), 3);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_delete_schema() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let request = CreateSchemaRequest {
        name: "to_delete".to_string(),
        description: None,
        table_name: format!("{}_delete", prefix),
        columns: vec![ColumnDefinition::new("value", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Delete the schema
    store
        .delete_schema("to_delete")
        .await
        .expect("Should delete schema");

    // Should not be found anymore (soft delete by default)
    let found = store
        .get_schema("to_delete")
        .await
        .expect("Should not error");

    assert!(found.is_none());

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_duplicate_schema_name_error() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let request = CreateSchemaRequest {
        name: "unique_name".to_string(),
        description: None,
        table_name: format!("{}_unique1", prefix),
        columns: vec![ColumnDefinition::new("x", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Try to create another with the same name
    let request2 = CreateSchemaRequest {
        name: "unique_name".to_string(), // Same name
        description: None,
        table_name: format!("{}_unique2", prefix), // Different table
        columns: vec![ColumnDefinition::new("y", ColumnType::String)],
        indexes: None,
    };

    let result = store.create_schema(request2).await;
    assert!(result.is_err());

    cleanup_test(&store, &prefix).await;
}

// ==================== Instance Tests ====================

#[tokio::test]
async fn test_create_and_get_instance() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "products".to_string(),
        description: None,
        table_name: format!("{}_products", prefix),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
            ColumnDefinition::new("in_stock", ColumnType::Boolean),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create instance
    let id = store
        .create_instance(
            "products",
            serde_json::json!({
                "name": "Widget",
                "price": 19.99,
                "in_stock": true
            }),
        )
        .await
        .expect("Should create instance");

    // Get instance
    let instance = store
        .get_instance("products", &id)
        .await
        .expect("Should not error")
        .expect("Instance should exist");

    assert_eq!(instance.id, id);
    assert_eq!(instance.properties["name"], "Widget");
    assert_eq!(instance.properties["price"], 19.99);
    assert_eq!(instance.properties["in_stock"], true);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_update_instance() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "items".to_string(),
        description: None,
        table_name: format!("{}_items", prefix),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("count", ColumnType::Integer),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create instance
    let id = store
        .create_instance(
            "items",
            serde_json::json!({
                "name": "Original",
                "count": 10
            }),
        )
        .await
        .expect("Should create instance");

    // Update instance
    store
        .update_instance(
            "items",
            &id,
            serde_json::json!({
                "name": "Updated",
                "count": 20
            }),
        )
        .await
        .expect("Should update instance");

    // Verify update
    let instance = store
        .get_instance("items", &id)
        .await
        .expect("Should not error")
        .expect("Instance should exist");

    assert_eq!(instance.properties["name"], "Updated");
    assert_eq!(instance.properties["count"], 20);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_delete_instance() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "temp".to_string(),
        description: None,
        table_name: format!("{}_temp", prefix),
        columns: vec![ColumnDefinition::new("value", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create instance
    let id = store
        .create_instance("temp", serde_json::json!({"value": "test"}))
        .await
        .expect("Should create instance");

    // Delete instance
    store
        .delete_instance("temp", &id)
        .await
        .expect("Should delete instance");

    // Should not be found
    let found = store
        .get_instance("temp", &id)
        .await
        .expect("Should not error");

    assert!(found.is_none());

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_query_instances_simple() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "products".to_string(),
        description: None,
        table_name: format!("{}_products", prefix),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("category", ColumnType::String),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create multiple instances
    for i in 1..=5 {
        store
            .create_instance(
                "products",
                serde_json::json!({
                    "name": format!("Product {}", i),
                    "category": if i % 2 == 0 { "even" } else { "odd" },
                    "price": i as f64 * 10.0
                }),
            )
            .await
            .expect("Should create instance");
    }

    // Query all
    let filter = SimpleFilter::new("products".to_string());
    let (instances, count) = store
        .query_instances(filter)
        .await
        .expect("Should query instances");

    assert_eq!(count, 5);
    assert_eq!(instances.len(), 5);

    // Query with limit
    let filter = SimpleFilter::new("products".to_string()).with_limit(2);
    let (instances, count) = store
        .query_instances(filter)
        .await
        .expect("Should query instances");

    assert_eq!(count, 5); // Total count still 5
    assert_eq!(instances.len(), 2); // But only 2 returned

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_filter_instances_with_condition() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "users".to_string(),
        description: None,
        table_name: format!("{}_users", prefix),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("age", ColumnType::Integer),
            ColumnDefinition::new("active", ColumnType::Boolean),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create users
    let users = vec![
        ("Alice", 25, true),
        ("Bob", 30, true),
        ("Charlie", 35, false),
        ("Diana", 28, true),
    ];

    for (name, age, active) in users {
        store
            .create_instance(
                "users",
                serde_json::json!({
                    "name": name,
                    "age": age,
                    "active": active
                }),
            )
            .await
            .expect("Should create instance");
    }

    // Filter by active = true
    let condition = Condition {
        op: "EQ".to_string(),
        arguments: Some(vec![serde_json::json!("active"), serde_json::json!(true)]),
    };

    let filter = FilterRequest {
        condition: Some(condition),
        sort_by: None,
        sort_order: None,
        limit: 100,
        offset: 0,
        score_expression: None,
        order_by: None,
    };

    let (instances, count) = store
        .filter_instances("users", filter)
        .await
        .expect("Should filter instances");

    assert_eq!(count, 3); // Alice, Bob, Diana
    assert_eq!(instances.len(), 3);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_instance_exists() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "flags".to_string(),
        description: None,
        table_name: format!("{}_flags", prefix),
        columns: vec![
            ColumnDefinition::new("key", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("enabled", ColumnType::Boolean),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    store
        .create_instance(
            "flags",
            serde_json::json!({
                "key": "feature_x",
                "enabled": true
            }),
        )
        .await
        .expect("Should create instance");

    // Check exists
    let filter = SimpleFilter::new("flags".to_string());
    let exists = store
        .instance_exists(filter)
        .await
        .expect("Should check existence");

    assert!(exists.is_some());

    cleanup_test(&store, &prefix).await;
}

// ==================== Validation Tests ====================

#[tokio::test]
async fn test_type_validation() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema with strict types
    let request = CreateSchemaRequest {
        name: "typed".to_string(),
        description: None,
        table_name: format!("{}_typed", prefix),
        columns: vec![
            ColumnDefinition::new("count", ColumnType::Integer).not_null(),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Valid types
    let result = store
        .create_instance(
            "typed",
            serde_json::json!({
                "count": 42,
                "price": 19.99
            }),
        )
        .await;

    assert!(result.is_ok());

    // Invalid types - string for integer (should fail validation)
    let result = store
        .create_instance(
            "typed",
            serde_json::json!({
                "count": "not a number",
                "price": 9.99
            }),
        )
        .await;

    assert!(result.is_err());

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_required_column_validation() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema with required column
    let request = CreateSchemaRequest {
        name: "required".to_string(),
        description: None,
        table_name: format!("{}_required", prefix),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("optional", ColumnType::String),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Missing required column
    let result = store
        .create_instance(
            "required",
            serde_json::json!({
                "optional": "value"
            }),
        )
        .await;

    assert!(result.is_err());

    cleanup_test(&store, &prefix).await;
}

// ==================== Configuration Tests ====================

#[tokio::test]
async fn test_store_without_soft_delete() {
    let Some(db_url) = get_database_url() else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let prefix = test_prefix();
    let metadata_table = format!("{}__schema", prefix);

    let config = StoreConfig::builder(&db_url)
        .metadata_table(&metadata_table)
        .soft_delete(false) // Hard delete
        .build();

    let store = ObjectStore::new(config).await.expect("Should create store");

    // Create and delete a schema
    let request = CreateSchemaRequest {
        name: "hard_delete_test".to_string(),
        description: None,
        table_name: format!("{}_hard", prefix),
        columns: vec![ColumnDefinition::new("x", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Delete (hard delete)
    store
        .delete_schema("hard_delete_test")
        .await
        .expect("Should hard delete");

    // Table should be dropped - verify by trying to query the metadata directly
    let count: (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM \"{}__schema\" WHERE name = 'hard_delete_test'",
        prefix
    ))
    .fetch_one(store.pool())
    .await
    .expect("Should query");

    assert_eq!(count.0, 0); // Row should be gone, not just soft-deleted

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_custom_metadata_table() {
    let Some(db_url) = get_database_url() else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let prefix = test_prefix();
    let custom_metadata = format!("{}_custom_meta", prefix);

    let config = StoreConfig::builder(&db_url)
        .metadata_table(&custom_metadata)
        .build();

    let store = ObjectStore::new(config).await.expect("Should create store");

    // Verify the custom metadata table exists
    let exists: (bool,) = sqlx::query_as(&format!(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = '{}')",
        custom_metadata
    ))
    .fetch_one(store.pool())
    .await
    .expect("Should query");

    assert!(exists.0);

    // Clean up
    let _ = sqlx::query(&format!(
        "DROP TABLE IF EXISTS \"{}\" CASCADE",
        custom_metadata
    ))
    .execute(store.pool())
    .await;
}

// ==================== Column Type Tests ====================

#[tokio::test]
async fn test_all_column_types() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema with all column types
    let request = CreateSchemaRequest {
        name: "all_types".to_string(),
        description: None,
        table_name: format!("{}_all_types", prefix),
        columns: vec![
            ColumnDefinition::new("string_col", ColumnType::String),
            ColumnDefinition::new("int_col", ColumnType::Integer),
            ColumnDefinition::new("float_col", ColumnType::decimal(10, 2)),
            ColumnDefinition::new("bool_col", ColumnType::Boolean),
            ColumnDefinition::new("json_col", ColumnType::Json),
            ColumnDefinition::new("decimal_col", ColumnType::decimal(10, 2)),
            ColumnDefinition::new("timestamp_col", ColumnType::Timestamp),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create instance with all types
    let id = store
        .create_instance(
            "all_types",
            serde_json::json!({
                "string_col": "hello",
                "int_col": 42,
                "float_col": 9.87,
                "bool_col": true,
                "json_col": {"nested": "value", "arr": [1, 2, 3]},
                "decimal_col": 123.45,
                "timestamp_col": "2024-01-15T10:30:00Z"
            }),
        )
        .await
        .expect("Should create instance");

    // Retrieve and verify
    let instance = store
        .get_instance("all_types", &id)
        .await
        .expect("Should not error")
        .expect("Instance should exist");

    assert_eq!(instance.properties["string_col"], "hello");
    assert_eq!(instance.properties["int_col"], 42);
    assert!((instance.properties["float_col"].as_f64().unwrap() - 9.87).abs() < 0.01);
    assert_eq!(instance.properties["bool_col"], true);
    assert_eq!(instance.properties["json_col"]["nested"], "value");
    assert!((instance.properties["decimal_col"].as_f64().unwrap() - 123.45).abs() < 0.01);

    cleanup_test(&store, &prefix).await;
}

// ==================== Sorting Tests ====================

#[tokio::test]
async fn test_sorting() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "sortable".to_string(),
        description: None,
        table_name: format!("{}_sortable", prefix),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("rank", ColumnType::Integer),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create instances
    for (name, rank) in [("Charlie", 3), ("Alice", 1), ("Bob", 2)] {
        store
            .create_instance(
                "sortable",
                serde_json::json!({
                    "name": name,
                    "rank": rank
                }),
            )
            .await
            .expect("Should create instance");
    }

    // Sort by name ascending
    let filter = FilterRequest {
        condition: None,
        sort_by: Some(vec!["name".to_string()]),
        sort_order: Some(vec!["asc".to_string()]),
        limit: 100,
        offset: 0,
        score_expression: None,
        order_by: None,
    };

    let (instances, _) = store
        .filter_instances("sortable", filter)
        .await
        .expect("Should filter");

    assert_eq!(instances[0].properties["name"], "Alice");
    assert_eq!(instances[1].properties["name"], "Bob");
    assert_eq!(instances[2].properties["name"], "Charlie");

    // Sort by rank descending
    let filter = FilterRequest {
        condition: None,
        sort_by: Some(vec!["rank".to_string()]),
        sort_order: Some(vec!["desc".to_string()]),
        limit: 100,
        offset: 0,
        score_expression: None,
        order_by: None,
    };

    let (instances, _) = store
        .filter_instances("sortable", filter)
        .await
        .expect("Should filter");

    assert_eq!(instances[0].properties["rank"], 3);
    assert_eq!(instances[1].properties["rank"], 2);
    assert_eq!(instances[2].properties["rank"], 1);

    cleanup_test(&store, &prefix).await;
}

// ==================== Pagination Tests ====================

#[tokio::test]
async fn test_pagination() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    // Create schema
    let request = CreateSchemaRequest {
        name: "paginated".to_string(),
        description: None,
        table_name: format!("{}_paginated", prefix),
        columns: vec![ColumnDefinition::new("index", ColumnType::Integer).not_null()],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create 10 instances
    for i in 1..=10 {
        store
            .create_instance("paginated", serde_json::json!({"index": i}))
            .await
            .expect("Should create instance");
    }

    // Page 1 (offset 0, limit 3)
    let filter = FilterRequest {
        condition: None,
        sort_by: Some(vec!["index".to_string()]),
        sort_order: Some(vec!["asc".to_string()]),
        limit: 3,
        offset: 0,
        score_expression: None,
        order_by: None,
    };

    let (instances, total) = store
        .filter_instances("paginated", filter)
        .await
        .expect("Should filter");

    assert_eq!(total, 10);
    assert_eq!(instances.len(), 3);

    // Page 2 (offset 3, limit 3)
    let filter = FilterRequest {
        condition: None,
        sort_by: Some(vec!["index".to_string()]),
        sort_order: Some(vec!["asc".to_string()]),
        limit: 3,
        offset: 3,
        score_expression: None,
        order_by: None,
    };

    let (instances, _) = store
        .filter_instances("paginated", filter)
        .await
        .expect("Should filter");

    assert_eq!(instances.len(), 3);

    cleanup_test(&store, &prefix).await;
}

// ==================== Bulk Operations Tests ====================

#[tokio::test]
async fn test_update_instances_with_condition() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_update", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_update".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("category", ColumnType::String).not_null(),
            ColumnDefinition::new("status", ColumnType::String).not_null(),
            ColumnDefinition::new("count", ColumnType::Integer),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create test instances
    for i in 0..5 {
        let category = if i < 3 { "electronics" } else { "clothing" };
        store
            .create_instance(
                "bulk_update",
                serde_json::json!({
                    "category": category,
                    "status": "active",
                    "count": i
                }),
            )
            .await
            .expect("Should create instance");
    }

    // Update all electronics to archived
    let affected = store
        .update_instances(
            "bulk_update",
            serde_json::json!({"status": "archived"}),
            Condition::eq("category", "electronics"),
        )
        .await
        .expect("Should update instances");

    assert_eq!(affected, 3);

    // Verify the update
    let (instances, _) = store
        .filter_instances(
            "bulk_update",
            FilterRequest::new().with_condition(Condition::eq("status", "archived")),
        )
        .await
        .expect("Should filter");

    assert_eq!(instances.len(), 3);

    // Verify clothing is still active
    let (instances, _) = store
        .filter_instances(
            "bulk_update",
            FilterRequest::new().with_condition(Condition::eq("status", "active")),
        )
        .await
        .expect("Should filter");

    assert_eq!(instances.len(), 2);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_update_instances_no_matches() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_update_empty", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_update_empty".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("status", ColumnType::String),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    store
        .create_instance(
            "bulk_update_empty",
            serde_json::json!({"name": "test", "status": "active"}),
        )
        .await
        .expect("Should create");

    // Update with condition that matches nothing
    let affected = store
        .update_instances(
            "bulk_update_empty",
            serde_json::json!({"status": "archived"}),
            Condition::eq("name", "nonexistent"),
        )
        .await
        .expect("Should succeed with 0 affected");

    assert_eq!(affected, 0);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_delete_instances_soft_delete() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_delete_soft", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_delete_soft".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("category", ColumnType::String).not_null(),
            ColumnDefinition::new("value", ColumnType::Integer),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create test instances
    for i in 0..5 {
        let category = if i < 3 { "to_delete" } else { "keep" };
        store
            .create_instance(
                "bulk_delete_soft",
                serde_json::json!({
                    "category": category,
                    "value": i
                }),
            )
            .await
            .expect("Should create instance");
    }

    // Delete all "to_delete" category
    let affected = store
        .delete_instances("bulk_delete_soft", Condition::eq("category", "to_delete"))
        .await
        .expect("Should delete instances");

    assert_eq!(affected, 3);

    // Verify only "keep" remains visible
    let (instances, total) = store
        .query_instances(SimpleFilter::new("bulk_delete_soft"))
        .await
        .expect("Should query");

    assert_eq!(total, 2);
    assert_eq!(instances.len(), 2);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_delete_instances_hard_delete() {
    let db_url = match get_database_url() {
        Some(url) => url,
        None => {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        }
    };

    let prefix = test_prefix();
    let metadata_table = format!("{}__schema", prefix);

    // Create store with soft_delete disabled
    let config = StoreConfig::builder(&db_url)
        .metadata_table(&metadata_table)
        .soft_delete(false)
        .build();

    let store = ObjectStore::new(config).await.expect("Should create store");

    let table_name = format!("{}_bulk_delete_hard", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_delete_hard".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("tag", ColumnType::String).not_null(),
            ColumnDefinition::new("value", ColumnType::Integer),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create test instances
    for i in 0..4 {
        let tag = if i < 2 { "remove" } else { "stay" };
        store
            .create_instance(
                "bulk_delete_hard",
                serde_json::json!({
                    "tag": tag,
                    "value": i
                }),
            )
            .await
            .expect("Should create instance");
    }

    // Hard delete "remove" tag
    let affected = store
        .delete_instances("bulk_delete_hard", Condition::eq("tag", "remove"))
        .await
        .expect("Should delete instances");

    assert_eq!(affected, 2);

    // Verify only "stay" remains
    let (instances, total) = store
        .query_instances(SimpleFilter::new("bulk_delete_hard"))
        .await
        .expect("Should query");

    assert_eq!(total, 2);
    assert_eq!(instances.len(), 2);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_batch() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_create", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_create".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("index", ColumnType::Integer).not_null(),
            ColumnDefinition::new("active", ColumnType::Boolean),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create multiple instances at once
    let instances: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            serde_json::json!({
                "name": format!("Item {}", i),
                "index": i,
                "active": i % 2 == 0
            })
        })
        .collect();

    let affected = store
        .create_instances("bulk_create", instances)
        .await
        .expect("Should create instances");

    assert_eq!(affected, 10);

    // Verify all were created
    let (results, total) = store
        .query_instances(SimpleFilter::new("bulk_create"))
        .await
        .expect("Should query");

    assert_eq!(total, 10);
    assert_eq!(results.len(), 10);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_validation_rollback() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_create_fail", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_create_fail".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("count", ColumnType::Integer).not_null(),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Try to create instances with one invalid (missing required field)
    let instances = vec![
        serde_json::json!({"name": "Valid 1", "count": 1}),
        serde_json::json!({"name": "Valid 2", "count": 2}),
        serde_json::json!({"name": "Invalid"}), // Missing required "count"
        serde_json::json!({"name": "Valid 3", "count": 3}),
    ];

    let result = store.create_instances("bulk_create_fail", instances).await;

    // Should fail due to validation
    assert!(result.is_err());

    // Verify no instances were created (pre-validation should prevent any insertion)
    let (results, total) = store
        .query_instances(SimpleFilter::new("bulk_create_fail"))
        .await
        .expect("Should query");

    assert_eq!(total, 0);
    assert_eq!(results.len(), 0);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_empty() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_create_empty", prefix);
    let request = CreateSchemaRequest {
        name: "bulk_create_empty".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![ColumnDefinition::new("name", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create with empty array
    let affected = store
        .create_instances("bulk_create_empty", vec![])
        .await
        .expect("Should succeed");

    assert_eq!(affected, 0);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_insert_only() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_insert", prefix);
    let request = CreateSchemaRequest {
        name: "upsert_insert".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("sku", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("name", ColumnType::String).not_null(),
            ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Upsert new instances
    let instances = vec![
        serde_json::json!({"sku": "SKU001", "name": "Product 1", "price": 10.00}),
        serde_json::json!({"sku": "SKU002", "name": "Product 2", "price": 20.00}),
        serde_json::json!({"sku": "SKU003", "name": "Product 3", "price": 30.00}),
    ];

    let affected = store
        .upsert_instances("upsert_insert", instances, vec!["sku".to_string()])
        .await
        .expect("Should upsert");

    assert_eq!(affected, 3);

    // Verify all were created
    let (results, total) = store
        .query_instances(SimpleFilter::new("upsert_insert"))
        .await
        .expect("Should query");

    assert_eq!(total, 3);
    assert_eq!(results.len(), 3);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_update_only() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_update", prefix);
    let request = CreateSchemaRequest {
        name: "upsert_update".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("code", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("value", ColumnType::Integer).not_null(),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create initial instances
    for code in ["A", "B", "C"] {
        store
            .create_instance(
                "upsert_update",
                serde_json::json!({"code": code, "value": 1}),
            )
            .await
            .expect("Should create");
    }

    // Upsert with updated values
    let instances = vec![
        serde_json::json!({"code": "A", "value": 100}),
        serde_json::json!({"code": "B", "value": 200}),
        serde_json::json!({"code": "C", "value": 300}),
    ];

    let affected = store
        .upsert_instances("upsert_update", instances, vec!["code".to_string()])
        .await
        .expect("Should upsert");

    assert_eq!(affected, 3);

    // Verify values were updated
    let (_results, total) = store
        .query_instances(SimpleFilter::new("upsert_update"))
        .await
        .expect("Should query");

    assert_eq!(total, 3); // Still only 3 (no new rows)

    // Check one of the updated values
    let instance = store
        .instance_exists(SimpleFilter::new("upsert_update").filter("code", "A"))
        .await
        .expect("Should find")
        .expect("Should exist");

    assert_eq!(instance.properties["value"], 100);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_mixed() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_mixed", prefix);
    let request = CreateSchemaRequest {
        name: "upsert_mixed".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("key", ColumnType::String)
                .unique()
                .not_null(),
            ColumnDefinition::new("data", ColumnType::String).not_null(),
        ],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create some initial instances
    store
        .create_instance(
            "upsert_mixed",
            serde_json::json!({"key": "existing1", "data": "old1"}),
        )
        .await
        .expect("Should create");
    store
        .create_instance(
            "upsert_mixed",
            serde_json::json!({"key": "existing2", "data": "old2"}),
        )
        .await
        .expect("Should create");

    // Upsert: 2 updates + 2 inserts
    let instances = vec![
        serde_json::json!({"key": "existing1", "data": "updated1"}), // Update
        serde_json::json!({"key": "new1", "data": "new1"}),          // Insert
        serde_json::json!({"key": "existing2", "data": "updated2"}), // Update
        serde_json::json!({"key": "new2", "data": "new2"}),          // Insert
    ];

    let affected = store
        .upsert_instances("upsert_mixed", instances, vec!["key".to_string()])
        .await
        .expect("Should upsert");

    assert_eq!(affected, 4);

    // Verify total count
    let (_, total) = store
        .query_instances(SimpleFilter::new("upsert_mixed"))
        .await
        .expect("Should query");

    assert_eq!(total, 4); // 2 existing + 2 new

    // Verify updates happened
    let instance = store
        .instance_exists(SimpleFilter::new("upsert_mixed").filter("key", "existing1"))
        .await
        .expect("Should find")
        .expect("Should exist");

    assert_eq!(instance.properties["data"], "updated1");

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_multi_column_conflict() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_multi", prefix);
    let request = CreateSchemaRequest {
        name: "upsert_multi".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![
            ColumnDefinition::new("tenant", ColumnType::String).not_null(),
            ColumnDefinition::new("code", ColumnType::String).not_null(),
            ColumnDefinition::new("value", ColumnType::Integer).not_null(),
        ],
        indexes: Some(vec![
            IndexDefinition::new(
                "tenant_code_unique",
                vec!["tenant".to_string(), "code".to_string()],
            )
            .unique(),
        ]),
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    // Create initial instances
    store
        .create_instance(
            "upsert_multi",
            serde_json::json!({"tenant": "A", "code": "X", "value": 1}),
        )
        .await
        .expect("Should create");
    store
        .create_instance(
            "upsert_multi",
            serde_json::json!({"tenant": "A", "code": "Y", "value": 2}),
        )
        .await
        .expect("Should create");

    // Upsert with multi-column conflict
    let instances = vec![
        serde_json::json!({"tenant": "A", "code": "X", "value": 100}), // Update
        serde_json::json!({"tenant": "A", "code": "Z", "value": 3}),   // Insert
        serde_json::json!({"tenant": "B", "code": "X", "value": 4}),   // Insert (different tenant)
    ];

    let affected = store
        .upsert_instances(
            "upsert_multi",
            instances,
            vec!["tenant".to_string(), "code".to_string()],
        )
        .await
        .expect("Should upsert");

    assert_eq!(affected, 3);

    // Verify total count: 2 original + 2 new = 4
    let (_, total) = store
        .query_instances(SimpleFilter::new("upsert_multi"))
        .await
        .expect("Should query");

    assert_eq!(total, 4);

    // Verify the update happened
    let (instances, _) = store
        .filter_instances(
            "upsert_multi",
            FilterRequest::new().with_condition(Condition::and(vec![
                Condition::eq("tenant", "A"),
                Condition::eq("code", "X"),
            ])),
        )
        .await
        .expect("Should filter");

    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].properties["value"], 100);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_empty_conflict_columns() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_empty_conflict", prefix);
    let request = CreateSchemaRequest {
        name: "upsert_empty_conflict".to_string(),
        description: None,
        table_name: table_name.clone(),
        columns: vec![ColumnDefinition::new("name", ColumnType::String)],
        indexes: None,
    };

    store
        .create_schema(request)
        .await
        .expect("Should create schema");

    let instances = vec![serde_json::json!({"name": "test"})];

    // Should fail with empty conflict columns
    let result = store
        .upsert_instances("upsert_empty_conflict", instances, vec![])
        .await;

    assert!(result.is_err());

    cleanup_test(&store, &prefix).await;
}

// ==================== update_instances_by_ids Tests ====================

#[tokio::test]
async fn test_update_instances_by_ids_mixed_values() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_update_by_ids", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "update_by_ids".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String).not_null(),
                ColumnDefinition::new("quantity", ColumnType::Integer).not_null(),
                ColumnDefinition::new("label", ColumnType::String),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    let id_a = store
        .create_instance(
            "update_by_ids",
            serde_json::json!({"sku": "A", "quantity": 1, "label": "orig"}),
        )
        .await
        .expect("create A");
    let id_b = store
        .create_instance(
            "update_by_ids",
            serde_json::json!({"sku": "B", "quantity": 2, "label": "orig"}),
        )
        .await
        .expect("create B");
    let id_c = store
        .create_instance(
            "update_by_ids",
            serde_json::json!({"sku": "C", "quantity": 3, "label": "orig"}),
        )
        .await
        .expect("create C");

    // Per-row update with different values.
    let updates = vec![
        (
            id_a.clone(),
            serde_json::json!({"quantity": 100, "label": "a-new"}),
        ),
        (id_b.clone(), serde_json::json!({"quantity": 200})),
    ];
    let affected = store
        .update_instances_by_ids("update_by_ids", updates)
        .await
        .expect("bulk update");
    assert_eq!(affected, 2);

    let fetched_a = store
        .get_instance("update_by_ids", &id_a)
        .await
        .unwrap()
        .expect("A present");
    assert_eq!(fetched_a.properties["quantity"], serde_json::json!(100));
    assert_eq!(fetched_a.properties["label"], serde_json::json!("a-new"));

    let fetched_b = store
        .get_instance("update_by_ids", &id_b)
        .await
        .unwrap()
        .expect("B present");
    assert_eq!(fetched_b.properties["quantity"], serde_json::json!(200));
    // B's label wasn't touched — should still be the original.
    assert_eq!(fetched_b.properties["label"], serde_json::json!("orig"));

    // C wasn't in the update list — unchanged.
    let fetched_c = store
        .get_instance("update_by_ids", &id_c)
        .await
        .unwrap()
        .expect("C present");
    assert_eq!(fetched_c.properties["quantity"], serde_json::json!(3));

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_update_instances_by_ids_validation_rolls_back() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_update_by_ids_rb", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "update_by_ids_rb".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String).not_null(),
                ColumnDefinition::new("quantity", ColumnType::Integer).not_null(),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    let id = store
        .create_instance(
            "update_by_ids_rb",
            serde_json::json!({"sku": "A", "quantity": 1}),
        )
        .await
        .expect("create");

    // Second row has a type mismatch (quantity as string).
    let updates = vec![
        (id.clone(), serde_json::json!({"quantity": 50})),
        (
            "nonexistent".to_string(),
            serde_json::json!({"quantity": "oops"}),
        ),
    ];

    let result = store
        .update_instances_by_ids("update_by_ids_rb", updates)
        .await;
    assert!(result.is_err(), "Should error on invalid row");

    // Confirm nothing was applied (rollback).
    let fetched = store
        .get_instance("update_by_ids_rb", &id)
        .await
        .unwrap()
        .expect("still present");
    assert_eq!(fetched.properties["quantity"], serde_json::json!(1));

    cleanup_test(&store, &prefix).await;
}

// ==================== create_instances_extended Tests ====================

#[tokio::test]
async fn test_create_instances_extended_skip_conflict() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_skip", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "bulk_skip".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("name", ColumnType::String).not_null(),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // Seed one row so the subsequent bulk will hit a conflict on sku=A.
    store
        .create_instance(
            "bulk_skip",
            serde_json::json!({"sku": "A", "name": "existing"}),
        )
        .await
        .expect("seed");

    let instances = vec![
        serde_json::json!({"sku": "A", "name": "duplicate"}),
        serde_json::json!({"sku": "B", "name": "new"}),
        serde_json::json!({"sku": "C", "name": "new"}),
    ];

    let result = store
        .create_instances_extended(
            "bulk_skip",
            instances,
            BulkCreateOptions {
                conflict_mode: ConflictMode::Skip {
                    conflict_columns: vec!["sku".to_string()],
                },
                validation_mode: ValidationMode::Stop,
            },
        )
        .await
        .expect("bulk insert");

    assert_eq!(result.created_count, 2, "B and C inserted");
    assert_eq!(result.skipped_count, 1, "A skipped by conflict");

    // Existing row's `name` should still be 'existing' (skip, not upsert).
    let (found, _) = store
        .filter_instances(
            "bulk_skip",
            FilterRequest {
                condition: Some(Condition::eq("sku", "A")),
                offset: 0,
                limit: 10,
                sort_by: None,
                sort_order: None,
                score_expression: None,
                order_by: None,
            },
        )
        .await
        .expect("query A");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].properties["name"], serde_json::json!("existing"));

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_extended_upsert_conflict() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_upsert", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "bulk_upsert".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("name", ColumnType::String).not_null(),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    store
        .create_instance(
            "bulk_upsert",
            serde_json::json!({"sku": "A", "name": "old"}),
        )
        .await
        .expect("seed");

    let instances = vec![
        serde_json::json!({"sku": "A", "name": "updated"}),
        serde_json::json!({"sku": "B", "name": "new"}),
    ];

    let result = store
        .create_instances_extended(
            "bulk_upsert",
            instances,
            BulkCreateOptions {
                conflict_mode: ConflictMode::Upsert {
                    conflict_columns: vec!["sku".to_string()],
                },
                validation_mode: ValidationMode::Stop,
            },
        )
        .await
        .expect("bulk upsert");

    // Postgres counts both INSERT and UPDATE as "rows affected" for ON CONFLICT DO UPDATE,
    // so created_count is 2 (one inserted, one updated).
    assert_eq!(result.created_count, 2);

    let (found_a, _) = store
        .filter_instances(
            "bulk_upsert",
            FilterRequest {
                condition: Some(Condition::eq("sku", "A")),
                offset: 0,
                limit: 10,
                sort_by: None,
                sort_order: None,
                score_expression: None,
                order_by: None,
            },
        )
        .await
        .expect("query A");
    assert_eq!(found_a[0].properties["name"], serde_json::json!("updated"));

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_extended_skip_validation() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_skip_invalid", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "bulk_skip_invalid".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String).not_null(),
                ColumnDefinition::new("quantity", ColumnType::Integer).not_null(),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    let instances = vec![
        serde_json::json!({"sku": "ok-1", "quantity": 1}),
        serde_json::json!({"sku": "bad"}), // missing quantity
        serde_json::json!({"sku": "ok-2", "quantity": 2}),
    ];

    let result = store
        .create_instances_extended(
            "bulk_skip_invalid",
            instances,
            BulkCreateOptions {
                conflict_mode: ConflictMode::Error,
                validation_mode: ValidationMode::Skip,
            },
        )
        .await
        .expect("bulk with skip");

    assert_eq!(result.created_count, 2);
    assert_eq!(result.skipped_count, 1);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].index, 1);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_extended_requires_conflict_columns() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_missing_cols", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "bulk_missing_cols".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![ColumnDefinition::new("sku", ColumnType::String).not_null()],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    let result = store
        .create_instances_extended(
            "bulk_missing_cols",
            vec![serde_json::json!({"sku": "x"})],
            BulkCreateOptions {
                conflict_mode: ConflictMode::Skip {
                    conflict_columns: vec![],
                },
                validation_mode: ValidationMode::Stop,
            },
        )
        .await;
    assert!(
        result.is_err(),
        "skip mode with no conflict columns should error"
    );

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_extended_partial_columns_typed_null() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_bulk_partial", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "bulk_partial".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("category_leaf_id", ColumnType::Integer),
                ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
                ColumnDefinition::new("active", ColumnType::Boolean),
                ColumnDefinition::new("released_at", ColumnType::Timestamp),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // Instance with only `sku` populated — mimics the columnar normalizer
    // output when `columns = ["sku"]`. Every other schema column is absent
    // from the map. Regression test for the bug where missing columns were
    // bound as `None::<String>`, causing Postgres to reject non-text columns
    // with "expression is of type text".
    let result = store
        .create_instances_extended(
            "bulk_partial",
            vec![serde_json::json!({"sku": "A1"})],
            BulkCreateOptions {
                conflict_mode: ConflictMode::Skip {
                    conflict_columns: vec!["sku".to_string()],
                },
                validation_mode: ValidationMode::Stop,
            },
        )
        .await
        .expect("partial-column insert should succeed with typed NULLs");

    assert_eq!(result.created_count, 1);

    let (found, _) = store
        .filter_instances(
            "bulk_partial",
            FilterRequest {
                condition: Some(Condition::eq("sku", "A1")),
                offset: 0,
                limit: 10,
                sort_by: None,
                sort_order: None,
                score_expression: None,
                order_by: None,
            },
        )
        .await
        .expect("query inserted row");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].properties["sku"], serde_json::json!("A1"));
    // NULL columns are absent from properties (see `extract_column_value`).
    let props = found[0].properties.as_object().expect("object properties");
    assert!(!props.contains_key("category_leaf_id"));
    assert!(!props.contains_key("price"));
    assert!(!props.contains_key("active"));
    assert!(!props.contains_key("released_at"));

    cleanup_test(&store, &prefix).await;
}

// ==================== Bulk-insert: JSONB null vs SQL NULL ====================
//
// Regression coverage for the bug where a bulk-insert payload that omits a
// `ColumnType::Json` column wrote JSONB `null` (a real JSONB value) rather
// than SQL NULL, making `WHERE col IS NULL` miss those rows. An explicit
// `{"col": null}` should still write JSONB `null`.

async fn setup_json_null_schema(store: &ObjectStore, schema_name: &str, table_name: &str) {
    store
        .create_schema(CreateSchemaRequest {
            name: schema_name.to_string(),
            description: None,
            table_name: table_name.to_string(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("attrs", ColumnType::Json),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");
}

async fn assert_json_null_semantics(store: &ObjectStore, table_name: &str) {
    let absent_is_null: i64 = sqlx::query_scalar(&format!(
        r#"SELECT COUNT(*) FROM "{}" WHERE attrs IS NULL"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("count IS NULL");
    assert_eq!(
        absent_is_null, 1,
        "absent JSON column should be stored as SQL NULL"
    );

    let jsonb_null: i64 = sqlx::query_scalar(&format!(
        r#"SELECT COUNT(*) FROM "{}" WHERE attrs IS NOT NULL AND jsonb_typeof(attrs) = 'null'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("count JSONB null");
    assert_eq!(
        jsonb_null, 1,
        "explicit null on JSON column should stay JSONB `null`"
    );

    let object_match: i64 = sqlx::query_scalar(&format!(
        r#"SELECT COUNT(*) FROM "{}" WHERE attrs @> '{{"x":1}}'::jsonb"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("count object");
    assert_eq!(object_match, 1, "object JSON value should be preserved");
}

#[tokio::test]
async fn test_create_instances_json_absent_vs_explicit_null() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_json_null", prefix);
    setup_json_null_schema(&store, "json_null", &table_name).await;

    let n = store
        .create_instances(
            "json_null",
            vec![
                serde_json::json!({"sku": "a"}),
                serde_json::json!({"sku": "b", "attrs": null}),
                serde_json::json!({"sku": "c", "attrs": {"x": 1}}),
            ],
        )
        .await
        .expect("bulk insert");
    assert_eq!(n, 3);

    assert_json_null_semantics(&store, &table_name).await;
    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_json_absent_vs_explicit_null() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_json_null_up", prefix);
    setup_json_null_schema(&store, "json_null_up", &table_name).await;

    let n = store
        .upsert_instances(
            "json_null_up",
            vec![
                serde_json::json!({"sku": "a"}),
                serde_json::json!({"sku": "b", "attrs": null}),
                serde_json::json!({"sku": "c", "attrs": {"x": 1}}),
            ],
            vec!["sku".to_string()],
        )
        .await
        .expect("bulk upsert");
    assert_eq!(n, 3);

    assert_json_null_semantics(&store, &table_name).await;
    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_extended_json_absent_vs_explicit_null() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_json_null_ext", prefix);
    setup_json_null_schema(&store, "json_null_ext", &table_name).await;

    let result = store
        .create_instances_extended(
            "json_null_ext",
            vec![
                serde_json::json!({"sku": "a"}),
                serde_json::json!({"sku": "b", "attrs": null}),
                serde_json::json!({"sku": "c", "attrs": {"x": 1}}),
            ],
            BulkCreateOptions {
                conflict_mode: ConflictMode::Error,
                validation_mode: ValidationMode::Stop,
            },
        )
        .await
        .expect("bulk extended insert");
    assert_eq!(result.created_count, 3);

    assert_json_null_semantics(&store, &table_name).await;
    cleanup_test(&store, &prefix).await;
}

// ==================== Bulk-insert: DB DEFAULT firing ====================
//
// Regression coverage for the bug where an absent column silently bound SQL
// NULL, bypassing the column's declared `DEFAULT` clause. For NOT NULL +
// DEFAULT columns this previously tripped the constraint after validation
// passed; for nullable columns it silently erased the default.

#[tokio::test]
async fn test_create_instances_fires_db_default_when_column_absent() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_default_ts", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "default_ts".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("snapshot_at", ColumnType::Timestamp)
                    .not_null()
                    .default("now()"),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // Mixed batch in a single call exercises per-row DEFAULT/$N placeholder
    // interleaving in one INSERT (the risky code path).
    let before = chrono::Utc::now();
    let n = store
        .create_instances(
            "default_ts",
            vec![
                serde_json::json!({"sku": "x"}),
                serde_json::json!({"sku": "y", "snapshot_at": "2020-01-01T00:00:00Z"}),
                serde_json::json!({"sku": "z"}),
            ],
        )
        .await
        .expect("bulk insert should succeed with DEFAULT firing");
    assert_eq!(n, 3);
    let after = chrono::Utc::now();
    let slack = chrono::Duration::seconds(5);

    for sku in ["x", "z"] {
        let ts: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&format!(
            r#"SELECT snapshot_at FROM "{}" WHERE sku = '{}'"#,
            table_name, sku
        ))
        .fetch_one(store.pool())
        .await
        .unwrap_or_else(|e| panic!("read {}: {}", sku, e));
        assert!(
            ts >= before - slack && ts <= after + slack,
            "absent DEFAULT-column for {} should be near now(); got {}",
            sku,
            ts
        );
    }

    let y_ts: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&format!(
        r#"SELECT snapshot_at FROM "{}" WHERE sku = 'y'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read y");
    assert_eq!(
        y_ts,
        chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        "explicit value should win over DEFAULT"
    );

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_default_on_insert_and_update() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_default_up", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "default_up".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("updated_count", ColumnType::Integer)
                    .not_null()
                    .default("0"),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    store
        .upsert_instances(
            "default_up",
            vec![serde_json::json!({"sku": "x"})],
            vec!["sku".to_string()],
        )
        .await
        .expect("first upsert (INSERT path, DEFAULT fires)");

    let initial: i64 = sqlx::query_scalar(&format!(
        r#"SELECT updated_count FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read initial");
    assert_eq!(initial, 0, "absent column should use DB default on INSERT");

    store
        .upsert_instances(
            "default_up",
            vec![serde_json::json!({"sku": "x", "updated_count": 5})],
            vec!["sku".to_string()],
        )
        .await
        .expect("second upsert (UPDATE path via ON CONFLICT)");

    let updated: i64 = sqlx::query_scalar(&format!(
        r#"SELECT updated_count FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read updated");
    assert_eq!(updated, 5, "explicit value should overwrite via UPDATE");

    cleanup_test(&store, &prefix).await;
}

// ==================== Upsert: absent columns untouched on UPDATE ====================
//
// Regression coverage for the pre-existing issue where the upsert path's
// `DO UPDATE SET col = EXCLUDED.col` clause listed every non-conflict schema
// column, so columns absent from the payload were overwritten on conflict —
// scalar absents became NULL, and defaulted absents were re-fired to the
// DEFAULT expression's value on every upsert. The fix groups rows by which
// columns they actually specify, so DO UPDATE SET only touches present ones.

#[tokio::test]
async fn test_upsert_instances_leaves_absent_columns_untouched() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_preserve", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "upsert_preserve".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                // Nullable: Postgres checks NOT NULL on the proposed INSERT row
                // before ON CONFLICT resolution, so a partial-upsert test has to
                // omit only nullable (or defaulted) columns.
                ColumnDefinition::new("name", ColumnType::String),
                ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // Seed a row with all columns populated.
    store
        .upsert_instances(
            "upsert_preserve",
            vec![serde_json::json!({"sku": "x", "name": "original", "price": 10.0})],
            vec!["sku".to_string()],
        )
        .await
        .expect("seed upsert");

    // Upsert again with only `price` present. `name` must be left alone.
    store
        .upsert_instances(
            "upsert_preserve",
            vec![serde_json::json!({"sku": "x", "price": 20.0})],
            vec!["sku".to_string()],
        )
        .await
        .expect("partial upsert");

    let name: String = sqlx::query_scalar(&format!(
        r#"SELECT name FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read name");
    assert_eq!(name, "original", "absent `name` must not be stomped");

    let price: f64 = sqlx::query_scalar(&format!(
        r#"SELECT price::float8 FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read price");
    assert_eq!(price, 20.0, "present `price` must be updated");

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_does_not_restamp_defaulted_absent_column() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_default_stable", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "upsert_default_stable".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("stamped_at", ColumnType::Timestamp)
                    .not_null()
                    .default("now()"),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // First upsert: DEFAULT now() fires on INSERT.
    store
        .upsert_instances(
            "upsert_default_stable",
            vec![serde_json::json!({"sku": "x"})],
            vec!["sku".to_string()],
        )
        .await
        .expect("first upsert");

    let t1: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&format!(
        r#"SELECT stamped_at FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read t1");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Second upsert with `created_at` absent: must NOT stomp to a new now().
    store
        .upsert_instances(
            "upsert_default_stable",
            vec![serde_json::json!({"sku": "x"})],
            vec!["sku".to_string()],
        )
        .await
        .expect("second upsert");

    let t2: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&format!(
        r#"SELECT stamped_at FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read t2");

    assert_eq!(
        t1, t2,
        "absent DEFAULTed column must keep its original value across upserts"
    );

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_upsert_instances_mixed_signatures_single_batch() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_upsert_multi_sig", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "upsert_multi_sig".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("a", ColumnType::String),
                ColumnDefinition::new("b", ColumnType::String),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // Seed two rows with both `a` and `b`.
    store
        .upsert_instances(
            "upsert_multi_sig",
            vec![
                serde_json::json!({"sku": "X", "a": "A1", "b": "B1"}),
                serde_json::json!({"sku": "Y", "a": "A2", "b": "B2"}),
            ],
            vec!["sku".to_string()],
        )
        .await
        .expect("seed upsert");

    // Same batch, different presence signatures per row:
    // X updates only `a`; Y updates only `b`. The other column on each row
    // must stay untouched.
    store
        .upsert_instances(
            "upsert_multi_sig",
            vec![
                serde_json::json!({"sku": "X", "a": "A1_new"}),
                serde_json::json!({"sku": "Y", "b": "B2_new"}),
            ],
            vec!["sku".to_string()],
        )
        .await
        .expect("mixed-signature upsert");

    let (x_a, x_b): (String, String) = sqlx::query_as(&format!(
        r#"SELECT a, b FROM "{}" WHERE sku = 'X'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read X");
    assert_eq!(x_a, "A1_new", "X.a updated");
    assert_eq!(x_b, "B1", "X.b untouched");

    let (y_a, y_b): (String, String) = sqlx::query_as(&format!(
        r#"SELECT a, b FROM "{}" WHERE sku = 'Y'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read Y");
    assert_eq!(y_a, "A2", "Y.a untouched");
    assert_eq!(y_b, "B2_new", "Y.b updated");

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_create_instances_extended_upsert_preserves_absent_columns() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let table_name = format!("{}_ext_upsert_preserve", prefix);
    store
        .create_schema(CreateSchemaRequest {
            name: "ext_upsert_preserve".to_string(),
            description: None,
            table_name: table_name.clone(),
            columns: vec![
                ColumnDefinition::new("sku", ColumnType::String)
                    .unique()
                    .not_null(),
                ColumnDefinition::new("name", ColumnType::String),
                ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
            ],
            indexes: None,
        })
        .await
        .expect("Should create schema");

    // Seed.
    store
        .create_instances_extended(
            "ext_upsert_preserve",
            vec![serde_json::json!({"sku": "x", "name": "original", "price": 10.0})],
            BulkCreateOptions {
                conflict_mode: ConflictMode::Upsert {
                    conflict_columns: vec!["sku".to_string()],
                },
                validation_mode: ValidationMode::Stop,
            },
        )
        .await
        .expect("seed");

    // Partial upsert: `name` absent.
    store
        .create_instances_extended(
            "ext_upsert_preserve",
            vec![serde_json::json!({"sku": "x", "price": 20.0})],
            BulkCreateOptions {
                conflict_mode: ConflictMode::Upsert {
                    conflict_columns: vec!["sku".to_string()],
                },
                validation_mode: ValidationMode::Stop,
            },
        )
        .await
        .expect("partial upsert");

    let (name, price): (String, f64) = sqlx::query_as(&format!(
        r#"SELECT name, price::float8 FROM "{}" WHERE sku = 'x'"#,
        table_name
    ))
    .fetch_one(store.pool())
    .await
    .expect("read");
    assert_eq!(name, "original", "absent `name` must not be stomped");
    assert_eq!(price, 20.0, "present `price` must be updated");

    cleanup_test(&store, &prefix).await;
}

// ==================== Aggregate Tests ====================

/// Seed a `stock_snapshot`-style schema with three SKUs × three dates. Returns
/// the canonical (first_qty, last_qty) expected per SKU when grouped by `sku`
/// and ordered by `snapshot_date ASC`.
async fn seed_stock_snapshot(
    store: &ObjectStore,
    prefix: &str,
    schema_name: &str,
) -> std::collections::HashMap<&'static str, (i64, i64)> {
    let request = CreateSchemaRequest {
        name: schema_name.to_string(),
        description: None,
        table_name: format!("{}_{}", prefix, schema_name),
        columns: vec![
            ColumnDefinition::new("sku", ColumnType::String).not_null(),
            ColumnDefinition::new("qty", ColumnType::Integer),
            ColumnDefinition::new("snapshot_date", ColumnType::Timestamp).not_null(),
        ],
        indexes: None,
    };
    store.create_schema(request).await.expect("create_schema");

    // (sku, qty, date)
    let rows = vec![
        ("A", 10, "2026-04-01T00:00:00Z"),
        ("A", 15, "2026-04-02T00:00:00Z"),
        ("A", 0, "2026-04-03T00:00:00Z"),
        ("B", 5, "2026-04-01T00:00:00Z"),
        ("B", 7, "2026-04-02T00:00:00Z"),
        ("B", 9, "2026-04-03T00:00:00Z"),
        ("C", 100, "2026-04-01T00:00:00Z"),
        ("C", 100, "2026-04-02T00:00:00Z"),
        ("C", 50, "2026-04-03T00:00:00Z"),
    ];
    for (sku, qty, date) in rows {
        store
            .create_instance(
                schema_name,
                serde_json::json!({
                    "sku": sku,
                    "qty": qty,
                    "snapshot_date": date,
                }),
            )
            .await
            .expect("create_instance");
    }

    std::collections::HashMap::from([("A", (10i64, 0i64)), ("B", (5, 9)), ("C", (100, 50))])
}

#[tokio::test]
async fn test_aggregate_first_last_value_per_group() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    let expected = seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let req = AggregateRequest {
        condition: None,
        group_by: vec!["sku".into()],
        aggregates: vec![
            AggregateSpec {
                alias: "first_qty".into(),
                fn_: AggregateFn::FirstValue,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![AggregateOrderBy {
                    column: "snapshot_date".into(),
                    direction: SortDirection::Asc,
                }],
                expression: None,
            },
            AggregateSpec {
                alias: "last_qty".into(),
                fn_: AggregateFn::LastValue,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![AggregateOrderBy {
                    column: "snapshot_date".into(),
                    direction: SortDirection::Asc,
                }],
                expression: None,
            },
        ],
        order_by: vec![AggregateOrderBy {
            column: "last_qty".into(),
            direction: SortDirection::Desc,
        }],
        limit: Some(200),
        offset: Some(0),
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    assert_eq!(result.columns, vec!["sku", "first_qty", "last_qty"]);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(result.group_count, 3);

    // Build a map for order-independent comparison of per-SKU values.
    let mut got: std::collections::HashMap<String, (i64, i64)> = Default::default();
    for row in &result.rows {
        let sku = row[0].as_str().expect("sku str").to_string();
        let first = row[1].as_i64().expect("first_qty i64");
        let last = row[2].as_i64().expect("last_qty i64");
        got.insert(sku, (first, last));
    }
    for (sku, (first, last)) in &expected {
        let actual = got
            .get(*sku)
            .unwrap_or_else(|| panic!("missing sku {}", sku));
        assert_eq!(actual.0, *first, "first_qty[{}]", sku);
        assert_eq!(actual.1, *last, "last_qty[{}]", sku);
    }

    // Top-level ORDER BY last_qty DESC ⇒ B(9) is ahead of A(0); C(50) is the
    // largest, so it must come first.
    let skus_in_order: Vec<&str> = result
        .rows
        .iter()
        .map(|r| r[0].as_str().expect("sku"))
        .collect();
    assert_eq!(skus_in_order, vec!["C", "B", "A"]);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_aggregate_count_sum_grouped() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let req = AggregateRequest {
        condition: None,
        group_by: vec!["sku".into()],
        aggregates: vec![
            AggregateSpec {
                alias: "n".into(),
                fn_: AggregateFn::Count,
                column: None,
                distinct: false,
                order_by: vec![],
                expression: None,
            },
            AggregateSpec {
                alias: "total_qty".into(),
                fn_: AggregateFn::Sum,
                column: Some("qty".into()),
                distinct: false,
                order_by: vec![],
                expression: None,
            },
        ],
        order_by: vec![AggregateOrderBy {
            column: "total_qty".into(),
            direction: SortDirection::Desc,
        }],
        limit: None,
        offset: None,
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    assert_eq!(result.columns, vec!["sku", "n", "total_qty"]);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(result.group_count, 3);

    // Each SKU has 3 rows → COUNT(*) = 3 everywhere.
    for row in &result.rows {
        let n = row[1].as_i64().expect("n is i64");
        assert_eq!(n, 3);
    }

    // Top row has highest SUM(qty). C: 100+100+50 = 250 is the largest.
    let top_sku = result.rows[0][0].as_str().expect("sku");
    assert_eq!(top_sku, "C");
    let top_total: f64 = result.rows[0][2]
        .as_f64()
        .or_else(|| result.rows[0][2].as_str().and_then(|s| s.parse().ok()))
        .expect("total_qty numeric");
    assert!((top_total - 250.0).abs() < 1e-9);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_aggregate_count_star_no_group_by() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let req = AggregateRequest {
        condition: None,
        group_by: vec![],
        aggregates: vec![AggregateSpec {
            alias: "n".into(),
            fn_: AggregateFn::Count,
            column: None,
            distinct: false,
            order_by: vec![],
            expression: None,
        }],
        order_by: vec![],
        limit: None,
        offset: None,
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    assert_eq!(result.columns, vec!["n"]);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.group_count, 1, "no group_by → group_count == 1");
    assert_eq!(result.rows[0][0].as_i64().unwrap(), 9);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_aggregate_with_condition_filter() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    // Only SKU "C" matches this condition — 3 rows, all with qty ≥ 50.
    let req = AggregateRequest {
        condition: Some(Condition {
            op: "EQ".into(),
            arguments: Some(vec![serde_json::json!("sku"), serde_json::json!("C")]),
        }),
        group_by: vec!["sku".into()],
        aggregates: vec![AggregateSpec {
            alias: "total".into(),
            fn_: AggregateFn::Sum,
            column: Some("qty".into()),
            distinct: false,
            order_by: vec![],
            expression: None,
        }],
        order_by: vec![],
        limit: None,
        offset: None,
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.group_count, 1);
    let total: f64 = result.rows[0][1]
        .as_f64()
        .or_else(|| result.rows[0][1].as_str().and_then(|s| s.parse().ok()))
        .expect("total numeric");
    assert!((total - 250.0).abs() < 1e-9);

    cleanup_test(&store, &prefix).await;
}

#[tokio::test]
async fn test_aggregate_unknown_column_rejected_at_store() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let req = AggregateRequest {
        condition: None,
        group_by: vec!["does_not_exist".into()],
        aggregates: vec![AggregateSpec {
            alias: "n".into(),
            fn_: AggregateFn::Count,
            column: None,
            distinct: false,
            order_by: vec![],
            expression: None,
        }],
        order_by: vec![],
        limit: None,
        offset: None,
    };

    let err = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect_err("should reject unknown group_by column");
    assert!(
        err.to_string().contains("Unknown group_by column"),
        "unexpected error: {}",
        err
    );

    cleanup_test(&store, &prefix).await;
}

// ==================== EXPR aggregate (v1.1) integration tests ====================

fn row_cell_as_f64(row: &[serde_json::Value], idx: usize) -> f64 {
    row[idx]
        .as_f64()
        .or_else(|| row[idx].as_str().and_then(|s| s.parse().ok()))
        .expect("numeric cell")
}

fn first_last_qty_agg_specs() -> Vec<AggregateSpec> {
    vec![
        AggregateSpec {
            alias: "first_qty".into(),
            fn_: AggregateFn::FirstValue,
            column: Some("qty".into()),
            distinct: false,
            order_by: vec![AggregateOrderBy {
                column: "snapshot_date".into(),
                direction: SortDirection::Asc,
            }],
            expression: None,
        },
        AggregateSpec {
            alias: "last_qty".into(),
            fn_: AggregateFn::LastValue,
            column: Some("qty".into()),
            distinct: false,
            order_by: vec![AggregateOrderBy {
                column: "snapshot_date".into(),
                direction: SortDirection::Asc,
            }],
            expression: None,
        },
    ]
}

fn make_expr_spec(alias: &str, expression: serde_json::Value) -> AggregateSpec {
    AggregateSpec {
        alias: alias.into(),
        fn_: AggregateFn::Expr,
        column: None,
        distinct: false,
        order_by: vec![],
        expression: Some(expression),
    }
}

/// Market-research example: delta = last_qty - first_qty, delta_abs = ABS(delta),
/// ordered by delta_abs DESC. With seed data A: 10→0, B: 5→9, C: 100→50
/// we expect delta_abs order: C(50), A(10), B(4).
#[tokio::test]
async fn test_aggregate_expr_delta_end_to_end() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let mut aggregates = first_last_qty_agg_specs();
    aggregates.push(make_expr_spec(
        "delta",
        serde_json::json!({
            "op": "SUB",
            "arguments": [
                {"valueType": "alias", "value": "last_qty"},
                {"valueType": "alias", "value": "first_qty"}
            ]
        }),
    ));
    aggregates.push(make_expr_spec(
        "delta_abs",
        serde_json::json!({
            "op": "ABS",
            "arguments": [
                {"valueType": "alias", "value": "delta"}
            ]
        }),
    ));

    let req = AggregateRequest {
        condition: None,
        group_by: vec!["sku".into()],
        aggregates,
        order_by: vec![AggregateOrderBy {
            column: "delta_abs".into(),
            direction: SortDirection::Desc,
        }],
        limit: Some(200),
        offset: Some(0),
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    assert_eq!(
        result.columns,
        vec![
            "sku".to_string(),
            "first_qty".to_string(),
            "last_qty".to_string(),
            "delta".to_string(),
            "delta_abs".to_string(),
        ]
    );
    assert_eq!(result.rows.len(), 3);

    let skus: Vec<&str> = result
        .rows
        .iter()
        .map(|r| r[0].as_str().expect("sku"))
        .collect();
    assert_eq!(skus, vec!["C", "A", "B"]);

    // Check the computed deltas.
    for row in &result.rows {
        let sku = row[0].as_str().unwrap();
        let delta = row_cell_as_f64(row, 3);
        let delta_abs = row_cell_as_f64(row, 4);
        assert_eq!(delta_abs, delta.abs(), "sku {}", sku);
        match sku {
            "A" => assert!((delta + 10.0).abs() < 1e-9),
            "B" => assert!((delta - 4.0).abs() < 1e-9),
            "C" => assert!((delta + 50.0).abs() < 1e-9),
            _ => panic!("unexpected sku {}", sku),
        }
    }

    cleanup_test(&store, &prefix).await;
}

/// `DIV` returns NULL on divide-by-zero. A's first_qty=10 last_qty=0, so
/// `last / first` is valid for A (=0) but `first / last` divides by zero.
#[tokio::test]
async fn test_aggregate_expr_div_by_zero_returns_null() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let mut aggregates = first_last_qty_agg_specs();
    // For A: first=10, last=0 → first/last divides by zero → NULL.
    // For B: first=5, last=9 → 5/9 ≈ 0.5555…
    // For C: first=100, last=50 → 2.0
    aggregates.push(make_expr_spec(
        "ratio",
        serde_json::json!({
            "op": "DIV",
            "arguments": [
                {"valueType": "alias", "value": "first_qty"},
                {"valueType": "alias", "value": "last_qty"}
            ]
        }),
    ));

    let req = AggregateRequest {
        condition: None,
        group_by: vec!["sku".into()],
        aggregates,
        order_by: vec![],
        limit: None,
        offset: None,
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    for row in &result.rows {
        let sku = row[0].as_str().unwrap();
        let ratio = &row[3];
        match sku {
            "A" => assert!(ratio.is_null(), "expected NULL for A, got {:?}", ratio),
            "B" => {
                let v = row_cell_as_f64(row, 3);
                assert!((v - (5.0 / 9.0)).abs() < 1e-6, "B ratio = {}", v);
            }
            "C" => {
                let v = row_cell_as_f64(row, 3);
                assert!((v - 2.0).abs() < 1e-9, "C ratio = {}", v);
            }
            _ => panic!("unexpected sku {}", sku),
        }
    }

    cleanup_test(&store, &prefix).await;
}

/// Boolean EXPR column should surface as JSON `true`/`false`.
#[tokio::test]
async fn test_aggregate_expr_boolean_column() {
    let Some((store, prefix)) = create_test_store().await else {
        eprintln!("Skipping test: TEST_DATABASE_URL not set");
        return;
    };

    seed_stock_snapshot(&store, &prefix, "stock_snapshot").await;

    let mut aggregates = first_last_qty_agg_specs();
    aggregates.push(make_expr_spec(
        "is_big",
        serde_json::json!({
            "op": "GT",
            "arguments": [
                {"valueType": "alias", "value": "last_qty"},
                {"valueType": "immediate", "value": 10}
            ]
        }),
    ));

    let req = AggregateRequest {
        condition: None,
        group_by: vec!["sku".into()],
        aggregates,
        order_by: vec![],
        limit: None,
        offset: None,
    };

    let result = store
        .aggregate_instances("stock_snapshot", req)
        .await
        .expect("aggregate_instances");

    for row in &result.rows {
        let sku = row[0].as_str().unwrap();
        let is_big = row[3]
            .as_bool()
            .unwrap_or_else(|| panic!("expected JSON bool for is_big, got {:?}", row[3]));
        match sku {
            "A" => assert!(!is_big, "A last_qty=0 → not > 10"),
            "B" => assert!(!is_big, "B last_qty=9 → not > 10"),
            "C" => assert!(is_big, "C last_qty=50 → > 10"),
            _ => panic!("unexpected sku {}", sku),
        }
    }

    cleanup_test(&store, &prefix).await;
}
