// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared row-marshaling helpers.
//!
//! Currently only houses the [`StepSummaryRecord`] extractor, which both
//! backends implement by hand because the record isn't a
//! `#[sqlx(FromRow)]` derive target — its `inputs`/`outputs`/`error`
//! columns are produced by CTEs and take different shapes per backend
//! (Postgres returns `jsonb` values directly; SQLite returns text that
//! must be parsed).
//!
//! Phase 1 (SYN-394) defines the shared status/text-column extraction
//! that *is* identical across backends; the JSON columns keep their
//! backend-specific marshaling until Phase 4 decides how to abstract
//! the JSON type difference cleanly.

use crate::persistence::StepStatus;

/// Parse the string form of [`StepStatus`] used by the step-summary CTE.
///
/// The CTE emits `"running"`, `"failed"`, or `"completed"` depending on
/// whether the paired end event exists and what its payload carries. Any
/// unexpected value degrades to `Completed` to match the current backend
/// behavior.
pub fn parse_step_status(s: &str) -> StepStatus {
    match s {
        "running" => StepStatus::Running,
        "failed" => StepStatus::Failed,
        _ => StepStatus::Completed,
    }
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
}
