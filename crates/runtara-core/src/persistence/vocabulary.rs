// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Caller-supplied naming for paired event records.
//!
//! This crate is a durable-execution kernel: it stores events, pairs them, and
//! hands the pairs back. It deliberately does **not** know what the events
//! mean. The names — which subtypes open and close a record, which payload key
//! correlates two events into one unit of work, which key carries the failure
//! envelope — belong to whatever protocol the caller's producer emits, so the
//! caller supplies them here and this crate treats every one as opaque.
//!
//! Core validates the pairing semantics, while names remain opaque strings.
//! Storage adapters own any restrictions their query implementations require.

use crate::error::CoreError;

/// The names one caller's event protocol uses, before validation.
///
/// Passed by value to [`EventVocabulary::new`], which validates the pairing.
/// Named fields rather than positional arguments: eleven adjacent strings of
/// the same type are otherwise trivial to transpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventVocabularySpec<'a> {
    /// Subtype of the event that opens a record.
    pub start_subtype: &'a str,
    /// Subtype of the event that closes a record.
    pub end_subtype: &'a str,
    /// Payload key whose value pairs a start with its end. Together with the
    /// crate's own `scope_id`, this is what makes two events the same unit of
    /// work.
    pub correlation_key: &'a str,
    /// Payload key carrying an opaque classifier, exposed for filtering. This
    /// crate never matches on its *value*.
    pub kind_key: &'a str,
    /// Payload key carrying a human-readable label.
    pub label_key: &'a str,
    /// Payload key on the start event carrying the record's input.
    pub inputs_key: &'a str,
    /// Payload key on the end event carrying the record's output.
    pub outputs_key: &'a str,
    /// Payload key on the end event carrying failure detail.
    pub error_key: &'a str,
    /// Key *inside* the output object which, when `true`, marks the record
    /// failed even though [`Self::error_key`] is absent. A producer-side
    /// return convention rather than part of the event protocol proper.
    pub error_flag_key: &'a str,
    /// Payload key on the end event carrying the real launch wall-clock, in
    /// epoch milliseconds, for work that ran concurrently.
    pub launched_at_key: &'a str,
    /// Payload key on the end event carrying the matching settle wall-clock.
    pub settled_at_key: &'a str,
}

/// A validated [`EventVocabularySpec`].
///
/// Construct with [`EventVocabulary::new`]. Opening and closing subtypes must
/// differ. Names may contain arbitrary characters, including JSON keys that
/// are not identifiers; individual storage adapters may impose restrictions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventVocabulary {
    start_subtype: String,
    end_subtype: String,
    correlation_key: String,
    kind_key: String,
    label_key: String,
    inputs_key: String,
    outputs_key: String,
    error_key: String,
    error_flag_key: String,
    launched_at_key: String,
    settled_at_key: String,
}

impl EventVocabulary {
    /// Validate a spec into a vocabulary.
    ///
    /// Returns [`CoreError::ValidationError`] if the opening and closing
    /// subtypes are identical.
    pub fn new(spec: EventVocabularySpec<'_>) -> Result<Self, CoreError> {
        // Opening and closing must be distinguishable; otherwise an event
        // would pair with itself and appear completed immediately.
        if spec.start_subtype == spec.end_subtype {
            return Err(CoreError::ValidationError {
                field: "end_subtype".to_string(),
                message: format!(
                    "must differ from start_subtype (both are {:?}): a single \
                     subtype for both ends pairs every record with itself",
                    spec.start_subtype
                ),
            });
        }

        Ok(Self {
            start_subtype: spec.start_subtype.to_string(),
            end_subtype: spec.end_subtype.to_string(),
            correlation_key: spec.correlation_key.to_string(),
            kind_key: spec.kind_key.to_string(),
            label_key: spec.label_key.to_string(),
            inputs_key: spec.inputs_key.to_string(),
            outputs_key: spec.outputs_key.to_string(),
            error_key: spec.error_key.to_string(),
            error_flag_key: spec.error_flag_key.to_string(),
            launched_at_key: spec.launched_at_key.to_string(),
            settled_at_key: spec.settled_at_key.to_string(),
        })
    }

    /// Subtype of the event that opens a record.
    pub fn start_subtype(&self) -> &str {
        &self.start_subtype
    }

