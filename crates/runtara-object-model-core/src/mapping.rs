//! Object property/SQL value mapping. The driver never interprets workflow mappings.
use crate::types::ColumnType;
use runtara_database_contract::{RowSet, SqlType, SqlValue};
use serde_json::{Map, Value};

pub fn sql_type(column: &ColumnType) -> Result<SqlType, String> {
    Ok(match column {
        ColumnType::String | ColumnType::Enum { .. } => SqlType::Text,
        ColumnType::Integer => SqlType::Integer,
        ColumnType::Decimal { .. } => SqlType::Decimal,
        ColumnType::Boolean => SqlType::Boolean,
        ColumnType::Timestamp => SqlType::TimestampTz,
        ColumnType::Json => SqlType::Json,
        ColumnType::Vector { .. } => SqlType::Vector,
        ColumnType::Tsvector { .. } => {
            return Err("Generated columns cannot be bound as parameters".into());
        }
    })
}

/// Object Model historically stores an explicit JSON null as JSON null, while
/// scalar nulls become SQL NULL. Missing/default fields are handled by planners.
pub fn object_param(column: &ColumnType, value: &Value) -> Result<SqlValue, String> {
    column.validate_value(value)?;
    let kind = sql_type(column)?;
    if value.is_null() && kind != SqlType::Json {
        return Ok(SqlValue::Null(kind));
    }
    Ok(match column {
        ColumnType::String | ColumnType::Enum { .. } => {
            SqlValue::Text(value.as_str().ok_or("Expected text")?.to_owned())
        }
        ColumnType::Integer => SqlValue::Integer(if let Some(v) = value.as_i64() {
            v.to_string()
        } else {
            value
                .as_str()
                .ok_or("Expected integer")?
                .parse::<i64>()
                .map_err(|_| "Expected integer")?
                .to_string()
        }),
        ColumnType::Decimal { .. } => SqlValue::Decimal(
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        ),
        ColumnType::Boolean => SqlValue::Boolean(match value {
            Value::Bool(v) => *v,
            Value::String(v) => match v.to_lowercase().as_str() {
                "true" | "1" | "yes" => true,
                "false" | "0" | "no" => false,
                _ => return Err("Expected boolean".into()),
            },
            _ => return Err("Expected boolean".into()),
        }),
        ColumnType::Timestamp => {
            SqlValue::TimestampTz(value.as_str().ok_or("Expected timestamp")?.to_owned())
        }
        ColumnType::Json => SqlValue::Json(value.clone()),
        ColumnType::Vector { .. } => SqlValue::Vector(
            value
                .as_array()
                .ok_or("Expected vector")?
                .iter()
                .map(|v| v.as_f64().map(|v| v as f32).ok_or("Expected vector number"))
                .collect::<Result<_, _>>()?,
        ),
        ColumnType::Tsvector { .. } => return Err("Generated columns cannot be written".into()),
    })
}

/// Compatibility with the existing query-sql/execute-sql capability input.
/// Unlike object properties, its explicit typed null means SQL NULL, even for JSON.
pub fn legacy_sql_param(column: &ColumnType, value: &Value) -> Result<SqlValue, String> {
    if value.is_null() {
        return Ok(SqlValue::Null(sql_type(column)?));
    }
    object_param(column, value)
}

/// Legacy result schemas describe Object Model type families, not individual
/// PostgreSQL codecs. Keep these rules in the guest and retain actual SQL types
/// across the driver boundary (notably integer/decimal and local/UTC time).
pub fn validate_sql_result(
    column: &ColumnType,
    value: &SqlValue,
    nullable: bool,
) -> Result<(), String> {
    if matches!(value, SqlValue::Null(_)) {
        return if nullable {
            Ok(())
        } else {
            Err("Non-nullable result column is NULL".into())
        };
    }
    let compatible = match column {
        ColumnType::String | ColumnType::Tsvector { .. } => {
            matches!(value, SqlValue::Text(_) | SqlValue::Uuid(_))
        }
        ColumnType::Enum { values } => match value {
            SqlValue::Text(text) | SqlValue::Uuid(text) => values.contains(text),
            _ => false,
        },
        ColumnType::Integer => matches!(value, SqlValue::Integer(_)),
        ColumnType::Decimal { .. } => matches!(
            value,
            SqlValue::Integer(_) | SqlValue::Decimal(_) | SqlValue::Float(_)
        ),
        ColumnType::Boolean => matches!(value, SqlValue::Boolean(_)),
        ColumnType::Timestamp => matches!(
            value,
            SqlValue::Timestamp(_) | SqlValue::TimestampTz(_) | SqlValue::Date(_)
        ),
        ColumnType::Json => matches!(value, SqlValue::Json(_)),
        ColumnType::Vector { .. } => matches!(value, SqlValue::Vector(_)),
    };
    if compatible {
        Ok(())
    } else {
        Err("SQL result does not match the requested Object Model type".into())
    }
}

