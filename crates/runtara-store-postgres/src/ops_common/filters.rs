// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared filter helpers — mapping [`super::super::PairedRecordStatus`] and
//! [`super::super::EventSortOrder`] onto the string/enum forms expected
//! by SQL.

use ::runtara_core::persistence::{EventSortOrder, PairedRecordStatus};

/// Convert [`PairedRecordStatus`] into the string form used by the
/// paired-record CTE's `status` column.
pub fn record_status_filter_str(status: PairedRecordStatus) -> &'static str {
    match status {
        PairedRecordStatus::Running => "running",
        PairedRecordStatus::Completed => "completed",
        PairedRecordStatus::Failed => "failed",
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
    fn record_status_strings_match_cte_convention() {
        assert_eq!(
            record_status_filter_str(PairedRecordStatus::Running),
            "running"
        );
        assert_eq!(
            record_status_filter_str(PairedRecordStatus::Completed),
            "completed"
        );
        assert_eq!(
            record_status_filter_str(PairedRecordStatus::Failed),
            "failed"
        );
    }

    #[test]
    fn sort_direction_renders_sql_keyword() {
        assert_eq!(sort_direction_sql(EventSortOrder::Asc), "ASC");
        assert_eq!(sort_direction_sql(EventSortOrder::Desc), "DESC");
    }
}
