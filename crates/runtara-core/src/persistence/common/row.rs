// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared row-marshaling helpers.
//!
//! Currently only houses the [`StepSummaryRecord`] extractor, which is
//! marshaled by hand because the record isn't a `#[sqlx(FromRow)]` derive
//! target — its `inputs`/`outputs`/`error` columns are computed by CTEs
//! rather than read straight off a table, and the CTE hands them back as
//! TEXT, so they need parsing rather than a direct `jsonb` decode.

use crate::persistence::StepStatus;

/// Parse the string form of [`StepStatus`] used by the step-summary CTE.
///
/// The CTE emits `"running"`, `"failed"`, or `"completed"` depending on
/// whether the paired end event exists and what its payload carries. Any
/// unexpected value degrades to `Completed`, matching the CTE's own
/// `ELSE` arm.
pub fn parse_step_status(s: &str) -> StepStatus {
    match s {
        "running" => StepStatus::Running,
        "failed" => StepStatus::Failed,
        _ => StepStatus::Completed,
    }
}

/// Parse a TEXT column that carries a JSON-serialized value into
/// `serde_json::Value`, yielding `None` if the column is NULL or the
/// text fails to parse.
///
/// Used by the shared `op_list_step_summaries` path: the step-summary CTE
/// emits `inputs`/`outputs`/`error` as TEXT (a `::text` cast on the JSONB
/// extraction), so every JSON column coming out of that query is parsed
/// through this one helper instead of each column growing its own decode.
pub fn decode_json_text(text: Option<String>) -> Option<serde_json::Value> {
    text.and_then(|s| serde_json::from_str(&s).ok())
}

/// Extract a structured error from a step output envelope produced by
/// generated agent/capability error paths.
pub fn error_from_output_envelope(output: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let output = output?;
    if output.get("_error").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }

    Some(
        output
            .get("error")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("Step output reported _error=true")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_status_strings() {
        assert_eq!(parse_step_status("running"), StepStatus::Running);
        assert_eq!(parse_step_status("failed"), StepStatus::Failed);
        assert_eq!(parse_step_status("completed"), StepStatus::Completed);
    }

    #[test]
    fn unknown_status_falls_back_to_completed() {
        assert_eq!(parse_step_status("weird"), StepStatus::Completed);
        assert_eq!(parse_step_status(""), StepStatus::Completed);
    }

    #[test]
    fn extracts_error_from_output_envelope() {
        let output = serde_json::json!({
            "_error": true,
            "error": {"message": "capability failed"}
        });

        assert_eq!(
            error_from_output_envelope(Some(&output)),
            Some(serde_json::json!({"message": "capability failed"}))
        );
    }

    #[test]
    fn ignores_non_error_output_envelope() {
        let output = serde_json::json!({"_error": false, "error": "ignored"});

        assert_eq!(error_from_output_envelope(Some(&output)), None);
        assert_eq!(error_from_output_envelope(None), None);
    }
}
