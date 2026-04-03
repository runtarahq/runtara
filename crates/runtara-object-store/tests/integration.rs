//! Integration tests for runtara-object-store
//!
//! These tests require a running PostgreSQL database.
//! Set the `TEST_DATABASE_URL` environment variable to run these tests.
//!
//! Example:
//! ```bash
//! TEST_DATABASE_URL="postgres://user:pass@localhost:5432/test_db" cargo test -p runtara-object-store --test integration
//! ```

use runtara_object_store::instance::Condition;
use runtara_object_store::types::{ColumnDefinition, ColumnType, IndexDefinition};
use runtara_object_store::{
    CreateSchemaRequest, FilterRequest, ObjectStore, SimpleFilter, StoreConfig,
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
