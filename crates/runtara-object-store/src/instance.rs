//! Instance-related types for Object Store
//!
//! Includes Instance, CreateInstanceRequest, Condition, FilterRequest.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Instance data stored in dynamic tables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    /// Unique identifier (UUID)
    pub id: String,
    /// Timestamp when the instance was created
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Timestamp when the instance was last updated
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Reference to the schema ID (optional, for tracking)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "schemaId")]
    pub schema_id: Option<String>,
    /// Reference to the schema name (optional, for convenience)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "schemaName")]
    pub schema_name: Option<String>,
    /// Dynamic properties stored as JSON
    pub properties: serde_json::Value,
}

impl Instance {
    /// Create a new instance with the given properties
    pub fn new(id: impl Into<String>, properties: serde_json::Value) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: id.into(),
            created_at: now.clone(),
            updated_at: now,
            schema_id: None,
            schema_name: None,
            properties,
        }
    }

    /// Set schema reference by ID
    pub fn with_schema_id(mut self, schema_id: impl Into<String>) -> Self {
        self.schema_id = Some(schema_id.into());
        self
    }

    /// Set schema reference by name
    pub fn with_schema_name(mut self, schema_name: impl Into<String>) -> Self {
        self.schema_name = Some(schema_name.into());
        self
    }
}

/// Request to create a new instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInstanceRequest {
    /// Schema ID (UUID) - use this OR schemaName
    #[serde(rename = "schemaId", skip_serializing_if = "Option::is_none")]
    pub schema_id: Option<String>,
    /// Schema name - use this OR schemaId (more convenient)
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    /// Properties to set on the instance
    pub properties: serde_json::Value,
}

impl CreateInstanceRequest {
    /// Create a new instance request by schema name
    pub fn by_name(schema_name: impl Into<String>, properties: serde_json::Value) -> Self {
        Self {
            schema_id: None,
            schema_name: Some(schema_name.into()),
            properties,
        }
    }

    /// Create a new instance request by schema ID
    pub fn by_id(schema_id: impl Into<String>, properties: serde_json::Value) -> Self {
        Self {
            schema_id: Some(schema_id.into()),
            schema_name: None,
            properties,
        }
    }
}

/// Request to update an existing instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInstanceRequest {
    /// Properties to update (merged with existing)
    pub properties: serde_json::Value,
}

impl UpdateInstanceRequest {
    /// Create a new update request
    pub fn new(properties: serde_json::Value) -> Self {
        Self { properties }
    }
}

// ============================================================================
// Condition-based Filtering
// ============================================================================

/// Filter condition for querying instances
///
/// Supports operators:
/// - Logical: AND, OR, NOT
/// - Comparison: EQ, NE, GT, LT, GTE, LTE
/// - Collection: IN, NOT_IN, CONTAINS
/// - Null checks: IS_EMPTY, IS_NOT_EMPTY, IS_DEFINED
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Operator (e.g., "EQ", "AND", "IN")
    pub op: String,
    /// Arguments for the operator
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<serde_json::Value>>,
}

impl Condition {
    /// Create a new condition with the given operator and arguments
    pub fn new(op: impl Into<String>, arguments: Vec<serde_json::Value>) -> Self {
        Self {
            op: op.into(),
            arguments: Some(arguments),
        }
    }

    /// Create an equality condition
    pub fn eq(field: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new("EQ", vec![serde_json::json!(field.into()), value.into()])
    }

    /// Create a not-equal condition
    pub fn ne(field: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new("NE", vec![serde_json::json!(field.into()), value.into()])
    }

    /// Create a greater-than condition
    pub fn gt(field: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new("GT", vec![serde_json::json!(field.into()), value.into()])
    }

    /// Create a less-than condition
    pub fn lt(field: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new("LT", vec![serde_json::json!(field.into()), value.into()])
    }

    /// Create a greater-than-or-equal condition
    pub fn gte(field: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new("GTE", vec![serde_json::json!(field.into()), value.into()])
    }

