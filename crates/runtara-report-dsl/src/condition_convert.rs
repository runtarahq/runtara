//! Lossless conversion between the two report condition wire-shapes.
//!
//! Reports historically grew two structurally incompatible JSON encodings for
//! "a condition", and which one an author had to write depended solely on
//! where it attached:
//!
//! - **Source filters** ([`Condition`]) are flat and positional:
//!   `{ "op": "EQ", "arguments": ["status", "active"] }`. The operator is a
//!   bare string and the field name is the first operand.
//! - **Row visibility** (`visibleWhen` / `hiddenWhen` / `disabledWhen`, typed
//!   as [`ConditionExpression`]) is internally tagged with typed operands:
//!   `{ "type": "operation", "op": "EQ",
//!      "arguments": [{"valueType":"reference","value":"status"},
//!                    {"valueType":"immediate","value":"active"}] }`.
//!
//! Neither shape deserializes as the other, so authors, the MCP schema, and
//! every consumer had to special-case by tree position. This module converts
//! between them so each location can accept either encoding and normalize to
//! the one its downstream validators and evaluators already expect.
//!
//! # Why the conversion is partial
//!
//! The two vocabularies are not subsets of each other, so a total conversion
//! would have to invent or discard meaning. Instead every operand that has no
//! counterpart is rejected with a stable code rather than silently degraded:
//!
//! - Source filters accept operands the in-memory evaluator has no notion of —
//!   filter bindings (`{"filter":"category","path":"value"}`) and same-store
//!   subqueries (`{"subquery":{...}}`), both of which only mean something once
//!   pushed down to SQL.
//! - Condition expressions carry operands the flat form cannot encode —
//!   composite and template mapping values, references outside the first
//!   position, and `type` / `default` hints on a reference.
//!
//! On the intersection of the two the conversion round-trips exactly, in both
//! directions, for every operator. The argument layout is read from
//! [`operand_shape`] rather than restated here, so the converter cannot drift
//! away from the save-time validators that share it.

use crate::condition::{Condition, condition_from_value};
use crate::operator_support::{OperandShape, operand_shape, parse_operator};
use runtara_dsl::{
    ConditionArgument, ConditionExpression, ConditionOperation, ImmediateValue, MappingValue,
    ReferenceValue,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// A condition that cannot be expressed in the target wire-shape.
///
/// `code` is a stable SCREAMING_SNAKE_CASE identifier, matching the convention
/// [`crate::ConditionValidationError`] uses, so callers can branch on the
/// reason without parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionConversionError {
    pub code: &'static str,
    pub message: String,
}

impl ConditionConversionError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConditionConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConditionConversionError {}

/// Convert a flat source-filter [`Condition`] into the tagged
/// [`ConditionExpression`] the row-visibility evaluator consumes.
///
/// The first operand of a comparison becomes a `reference`, the remaining
/// operands become `immediate` values, and logical connectives recurse. Fails
/// when the operator is unknown, the argument count disagrees with
/// [`operand_shape`], or an operand is a filter binding or subquery — none of
/// which the in-memory evaluator can execute.
pub fn condition_to_expression(
    condition: &Condition,
) -> Result<ConditionExpression, ConditionConversionError> {
    condition_to_expression_at(condition, "condition")
}

