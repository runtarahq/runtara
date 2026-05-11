//! Positional SQLx-backed query helpers.
//!
//! These helpers intentionally expose Postgres / SQLx positional placeholders
//! (`$1`, `$2`, ...) rather than implementing a separate named-parameter layer.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use sqlx::{Column, Executor, Row, TypeInfo};
use std::str::FromStr;

use crate::error::{ObjectStoreError, Result};
use crate::store::ObjectStore;
use crate::types::ColumnType;

/// A typed positional SQL parameter. Parameters are bound in vector order, so
/// the first item is `$1`, the second is `$2`, and so on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SqlParam {
    #[serde(flatten)]
    pub column_type: ColumnType,
    pub value: serde_json::Value,
}

/// Expected output column for typed query helpers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SqlResultColumn {
    pub name: String,
    #[serde(flatten)]
    pub column_type: ColumnType,
    #[serde(default = "default_nullable")]
    pub nullable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SqlRows {
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SqlExecuteResult {
    pub rows_affected: u64,
}

fn default_nullable() -> bool {
    true
}

impl ObjectStore {
    /// Execute a SQL query and validate/project rows to the supplied result
    /// schema. SQL must use native Postgres placeholders (`$1`, `$2`, ...).
    pub async fn query(
        &self,
        sql: &str,
        params: &[SqlParam],
        result_schema: &[SqlResultColumn],
    ) -> Result<SqlRows> {
        validate_sql(sql)?;
        validate_result_schema(result_schema)?;

        let mut query = sqlx::query(sql);
        for (index, param) in params.iter().enumerate() {
            query = bind_param(query, param, index)?;
        }

        let rows = query.fetch_all(self.pool()).await?;
        let rows = rows
            .iter()
            .map(|row| row_to_typed_json(row, result_schema))
            .collect::<Result<Vec<_>>>()?;

        Ok(SqlRows { rows })
    }

    /// Execute a typed query that must return exactly one row.
    pub async fn query_one(
        &self,
        sql: &str,
        params: &[SqlParam],
        result_schema: &[SqlResultColumn],
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let rows = self.query(sql, params, result_schema).await?.rows;
        match rows.len() {
            1 => Ok(rows.into_iter().next().expect("len checked")),
            0 => Err(ObjectStoreError::validation(
                "query_one expected exactly one row, got 0",
            )),
            n => Err(ObjectStoreError::validation(format!(
                "query_one expected exactly one row, got {}",
                n
            ))),
        }
    }

    /// Execute a SQL query and return raw JSON rows. SQL must use native
    /// Postgres placeholders (`$1`, `$2`, ...).
    pub async fn query_raw(&self, sql: &str, params: &[SqlParam]) -> Result<SqlRows> {
        validate_sql(sql)?;

        let mut query = sqlx::query(sql);
        for (index, param) in params.iter().enumerate() {
            query = bind_param(query, param, index)?;
        }

        let rows = query.fetch_all(self.pool()).await?;
        let rows = rows
            .iter()
            .map(row_to_raw_json)
            .collect::<Result<Vec<_>>>()?;

        Ok(SqlRows { rows })
    }

    /// Execute a SQL command and return the number of affected rows.
    pub async fn execute(&self, sql: &str, params: &[SqlParam]) -> Result<SqlExecuteResult> {
        validate_sql(sql)?;

        let mut query = sqlx::query(sql);
        for (index, param) in params.iter().enumerate() {
            query = bind_param(query, param, index)?;
        }

        let result = self.pool().execute(query).await?;
        Ok(SqlExecuteResult {
            rows_affected: result.rows_affected(),
        })
    }
}

fn validate_sql(sql: &str) -> Result<()> {
    if sql.trim().is_empty() {
        return Err(ObjectStoreError::validation("sql cannot be empty"));
    }
    Ok(())
}

fn validate_result_schema(result_schema: &[SqlResultColumn]) -> Result<()> {
    if result_schema.is_empty() {
        return Err(ObjectStoreError::validation(
            "result_schema must contain at least one column",
        ));
    }

    let mut names = std::collections::HashSet::new();
    for column in result_schema {
        if column.name.trim().is_empty() {
            return Err(ObjectStoreError::validation(
                "result_schema column name cannot be empty",
            ));
        }
        if !names.insert(column.name.as_str()) {
            return Err(ObjectStoreError::validation(format!(
                "result_schema has duplicate column '{}'",
                column.name
            )));
        }
    }

    Ok(())
}

fn bind_param<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    param: &'q SqlParam,
    index: usize,
) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>> {
    let label = format!("param ${}", index + 1);
    Ok(match &param.column_type {
        ColumnType::String => {
            if param.value.is_null() {
                query.bind(None::<String>)
            } else {
                query.bind(param.value.as_str().ok_or_else(|| {
                    ObjectStoreError::validation(format!("{} expected string", label))
                })?)
            }
        }
        ColumnType::Enum { values } => {
            if param.value.is_null() {
                query.bind(None::<String>)
            } else {
                let value = param.value.as_str().ok_or_else(|| {
                    ObjectStoreError::validation(format!("{} expected enum string", label))
                })?;
                if !values.iter().any(|allowed| allowed == value) {
                    return Err(ObjectStoreError::validation(format!(
                        "{} value '{}' is not in enum values {:?}",
                        label, value, values
                    )));
                }
                query.bind(value)
            }
        }
        ColumnType::Integer => {
            if param.value.is_null() {
                query.bind(None::<i64>)
            } else {
                let value = param
                    .value
                    .as_i64()
                    .or_else(|| param.value.as_str().and_then(|s| s.parse::<i64>().ok()))
                    .ok_or_else(|| {
                        ObjectStoreError::validation(format!("{} expected integer", label))
                    })?;
                query.bind(value)
            }
        }
        ColumnType::Decimal { .. } => {
            if param.value.is_null() {
                query.bind(None::<rust_decimal::Decimal>)
            } else {
                let value = match &param.value {
                    serde_json::Value::Number(number) => {
                        rust_decimal::Decimal::from_str(&number.to_string())
                    }
                    serde_json::Value::String(s) => rust_decimal::Decimal::from_str(s),
                    _ => {
                        return Err(ObjectStoreError::validation(format!(
                            "{} expected decimal",
                            label
                        )));
                    }
                }
                .map_err(|_| ObjectStoreError::validation(format!("{} expected decimal", label)))?;
                query.bind(value)
            }
        }
        ColumnType::Boolean => {
            if param.value.is_null() {
                query.bind(None::<bool>)
            } else {
                let value = param
                    .value
                    .as_bool()
                    .or_else(|| {
                        param
                            .value
                            .as_str()
                            .and_then(|s| match s.to_lowercase().as_str() {
                                "true" | "1" | "yes" => Some(true),
                                "false" | "0" | "no" => Some(false),
                                _ => None,
                            })
                    })
                    .ok_or_else(|| {
                        ObjectStoreError::validation(format!("{} expected boolean", label))
                    })?;
                query.bind(value)
            }
        }
        ColumnType::Timestamp => {
            if param.value.is_null() {
                query.bind(None::<DateTime<Utc>>)
            } else {
                let value = param.value.as_str().ok_or_else(|| {
                    ObjectStoreError::validation(format!("{} expected timestamp string", label))
                })?;
                let parsed = DateTime::parse_from_rfc3339(value)
                    .map_err(|err| {
                        ObjectStoreError::validation(format!(
                            "{} has invalid timestamp: {}",
                            label, err
                        ))
                    })?
                    .with_timezone(&Utc);
                query.bind(parsed)
            }
        }
        ColumnType::Json => {
            if param.value.is_null() {
                query.bind(None::<serde_json::Value>)
            } else {
                query.bind(&param.value)
            }
        }
        ColumnType::Tsvector { .. } => {
            return Err(ObjectStoreError::validation(format!(
                "{} cannot use generated tsvector type",
                label
            )));
        }
        ColumnType::Vector { dimension, .. } => {
            if param.value.is_null() {
                query.bind(None::<pgvector::Vector>)
            } else {
                let value = json_value_to_vector(&param.value, *dimension, &label)?;
                query.bind(value)
            }
        }
    })
}