    /// Subtype of the event that closes a record.
    pub fn end_subtype(&self) -> &str {
        &self.end_subtype
    }

    /// Payload key that pairs a start with its end.
    pub fn correlation_key(&self) -> &str {
        &self.correlation_key
    }

    /// Payload key carrying the opaque classifier.
    pub fn kind_key(&self) -> &str {
        &self.kind_key
    }

    /// Payload key carrying the human-readable label.
    pub fn label_key(&self) -> &str {
        &self.label_key
    }

    /// Payload key carrying the record's input.
    pub fn inputs_key(&self) -> &str {
        &self.inputs_key
    }

    /// Payload key carrying the record's output.
    pub fn outputs_key(&self) -> &str {
        &self.outputs_key
    }

    /// Payload key carrying failure detail.
    pub fn error_key(&self) -> &str {
        &self.error_key
    }

    /// Key inside the output object that marks the record failed.
    pub fn error_flag_key(&self) -> &str {
        &self.error_flag_key
    }

    /// Payload key carrying the real launch wall-clock.
    pub fn launched_at_key(&self) -> &str {
        &self.launched_at_key
    }

    /// Payload key carrying the real settle wall-clock.
    pub fn settled_at_key(&self) -> &str {
        &self.settled_at_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spec whose fields are all distinct, so a test that swaps one can tell
    /// which one moved.
    fn spec() -> EventVocabularySpec<'static> {
        EventVocabularySpec {
            start_subtype: "unit_start",
            end_subtype: "unit_end",
            correlation_key: "unit_id",
            kind_key: "unit_kind",
            label_key: "unit_label",
            inputs_key: "given",
            outputs_key: "produced",
            error_key: "failure",
            error_flag_key: "_failed",
            launched_at_key: "began_ms",
            settled_at_key: "ended_ms",
        }
    }

    #[test]
    fn accepts_identifiers_including_a_leading_underscore() {
        let vocab = EventVocabulary::new(spec()).expect("valid spec");

        assert_eq!(vocab.start_subtype(), "unit_start");
        assert_eq!(vocab.end_subtype(), "unit_end");
        assert_eq!(vocab.correlation_key(), "unit_id");
        assert_eq!(vocab.kind_key(), "unit_kind");
        assert_eq!(vocab.label_key(), "unit_label");
        assert_eq!(vocab.inputs_key(), "given");
        assert_eq!(vocab.outputs_key(), "produced");
        assert_eq!(vocab.error_key(), "failure");
        assert_eq!(vocab.error_flag_key(), "_failed");
        assert_eq!(vocab.launched_at_key(), "began_ms");
        assert_eq!(vocab.settled_at_key(), "ended_ms");
    }

    #[test]
    fn accepts_opaque_names_including_non_identifiers() {
        let vocabulary = EventVocabulary::new(EventVocabularySpec {
            start_subtype: "unit-start",
            end_subtype: "unit-end",
            correlation_key: "unit.id",
            kind_key: "kind name",
            label_key: "étiquette",
            inputs_key: "in'put",
            outputs_key: "out.put",
            error_key: "failure detail",
            error_flag_key: "is-error",
            launched_at_key: "began.ms",
            settled_at_key: "settled.ms",
        })
        .unwrap();
        assert_eq!(vocabulary.correlation_key(), "unit.id");
        assert_eq!(vocabulary.label_key(), "étiquette");
        assert_eq!(vocabulary.inputs_key(), "in'put");
    }

    /// A vocabulary whose two subtypes are the same is structurally broken,
    /// not merely odd: every name in it passes the character check, so nothing
    /// else would catch it, and the paired query would silently return a
    /// self-pair instead of an error.
    #[test]
    fn rejects_one_subtype_serving_as_both_ends() {
        let mut s = spec();
        s.end_subtype = s.start_subtype;

        let err = EventVocabulary::new(s).expect_err("a single subtype must be rejected");
        match err {
            CoreError::ValidationError { field, message } => {
                assert_eq!(field, "end_subtype");
                assert!(
                    message.contains("must differ from start_subtype"),
                    "the message must say what is wrong: {message}"
                );
            }
            other => panic!("expected a validation error, got {other:?}"),
        }

        // Distinct subtypes remain acceptable, so the check is not simply
        // refusing everything.
        assert!(EventVocabulary::new(spec()).is_ok());
    }
}