fn condition_to_expression_at(
    condition: &Condition,
    path: &str,
) -> Result<ConditionExpression, ConditionConversionError> {
    let op = parse_operator(&condition.op).ok_or_else(|| {
        ConditionConversionError::new(
            "UNSUPPORTED_CONDITION_OPERATOR",
            format!("{path} uses unknown condition operator '{}'", condition.op),
        )
    })?;

    let args = condition.arguments.as_deref().unwrap_or(&[]);
    let shape = operand_shape(op.clone());
    check_arity(&shape, args.len(), &condition.op, path)?;

    let arguments = match shape {
        OperandShape::Logical { .. } => args
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                let child_path = format!("{path}.arguments[{index}]");
                let child = condition_from_value(argument).ok_or_else(|| {
                    ConditionConversionError::new(
                        "INVALID_CONDITION_ARGUMENTS",
                        format!("{child_path} must be a nested condition object"),
                    )
                })?;
                condition_to_expression_at(&child, &child_path)
                    .map(|expression| ConditionArgument::Expression(Box::new(expression)))
            })
            .collect::<Result<Vec<_>, _>>()?,
        OperandShape::FieldFirst { .. } => {
            let mut arguments = Vec::with_capacity(args.len());
            for (index, argument) in args.iter().enumerate() {
                let child_path = format!("{path}.arguments[{index}]");
                reject_sql_only_operand(argument, &child_path)?;
                arguments.push(if index == 0 {
                    let field = argument
                        .as_str()
                        .map(str::trim)
                        .filter(|field| !field.is_empty())
                        .ok_or_else(|| {
                            ConditionConversionError::new(
                                "INVALID_CONDITION_FIELD",
                                format!("{child_path} must be a non-empty field name"),
                            )
                        })?;
                    ConditionArgument::Value(MappingValue::Reference(ReferenceValue {
                        value: field.to_string(),
                        type_hint: None,
                        default: None,
                    }))
                } else {
                    ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                        value: argument.clone(),
                    }))
                });
            }
            arguments
        }
    };

    Ok(ConditionExpression::Operation(ConditionOperation {
        op,
        arguments,
    }))
}

/// Convert a tagged [`ConditionExpression`] into the flat [`Condition`] the
/// object-store SQL builder consumes.
///
/// The inverse of [`condition_to_expression`]: a leading `reference` operand
/// collapses to its bare path, `immediate` operands unwrap to their literal,
/// and nested expressions recurse. Fails on anything the flat form cannot
/// encode — a bare value expression, composite or template operands, a
/// reference carrying a `type` or `default`, or a reference outside the first
/// position.
pub fn expression_to_condition(
    expression: &ConditionExpression,
) -> Result<Condition, ConditionConversionError> {
    expression_to_condition_at(expression, "condition")
}

