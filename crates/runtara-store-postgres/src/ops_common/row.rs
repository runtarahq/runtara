// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared row-marshaling helpers.
//!
//! Currently only houses the [`PairedRecordSummary`] extractor, which is
//! marshaled by hand because the record isn't a `#[sqlx(FromRow)]` derive
//! target — its `inputs`/`outputs`/`error` columns are computed by CTEs
//! rather than read straight off a table, and the CTE hands them back as
//! TEXT, so they need parsing rather than a direct `jsonb` decode.
//!
//! [`PairedRecordSummary`]: ::runtara_core::persistence::PairedRecordSummary

use ::runtara_core::persistence::PairedRecordStatus;

/// Parse the string form of [`PairedRecordStatus`] used by the paired-record CTE.
///
/// The CTE emits `"running"`, `"failed"`, or `"completed"` depending on
/// whether the paired end event exists and what its payload carries. Any
/// unexpected value degrades to `Completed`, matching the CTE's own
/// `ELSE` arm.
pub fn parse_record_status(s: &str) -> PairedRecordStatus {
    match s {
        "running" => PairedRecordStatus::Running,
        "failed" => PairedRecordStatus::Failed,
        _ => PairedRecordStatus::Completed,
    }
}

/// Parse a TEXT column that carries a JSON-serialized value into
/// `serde_json::Value`, yielding `None` if the column is NULL or the
/// text fails to parse.
///
/// Used by the shared `op_list_paired_records` path: the paired-record CTE
/// emits `inputs`/`outputs`/`error` as TEXT (a `::text` cast on the JSONB
/// extraction), so every JSON column coming out of that query is parsed
/// through this one helper instead of each column growing its own decode.
pub fn decode_json_text(text: Option<String>) -> Option<serde_json::Value> {
    text.and_then(|s| serde_json::from_str(&s).ok())
}

/// Extract failure detail from an output envelope that flags its own failure.
///
/// Some producers report a failure by setting a boolean flag inside the output
/// object rather than by populating the end event's error key — a return
/// convention of the code that emits the events, one layer below the event
/// protocol itself. Both key names come from the caller's
/// [`EventVocabulary`](::runtara_core::persistence::EventVocabulary); this crate knows
/// neither.
pub fn error_from_output_envelope(
    output: Option<&serde_json::Value>,
    error_flag_key: &str,
    error_key: &str,
) -> Option<serde_json::Value> {
    let output = output?;
    if output.get(error_flag_key).and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }

    Some(
        output
            .get(error_key)
            .cloned()
            .unwrap_or_else(|| serde_json::json!(format!("Output reported {error_flag_key}=true"))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_status_strings() {
        assert_eq!(parse_record_status("running"), PairedRecordStatus::Running);
        assert_eq!(parse_record_status("failed"), PairedRecordStatus::Failed);
        assert_eq!(
            parse_record_status("completed"),
            PairedRecordStatus::Completed
        );
    }

    #[test]
    fn unknown_status_falls_back_to_completed() {
        assert_eq!(parse_record_status("weird"), PairedRecordStatus::Completed);
        assert_eq!(parse_record_status(""), PairedRecordStatus::Completed);
    }

    #[test]
    fn extracts_error_from_output_envelope() {
        let output = serde_json::json!({
            "_error": true,
            "error": {"message": "capability failed"}
        });

        assert_eq!(
            error_from_output_envelope(Some(&output), "_error", "error"),
            Some(serde_json::json!({"message": "capability failed"}))
        );
    }

    #[test]
    fn ignores_non_error_output_envelope() {
        let output = serde_json::json!({"_error": false, "error": "ignored"});

        assert_eq!(
            error_from_output_envelope(Some(&output), "_error", "error"),
            None
        );
        assert_eq!(error_from_output_envelope(None, "_error", "error"), None);
    }

    /// The envelope keys come from the caller, so a producer using entirely
    /// different names must work — and the workflow DSL's own names must have
    /// no special standing.
    #[test]
    fn honours_the_callers_envelope_keys() {
        let output = serde_json::json!({
            "_failed": true,
            "failure": {"message": "unit failed"},
            "_error": false,
            "error": "must be ignored"
        });

        assert_eq!(
            error_from_output_envelope(Some(&output), "_failed", "failure"),
            Some(serde_json::json!({"message": "unit failed"}))
        );
        assert_eq!(
            error_from_output_envelope(Some(&output), "_error", "error"),
            None
        );
    }

    /// A flagged failure with no detail still reads as a failure, and the
    /// stand-in message names the caller's flag rather than a fixed one.
    #[test]
    fn flagged_failure_without_detail_names_the_callers_flag() {
        let output = serde_json::json!({"_failed": true});

        assert_eq!(
            error_from_output_envelope(Some(&output), "_failed", "failure"),
            Some(serde_json::json!("Output reported _failed=true"))
        );
    }
}
