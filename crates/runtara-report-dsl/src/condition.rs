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

use crate::operator_support::{operator_support, parse_operator};
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
        "EQ" | "NE" | "GT" | "LT" | "GTE" | "LTE" | "CONTAINS" | "IN" | "NOT_IN" | "MATCH" => {
            // `MATCH` shares this shape — two operands, the first a field
            // reference — and the SQL builder runs it as
            // `field @@ plainto_tsquery(...)` on a tsvector column. Keep it
            // listed here so a full-text filter validates at save time; the
            // cross-crate parity test below guards the two sets against drift.
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
            // The operator names nothing the SQL builder can emit. Split the
            // hint so an author isn't sent hunting for a typo when the operator
            // is real but only runs on the other report-condition surface: ops
            // like STARTS_WITH / ENDS_WITH / LENGTH evaluate in-memory (row
            // visibility), not in an Object Model source filter that pushes down
            // to SQL. The shared classification keeps this in step with the
            // row-condition evaluator.
            let hint = match parse_operator(&op) {
                Some(known) if operator_support(known.clone()).client_evaluable => Some(
                    "This operator runs only in row-visibility conditions \
                     (visibleWhen/hiddenWhen/disabledWhen), which evaluate in-memory. \
                     Object Model source filters push down to SQL — use CONTAINS for \
                     substring matching, or one of EQ, NE, GT, GTE, LT, LTE, IN, NOT_IN, \
                     IS_DEFINED, IS_EMPTY, IS_NOT_EMPTY, MATCH.",
                ),
                _ => Some(
                    "Use Object Model condition operators such as EQ, NE, GT, GTE, LT, LTE, IN, NOT_IN, CONTAINS, MATCH, IS_DEFINED, IS_EMPTY, or IS_NOT_EMPTY.",
                ),
            };
            return Err(ConditionValidationError {
                code: "UNSUPPORTED_CONDITION_OPERATOR",
                message: format!(
                    "{} {} uses unsupported condition operator '{}'",
                    context, path, condition.op
                ),
                hint,
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

    #[test]
    fn accepts_match_on_known_field() {
        // Full-text `MATCH` runs at the SQL layer as `@@ plainto_tsquery(...)`;
        // the validator must let it through so the filter saves.
        let c = cond("MATCH", vec![json!("body"), json!("search terms")]);
        validate_condition_field_refs(&c, &|f| f == "body", "block").unwrap();
    }

    #[test]
    fn match_unknown_field_is_unknown_not_unsupported() {
        let c = cond("MATCH", vec![json!("nope"), json!("search terms")]);
        let err = validate_condition_field_refs(&c, &|f| f == "body", "block")
            .expect_err("expected unknown-field error");
        assert_eq!(err.code, "UNKNOWN_CONDITION_FIELD");
    }

    #[test]
    fn match_requires_two_arguments() {
        let c = cond("MATCH", vec![json!("body")]);
        let err = validate_condition_field_refs(&c, &|_| true, "block")
            .expect_err("expected arity error");
        assert_eq!(err.code, "INVALID_CONDITION_ARGUMENTS");
    }

    #[test]
    fn client_only_ops_rejected_with_row_visibility_hint() {
        // STARTS_WITH / ENDS_WITH / LENGTH are real operators that run in
        // row-visibility conditions but can't push down to SQL, so a source
        // filter must reject them — and the hint must send the author to the
        // surface where they work rather than implying a typo.
        for op in ["STARTS_WITH", "ENDS_WITH", "LENGTH"] {
            let c = cond(op, vec![json!("field"), json!("value")]);
            let err = validate_condition_field_refs(&c, &|_| true, "block")
                .expect_err("client-only op must be rejected by the source-filter validator");
            assert_eq!(err.code, "UNSUPPORTED_CONDITION_OPERATOR", "{op}");
            let hint = err.hint.expect("client-only rejection should carry a hint");
            assert!(
                hint.contains("row-visibility"),
                "{op} hint should point at row-visibility conditions: {hint}"
            );
        }
    }

    #[test]
    fn truly_unknown_op_keeps_generic_hint() {
        let c = cond("XOR", vec![json!("a"), json!("b")]);
        let err = validate_condition_field_refs(&c, &|_| true, "block")
            .expect_err("expected unsupported-op error");
        assert_eq!(err.code, "UNSUPPORTED_CONDITION_OPERATOR");
        let hint = err.hint.expect("hint present");
        assert!(
            !hint.contains("row-visibility"),
            "a nonexistent operator should get the generic hint, not the row-visibility one: {hint}"
        );
    }

    /// Drift guard: the source-filter surface must recognize exactly the
    /// operators the shared classification marks `sql_pushdown`. Mirrors the
    /// cross-crate `sql_parity_tests` below, but pins against the single
    /// `operator_support` source of truth (available without the `aggregate`
    /// feature).
    #[test]
    fn source_filter_recognition_matches_sql_pushdown_tier() {
        use crate::operator_support::operator_support;
        use runtara_dsl::ConditionOperator::*;

        let all = [
            And,
            Or,
            Not,
            Gt,
            Gte,
            Lt,
            Lte,
            Eq,
            Ne,
            StartsWith,
            EndsWith,
            Contains,
            In,
            NotIn,
            Length,
            IsDefined,
            IsEmpty,
            IsNotEmpty,
            SimilarityGte,
            Match,
            CosineDistanceLte,
            L2DistanceLte,
        ];
        for op in all {
            let wire = serde_json::to_value(&op)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .expect("operator serializes to a wire string");
            // Generous args so a recognized operator doesn't trip an arity gate
            // before the operator itself is classified.
            let condition = cond(&wire, vec![json!("f"), json!("v"), json!(0.5)]);
            let recognized = match validate_condition_field_refs(&condition, &|_| true, "parity") {
                Ok(()) => true,
                Err(e) => e.code != "UNSUPPORTED_CONDITION_OPERATOR",
            };
            assert_eq!(
                recognized,
                operator_support(op.clone()).sql_pushdown,
                "source-filter recognition of {op:?} disagrees with its sql_pushdown tier"
            );
        }
    }
}

/// Cross-crate guard: the operator vocabulary this validator accepts must
/// stay identical to the set the object-store SQL builder can execute.
/// `MATCH` diverging (accepted by the runtime, rejected here) is exactly the
/// bug this pins down. Runs under the default `aggregate` feature, which
/// pulls `runtara-object-store` in as a dependency.
#[cfg(all(test, feature = "aggregate"))]
mod sql_parity_tests {
    use super::*;
    use runtara_dsl::ConditionOperator;
    use runtara_object_store::{
        ColumnDefinition, ColumnType, Condition as SqlCondition, Schema, build_condition_clause,
    };
    use serde_json::json;

    /// Every operator string both layers reason about: the logical operators
    /// plus every `ConditionOperator` variant, in its SCREAMING_SNAKE wire
    /// form. The exhaustive `match` means adding a variant is a compile error
    /// here until the author decides how the two sets should treat it.
    fn all_operator_strings() -> Vec<&'static str> {
        use ConditionOperator::*;
        // Touch every variant so a new one forces this list to be revisited.
        let variant_wire = |op: ConditionOperator| -> &'static str {
            match op {
                And => "AND",
                Or => "OR",
                Not => "NOT",
                Gt => "GT",
                Gte => "GTE",
                Lt => "LT",
                Lte => "LTE",
                Eq => "EQ",
                Ne => "NE",
                StartsWith => "STARTS_WITH",
                EndsWith => "ENDS_WITH",
                Contains => "CONTAINS",
                In => "IN",
                NotIn => "NOT_IN",
                Length => "LENGTH",
                IsDefined => "IS_DEFINED",
                IsEmpty => "IS_EMPTY",
                IsNotEmpty => "IS_NOT_EMPTY",
                SimilarityGte => "SIMILARITY_GTE",
                Match => "MATCH",
                CosineDistanceLte => "COSINE_DISTANCE_LTE",
                L2DistanceLte => "L2_DISTANCE_LTE",
            }
        };
        [
            And,
            Or,
            Not,
            Gt,
            Gte,
            Lt,
            Lte,
            Eq,
            Ne,
            StartsWith,
            EndsWith,
            Contains,
            In,
            NotIn,
            Length,
            IsDefined,
            IsEmpty,
            IsNotEmpty,
            SimilarityGte,
            Match,
            CosineDistanceLte,
            L2DistanceLte,
        ]
        .into_iter()
        .map(variant_wire)
        .collect()
    }

    /// True when the save-time validator treats `op` as a known operator
    /// (i.e. any failure other than "unsupported operator"). Args are shaped
    /// generously so recognized operators don't trip an arity gate before the
    /// operator itself is classified.
    fn validator_recognizes(op: &str) -> bool {
        let args = vec![json!("f"), json!("v"), json!(0.5)];
        let condition = Condition {
            op: op.to_string(),
            arguments: Some(args),
        };
        match validate_condition_field_refs(&condition, &|_| true, "parity") {
            Ok(()) => true,
            Err(e) => e.code != "UNSUPPORTED_CONDITION_OPERATOR",
        }
    }

    /// True when the object-store SQL builder dispatches `op` to a real arm
    /// (i.e. any failure other than its "Unsupported operation" fallback).
    fn sql_builder_recognizes(op: &str) -> bool {
        let schema = Schema {
            id: "parity".to_string(),
            name: "parity".to_string(),
            description: None,
            table_name: "parity".to_string(),
            columns: vec![ColumnDefinition::new("f", ColumnType::String)],
            indexes: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let condition = SqlCondition::new(op, vec![json!("f"), json!("v"), json!(0.5)]);
        let mut offset = 1;
        match build_condition_clause(&condition, &mut offset, &schema) {
            Ok(_) => true,
            Err(message) => !message.contains("Unsupported operation"),
        }
    }

    #[test]
    fn accepted_set_equals_supported_set() {
        let mut mismatched = Vec::new();
        for op in all_operator_strings() {
            let accepted = validator_recognizes(op);
            let supported = sql_builder_recognizes(op);
            if accepted != supported {
                mismatched.push(format!(
                    "{op}: validator_accepts={accepted}, sql_supports={supported}"
                ));
            }
        }
        assert!(
            mismatched.is_empty(),
            "condition operator vocabularies diverged between the save-time \
             validator and the object-store SQL builder: {mismatched:?}"
        );
    }

    #[test]
    fn parity_probe_actually_reaches_match() {
        // Guard the guard: confirm both probes classify `MATCH` as recognized,
        // so a future refactor can't make the parity test vacuously pass by
        // having both probes silently stop recognizing it.
        assert!(validator_recognizes("MATCH"));
        assert!(sql_builder_recognizes("MATCH"));
    }
}