fn row_to_typed_json(
    row: &sqlx::postgres::PgRow,
    result_schema: &[SqlResultColumn],
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut object = serde_json::Map::with_capacity(result_schema.len());
    for column in result_schema {
        let value = extract_typed_cell(row, column)?;
        if value.is_none() && !column.nullable {
            return Err(ObjectStoreError::validation(format!(
                "column '{}' returned NULL but is not nullable",
                column.name
            )));
        }
        object.insert(
            column.name.clone(),
            value.unwrap_or(serde_json::Value::Null),
        );
    }
    Ok(object)
}

fn extract_typed_cell(
    row: &sqlx::postgres::PgRow,
    column: &SqlResultColumn,
) -> Result<Option<serde_json::Value>> {
    let name = column.name.as_str();
    let type_name = column_type_name(row, name);

    match &column.column_type {
        ColumnType::String | ColumnType::Tsvector { .. } => match type_name.as_deref() {
            Some("UUID") => row
                .try_get::<Option<uuid::Uuid>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::String(v.to_string()))),
            _ => row
                .try_get::<Option<String>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(serde_json::Value::String)),
        },
        ColumnType::Enum { values } => match type_name.as_deref() {
            Some("UUID") => row
                .try_get::<Option<uuid::Uuid>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .and_then(|value| {
                    validate_enum_result(column, values, value.map(|v| v.to_string()))
                }),
            _ => row
                .try_get::<Option<String>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .and_then(|value| validate_enum_result(column, values, value)),
        },
        ColumnType::Integer => match type_name.as_deref() {
            Some("INT2") | Some("SMALLINT") => row
                .try_get::<Option<i16>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::Number(i64::from(v).into()))),
            Some("INT4") | Some("INTEGER") => row
                .try_get::<Option<i32>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::Number(i64::from(v).into()))),
            _ => row
                .try_get::<Option<i64>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::Number(v.into()))),
        },
        ColumnType::Decimal { .. } => match type_name.as_deref() {
            Some("INT2") | Some("SMALLINT") => row
                .try_get::<Option<i16>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::Number(i64::from(v).into()))),
            Some("INT4") | Some("INTEGER") => row
                .try_get::<Option<i32>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::Number(i64::from(v).into()))),
            Some("INT8") | Some("BIGINT") => row
                .try_get::<Option<i64>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::Number(v.into()))),
            Some("FLOAT4") | Some("REAL") => row
                .try_get::<Option<f32>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| {
                    value.map(|v| {
                        serde_json::Number::from_f64(v as f64)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    })
                }),
            Some("FLOAT8") | Some("DOUBLE PRECISION") => row
                .try_get::<Option<f64>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| {
                    value.map(|v| {
                        serde_json::Number::from_f64(v)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    })
                }),
            _ => row
                .try_get::<Option<rust_decimal::Decimal>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| {
                    value.map(|decimal| {
                        decimal
                            .to_f64()
                            .and_then(serde_json::Number::from_f64)
                            .map(serde_json::Value::Number)
                            .unwrap_or_else(|| serde_json::Value::String(decimal.to_string()))
                    })
                }),
        },
        ColumnType::Boolean => row
            .try_get::<Option<bool>, _>(name)
            .map_err(|err| typed_cell_error(column, err))
            .map(|value| value.map(serde_json::Value::Bool)),
        ColumnType::Timestamp => match type_name.as_deref() {
            Some("TIMESTAMP") => row
                .try_get::<Option<NaiveDateTime>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::String(v.to_string()))),
            Some("DATE") => row
                .try_get::<Option<NaiveDate>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::String(v.to_string()))),
            _ => row
                .try_get::<Option<DateTime<Utc>>, _>(name)
                .map_err(|err| typed_cell_error(column, err))
                .map(|value| value.map(|v| serde_json::Value::String(v.to_rfc3339()))),
        },
        ColumnType::Json => row
            .try_get::<Option<serde_json::Value>, _>(name)
            .map_err(|err| typed_cell_error(column, err)),
        ColumnType::Vector { .. } => row
            .try_get::<Option<pgvector::Vector>, _>(name)
            .map_err(|err| typed_cell_error(column, err))
            .map(|value| value.map(vector_to_json)),
    }
}