fn expression_to_condition_at(
    expression: &ConditionExpression,
    path: &str,
) -> Result<Condition, ConditionConversionError> {
    let operation = match expression {
        ConditionExpression::Operation(operation) => operation,
        ConditionExpression::Value(_) => {
            return Err(ConditionConversionError::new(
                "INVALID_CONDITION_ARGUMENTS",
                format!(
                    "{path} must be a condition operation; a bare value has no source-filter form"
                ),
            ));
        }
    };

    let wire = operator_wire_form(&operation.op)?;
    let shape = operand_shape(operation.op.clone());
    check_arity(&shape, operation.arguments.len(), &wire, path)?;

    let arguments = operation
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let child_path = format!("{path}.arguments[{index}]");
            match (&shape, argument) {
                (OperandShape::Logical { .. }, ConditionArgument::Expression(expression)) => {
                    expression_to_condition_at(expression, &child_path)
                        .and_then(|child| to_json(&child, &child_path))
                }
                (OperandShape::Logical { .. }, ConditionArgument::Value(_)) => {
                    Err(ConditionConversionError::new(
                        "INVALID_CONDITION_ARGUMENTS",
                        format!("{child_path} must be a nested condition expression"),
                    ))
                }
                // A nested expression under a comparison operator is a shape the
                // flat form has no slot for: its operands are values, not
                // conditions.
                (OperandShape::FieldFirst { .. }, ConditionArgument::Expression(_)) => {
                    Err(ConditionConversionError::new(
                        "INVALID_CONDITION_ARGUMENTS",
                        format!("{child_path} must be a value operand, not a nested expression"),
                    ))
                }
                (OperandShape::FieldFirst { .. }, ConditionArgument::Value(value)) => {
                    mapping_value_to_json(value, index == 0, &child_path)
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Condition {
        op: wire,
        arguments: Some(arguments),
    })
}

/// Flatten one operand. `expects_field` marks the leading position, which must
/// hold a reference (the field name) and nothing else.
fn mapping_value_to_json(
    value: &MappingValue,
    expects_field: bool,
    path: &str,
) -> Result<Value, ConditionConversionError> {
    match value {
        MappingValue::Reference(reference) => {
            if !expects_field {
                return Err(ConditionConversionError::new(
                    "UNCONVERTIBLE_CONDITION_OPERAND",
                    format!(
                        "{path} is a reference outside the leading operand; source filters can \
                         only reference a field in the first position"
                    ),
                ));
            }
            // A `type` hint or `default` is resolved by the in-memory evaluator
            // and has nowhere to live in a bare field name, so dropping it
            // would change what the condition means.
            if reference.type_hint.is_some() || reference.default.is_some() {
                return Err(ConditionConversionError::new(
                    "UNCONVERTIBLE_CONDITION_OPERAND",
                    format!(
                        "{path} carries a reference 'type' or 'default', which a source filter \
                         cannot express"
                    ),
                ));
            }
            let field = reference.value.trim();
            if field.is_empty() {
                return Err(ConditionConversionError::new(
                    "INVALID_CONDITION_FIELD",
                    format!("{path} must be a non-empty field reference"),
                ));
            }
            Ok(Value::String(field.to_string()))
        }
        MappingValue::Immediate(immediate) => {
            if expects_field {
                return Err(ConditionConversionError::new(
                    "INVALID_CONDITION_FIELD",
                    format!("{path} must be a field reference, not an immediate value"),
                ));
            }
            Ok(immediate.value.clone())
        }
        MappingValue::Composite(_) | MappingValue::Template(_) => {
            Err(ConditionConversionError::new(
                "UNCONVERTIBLE_CONDITION_OPERAND",
                format!(
                    "{path} is a composite or template value, which a source filter cannot express"
                ),
            ))
        }
    }
}

/// Reject the two source-filter operand kinds that exist only to be pushed
/// down to SQL and so have no in-memory counterpart.
fn reject_sql_only_operand(argument: &Value, path: &str) -> Result<(), ConditionConversionError> {
    let Some(object) = argument.as_object() else {
        return Ok(());
    };
    let kind = if object.contains_key("subquery") {
        "a subquery"
    } else if object.contains_key("filter") {
        "a report filter binding"
    } else {
        return Ok(());
    };
    Err(ConditionConversionError::new(
        "UNCONVERTIBLE_CONDITION_OPERAND",
        format!(
            "{path} is {kind}, which resolves against the report's filters at query time and has \
             no row-visibility equivalent"
        ),
    ))
}

fn check_arity(
    shape: &OperandShape,
    got: usize,
    op: &str,
    path: &str,
) -> Result<(), ConditionConversionError> {
    let ok = match shape {
        OperandShape::Logical { arity: None } => got >= 1,
        OperandShape::Logical { arity: Some(arity) } | OperandShape::FieldFirst { arity } => {
            got == *arity
        }
    };
    if ok {
        return Ok(());
    }
    let expected = match shape {
        OperandShape::Logical { arity: None } => "at least 1".to_string(),
        OperandShape::Logical { arity: Some(arity) } | OperandShape::FieldFirst { arity } => {
            arity.to_string()
        }
    };
    Err(ConditionConversionError::new(
        "INVALID_CONDITION_ARGUMENTS",
        format!("{path} operator '{op}' requires {expected} argument(s), got {got}"),
    ))
}

fn operator_wire_form(
    op: &runtara_dsl::ConditionOperator,
) -> Result<String, ConditionConversionError> {
    serde_json::to_value(op)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| {
            ConditionConversionError::new(
                "UNSUPPORTED_CONDITION_OPERATOR",
                format!("operator {op:?} has no wire form"),
            )
        })
}

fn to_json<T: serde::Serialize>(value: &T, path: &str) -> Result<Value, ConditionConversionError> {
    serde_json::to_value(value).map_err(|err| {
        ConditionConversionError::new(
            "INVALID_CONDITION_ARGUMENTS",
            format!("{path} could not be serialized: {err}"),
        )
    })
}

// ============================================================================
// serde adapters
// ============================================================================

/// True when a JSON object is written in the tagged expression shape.
///
/// The discriminator is unambiguous: [`Condition`] has only `op` and
/// `arguments`, while every [`ConditionExpression`] variant is tagged with
/// `type`.
fn looks_like_expression(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("type"))
}