/// Existing capabilities expose ordinary JSON values. This is the explicit
/// legacy presentation boundary; the native SQL wire remains exactly typed.
pub fn legacy_json(value: SqlValue) -> Result<Value, String> {
    Ok(match value {
        SqlValue::Null(_) => Value::Null,
        SqlValue::Text(v)
        | SqlValue::Timestamp(v)
        | SqlValue::TimestampTz(v)
        | SqlValue::Date(v)
        | SqlValue::Time(v)
        | SqlValue::Uuid(v) => Value::String(v),
        SqlValue::Integer(v) => {
            Value::Number(v.parse::<i64>().map_err(|_| "Invalid SQL integer")?.into())
        }
        SqlValue::Decimal(v) => {
            // Match the former numeric JSON presentation. Consumers needing
            // exact decimals use the typed SQL contract rather than this adapter.
            v.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::String(v))
        }
        SqlValue::Float(v) => {
            Value::Number(serde_json::Number::from_f64(v).ok_or("Invalid SQL float")?)
        }
        SqlValue::Boolean(v) => Value::Bool(v),
        SqlValue::Json(v) => v,
        SqlValue::Vector(v) => serde_json::to_value(v).map_err(|_| "Invalid SQL vector")?,
    })
}

pub fn row_objects(rows: RowSet) -> Result<Vec<Map<String, Value>>, String> {
    map_rows(rows, false)
}

/// Instance outputs historically omit SQL NULL properties, while an explicit
/// JSON null remains present. Raw SQL output keeps both as JSON null instead.
pub fn instance_objects(rows: RowSet) -> Result<Vec<Map<String, Value>>, String> {
    map_rows(rows, true)
}

fn map_rows(rows: RowSet, omit_sql_null: bool) -> Result<Vec<Map<String, Value>>, String> {
    let mut names = std::collections::HashSet::new();
    for column in &rows.columns {
        if !names.insert(&column.name) {
            return Err("Duplicate SQL column names require aliases for object output".into());
        }
    }
    rows.rows
        .into_iter()
        .map(|row| {
            if row.len() != rows.columns.len() {
                return Err("Invalid SQL row width".into());
            }
            rows.columns
                .iter()
                .zip(row)
                .filter(|(_, value)| !omit_sql_null || !matches!(value, SqlValue::Null(_)))
                .map(|(column, value)| Ok((column.name.clone(), legacy_json(value)?)))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_the_two_existing_json_null_conventions() {
        assert_eq!(
            object_param(&ColumnType::Json, &Value::Null).unwrap(),
            SqlValue::Json(Value::Null)
        );
        assert_eq!(
            legacy_sql_param(&ColumnType::Json, &Value::Null).unwrap(),
            SqlValue::Null(SqlType::Json)
        );
    }
    #[test]
    fn integers_do_not_pass_through_floating_point() {
        let value = serde_json::json!(i64::MAX);
        let sql = object_param(&ColumnType::Integer, &value).unwrap();
        assert_eq!(sql, SqlValue::Integer(i64::MAX.to_string()));
        assert_eq!(legacy_json(sql).unwrap(), value);
    }
    #[test]
    fn decimal_strings_are_exact_until_legacy_presentation() {
        let value = Value::String("12345678901234567890.12345678".into());
        assert_eq!(
            object_param(&ColumnType::decimal(28, 8), &value).unwrap(),
            SqlValue::Decimal("12345678901234567890.12345678".into())
        );
    }
}
