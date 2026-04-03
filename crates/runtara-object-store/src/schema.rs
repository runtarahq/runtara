//! Schema-related types for Object Store
//!
//! Includes Schema, CreateSchemaRequest, UpdateSchemaRequest.

use serde::{Deserialize, Serialize};

use crate::types::{ColumnDefinition, IndexDefinition};

/// Schema metadata stored in the `__schema` table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    /// Unique identifier (UUID)
    pub id: String,
    /// Timestamp when the schema was created
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Timestamp when the schema was last updated
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Human-readable name of the schema
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Database table name for instances of this schema
    #[serde(rename = "tableName")]
    pub table_name: String,
    /// Column definitions for the table
    pub columns: Vec<ColumnDefinition>,
    /// Optional index definitions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexes: Option<Vec<IndexDefinition>>,
}

impl Schema {
    /// Create a new Schema with the given parameters
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        table_name: impl Into<String>,
        columns: Vec<ColumnDefinition>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: id.into(),
            created_at: now.clone(),
            updated_at: now,
            name: name.into(),
            description: None,
            table_name: table_name.into(),
            columns,
            indexes: None,
        }
    }

    /// Set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set indexes
    pub fn with_indexes(mut self, indexes: Vec<IndexDefinition>) -> Self {
        self.indexes = Some(indexes);
        self
    }
}

/// Request to create a new schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSchemaRequest {
    /// Human-readable name of the schema
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Database table name for instances of this schema
    #[serde(rename = "tableName")]
    pub table_name: String,
    /// Column definitions for the table
    pub columns: Vec<ColumnDefinition>,
    /// Optional index definitions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexes: Option<Vec<IndexDefinition>>,
}

impl CreateSchemaRequest {
    /// Create a new CreateSchemaRequest
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        columns: Vec<ColumnDefinition>,
    ) -> Self {
        Self {
            name: name.into(),
            description: None,
            table_name: table_name.into(),
            columns,
            indexes: None,
        }
    }

    /// Set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set indexes
    pub fn with_indexes(mut self, indexes: Vec<IndexDefinition>) -> Self {
        self.indexes = Some(indexes);
        self
    }
}

/// Request to update an existing schema
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSchemaRequest {
    /// New name (optional)
    pub name: Option<String>,
    /// New description (optional)
    pub description: Option<String>,
    /// New column definitions (optional)
    pub columns: Option<Vec<ColumnDefinition>>,
    /// New index definitions (optional)
    pub indexes: Option<Vec<IndexDefinition>>,
}

impl UpdateSchemaRequest {
    /// Create an empty update request
    pub fn new() -> Self {
        Self::default()
    }

    /// Set new name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set new description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set new columns
    pub fn with_columns(mut self, columns: Vec<ColumnDefinition>) -> Self {
        self.columns = Some(columns);
        self
    }

    /// Set new indexes
    pub fn with_indexes(mut self, indexes: Vec<IndexDefinition>) -> Self {
        self.indexes = Some(indexes);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ColumnType;

    #[test]
    fn test_schema_builder() {
        let schema = Schema::new(
            "schema-123",
            "Products",
            "products",
            vec![ColumnDefinition::new("sku", ColumnType::String)],
        )
        .with_description("Product catalog")
        .with_indexes(vec![IndexDefinition::new(
            "sku_idx",
            vec!["sku".to_string()],
        )]);

        assert_eq!(schema.id, "schema-123");
        assert_eq!(schema.name, "Products");
        assert_eq!(schema.table_name, "products");
        assert_eq!(schema.description, Some("Product catalog".to_string()));
        assert!(schema.indexes.is_some());
    }

    #[test]
    fn test_create_schema_request_builder() {
        let request = CreateSchemaRequest::new(
            "Products",
            "products",
            vec![ColumnDefinition::new("name", ColumnType::String)],
        )
        .with_description("Product catalog");

        assert_eq!(request.name, "Products");
        assert_eq!(request.table_name, "products");
        assert_eq!(request.description, Some("Product catalog".to_string()));
    }

    #[test]
    fn test_update_schema_request_builder() {
        let request = UpdateSchemaRequest::new()
            .with_name("New Name")
            .with_description("New description");

        assert_eq!(request.name, Some("New Name".to_string()));
        assert_eq!(request.description, Some("New description".to_string()));
        assert!(request.columns.is_none());
    }

    #[test]
    fn test_schema_serialization() {
        let schema = Schema::new(
            "123",
            "Test",
            "test_table",
            vec![ColumnDefinition::new("field", ColumnType::String)],
        );

        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains("\"createdAt\""));
        assert!(json.contains("\"updatedAt\""));
        assert!(json.contains("\"tableName\""));
    }
}
