// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checking and canonicalising run state against a workflow's `stateSchema`.
//!
//! A `SetState` patch is checked field by field: the field must be declared,
//! and a non-null value must match the declared type, `enum` and value
//! `format` (`datetime`, `date`, `email`, `url`). `null` clears a field and is
//! always allowed. Nested objects and arrays are checked by type only.
//!
//! Values are stored in canonical form so that filters compare them
//! correctly: a `datetime` becomes UTC with millisecond precision
//! (`YYYY-MM-DDTHH:MM:SS.sssZ`; a value without a zone is taken as UTC).
//!
//! Shared by the validator (immediate values) and the workflow stdlib (every
//! value, at run time), so both refuse the same inputs.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::SchemaField;
use crate::form::{value_matches_format, value_matches_type};

/// Why a state patch was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateIssue {
    /// Stable code: `STATE_UNKNOWN_FIELD`, `STATE_TYPE_MISMATCH`,
    /// `STATE_ENUM_MISMATCH` or `STATE_FORMAT_MISMATCH`.
    pub code: &'static str,
    /// The state field.
    pub field: String,
    /// Human-readable reason.
    pub message: String,
}

impl std::fmt::Display for StateIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StateIssue {}

/// Check `patch` against `schema` and return it in canonical form.
pub fn check_patch(
    schema: &HashMap<String, SchemaField>,
    patch: &Map<String, Value>,
) -> Result<Map<String, Value>, StateIssue> {
    let mut canonical = Map::with_capacity(patch.len());
    for (field, value) in patch {
        let Some(declared) = schema.get(field) else {
            return Err(StateIssue {
                code: "STATE_UNKNOWN_FIELD",
                field: field.clone(),
                message: format!("state field '{field}' is not declared in stateSchema"),
            });
        };
        canonical.insert(field.clone(), check_value(field, declared, value)?);
    }
    Ok(canonical)
}

/// Check one state value against its declaration and return it in canonical
/// form. `null` is always accepted.
pub fn check_value(
    field: &str,
    declared: &SchemaField,
    value: &Value,
) -> Result<Value, StateIssue> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    if !value_matches_type(value, &declared.field_type) {
        return Err(StateIssue {
            code: "STATE_TYPE_MISMATCH",
            field: field.to_owned(),
            message: format!(
                "state field '{field}' expects {} but got {}",
                type_name(declared),
                json_type(value)
            ),
        });
    }
    if let Some(allowed) = &declared.enum_values
        && !allowed.is_empty()
        && !allowed.contains(value)
    {
        return Err(StateIssue {
            code: "STATE_ENUM_MISMATCH",
            field: field.to_owned(),
            message: format!(
                "state field '{field}' must be one of {}",
                Value::Array(allowed.clone())
            ),
        });
    }
    if let (Some(format), Some(text)) = (declared.format.as_deref(), value.as_str()) {
        let bad = || StateIssue {
            code: "STATE_FORMAT_MISMATCH",
            field: field.to_owned(),
            message: format!("state field '{field}' is not a valid {format}"),
        };
        if format == "datetime" {
            return canonical_datetime(text).map(Value::String).ok_or_else(bad);
        }
        if !value_matches_format(text, format) {
            return Err(bad());
        }
    }
    Ok(value.clone())
}

/// A date-time in canonical UTC form (`YYYY-MM-DDTHH:MM:SS.sssZ`), or `None`
/// when `text` is not an ISO 8601 date-time. A value without a zone is taken
/// as UTC; fractions beyond milliseconds are truncated.
pub fn canonical_datetime(text: &str) -> Option<String> {
    if !value_matches_format(text, "datetime") {
        return None;
    }
    let (date, time) = text.split_once('T')?;
    let year: i64 = date.get(0..4)?.parse().ok()?;
    let month: i64 = date.get(5..7)?.parse().ok()?;
    let day: i64 = date.get(8..10)?.parse().ok()?;

    // Split the zone off the clock time.
    let (clock, offset_minutes) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else if let Some(at) = time.rfind(['+', '-']) {
        let (clock, zone) = time.split_at(at);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let hours: i64 = zone.get(1..3)?.parse().ok()?;
        let minutes: i64 = zone.get(4..6)?.parse().ok()?;
        (clock, sign * (hours * 60 + minutes))
    } else {
        (time, 0)
    };
    let hour: i64 = clock.get(0..2)?.parse().ok()?;
    let minute: i64 = clock.get(3..5)?.parse().ok()?;
    let (second, millis) = match clock.get(6..) {
        None | Some("") => (0, 0),
        Some(rest) => {
            let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
            let second: i64 = whole.parse().ok()?;
            let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
            (second, digits.parse().ok()?)
        }
    };

    let local_ms = ((days_from_civil(year, month, day) * 24 + hour) * 60 + minute) * 60_000
        + second * 1000
        + millis;
    let utc_ms = local_ms - offset_minutes * 60_000;
    let days = utc_ms.div_euclid(86_400_000);
    let in_day = utc_ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    if !(0..=9999).contains(&y) {
        return None;
    }
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000
    ))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