fn column_type_name(row: &sqlx::postgres::PgRow, name: &str) -> Option<String> {
    row.columns()
        .iter()
        .find(|column| column.name() == name)
        .map(|column| column.type_info().name().to_ascii_uppercase())
}

fn typed_cell_error(column: &SqlResultColumn, err: sqlx::Error) -> ObjectStoreError {
    ObjectStoreError::validation(format!(
        "column '{}' could not be decoded as {:?}: {}",
        column.name, column.column_type, err
    ))
}

fn validate_enum_result(
    column: &SqlResultColumn,
    values: &[String],
    value: Option<String>,
) -> Result<Option<serde_json::Value>> {
    match value {
        Some(value) if values.iter().any(|allowed| allowed == &value) => {
            Ok(Some(serde_json::Value::String(value)))
        }
        Some(value) => Err(ObjectStoreError::validation(format!(
            "column '{}' value '{}' is not in enum values {:?}",
            column.name, value, values
        ))),
        None => Ok(None),
    }
}

fn row_to_raw_json(
    row: &sqlx::postgres::PgRow,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut object = serde_json::Map::with_capacity(row.columns().len());
    for column in row.columns() {
        let name = column.name();
        let type_name = column.type_info().name();
        let value = extract_raw_cell(row, name, type_name)?;
        object.insert(name.to_string(), value);
    }
    Ok(object)
}