/// Deserialize a source-filter condition, accepting either wire-shape.
///
/// An expression-shaped payload is normalized down to the flat form, so
/// everything downstream — the field-ref validator, the filter resolver, the
/// SQL builder — keeps seeing the single shape it already handles.
pub fn deserialize_condition_opt<'de, D>(deserializer: D) -> Result<Option<Condition>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    if looks_like_expression(&value) {
        let expression: ConditionExpression =
            serde_json::from_value(value).map_err(D::Error::custom)?;
        return expression_to_condition(&expression)
            .map(Some)
            .map_err(D::Error::custom);
    }

    serde_json::from_value(value)
        .map(Some)
        .map_err(D::Error::custom)
}

/// Deserialize a row-visibility condition, accepting either wire-shape.
///
/// A flat payload is normalized up to the tagged form, so the row-condition
/// validator and the shared in-memory evaluator keep seeing the single shape
/// they already handle.
pub fn deserialize_condition_expression_opt<'de, D>(
    deserializer: D,
) -> Result<Option<ConditionExpression>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    if !looks_like_expression(&value) && condition_from_value(&value).is_some() {
        let condition: Condition = serde_json::from_value(value).map_err(D::Error::custom)?;
        return condition_to_expression(&condition)
            .map(Some)
            .map_err(D::Error::custom);
    }

    serde_json::from_value(value)
        .map(Some)
        .map_err(D::Error::custom)
}

// ============================================================================
// JSON Schema
// ============================================================================

/// Advertise both condition wire-shapes for a field that accepts either.
///
/// The published report schema is what `validate_report` (syntax mode) checks
/// raw JSON against, so it has to describe what the DTO actually accepts. A
/// `deserialize_with` adapter is invisible to `schemars`, and without these
/// helpers the schema would reject payloads the server then happily saves.
#[cfg(feature = "json-schema")]
fn any_condition_shape_schema(
    generator: &mut schemars::SchemaGenerator,
    description: &str,
) -> schemars::Schema {
    let flat = generator.subschema_for::<Condition>().to_value();
    let expression = generator.subschema_for::<ConditionExpression>().to_value();
    schemars::json_schema!({
        "description": description,
        "anyOf": [flat, expression, { "type": "null" }]
    })
}

/// Schema for a source-filter condition: canonically flat, expression shape
/// accepted and normalized on load.
#[cfg(feature = "json-schema")]
pub fn condition_opt_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    any_condition_shape_schema(
        generator,
        "Object Model source filter. Canonically the flat { op, arguments } form with the \
         field name as the first operand; the tagged ConditionExpression form is also \
         accepted and normalized to the flat form on save. Filter bindings ({ filter, path }) \
         and subquery operands exist only in the flat form.",
    )
}

