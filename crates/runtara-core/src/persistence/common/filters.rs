// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared filter helpers — mapping [`super::super::StepStatus`] and
//! [`super::super::EventSortOrder`] onto the string/enum forms expected
//! by SQL.
//!
//! Phase 1 (SYN-394) scaffolding; call sites migrate in Phases 2–5.

use crate::persistence::{EventSortOrder, StepStatus};

/// Convert [`StepStatus`] into the string form used by the step-summary
/// CTE's `status` column.
pub fn step_status_filter_str(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Running => "running",
        StepStatus::Completed => "completed",
        StepStatus::Failed => "failed",
    }
}

/// SQL `ORDER BY` direction keyword for the given sort order.
///
/// Returned as a `&'static str` so it can be splice-formatted into SQL
/// without introducing an injection vector (the two possibilities are
/// compile-time constants).
pub fn sort_direction_sql(order: EventSortOrder) -> &'static str {
    match order {
        EventSortOrder::Asc => "ASC",
        EventSortOrder::Desc => "DESC",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_status_strings_match_cte_convention() {
        assert_eq!(step_status_filter_str(StepStatus::Running), "running");
        assert_eq!(step_status_filter_str(StepStatus::Completed), "completed");
        assert_eq!(step_status_filter_str(StepStatus::Failed), "failed");
    }

    #[test]
    fn sort_direction_renders_sql_keyword() {
        assert_eq!(sort_direction_sql(EventSortOrder::Asc), "ASC");
        assert_eq!(sort_direction_sql(EventSortOrder::Desc), "DESC");
    }
}