    /// Create a less-than-or-equal condition
    pub fn lte(field: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::new("LTE", vec![serde_json::json!(field.into()), value.into()])
    }

    /// Create an IN condition
    pub fn r#in(field: impl Into<String>, values: Vec<serde_json::Value>) -> Self {
        Self::new(
            "IN",
            vec![serde_json::json!(field.into()), serde_json::json!(values)],
        )
    }

    /// Create a NOT IN condition
    pub fn not_in(field: impl Into<String>, values: Vec<serde_json::Value>) -> Self {
        Self::new(
            "NOT_IN",
            vec![serde_json::json!(field.into()), serde_json::json!(values)],
        )
    }

    /// Create a CONTAINS condition (for text search)
    pub fn contains(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            "CONTAINS",
            vec![
                serde_json::json!(field.into()),
                serde_json::json!(value.into()),
            ],
        )
    }

    /// Create an IS_EMPTY condition
    pub fn is_empty(field: impl Into<String>) -> Self {
        Self::new("IS_EMPTY", vec![serde_json::json!(field.into())])
    }

    /// Create an IS_NOT_EMPTY condition
    pub fn is_not_empty(field: impl Into<String>) -> Self {
        Self::new("IS_NOT_EMPTY", vec![serde_json::json!(field.into())])
    }

    /// Create an AND condition combining multiple conditions
    pub fn and(conditions: Vec<Condition>) -> Self {
        Self {
            op: "AND".to_string(),
            arguments: Some(
                conditions
                    .into_iter()
                    .map(|c| serde_json::to_value(c).unwrap())
                    .collect(),
            ),
        }
    }

    /// Create an OR condition combining multiple conditions
    pub fn or(conditions: Vec<Condition>) -> Self {
        Self {
            op: "OR".to_string(),
            arguments: Some(
                conditions
                    .into_iter()
                    .map(|c| serde_json::to_value(c).unwrap())
                    .collect(),
            ),
        }
    }

    /// Create a NOT condition
    #[allow(clippy::should_implement_trait)]
    pub fn not(condition: Condition) -> Self {
        Self {
            op: "NOT".to_string(),
            arguments: Some(vec![serde_json::to_value(condition).unwrap()]),
        }
    }
}

fn default_offset() -> i64 {
    0
}

fn default_limit() -> i64 {
    100
}

/// Request to filter instances
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterRequest {
    /// Number of results to skip
    #[serde(default = "default_offset")]
    pub offset: i64,
    /// Maximum number of results to return
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Filter condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// Fields to sort by (e.g., ["createdAt", "name"])
    #[serde(rename = "sortBy", skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<Vec<String>>,
    /// Sort order for each field (e.g., ["desc", "asc"])
    #[serde(rename = "sortOrder", skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<Vec<String>>,
}

impl Default for FilterRequest {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 100,
            condition: None,
            sort_by: None,
            sort_order: None,
        }
    }
}

impl FilterRequest {
    /// Create a new filter request
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the condition
    pub fn with_condition(mut self, condition: Condition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Set pagination
    pub fn with_pagination(mut self, offset: i64, limit: i64) -> Self {
        self.offset = offset;
        self.limit = limit;
        self
    }

    /// Set sorting
    pub fn with_sort(mut self, sort_by: Vec<String>, sort_order: Vec<String>) -> Self {
        self.sort_by = Some(sort_by);
        self.sort_order = Some(sort_order);
        self
    }
}

/// Simple filter using key-value pairs (for convenience)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleFilter {
    /// Schema name to query
    pub schema_name: String,
    /// Key-value filters (all must match)
    #[serde(default)]
    pub filters: HashMap<String, serde_json::Value>,
    /// Maximum number of results
    #[serde(default = "default_simple_limit")]
    pub limit: i32,
    /// Number of results to skip
    #[serde(default)]
    pub offset: i32,
}

fn default_simple_limit() -> i32 {
    100
}

