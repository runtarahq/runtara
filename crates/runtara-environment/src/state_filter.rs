// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Filters over a run's published state (`instance_state`).
//!
//! A filter compares one top-level state field with a literal JSON value;
//! filters combine with AND. A run whose state lacks the field does not match
//! (except `exists: false`). Values are literals: there is no `now`, callers
//! pass the time they mean. A string that parses as an ISO 8601 date-time is
//! compared in the canonical UTC form state stores date-times in.
//!
//! Filters only narrow a listing; they never return state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most filters one listing takes.
pub const MAX_STATE_FILTERS: usize = 16;
/// Most values one `in` filter takes.
pub const MAX_IN_VALUES: usize = 100;
/// Longest field name a filter takes.
pub const MAX_FIELD_BYTES: usize = 256;

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateFilterOp {
    /// Equal to the value.
    Eq,
    /// Present and not equal to the value.
    Ne,
    /// Equal to one of the values.
    In,
    /// Less than the value (same JSON type).
    Lt,
    /// Less than or equal to the value (same JSON type).
    Lte,
    /// Greater than the value (same JSON type).
    Gt,
    /// Greater than or equal to the value (same JSON type).
    Gte,
    /// The field is present (`true`) or absent (`false`).
    Exists,
}

/// One filter on a top-level state field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateFilter {
    /// A top-level field of the run's state.
    pub field: String,
    /// The comparison.
    pub op: StateFilterOp,
    /// A JSON scalar; an array of scalars for `in`; a boolean (default
    /// `true`) for `exists`.
    #[serde(default)]
    pub value: Value,
}

/// Parse and validate filters from their JSON form (an array).
pub fn parse_state_filters(json: &[u8]) -> Result<Vec<StateFilter>, String> {
    let filters: Vec<StateFilter> = serde_json::from_slice(json).map_err(|error| {
        format!("state filters must be an array of {{field, op, value}}: {error}")
    })?;
    normalize_state_filters(filters)
}

/// Validate filters and canonicalise their date-time values.
pub fn normalize_state_filters(filters: Vec<StateFilter>) -> Result<Vec<StateFilter>, String> {
    if filters.len() > MAX_STATE_FILTERS {
        return Err(format!(
            "at most {MAX_STATE_FILTERS} state filters, not {}",
            filters.len()
        ));
    }
    filters
        .into_iter()
        .map(|mut filter| {
            if filter.field.is_empty()
                || filter.field.len() > MAX_FIELD_BYTES
                || filter.field.chars().any(char::is_control)
            {
                return Err("a state filter needs a field name".to_string());
            }
            let field = filter.field.clone();
            filter.value = match filter.op {
                StateFilterOp::Exists => match filter.value {
                    Value::Null => Value::Bool(true),
                    Value::Bool(value) => Value::Bool(value),
                    _ => return Err(format!("`exists` on '{field}' takes true or false")),
                },
                StateFilterOp::In => {
                    let Value::Array(values) = filter.value else {
                        return Err(format!("`in` on '{field}' takes an array of values"));
                    };
                    if values.is_empty() || values.len() > MAX_IN_VALUES {
                        return Err(format!(
                            "`in` on '{field}' takes 1 to {MAX_IN_VALUES} values"
                        ));
                    }
                    Value::Array(
                        values
                            .into_iter()
                            .map(|value| scalar(&field, value))
                            .collect::<Result<_, _>>()?,
                    )
                }
                StateFilterOp::Lt | StateFilterOp::Lte | StateFilterOp::Gt | StateFilterOp::Gte => {
                    match scalar(&field, filter.value)? {
                        value @ (Value::String(_) | Value::Number(_)) => value,
                        _ => {
                            return Err(format!(
                                "a range filter on '{field}' compares a string or a number"
                            ));
                        }
                    }
                }
                StateFilterOp::Eq | StateFilterOp::Ne => scalar(&field, filter.value)?,
            };
            Ok(filter)
        })
        .collect()
}

/// A scalar filter value, a date-time in canonical form.
fn scalar(field: &str, value: Value) -> Result<Value, String> {
    match value {
        Value::String(text) => Ok(Value::String(
            runtara_dsl::state::canonical_datetime(&text).unwrap_or(text),
        )),
        value @ (Value::Number(_) | Value::Bool(_)) => Ok(value),
        _ => Err(format!(
            "a filter on '{field}' compares a string, number or boolean"
        )),
    }
}

