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
