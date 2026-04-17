# runtara-object-store

Schema-driven dynamic PostgreSQL object store: define schemas at runtime, get managed tables and typed CRUD.

## What it is

A library for managing dynamic object models on top of PostgreSQL. Schemas are stored as rows in a metadata table (default `__schema`), and each schema gets its own dynamically created data table — DDL and DML are generated from `ColumnDefinition`s. The public API centers on `ObjectStore`, with request types for creating/updating schemas and instances, a `SimpleFilter` / `FilterRequest` pair for querying, and typed columns (`String`, `Integer`, `Decimal`, `Boolean`, `Timestamp`, `JSON`, `Enum`). Identifiers are validated and quoted to prevent SQL injection. Multi-tenancy is database-per-tenant — there is no tenant_id column; callers point the store at different databases instead.

## Using it standalone

```rust
use runtara_object_store::{
    ObjectStore, StoreConfig, CreateSchemaRequest,
    ColumnDefinition, ColumnType, SimpleFilter,
};

let config = StoreConfig::builder("postgres://localhost/mydb").build();
let store = ObjectStore::new(config).await?;

store.create_schema(CreateSchemaRequest::new(
    "Products",
    "products",
    vec![
        ColumnDefinition::new("sku", ColumnType::String).unique().not_null(),
        ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
    ],
)).await?;

let id = store.create_instance("Products", serde_json::json!({
    "sku": "WIDGET-001", "price": 29.99,
})).await?;

let (rows, total) = store.query_instances(
    SimpleFilter::new("Products").paginate(0, 10)
).await?;
```

## Inside Runtara

- Consumed only by `runtara-server`: the `api::repositories::object_model` module wraps `ObjectStore` (via `from_pool`) and caches per-tenant instances.
- `api::services::object_model` translates HTTP-facing requests into `CreateSchemaRequest`, `UpdateSchemaRequest`, `SimpleFilter`, and `FilterRequest` calls on the store.
- `api::services::csv_import_export` relies on the store's string-to-integer/decimal coercion when ingesting CSV rows.
- `api::services::schema_validator` reuses `runtara_object_store::sql::validate_identifier` to guard user-supplied schema/column names.
- Runs server-side (native), backed by `sqlx` with `tls-rustls`; tenant isolation is achieved by handing the store a different `PgPool` per tenant database.

## License

AGPL-3.0-or-later.