/// Append one `AND EXISTS (...)` clause per filter over the instance alias
/// `i`. `filters` must be normalised ([`normalize_state_filters`]).
pub(crate) fn push_state_filters(
    query: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    filters: &[StateFilter],
) {
    for filter in filters {
        let field = filter.field.clone();
        if filter.op == StateFilterOp::Exists && filter.value == Value::Bool(false) {
            query
                .push(" AND NOT EXISTS (SELECT 1 FROM instance_state s WHERE s.instance_id = i.instance_id AND s.state ? ")
                .push_bind(field)
                .push(")");
            continue;
        }
        query.push(
            " AND EXISTS (SELECT 1 FROM instance_state s WHERE s.instance_id = i.instance_id AND ",
        );
        match filter.op {
            StateFilterOp::Exists => {
                query.push("s.state ? ").push_bind(field);
            }
            // Containment uses the GIN index.
            StateFilterOp::Eq => {
                query
                    .push("s.state @> jsonb_build_object(")
                    .push_bind(field)
                    .push("::text, ")
                    .push_bind(filter.value.clone())
                    .push("::jsonb)");
            }
            StateFilterOp::In => {
                query.push("(FALSE");
                for value in filter.value.as_array().into_iter().flatten() {
                    query
                        .push(" OR s.state @> jsonb_build_object(")
                        .push_bind(field.clone())
                        .push("::text, ")
                        .push_bind(value.clone())
                        .push("::jsonb)");
                }
                query.push(")");
            }
            StateFilterOp::Ne => {
                query
                    .push("s.state ? ")
                    .push_bind(field.clone())
                    .push(" AND s.state -> ")
                    .push_bind(field)
                    .push(" <> ")
                    .push_bind(filter.value.clone())
                    .push("::jsonb");
            }
            // jsonb orders numbers numerically and strings lexically, but
            // orders across types too: compare only values of the same type.
            StateFilterOp::Lt | StateFilterOp::Lte | StateFilterOp::Gt | StateFilterOp::Gte => {
                let operator = match filter.op {
                    StateFilterOp::Lt => " < ",
                    StateFilterOp::Lte => " <= ",
                    StateFilterOp::Gt => " > ",
                    _ => " >= ",
                };
                query
                    .push("jsonb_typeof(s.state -> ")
                    .push_bind(field.clone())
                    .push(") = jsonb_typeof(")
                    .push_bind(filter.value.clone())
                    .push("::jsonb) AND s.state -> ")
                    .push_bind(field)
                    .push(operator)
                    .push_bind(filter.value.clone())
                    .push("::jsonb");
            }
        }
        query.push(")");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<Vec<StateFilter>, String> {
        parse_state_filters(value.to_string().as_bytes())
    }

    #[test]
    fn filters_parse_and_normalise() {
        let filters = parse(json!([
            {"field": "stage", "op": "eq", "value": "approval"},
            {"field": "dueAt", "op": "lt", "value": "2026-09-29T12:00:00+02:00"},
            {"field": "note", "op": "exists"},
            {"field": "stage", "op": "in", "value": ["a", "b"]}
        ]))
        .unwrap();
        assert_eq!(filters[1].value, json!("2026-09-29T10:00:00.000Z"));
        assert_eq!(filters[2].value, json!(true));
    }

    #[test]
    fn malformed_filters_are_refused() {
        for bad in [
            json!({"field": "a", "op": "eq", "value": 1}),
            json!([{"field": "", "op": "eq", "value": 1}]),
            json!([{"field": "a", "op": "like", "value": 1}]),
            json!([{"field": "a", "op": "eq", "value": {"x": 1}}]),
            json!([{"field": "a", "op": "in", "value": []}]),
            json!([{"field": "a", "op": "in", "value": 3}]),
            json!([{"field": "a", "op": "lt", "value": true}]),
            json!([{"field": "a", "op": "exists", "value": "yes"}]),
            json!([{"field": "a", "op": "eq", "value": 1, "extra": 2}]),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        let many: Vec<Value> = (0..=MAX_STATE_FILTERS)
            .map(|n| json!({"field": format!("f{n}"), "op": "exists"}))
            .collect();
        assert!(parse(Value::Array(many)).is_err());
    }

    #[test]
    fn every_filter_becomes_one_exists_clause() {
        let filters = parse(json!([
            {"field": "stage", "op": "eq", "value": "approval"},
            {"field": "count", "op": "gte", "value": 3},
            {"field": "gone", "op": "exists", "value": false}
        ]))
        .unwrap();
        let mut query = sqlx::QueryBuilder::new("SELECT 1 FROM instances i WHERE TRUE");
        push_state_filters(&mut query, &filters);
        let sql = query.sql();
        assert_eq!(sql.matches(" AND EXISTS (").count(), 2);
        assert_eq!(sql.matches(" AND NOT EXISTS (").count(), 1);
        assert!(sql.contains("s.state @> jsonb_build_object("));
        assert!(sql.contains("jsonb_typeof(s.state -> "));
    }
}
