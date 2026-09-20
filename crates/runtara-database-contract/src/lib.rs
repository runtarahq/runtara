//! Versioned SQL vocabulary shared by native drivers and WASM clients.
//!
//! Exact values use decimal strings, independently of JavaScript/JSON number
//! precision. SQL NULL and JSON null have distinct representations.
use serde::{Deserialize, Serialize};

pub const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_BATCH_STATEMENTS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqlType {
    Text,
    Integer,
    Decimal,
    Float,
    Boolean,
    Json,
    Timestamp,
    TimestampTz,
    Date,
    Time,
    Uuid,
    Vector,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SqlValue {
    Null(SqlType),
    Text(String),
    Integer(String),
    Decimal(String),
    Float(f64),
    Boolean(bool),
    Json(serde_json::Value),
    Timestamp(String),
    TimestampTz(String),
    Date(String),
    Time(String),
    Uuid(String),
    Vector(Vec<f32>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultColumn {
    pub name: String,
    pub value_type: SqlType,
    #[serde(default = "yes")]
    pub nullable: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "columns", rename_all = "snake_case")]
pub enum ResultSpec {
    #[default]
    Raw,
    /// Decode only these named columns, preserving their actual SQL types.
    /// Higher-level type families (enums, Object Model timestamps) are guest policy.
    Selected(Vec<String>),
    Columns(Vec<ResultColumn>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<SqlValue>,
    #[serde(default)]
    pub result_schema: ResultSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statement {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<SqlValue>,
    #[serde(default)]
    pub returning: Option<ResultSpec>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchMode {
    #[default]
    Atomic,
    Independent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    #[serde(default)]
    pub mode: BatchMode,
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub database_type: String,
    pub value_type: SqlType,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RowSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<SqlValue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub rows_affected: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned: Option<RowSet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    NotStarted,
    RolledBack,
    Committed,
    Unknown,
}

/// Safe diagnostics only: never include SQL text, bound values, or driver errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseError {
    pub code: String,
    pub message: String,
    pub outcome: Outcome,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement_index: Option<usize>,
}

impl DatabaseError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "DATABASE_INVALID_REQUEST".into(),
            message: message.into(),
            outcome: Outcome::NotStarted,
            retryable: false,
            sqlstate: None,
            statement_index: None,
        }
    }
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for DatabaseError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementResult {
    pub index: usize,
    pub result: Result<ExecutionResult, DatabaseError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchResult {
    pub mode: BatchMode,
    pub results: Vec<StatementResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values_and_two_nulls_round_trip_without_json_numbers() {
        let values = vec![
            SqlValue::Integer(i64::MAX.to_string()),
            SqlValue::Integer(i64::MIN.to_string()),
            SqlValue::Decimal("12345678901234567890.12345678".into()),
            SqlValue::Null(SqlType::Json),
            SqlValue::Json(serde_json::Value::Null),
        ];
        let json = serde_json::to_string(&values).unwrap();
        assert_eq!(
            values,
            serde_json::from_str::<Vec<SqlValue>>(&json).unwrap()
        );
        assert_ne!(
            serde_json::to_value(&values[3]).unwrap(),
            serde_json::to_value(&values[4]).unwrap()
        );
        assert!(json.contains("\"9223372036854775807\""));
    }

    #[test]
    fn identity_and_policy_cannot_be_smuggled_into_requests() {
        assert!(
            serde_json::from_value::<QueryRequest>(serde_json::json!({
                "sql": "SELECT 1", "tenant_id": "another-tenant"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Statement>(serde_json::json!({
                "sql": "SELECT 1", "timeout_ms": 0
            }))
            .is_err()
        );
    }

    #[test]
    fn rows_preserve_duplicate_names_and_empty_result_metadata() {
        let column = Column {
            name: "value".into(),
            database_type: "INT8".into(),
            value_type: SqlType::Integer,
        };
        let rows = RowSet {
            columns: vec![column.clone(), column],
            rows: vec![],
        };
        assert_eq!(
            rows,
            serde_json::from_slice::<RowSet>(&serde_json::to_vec(&rows).unwrap()).unwrap()
        );
    }
}
