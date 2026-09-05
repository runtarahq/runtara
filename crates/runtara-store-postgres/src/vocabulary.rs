// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Validated names used by PostgreSQL's paired-event queries.
//!
//! The queries interpolate JSON keys and subtype literals so their predicates
//! can use partial indexes. Only plain ASCII identifiers may be interpolated.
//! This restriction belongs to this query implementation, not the event model.

use runtara_core::error::CoreError;
use runtara_core::persistence::EventVocabulary;

/// A vocabulary checked for safe interpolation by this backend.
///
/// The private field and immutable borrow ensure every SQL builder receives
/// validated names, including retention queries that interpolate only subtypes.
pub(crate) struct SqlVocabulary<'a>(&'a EventVocabulary);

impl<'a> SqlVocabulary<'a> {
    pub(crate) fn new(vocabulary: &'a EventVocabulary) -> Result<Self, CoreError> {
        for (field, value) in [
            ("start_subtype", vocabulary.start_subtype()),
            ("end_subtype", vocabulary.end_subtype()),
            ("correlation_key", vocabulary.correlation_key()),
            ("kind_key", vocabulary.kind_key()),
            ("label_key", vocabulary.label_key()),
            ("inputs_key", vocabulary.inputs_key()),
            ("outputs_key", vocabulary.outputs_key()),
            ("error_key", vocabulary.error_key()),
            ("error_flag_key", vocabulary.error_flag_key()),
            ("launched_at_key", vocabulary.launched_at_key()),
            ("settled_at_key", vocabulary.settled_at_key()),
        ] {
            let mut chars = value.chars();
            let valid = chars
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return Err(CoreError::ValidationError {
                    field: field.into(),
                    message: "PostgreSQL event queries require a plain ASCII identifier matching [A-Za-z_][A-Za-z0-9_]*".into(),
                });
            }
        }
        Ok(Self(vocabulary))
    }
}

impl std::ops::Deref for SqlVocabulary<'_> {
    type Target = EventVocabulary;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::persistence::EventVocabularySpec;

    fn spec() -> EventVocabularySpec<'static> {
        EventVocabularySpec {
            start_subtype: "unit_start",
            end_subtype: "unit_end",
            correlation_key: "unit_id",
            kind_key: "kind",
            label_key: "label",
            inputs_key: "inputs",
            outputs_key: "outputs",
            error_key: "error",
            error_flag_key: "_error",
            launched_at_key: "launched_at",
            settled_at_key: "settled_at",
        }
    }

    #[test]
    fn validates_every_interpolated_name() {
        assert!(SqlVocabulary::new(&EventVocabulary::new(spec()).unwrap()).is_ok());
        for field in 0..11 {
            for bad in [
                "'; DROP TABLE instance_events; --",
                "unit'",
                "unit name",
                "unit-name",
                "unit.name",
                "1unit",
                "",
                "unité",
                "unit\n",
            ] {
                let mut candidate = spec();
                match field {
                    0 => candidate.start_subtype = bad,
                    1 => candidate.end_subtype = bad,
                    2 => candidate.correlation_key = bad,
                    3 => candidate.kind_key = bad,
                    4 => candidate.label_key = bad,
                    5 => candidate.inputs_key = bad,
                    6 => candidate.outputs_key = bad,
                    7 => candidate.error_key = bad,
                    8 => candidate.error_flag_key = bad,
                    9 => candidate.launched_at_key = bad,
                    10 => candidate.settled_at_key = bad,
                    _ => unreachable!(),
                }
                let vocabulary = EventVocabulary::new(candidate).expect("valid domain vocabulary");
                assert!(
                    matches!(
                        SqlVocabulary::new(&vocabulary),
                        Err(CoreError::ValidationError { .. })
                    ),
                    "field {field}: {bad:?}"
                );
            }
        }
    }
    #[tokio::test]
    async fn every_query_entrypoint_validates_before_storage_access() {
        use runtara_core::persistence::{ListPairedRecordsFilter, Persistence};
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect_lazy("postgresql://localhost:1/unused")
            .unwrap();
        let backend = crate::PostgresPersistence::new(pool);
        let mut candidate = spec();
        candidate.start_subtype = "unit' OR TRUE --";
        let vocabulary = EventVocabulary::new(candidate).unwrap();
        let filter = ListPairedRecordsFilter::default();
        let results = [
            backend
                .list_paired_records("instance", &vocabulary, &filter, 10, 0)
                .await
                .map(|_| ()),
            backend
                .count_paired_records("instance", &vocabulary, &filter)
                .await
                .map(|_| ()),
            backend
                .delete_paired_events_older_than(&vocabulary, chrono::Utc::now(), 10)
                .await
                .map(|_| ()),
        ];
        for result in results {
            assert!(
                matches!(result, Err(CoreError::ValidationError { field, .. }) if field == "start_subtype")
            );
        }
    }
}
