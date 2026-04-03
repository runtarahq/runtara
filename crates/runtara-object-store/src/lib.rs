//! # runtara-object-store
//!
//! A schema-driven dynamic PostgreSQL object store.
//!
//! This crate provides a flexible way to manage dynamic schemas and their instances
//! in a PostgreSQL database. Schemas are stored as metadata and the corresponding
//! data tables are created and managed automatically.
//!
//! ## Features
//!
//! - **Dynamic Schema Management**: Create, update, and delete schemas at runtime
//! - **Type-Safe Column Definitions**: Support for String, Integer, Decimal, Boolean, Timestamp, JSON, and Enum types
//! - **Automatic Columns**: Configurable auto-managed columns (id, created_at, updated_at)
//! - **Soft Delete**: Optional soft delete support with `deleted` flag
//! - **Flexible Querying**: Condition-based filtering with AND/OR/NOT operators
//! - **SQL Injection Prevention**: All identifiers are properly quoted and validated
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use runtara_object_store::{ObjectStore, StoreConfig, CreateSchemaRequest, ColumnDefinition, ColumnType};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create store with default configuration
//!     let config = StoreConfig::builder("postgres://localhost/mydb").build();
//!     let store = ObjectStore::new(config).await?;
//!
//!     // Create a schema
//!     let schema = store.create_schema(
//!         CreateSchemaRequest::new(
//!             "Products",
//!             "products",
//!             vec![
//!                 ColumnDefinition::new("sku", ColumnType::String).unique().not_null(),
//!                 ColumnDefinition::new("name", ColumnType::String).not_null(),
//!                 ColumnDefinition::new("price", ColumnType::decimal(10, 2)),
//!                 ColumnDefinition::new("in_stock", ColumnType::Boolean).default("true"),
//!             ],
//!         )
//!     ).await?;
//!
//!     // Create an instance
//!     let id = store.create_instance(
//!         "Products",
//!         serde_json::json!({
//!             "sku": "WIDGET-001",
//!             "name": "Blue Widget",
//!             "price": 29.99,
//!             "in_stock": true
//!         })
//!     ).await?;
//!
//!     // Query instances
//!     use runtara_object_store::{SimpleFilter, Condition, FilterRequest};
//!
//!     let (products, count) = store.query_instances(
//!         SimpleFilter::new("Products")
//!             .filter("in_stock", true)
//!             .paginate(0, 10)
//!     ).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Configuration
//!
//! The store is configured using `StoreConfig`:
//!
//! ```rust
//! use runtara_object_store::StoreConfig;
//!
//! let config = StoreConfig::builder("postgres://localhost/mydb")
//!     .metadata_table("__schema")  // Default metadata table name
//!     .soft_delete(true)           // Enable soft delete (default)
//!     .auto_id(true)               // Auto-generate UUID id column
//!     .auto_created_at(true)       // Auto-manage created_at column
//!     .auto_updated_at(true)       // Auto-manage updated_at column
//!     .build();
//! ```
//!
//! ## Multi-Tenancy
//!
//! This crate uses a database-per-tenant strategy. There is no tenant_id column;
//! instead, tenant isolation is achieved by connecting to different databases.
//! The caller is responsible for managing database connections for each tenant.

pub mod config;
pub mod error;
pub mod instance;
pub mod schema;
pub mod sql;
pub mod store;
pub mod types;

// Re-export main types for convenience
pub use config::{AutoColumns, StoreConfig, StoreConfigBuilder};
pub use error::{ObjectStoreError, Result};
pub use instance::{
    Condition, CreateInstanceRequest, FilterRequest, Instance, SimpleFilter, UpdateInstanceRequest,
};
pub use schema::{CreateSchemaRequest, Schema, UpdateSchemaRequest};
pub use store::ObjectStore;
pub use types::{ColumnDefinition, ColumnType, IndexDefinition};

// Re-export SQL utilities for advanced users
pub use sql::condition::{build_condition_clause, build_order_by_clause};
pub use sql::ddl::DdlGenerator;
pub use sql::sanitize::{quote_identifier, validate_identifier};