impl SimpleFilter {
    /// Create a new simple filter for a schema
    pub fn new(schema_name: impl Into<String>) -> Self {
        Self {
            schema_name: schema_name.into(),
            filters: HashMap::new(),
            limit: 100,
            offset: 0,
        }
    }

    /// Add a filter condition
    pub fn filter(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.filters.insert(key.into(), value.into());
        self
    }

    /// Set pagination
    pub fn paginate(mut self, offset: i32, limit: i32) -> Self {
        self.offset = offset;
        self.limit = limit;
        self
    }

    /// Set the maximum number of results
    pub fn with_limit(mut self, limit: i32) -> Self {
        self.limit = limit;
        self
    }

    /// Set the number of results to skip
    pub fn with_offset(mut self, offset: i32) -> Self {
        self.offset = offset;
        self
    }

    /// Convert simple filter to FilterRequest with condition
    pub fn to_filter_request(&self) -> FilterRequest {
        let condition = if self.filters.is_empty() {
            None
        } else {
            let conditions: Vec<serde_json::Value> = self
                .filters
                .iter()
                .map(|(key, value)| {
                    serde_json::json!({
                        "op": "EQ",
                        "arguments": [key, value]
                    })
                })
                .collect();

            if conditions.len() == 1 {
                Some(Condition {
                    op: "EQ".to_string(),
                    arguments: Some(vec![
                        serde_json::json!(self.filters.keys().next().unwrap()),
                        self.filters.values().next().unwrap().clone(),
                    ]),
                })
            } else {
                Some(Condition {
                    op: "AND".to_string(),
                    arguments: Some(conditions),
                })
            }
        };

        FilterRequest {
            offset: self.offset as i64,
            limit: self.limit as i64,
            condition,
            sort_by: None,
            sort_order: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_builder() {
        let instance = Instance::new("inst-123", serde_json::json!({"name": "Test"}))
            .with_schema_name("products")
            .with_schema_id("schema-456");

        assert_eq!(instance.id, "inst-123");
        assert_eq!(instance.schema_name, Some("products".to_string()));
        assert_eq!(instance.schema_id, Some("schema-456".to_string()));
    }

    #[test]
    fn test_condition_builders() {
        let cond = Condition::eq("status", "active");
        assert_eq!(cond.op, "EQ");

        let cond = Condition::and(vec![
            Condition::eq("status", "active"),
            Condition::gt("price", 100),
        ]);
        assert_eq!(cond.op, "AND");
    }

    #[test]
    fn test_simple_filter() {
        let filter = SimpleFilter::new("products")
            .filter("status", "active")
            .filter("category", "electronics")
            .paginate(10, 50);

        assert_eq!(filter.schema_name, "products");
        assert_eq!(filter.filters.len(), 2);
        assert_eq!(filter.offset, 10);
        assert_eq!(filter.limit, 50);

        let request = filter.to_filter_request();
        assert_eq!(request.offset, 10);
        assert_eq!(request.limit, 50);
        assert!(request.condition.is_some());
        assert_eq!(request.condition.unwrap().op, "AND");
    }

    #[test]
    fn test_filter_request_builder() {
        let request = FilterRequest::new()
            .with_condition(Condition::eq("active", true))
            .with_pagination(0, 25)
            .with_sort(vec!["createdAt".to_string()], vec!["desc".to_string()]);

        assert_eq!(request.limit, 25);
        assert!(request.condition.is_some());
        assert_eq!(request.sort_by.unwrap()[0], "createdAt");
    }

    #[test]
    fn test_create_instance_request() {
        let request =
            CreateInstanceRequest::by_name("products", serde_json::json!({"sku": "ABC123"}));
        assert_eq!(request.schema_name, Some("products".to_string()));
        assert!(request.schema_id.is_none());

        let request =
            CreateInstanceRequest::by_id("schema-123", serde_json::json!({"sku": "ABC123"}));
        assert_eq!(request.schema_id, Some("schema-123".to_string()));
        assert!(request.schema_name.is_none());
    }
}
