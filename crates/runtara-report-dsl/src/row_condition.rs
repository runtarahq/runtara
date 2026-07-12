//! Report compatibility facade for the shared condition evaluator.
//!
//! The implementation lives in `runtara-dsl` so reports, forms, backend
//! services, and browser WASM all execute the same condition semantics.

pub use runtara_dsl::condition_eval::{
    ConditionEvaluationError as RowConditionError, evaluate_condition as evaluate_row_condition,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_support::operator_support;
    use runtara_dsl::{ConditionExpression, ConditionOperation, ConditionOperator};
    use serde_json::json;

    #[test]
    fn facade_preserves_report_row_evaluation() {
        let expression: ConditionExpression = serde_json::from_value(json!({
            "type": "operation",
            "op": "EQ",
            "arguments": [
                { "valueType": "reference", "value": "status" },
                { "valueType": "immediate", "value": "active" }
            ]
        }))
        .unwrap();

        assert!(evaluate_row_condition(&expression, &json!({ "status": "active" })).unwrap());
        assert!(!evaluate_row_condition(&expression, &json!({ "status": "paused" })).unwrap());
    }

    #[test]
    fn shared_evaluator_matches_report_operator_classification() {
        use ConditionOperator::*;

        let operators = [
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

        for operator in operators {
            let expression = ConditionExpression::Operation(ConditionOperation {
                op: operator.clone(),
                arguments: vec![],
            });
            let is_server_only = matches!(
                evaluate_row_condition(&expression, &json!({})),
                Err(RowConditionError::ServerOnly(_))
            );
            assert_eq!(
                is_server_only,
                !operator_support(operator.clone()).client_evaluable,
                "shared evaluator classification of {operator:?} disagrees with reports"
            );
        }
    }
}
