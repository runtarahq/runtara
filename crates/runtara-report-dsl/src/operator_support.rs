//! Single source of truth for *where* a condition operator can be evaluated.
//!
//! Report conditions are enforced at two save-time surfaces that run in
//! different engines:
//!
//! - **Object Model source filters** push down to SQL (the object-store
//!   condition builder). They can only use operators the SQL builder emits.
//! - **Row-visibility conditions** (`visibleWhen` / `hiddenWhen` /
//!   `disabledWhen`) run in-memory via the [`crate::row_condition`] evaluator
//!   (WASM in the browser, native on the server). They can only use operators
//!   that evaluator understands.
//!
//! Historically each checkpoint hard-coded its own operator list, so the two
//! surfaces disagreed (e.g. `STARTS_WITH` worked in row-visibility but was
//! rejected by source filters) and authors got errors that omitted the surface
//! where the operator actually works. This module derives the classification
//! once from [`ConditionOperator`]; every checkpoint consults it, so the sets
//! can only agree.

use runtara_dsl::ConditionOperator;

/// Which evaluation engines can execute a given [`ConditionOperator`].
///
/// The two axes are independent: `STARTS_WITH` is client-only, `MATCH` is
/// SQL-only, and most comparison operators are both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorSupport {
    /// Evaluable in-memory by [`crate::row_condition::evaluate_row_condition`].
    pub client_evaluable: bool,
    /// Executable as a SQL `WHERE` clause by the object-store condition builder.
    pub sql_pushdown: bool,
}

/// Classify an operator by where it can execute.
///
/// The `match` is exhaustive, so adding a new [`ConditionOperator`] variant is
/// a compile error here until it is classified — which forces every checkpoint
/// that derives from this function to be revisited before the variant ships.
pub const fn operator_support(op: ConditionOperator) -> OperatorSupport {
    use ConditionOperator::*;

    // Both engines evaluate these — the logical connectives are structural
    // (both recurse into their arguments) and the comparison / containment /
    // nullability operators have a direct implementation on each side.
    const BOTH: OperatorSupport = OperatorSupport {
        client_evaluable: true,
        sql_pushdown: true,
    };
    // In-memory only: the SQL builder has no arm for these.
    const CLIENT_ONLY: OperatorSupport = OperatorSupport {
        client_evaluable: true,
        sql_pushdown: false,
    };
    // SQL only: full-text / similarity / vector-distance operators translate to
    // Postgres constructs (`@@ plainto_tsquery`, `similarity()`, `<=>`/`<->`)
    // and have no in-memory equivalent in the row evaluator.
    const SQL_ONLY: OperatorSupport = OperatorSupport {
        client_evaluable: false,
        sql_pushdown: true,
    };

    match op {
        And | Or | Not => BOTH,
        Eq | Ne | Gt | Gte | Lt | Lte | Contains | In | NotIn | IsDefined | IsEmpty
        | IsNotEmpty => BOTH,
        StartsWith | EndsWith | Length => CLIENT_ONLY,
        SimilarityGte | Match | CosineDistanceLte | L2DistanceLte => SQL_ONLY,
    }
}

/// How an operator's arguments are laid out, independent of which engine runs
/// it.
///
/// Reports carry two condition wire-shapes — the flat positional
/// [`crate::Condition`] used by source filters and `runtara_dsl`'s tagged
/// `ConditionExpression` used by row visibility — and converting between them
/// requires knowing, per operator, which arguments are nested conditions,
/// which one names a field, and how many there should be. Both save-time
/// validators already encode this; this type is where the encoding lives so
/// the converter and the validators cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandShape {
    /// A logical connective: every argument is itself a condition.
    Logical {
        /// Exact argument count, or `None` for "one or more" (`AND` / `OR`).
        arity: Option<usize>,
    },
    /// A leaf comparison: argument 0 names a field, the rest are literals.
    FieldFirst {
        /// Exact argument count, field operand included.
        arity: usize,
    },
}

/// Describe the argument layout of a [`ConditionOperator`].
///
/// The `match` is exhaustive, so adding a new variant is a compile error here
/// until its operand layout is declared — the same forcing function
/// [`operator_support`] applies to evaluation tiers. The arities mirror the
/// groupings [`crate::validate_condition_field_refs`] enforces; a test pins the
/// two against each other.
pub const fn operand_shape(op: ConditionOperator) -> OperandShape {
    use ConditionOperator::*;

    match op {
        // `AND` / `OR` take one or more nested conditions; `NOT` wraps exactly one.
        And | Or => OperandShape::Logical { arity: None },
        Not => OperandShape::Logical { arity: Some(1) },
        // Binary comparisons, containment, and the string operators: field, value.
        Eq | Ne | Gt | Gte | Lt | Lte | Contains | In | NotIn | StartsWith | EndsWith | Match => {
            OperandShape::FieldFirst { arity: 2 }
        }
        // `LENGTH` measures its single operand; the nullability checks read the
        // field alone.
        Length | IsDefined | IsEmpty | IsNotEmpty => OperandShape::FieldFirst { arity: 1 },
        // Similarity / vector distance take field, operand, threshold.
        SimilarityGte | CosineDistanceLte | L2DistanceLte => OperandShape::FieldFirst { arity: 3 },
    }
}