fn type_name(field: &SchemaField) -> &'static str {
    use crate::SchemaFieldType::*;
    match field.field_type {
        String => "a string",
        Integer => "an integer",
        Number => "a number",
        Boolean => "a boolean",
        Array => "an array",
        Object => "an object",
        File => "a file object",
        Connection => "a connection id",
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "an integer",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> HashMap<String, SchemaField> {
        serde_json::from_value(json!({
            "stage": {"type": "string", "enum": ["received", "approval", "done"]},
            "amount": {"type": "number", "format": "currency"},
            "count": {"type": "integer"},
            "dueAt": {"type": "string", "format": "datetime"},
            "day": {"type": "string", "format": "date"},
            "tags": {"type": "array"},
            "meta": {"type": "object"}
        }))
        .unwrap()
    }

    fn patch(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_valid_patch_is_returned_canonical() {
        let out = check_patch(
            &schema(),
            &patch(json!({
                "stage": "approval",
                "amount": 12.5,
                "count": 3,
                "dueAt": "2026-09-29T10:00:00+02:00",
                "day": "2026-09-29",
                "tags": ["a"],
                "meta": {"k": null}
            })),
        )
        .unwrap();
        assert_eq!(out["dueAt"], json!("2026-09-29T08:00:00.000Z"));
        assert_eq!(out["stage"], json!("approval"));
        assert_eq!(out["meta"], json!({"k": null}));
    }

    #[test]
    fn null_clears_any_declared_field() {
        let out = check_patch(&schema(), &patch(json!({"stage": null, "count": null}))).unwrap();
        assert_eq!(out["stage"], Value::Null);
    }

    #[test]
    fn issues_have_stable_codes() {
        let code = |p: Value| check_patch(&schema(), &patch(p)).unwrap_err().code;
        assert_eq!(code(json!({"nope": 1})), "STATE_UNKNOWN_FIELD");
        assert_eq!(code(json!({"count": 1.5})), "STATE_TYPE_MISMATCH");
        assert_eq!(code(json!({"count": "1"})), "STATE_TYPE_MISMATCH");
        assert_eq!(code(json!({"stage": "shipped"})), "STATE_ENUM_MISMATCH");
        assert_eq!(code(json!({"dueAt": "tomorrow"})), "STATE_FORMAT_MISMATCH");
        assert_eq!(code(json!({"day": "2026-02-30"})), "STATE_FORMAT_MISMATCH");
    }

    #[test]
    fn datetimes_canonicalise_to_utc_milliseconds() {
        let c = |s: &str| canonical_datetime(s);
        assert_eq!(
            c("2026-09-29T10:00").as_deref(),
            Some("2026-09-29T10:00:00.000Z")
        );
        assert_eq!(
            c("2026-09-29T10:00:05.123456Z").as_deref(),
            Some("2026-09-29T10:00:05.123Z")
        );
        assert_eq!(
            c("2026-01-01T00:30:00+01:00").as_deref(),
            Some("2025-12-31T23:30:00.000Z")
        );
        assert_eq!(
            c("2024-02-28T23:00:00-02:00").as_deref(),
            Some("2024-02-29T01:00:00.000Z")
        );
        assert_eq!(
            c("2026-09-29T10:00:00.5-00:30").as_deref(),
            Some("2026-09-29T10:30:00.500Z")
        );
        assert_eq!(c("2026-09-29"), None);
        assert_eq!(c("not a date"), None);
        assert_eq!(c("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn canonical_datetimes_sort_like_instants() {
        let a = canonical_datetime("2026-09-29T10:00:00+02:00").unwrap();
        let b = canonical_datetime("2026-09-29T09:00:00Z").unwrap();
        assert!(a < b);
    }
}
