//! Wire-shape `Condition` used by `ReportSource`.
//!
//! Object Model conditions on the request boundary look like
//! `{ "op": "EQ", "arguments": [...] }`. This is intentionally the same
//! shape as `runtara_server::api::dto::object_model::Condition` — that type
//! re-exports from here once the server takes a dependency on this crate,
//! so there is one definition.
//!
//! This module also owns the field-ref validator that the server and any
//! future FE save-time check both call to confirm that condition operands
//! reference known schema fields. The validator takes a closure for
//! field-known lookup so callers wire their own schema source.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<Value>>,
}

/// Parse a JSON value into a `Condition` if it has the right shape
/// (`op` + `arguments`). Used to recurse into nested condition objects
/// inside `AND` / `OR` / `NOT` arguments.
pub fn condition_from_value(value: &Value) -> Option<Condition> {
    let object = value.as_object()?;
    if !(object.contains_key("op") || object.contains_key("arguments")) {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

/// Validation failure from [`validate_condition_field_refs`]. `code` is a
/// stable SCREAMING_SNAKE_CASE identifier; `message` carries the
/// caller-supplied context (block id, source path, etc.); `hint` is a
/// fixed-string suggestion.
#[derive(Debug, Clone)]
pub struct ConditionValidationError {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<&'static str>,
}

/// Validate that every field reference inside a legacy-shape
/// `Condition` (the SQL-bound `{op, arguments}` form) names a known
/// field. Returns `Ok(())` on success.
///
/// The caller passes:
/// - `context`: opaque string baked into error messages (e.g.
///   `"block 'orders'"`)
/// - `is_known_field`: predicate over field paths
///
/// Errors land with stable codes:
/// - `INVALID_CONDITION_ARGUMENTS` — missing / wrong-arity arguments
/// - `INVALID_CONDITION_FIELD` — first arg not a non-empty string
/// - `UNKNOWN_CONDITION_FIELD` — field name not known to the predicate
/// - `UNSUPPORTED_CONDITION_OPERATOR` — op not in the Object Model set
pub fn validate_condition_field_refs(
    condition: &Condition,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
) -> Result<(), ConditionValidationError> {
    validate_condition_field_refs_at(condition, is_known_field, context, "condition")
}

fn validate_condition_field_refs_at(
    condition: &Condition,
    is_known_field: &dyn Fn(&str) -> bool,
    context: &str,
    path: &str,
) -> Result<(), ConditionValidationError> {
    let op = condition.op.to_ascii_uppercase();
    let args = condition.arguments.as_deref().ok_or_else(|| ConditionValidationError {
        code: "INVALID_CONDITION_ARGUMENTS",
        message: format!(
            "{} {} operator '{}' requires arguments",
            context, path, condition.op
        ),
        hint: Some("Conditions must use { op, arguments } with the field name as the first operand for comparison operators."),
    })?;

    match op.as_str() {
        "AND" | "OR" => {
            if args.is_empty() {
                return Err(ConditionValidationError {
                    code: "INVALID_CONDITION_ARGUMENTS",
                    message: format!(
                        "{} {} operator '{}' requires at least one condition argument",
                        context, path, condition.op
                    ),
                    hint: Some("AND/OR arguments must be condition objects."),
                });
            }
            for (index, argument) in args.iter().enumerate() {
                let child =
                    condition_from_value(argument).ok_or_else(|| ConditionValidationError {
                        code: "INVALID_CONDITION_ARGUMENTS",
                        message: format!(
                            "{} {} operator '{}' argument {} must be a condition object",
                            context, path, condition.op, index
                        ),
                        hint: Some("Use nested condition objects inside logical operators."),
                    })?;
                validate_condition_field_refs_at(
                    &child,
                    is_known_field,
                    context,
                    &format!("{path}.arguments[{index}]"),
                )?;
            }
        }
        "NOT" => {
            if args.len() != 1 {
                return Err(ConditionValidationError {
                    code: "INVALID_CONDITION_ARGUMENTS",
                    message: format!(
                        "{} {} operator '{}' requires exactly one condition argument",
                        context, path, condition.op
                    ),
                    hint: Some("NOT must wrap exactly one condition object."),
                });
            }
            let child = condition_from_value(&args[0]).ok_or_else(|| ConditionValidationError {
                code: "INVALID_CONDITION_ARGUMENTS",
                message: format!(
                    "{} {} operator '{}' argument 0 must be a condition object",
                    context, path, condition.op
                ),
                hint: Some("NOT must wrap exactly one condition object."),
            })?;
            validate_condition_field_refs_at(
                &child,
                is_known_field,
                context,
                &format!("{path}.arguments[0]"),
            )?;
        }
        "EQ" | "NE" | "GT" | "LT" | "GTE" | "LTE" | "CONTAINS" | "IN" | "NOT_IN" => {
            validate_condition_arg_count(context, path, &condition.op, args, 2)?;
            validate_condition_field_arg(context, path, &condition.op, args, is_known_field)?;
        }
        "IS_EMPTY" | "IS_NOT_EMPTY" | "IS_DEFINED" => {
            validate_condition_arg_count(context, path, &condition.op, args, 1)?;
            validate_condition_field_arg(context, path, &condition.op, args, is_known_field)?;
        }
        "SIMILARITY_GTE" | "COSINE_DISTANCE_LTE" | "L2_DISTANCE_LTE" => {
            validate_condition_arg_count(context, path, &condition.op, args, 3)?;
            validate_condition_field_arg(context, path, &condition.op, args, is_known_field)?;
        }
        _ => {
            return Err(ConditionValidationError {
                code: "UNSUPPORTED_CONDITION_OPERATOR",
                message: format!(
                    "{} {} uses unsupported condition operator '{}'",
                    context, path, condition.op
                ),
                hint: Some(
                    "Use Object Model condition operators such as EQ, NE, GT, GTE, LT, LTE, IN, NOT_IN, CONTAINS, IS_DEFINED, IS_EMPTY, or IS_NOT_EMPTY.",
                ),
            });
        }
    }

    Ok(())
}

fn validate_condition_arg_count(
    context: &str,
    path: &str,
    op: &str,
    args: &[Value],
    expected: usize,
) -> Result<(), ConditionValidationError> {
    if args.len() != expected {
        return Err(ConditionValidationError {
            code: "INVALID_CONDITION_ARGUMENTS",
            message: format!(
                "{} {} operator '{}' requires exactly {} argument{}",
                context,
                path,
                op,
                expected,
                if expected == 1 { "" } else { "s" }
            ),
            hint: Some("Check the condition operator arity and operand order."),
        });
    }
    Ok(())
}

fn validate_condition_field_arg(
    context: &str,
    path: &str,
    op: &str,
    args: &[Value],
    is_known_field: &dyn Fn(&str) -> bool,
) -> Result<(), ConditionValidationError> {
    let field = args
        .first()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .ok_or_else(|| ConditionValidationError {
            code: "INVALID_CONDITION_FIELD",
            message: format!(
                "{} {} operator '{}' first argument must be a non-empty field name",
                context, path, op
            ),
            hint: Some("The first operand must be a field available from the report source."),
        })?;
    if !is_known_field(field) {
        return Err(ConditionValidationError {
            code: "UNKNOWN_CONDITION_FIELD",
            message: format!("{} {} references unknown field '{}'", context, path, field),
            hint: Some(
                "Use a field from the source schema, joined schema alias, dataset output, workflow runtime entity, or system entity for this condition.",
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cond(op: &str, args: Vec<Value>) -> Condition {
        Condition {
            op: op.to_string(),
            arguments: Some(args),
        }
    }

    #[test]
    fn rejects_unsupported_op() {
        let c = cond("XOR", vec![json!("a"), json!(1)]);
        let err = validate_condition_field_refs(&c, &|_| true, "block")
            .expect_err("expected unsupported-op error");
        assert_eq!(err.code, "UNSUPPORTED_CONDITION_OPERATOR");
    }

    #[test]
    fn rejects_unknown_field() {
        let c = cond("EQ", vec![json!("nope"), json!("v")]);
        let known = |f: &str| f == "yep";
        let err = validate_condition_field_refs(&c, &known, "block")
            .expect_err("expected unknown-field error");
        assert_eq!(err.code, "UNKNOWN_CONDITION_FIELD");
    }

    #[test]
    fn rejects_wrong_arity() {
        let c = cond("EQ", vec![json!("yep")]);
        let err = validate_condition_field_refs(&c, &|_| true, "block")
            .expect_err("expected arity error");
        assert_eq!(err.code, "INVALID_CONDITION_ARGUMENTS");
    }

    #[test]
    fn accepts_nested_and_or_not() {
        let inner = cond("EQ", vec![json!("yep"), json!("v")]);
        let nested = cond(
            "AND",
            vec![
                serde_json::to_value(&inner).unwrap(),
                serde_json::to_value(cond("NOT", vec![serde_json::to_value(&inner).unwrap()]))
                    .unwrap(),
            ],
        );
        validate_condition_field_refs(&nested, &|f| f == "yep", "block").unwrap();
    }
}