/// Schema for a row-visibility condition: canonically the tagged expression,
/// flat shape accepted and normalized on load.
#[cfg(feature = "json-schema")]
pub fn condition_expression_opt_schema(
    generator: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    any_condition_shape_schema(
        generator,
        "Row-level visibility condition evaluated against the rendered row. Canonically the \
         tagged ConditionExpression form; the flat { op, arguments } form used by source \
         filters is also accepted and normalized to the tagged form on save.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_support::operator_support;
    use runtara_dsl::ConditionOperator;
    use serde_json::json;

    /// Every operator, so a new variant has to be considered here too. Kept in
    /// the same order as the sibling lists in `condition.rs` and
    /// `operator_support.rs`.
    fn all_operators() -> Vec<ConditionOperator> {
        use ConditionOperator::*;
        vec![
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
    }

    fn wire(op: &ConditionOperator) -> String {
        serde_json::to_value(op)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .expect("operator serializes to a wire string")
    }

    /// A representative flat condition for `op`, shaped to its declared operand
    /// layout. Logical connectives wrap a trivial leaf.
    fn sample_condition(op: &ConditionOperator) -> Condition {
        let leaf = json!({ "op": "EQ", "arguments": ["status", "active"] });
        let arguments = match operand_shape(op.clone()) {
            OperandShape::Logical { arity: None } => vec![leaf.clone(), leaf],
            OperandShape::Logical { arity: Some(_) } => vec![leaf],
            OperandShape::FieldFirst { arity } => {
                let mut args = vec![json!("status")];
                // Distinct literals so a converter that reorders or duplicates
                // operands can't round-trip by accident.
                args.extend((1..arity).map(|index| json!(format!("operand-{index}"))));
                args
            }
        };
        Condition {
            op: wire(op),
            arguments: Some(arguments),
        }
    }

    #[test]
    fn every_operator_round_trips_from_the_flat_shape() {
        for op in all_operators() {
            let original = sample_condition(&op);
            let expression = condition_to_expression(&original)
                .unwrap_or_else(|e| panic!("{op:?} flat -> expression: {e}"));
            let back = expression_to_condition(&expression)
                .unwrap_or_else(|e| panic!("{op:?} expression -> flat: {e}"));
            assert_eq!(
                serde_json::to_value(&original).unwrap(),
                serde_json::to_value(&back).unwrap(),
                "{op:?} did not survive a flat -> expression -> flat round trip"
            );
        }
    }

    #[test]
    fn every_operator_round_trips_from_the_expression_shape() {
        for op in all_operators() {
            let original = condition_to_expression(&sample_condition(&op))
                .unwrap_or_else(|e| panic!("{op:?} fixture: {e}"));
            let flat = expression_to_condition(&original)
                .unwrap_or_else(|e| panic!("{op:?} expression -> flat: {e}"));
            let back = condition_to_expression(&flat)
                .unwrap_or_else(|e| panic!("{op:?} flat -> expression: {e}"));
            assert_eq!(
                serde_json::to_value(&original).unwrap(),
                serde_json::to_value(&back).unwrap(),
                "{op:?} did not survive an expression -> flat -> expression round trip"
            );
        }
    }

    /// The converted output must be exactly the encoding each surface documents,
    /// not merely something that round-trips.
    #[test]
    fn produces_the_documented_encodings() {
        let flat = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![json!("status"), json!("active")]),
        };
        let expression = condition_to_expression(&flat).unwrap();
        assert_eq!(
            serde_json::to_value(&expression).unwrap(),
            json!({
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            })
        );
        assert_eq!(
            serde_json::to_value(expression_to_condition(&expression).unwrap()).unwrap(),
            json!({ "op": "EQ", "arguments": ["status", "active"] })
        );
    }

    #[test]
    fn nested_logical_conditions_convert_both_ways() {
        let flat: Condition = serde_json::from_value(json!({
            "op": "AND",
            "arguments": [
                { "op": "EQ", "arguments": ["status", "active"] },
                { "op": "NOT", "arguments": [
                    { "op": "IS_EMPTY", "arguments": ["notes"] }
                ]}
            ]
        }))
        .unwrap();
        let expression = condition_to_expression(&flat).unwrap();
        let back = expression_to_condition(&expression).unwrap();
        assert_eq!(
            serde_json::to_value(&flat).unwrap(),
            serde_json::to_value(&back).unwrap()
        );
    }

    /// Lowercase operators are accepted by the source-filter validator, so the
    /// converter has to normalize rather than reject them.
    #[test]
    fn lowercase_operators_normalize_to_wire_form() {
        let flat = Condition {
            op: "eq".to_string(),
            arguments: Some(vec![json!("status"), json!("active")]),
        };
        let expression = condition_to_expression(&flat).unwrap();
        assert_eq!(
            expression_to_condition(&expression).unwrap().op,
            "EQ",
            "conversion should emit the canonical wire form"
        );
    }

    // --- rejections: operands with no counterpart in the target shape -------

    #[test]
    fn rejects_unknown_operator() {
        let flat = Condition {
            op: "XOR".to_string(),
            arguments: Some(vec![json!("a"), json!("b")]),
        };
        assert_eq!(
            condition_to_expression(&flat).unwrap_err().code,
            "UNSUPPORTED_CONDITION_OPERATOR"
        );
    }

    #[test]
    fn rejects_filter_binding_operand() {
        let flat = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![
                json!("status"),
                json!({"filter": "s", "path": "value"}),
            ]),
        };
        assert_eq!(
            condition_to_expression(&flat).unwrap_err().code,
            "UNCONVERTIBLE_CONDITION_OPERAND"
        );
    }

    #[test]
    fn rejects_subquery_operand() {
        let flat = Condition {
            op: "IN".to_string(),
            arguments: Some(vec![
                json!("id"),
                json!({"subquery": {"schema": "Order", "select": "id"}}),
            ]),
        };
        assert_eq!(
            condition_to_expression(&flat).unwrap_err().code,
            "UNCONVERTIBLE_CONDITION_OPERAND"
        );
    }

    #[test]
    fn rejects_bare_value_expression() {
        let expression: ConditionExpression = serde_json::from_value(json!({
            "type": "value",
            "valueType": "reference",
            "value": "status"
        }))
        .unwrap();
        assert_eq!(
            expression_to_condition(&expression).unwrap_err().code,
            "INVALID_CONDITION_ARGUMENTS"
        );
    }

    #[test]
    fn rejects_reference_outside_the_leading_operand() {
        let expression: ConditionExpression = serde_json::from_value(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "status" },
                { "valueType": "reference", "value": "other" }
            ]
        }))
        .unwrap();
        assert_eq!(
            expression_to_condition(&expression).unwrap_err().code,
            "UNCONVERTIBLE_CONDITION_OPERAND"
        );
    }

    #[test]
    fn rejects_reference_with_type_or_default() {
        for extra in [json!({"type": "integer"}), json!({"default": 0})] {
            let mut reference = json!({ "valueType": "reference", "value": "status" });
            for (key, value) in extra.as_object().unwrap() {
                reference[key] = value.clone();
            }
            let expression: ConditionExpression = serde_json::from_value(json!({
                "type": "operation",
                "op": "EQ",
                "arguments": [reference, { "valueType": "immediate", "value": 1 }]
            }))
            .unwrap();
            assert_eq!(
                expression_to_condition(&expression).unwrap_err().code,
                "UNCONVERTIBLE_CONDITION_OPERAND"
            );
        }
    }

    #[test]
    fn rejects_composite_and_template_operands() {
        for operand in [
            json!({ "valueType": "composite", "value": { "a": { "valueType": "immediate", "value": 1 } } }),
            json!({ "valueType": "template", "value": "{{ row.status }}" }),
        ] {
            let expression: ConditionExpression = serde_json::from_value(json!({
                "type": "operation",
                "op": "EQ",
                "arguments": [{ "valueType": "reference", "value": "status" }, operand]
            }))
            .unwrap();
            assert_eq!(
                expression_to_condition(&expression).unwrap_err().code,
                "UNCONVERTIBLE_CONDITION_OPERAND"
            );
        }
    }

    #[test]
    fn rejects_leading_immediate_where_a_field_belongs() {
        let expression: ConditionExpression = serde_json::from_value(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "immediate", "value": "status" },
                { "valueType": "immediate", "value": "active" }
            ]
        }))
        .unwrap();
        assert_eq!(
            expression_to_condition(&expression).unwrap_err().code,
            "INVALID_CONDITION_FIELD"
        );
    }

    #[test]
    fn rejects_wrong_arity_in_both_directions() {
        let flat = Condition {
            op: "EQ".to_string(),
            arguments: Some(vec![json!("status")]),
        };
        assert_eq!(
            condition_to_expression(&flat).unwrap_err().code,
            "INVALID_CONDITION_ARGUMENTS"
        );

        let expression: ConditionExpression = serde_json::from_value(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [{ "valueType": "reference", "value": "status" }]
        }))
        .unwrap();
        assert_eq!(
            expression_to_condition(&expression).unwrap_err().code,
            "INVALID_CONDITION_ARGUMENTS"
        );
    }

    #[test]
    fn rejects_non_condition_argument_under_a_logical_operator() {
        let flat = Condition {
            op: "AND".to_string(),
            arguments: Some(vec![json!("status")]),
        };
        assert_eq!(
            condition_to_expression(&flat).unwrap_err().code,
            "INVALID_CONDITION_ARGUMENTS"
        );
    }

    /// Drift guard: anything the converter can lift out of the flat shape must
    /// also be something the source-filter validator accepts, and vice versa
    /// for the operators that surface runs. If the two disagree, an author
    /// could save a filter that no longer converts, or convert one that then
    /// fails to save.
    #[test]
    fn converter_agrees_with_the_source_filter_validator() {
        use crate::condition::validate_condition_field_refs;

        for op in all_operators() {
            let condition = sample_condition(&op);
            let converts = condition_to_expression(&condition).is_ok();
            let validates = validate_condition_field_refs(&condition, &|_| true, "parity").is_ok();
            // The validator only runs the SQL-pushdown tier; client-only
            // operators are legitimately convertible but not saveable as a
            // source filter, which `operator_support` already records.
            let expected = validates || !operator_support(op.clone()).sql_pushdown;
            assert_eq!(
                converts, expected,
                "{op:?}: converter accepts={converts}, source-filter validator accepts={validates}"
            );
        }
    }

    // --- serde adapters -----------------------------------------------------

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct FlatHolder {
        #[serde(default, deserialize_with = "deserialize_condition_opt")]
        condition: Option<Condition>,
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct ExpressionHolder {
        #[serde(
            default,
            rename = "visibleWhen",
            deserialize_with = "deserialize_condition_expression_opt"
        )]
        visible_when: Option<ConditionExpression>,
    }

    #[test]
    fn flat_field_accepts_an_expression_payload_and_normalizes_down() {
        let holder: FlatHolder = serde_json::from_value(json!({
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&holder).unwrap(),
            json!({ "condition": { "op": "EQ", "arguments": ["status", "active"] } })
        );
    }

    #[test]
    fn expression_field_accepts_a_flat_payload_and_normalizes_up() {
        let holder: ExpressionHolder = serde_json::from_value(json!({
            "visibleWhen": { "op": "EQ", "arguments": ["status", "active"] }
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&holder).unwrap(),
            json!({
                "visibleWhen": {
                    "type": "operation",
                    "op": "EQ",
                    "arguments": [
                        { "valueType": "reference", "value": "status" },
                        { "valueType": "immediate", "value": "active" }
                    ]
                }
            })
        );
    }

    /// Normalization has to be idempotent, or the corpus round-trip test (which
    /// requires convergence after one pass) would oscillate.
    #[test]
    fn normalization_is_idempotent() {
        let once: FlatHolder = serde_json::from_value(json!({
            "condition": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        }))
        .unwrap();
        let first = serde_json::to_value(&once).unwrap();
        let twice: FlatHolder = serde_json::from_value(first.clone()).unwrap();
        assert_eq!(first, serde_json::to_value(&twice).unwrap());
    }

    #[test]
    fn canonical_payloads_are_untouched() {
        let flat = json!({ "condition": { "op": "EQ", "arguments": ["status", "active"] } });
        let holder: FlatHolder = serde_json::from_value(flat.clone()).unwrap();
        assert_eq!(serde_json::to_value(&holder).unwrap(), flat);

        let expression = json!({
            "visibleWhen": {
                "type": "operation",
                "op": "EQ",
                "arguments": [
                    { "valueType": "reference", "value": "status" },
                    { "valueType": "immediate", "value": "active" }
                ]
            }
        });
        let holder: ExpressionHolder = serde_json::from_value(expression.clone()).unwrap();
        assert_eq!(serde_json::to_value(&holder).unwrap(), expression);
    }

    /// A filter binding is only meaningful in a source filter, so it must keep
    /// deserializing there — the normalizer must not reject operands it merely
    /// cannot lift into the other shape.
    #[test]
    fn flat_field_keeps_accepting_sql_only_operands() {
        let payload = json!({
            "condition": {
                "op": "EQ",
                "arguments": ["status", { "filter": "status", "path": "value" }]
            }
        });
        let holder: FlatHolder = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(serde_json::to_value(&holder).unwrap(), payload);
    }

    #[test]
    fn absent_and_null_stay_none() {
        let holder: FlatHolder = serde_json::from_value(json!({})).unwrap();
        assert!(holder.condition.is_none());
        let holder: FlatHolder = serde_json::from_value(json!({ "condition": null })).unwrap();
        assert!(holder.condition.is_none());
        let holder: ExpressionHolder =
            serde_json::from_value(json!({ "visibleWhen": null })).unwrap();
        assert!(holder.visible_when.is_none());
    }

    #[test]
    fn unconvertible_payload_surfaces_the_reason() {
        let err = serde_json::from_value::<ExpressionHolder>(json!({
            "visibleWhen": {
                "op": "EQ",
                "arguments": ["status", { "filter": "status", "path": "value" }]
            }
        }))
        .expect_err("a filter binding has no row-visibility form");
        assert!(
            err.to_string().contains("UNCONVERTIBLE_CONDITION_OPERAND"),
            "error should carry the conversion code: {err}"
        );
    }
}