fn extract_raw_cell(
    row: &sqlx::postgres::PgRow,
    name: &str,
    type_name: &str,
) -> Result<serde_json::Value> {
    let normalized = type_name.to_ascii_uppercase();
    match normalized.as_str() {
        "BOOL" | "BOOLEAN" => {
            decode_optional(row, name, type_name, |v: bool| serde_json::Value::Bool(v))
        }
        "INT2" | "SMALLINT" => decode_optional(row, name, type_name, |v: i16| {
            serde_json::Value::Number(i64::from(v).into())
        }),
        "INT4" | "INTEGER" => decode_optional(row, name, type_name, |v: i32| {
            serde_json::Value::Number(i64::from(v).into())
        }),
        "INT8" | "BIGINT" => decode_optional(row, name, type_name, |v: i64| {
            serde_json::Value::Number(v.into())
        }),
        "FLOAT4" | "REAL" => decode_optional(row, name, type_name, |v: f32| {
            serde_json::Number::from_f64(v as f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        }),
        "FLOAT8" | "DOUBLE PRECISION" => decode_optional(row, name, type_name, |v: f64| {
            serde_json::Number::from_f64(v)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        }),
        "NUMERIC" | "DECIMAL" => {
            decode_optional(row, name, type_name, |v: rust_decimal::Decimal| {
                v.to_f64()
                    .and_then(serde_json::Number::from_f64)
                    .map(serde_json::Value::Number)
                    .unwrap_or_else(|| serde_json::Value::String(v.to_string()))
            })
        }
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" => {
            decode_optional(row, name, type_name, serde_json::Value::String)
        }
        "UUID" => decode_optional(row, name, type_name, |v: uuid::Uuid| {
            serde_json::Value::String(v.to_string())
        }),
        "JSON" | "JSONB" => decode_optional(row, name, type_name, |v: serde_json::Value| v),
        "TIMESTAMPTZ" => decode_optional(row, name, type_name, |v: DateTime<Utc>| {
            serde_json::Value::String(v.to_rfc3339())
        }),
        "TIMESTAMP" => decode_optional(row, name, type_name, |v: NaiveDateTime| {
            serde_json::Value::String(v.to_string())
        }),
        "DATE" => decode_optional(row, name, type_name, |v: NaiveDate| {
            serde_json::Value::String(v.to_string())
        }),
        "TIME" => decode_optional(row, name, type_name, |v: NaiveTime| {
            serde_json::Value::String(v.to_string())
        }),
        "VECTOR" => decode_optional(row, name, type_name, vector_to_json),
        _ => Err(ObjectStoreError::validation(format!(
            "column '{}' has unsupported raw SQL type '{}'; use query with result_schema for explicit decoding",
            name, type_name
        ))),
    }
}

fn decode_optional<T, F>(
    row: &sqlx::postgres::PgRow,
    name: &str,
    type_name: &str,
    convert: F,
) -> Result<serde_json::Value>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    F: FnOnce(T) -> serde_json::Value,
{
    row.try_get::<Option<T>, _>(name)
        .map_err(|err| {
            ObjectStoreError::validation(format!(
                "column '{}' could not be decoded as raw SQL type '{}': {}",
                name, type_name, err
            ))
        })
        .map(|value| value.map(convert).unwrap_or(serde_json::Value::Null))
}

fn json_value_to_vector(
    value: &serde_json::Value,
    dimension: u32,
    label: &str,
) -> Result<pgvector::Vector> {
    let array = value
        .as_array()
        .ok_or_else(|| ObjectStoreError::validation(format!("{} expected vector array", label)))?;
    if array.len() as u32 != dimension {
        return Err(ObjectStoreError::validation(format!(
            "{} vector dimension mismatch: expected {}, got {}",
            label,
            dimension,
            array.len()
        )));
    }

    let floats = array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let number = value.as_f64().ok_or_else(|| {
                ObjectStoreError::validation(format!(
                    "{} vector element {} expected number",
                    label, index
                ))
            })?;
            if !number.is_finite() {
                return Err(ObjectStoreError::validation(format!(
                    "{} vector element {} must be finite",
                    label, index
                )));
            }
            Ok(number as f32)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(pgvector::Vector::from(floats))
}

fn vector_to_json(vector: pgvector::Vector) -> serde_json::Value {
    serde_json::Value::Array(
        vector
            .to_vec()
            .into_iter()
            .filter_map(|f| serde_json::Number::from_f64(f as f64))
            .map(serde_json::Value::Number)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_result_schema_rejects_empty() {
        let err = validate_result_schema(&[]).unwrap_err();
        assert!(err.to_string().contains("at least one column"));
    }

    #[test]
    fn validates_result_schema_rejects_duplicate_columns() {
        let schema = vec![
            SqlResultColumn {
                name: "id".to_string(),
                column_type: ColumnType::String,
                nullable: false,
            },
            SqlResultColumn {
                name: "id".to_string(),
                column_type: ColumnType::String,
                nullable: true,
            },
        ];
        let err = validate_result_schema(&schema).unwrap_err();
        assert!(err.to_string().contains("duplicate column"));
    }

    #[test]
    fn vector_param_validation_checks_dimension() {
        let err = json_value_to_vector(&serde_json::json!([1.0, 2.0]), 3, "param $1").unwrap_err();
        assert!(err.to_string().contains("dimension mismatch"));
    }
}