/// Parse a wire-form operator string (e.g. `"STARTS_WITH"`) into a
/// [`ConditionOperator`]. Case-insensitive: the source-filter surface accepts
/// lowercase ops and upper-cases before matching, so mirror that here.
///
/// Returns `None` for strings that name no known operator — callers treat that
/// as "unsupported operator" rather than "wrong surface".
pub fn parse_operator(op: &str) -> Option<ConditionOperator> {
    serde_json::from_value(serde_json::Value::String(op.to_ascii_uppercase())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock the exact tiers. If a new `ConditionOperator` variant is added, the
    /// exhaustive match in `operator_support` fails to compile first; if an
    /// existing operator is re-tiered, this test fails and forces the author to
    /// confirm the change is intended (and update the two save-time surfaces).
    #[test]
    fn tiers_are_stable() {
        use ConditionOperator::*;

        let both = [
            And, Or, Not, Eq, Ne, Gt, Gte, Lt, Lte, Contains, In, NotIn, IsDefined, IsEmpty,
            IsNotEmpty,
        ];
        let client_only = [StartsWith, EndsWith, Length];
        let sql_only = [SimilarityGte, Match, CosineDistanceLte, L2DistanceLte];

        for op in both {
            let s = operator_support(op.clone());
            assert!(
                s.client_evaluable && s.sql_pushdown,
                "{op:?} expected to be evaluable by both engines"
            );
        }
        for op in client_only {
            let s = operator_support(op.clone());
            assert!(
                s.client_evaluable && !s.sql_pushdown,
                "{op:?} expected to be client-only"
            );
        }
        for op in sql_only {
            let s = operator_support(op.clone());
            assert!(
                !s.client_evaluable && s.sql_pushdown,
                "{op:?} expected to be SQL-only"
            );
        }
    }

    /// The operand layouts must match what the source-filter validator
    /// enforces, otherwise the wire-shape converter would build conditions the
    /// validator then rejects. Probe the validator with the declared arity and
    /// with one argument too many; only the declared arity may pass.
    #[test]
    fn field_first_arities_match_the_source_filter_validator() {
        use crate::condition::{Condition, validate_condition_field_refs};
        use ConditionOperator::*;
        use serde_json::json;

        let arity_accepted = |wire: &str, count: usize| {
            let condition = Condition {
                op: wire.to_string(),
                arguments: Some(vec![json!("f"); count]),
            };
            match validate_condition_field_refs(&condition, &|_| true, "arity") {
                Ok(()) => true,
                // Anything but an arity complaint means the count was fine and
                // the operator failed some other gate.
                Err(e) => e.code != "INVALID_CONDITION_ARGUMENTS",
            }
        };

        for op in [
            Eq,
            Ne,
            Gt,
            Gte,
            Lt,
            Lte,
            Contains,
            In,
            NotIn,
            IsDefined,
            IsEmpty,
            IsNotEmpty,
            SimilarityGte,
            Match,
            CosineDistanceLte,
            L2DistanceLte,
        ] {
            // Only operators the source-filter surface actually runs can be
            // probed through it; the client-only ones are pinned by the
            // row-condition evaluator instead.
            if !operator_support(op.clone()).sql_pushdown {
                continue;
            }
            let OperandShape::FieldFirst { arity } = operand_shape(op.clone()) else {
                panic!("{op:?} should be a field-first operator");
            };
            let wire = serde_json::to_value(&op)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .expect("operator serializes to a wire string");
            assert!(
                arity_accepted(&wire, arity),
                "{op:?} declares arity {arity} but the source-filter validator rejects it"
            );
            assert!(
                !arity_accepted(&wire, arity + 1),
                "{op:?} declares arity {arity} but the source-filter validator also accepts {}",
                arity + 1
            );
        }
    }

    /// The client-evaluable operators can't be probed through the SQL-bound
    /// validator, so pin their layouts against the shared in-memory evaluator,
    /// which enforces its own arity.
    #[test]
    fn client_only_arities_match_the_shared_evaluator() {
        use ConditionOperator::*;
        use runtara_dsl::condition_eval::evaluate_condition;
        use runtara_dsl::{
            ConditionArgument, ConditionExpression, ConditionOperation, ImmediateValue,
            MappingValue,
        };
        use serde_json::json;

        let arity_accepted = |op: ConditionOperator, count: usize| {
            let expression = ConditionExpression::Operation(ConditionOperation {
                op,
                arguments: (0..count)
                    .map(|_| {
                        ConditionArgument::Value(MappingValue::Immediate(ImmediateValue {
                            value: json!("x"),
                        }))
                    })
                    .collect(),
            });
            !matches!(
                evaluate_condition(&expression, &json!({})),
                Err(runtara_dsl::condition_eval::ConditionEvaluationError::ArgCount { .. })
            )
        };

        for op in [StartsWith, EndsWith, Length] {
            let OperandShape::FieldFirst { arity } = operand_shape(op.clone()) else {
                panic!("{op:?} should be a field-first operator");
            };
            assert!(
                arity_accepted(op.clone(), arity),
                "{op:?} declares arity {arity} but the shared evaluator rejects it"
            );
            assert!(
                !arity_accepted(op.clone(), arity + 1),
                "{op:?} declares arity {arity} but the shared evaluator also accepts {}",
                arity + 1
            );
        }
    }

    #[test]
    fn parses_wire_form_case_insensitively() {
        assert_eq!(
            parse_operator("STARTS_WITH"),
            Some(ConditionOperator::StartsWith)
        );
        assert_eq!(
            parse_operator("starts_with"),
            Some(ConditionOperator::StartsWith)
        );
        assert_eq!(parse_operator("Match"), Some(ConditionOperator::Match));
        assert_eq!(parse_operator("NOPE"), None);
    }
}
