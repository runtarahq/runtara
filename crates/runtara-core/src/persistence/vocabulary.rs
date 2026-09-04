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
//! An [`EventVocabulary`] is therefore a *typed* upward dependency: the crate
//! above states its own naming once, and the compiler can see it. The
//! alternative, string literals buried in this crate's SQL, is the same
//! dependency with the compiler blinded to it.
//!
//! # Why the names are restricted to identifiers
//!
//! These names are spliced into SQL — as JSON path keys and as subtype
//! comparison literals — rather than bound as parameters, because the paired
//! query's plan is tuned around literal subtype predicates that the partial
//! index on `(instance_id, subtype)` can use. Splicing is only safe on a
//! closed character set, so [`EventVocabulary::new`] rejects anything that is
//! not a plain ASCII identifier. Values that originate from users are always
//! bound, never spliced.

use crate::error::CoreError;

/// The names one caller's event protocol uses, before validation.
///
/// Passed by value to [`EventVocabulary::new`], which validates every field.
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
/// Construct with [`EventVocabulary::new`]. Every accessor returns a name that
/// has already been checked to be a plain ASCII identifier, which is what makes
/// splicing it into SQL safe.
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

/// Accept `[A-Za-z_][A-Za-z0-9_]*` and nothing else.
///
/// This is the guard that lets the paired-record SQL splice these names
/// instead of binding them: a value that passes cannot carry a quote, a
/// comment marker, or whitespace, so it cannot end the literal or the
/// identifier it is spliced into.
fn validate_name(field: &'static str, value: &str) -> Result<(), CoreError> {
    let mut chars = value.chars();
    let valid = match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    };

    if valid {
        Ok(())
    } else {
        Err(CoreError::ValidationError {
            field: field.to_string(),
            message: format!(
                "must be a plain ASCII identifier matching [A-Za-z_][A-Za-z0-9_]*, got {value:?}"
            ),
        })
    }
}

impl EventVocabulary {
    /// Validate a spec into a vocabulary.
    ///
    /// Every field must be a plain ASCII identifier — see the module docs for
    /// why. Returns [`CoreError::ValidationError`] naming the offending field.
    pub fn new(spec: EventVocabularySpec<'_>) -> Result<Self, CoreError> {
        let fields: [(&'static str, &str); 11] = [
            ("start_subtype", spec.start_subtype),
            ("end_subtype", spec.end_subtype),
            ("correlation_key", spec.correlation_key),
            ("kind_key", spec.kind_key),
            ("label_key", spec.label_key),
            ("inputs_key", spec.inputs_key),
            ("outputs_key", spec.outputs_key),
            ("error_key", spec.error_key),
            ("error_flag_key", spec.error_flag_key),
            ("launched_at_key", spec.launched_at_key),
            ("settled_at_key", spec.settled_at_key),
        ];
        for (field, value) in fields {
            validate_name(field, value)?;
        }

        // One subtype for both ends is not a naming choice, it is a broken
        // vocabulary: `se` and `ee` would select the same rows, so the pairing
        // join degenerates into a self-join. A lone event would pair with
        // itself and read as settled, and n events sharing a correlation id
        // would multiply into n*n rows. Refuse it here rather than return
        // nonsense from every query built on it.
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

    /// The names reach SQL by splicing, so anything that could close a literal
    /// or open a comment must be refused at construction — this test is the
    /// injection guard, not a style check.
    #[test]
    fn rejects_anything_that_is_not_an_identifier() {
        let rejected = [
            "'; DROP TABLE instance_events; --",
            "step_id'",
            "step id",
            "step-id",
            "step.id",
            "1step",
            "",
            "stép_id",
            "step_id\n",
        ];

        for bad in rejected {
            let mut s = spec();
            s.correlation_key = bad;
            let err = EventVocabulary::new(s)
                .expect_err(&format!("{bad:?} must be rejected as a correlation key"));
            match err {
                CoreError::ValidationError { field, .. } => assert_eq!(field, "correlation_key"),
                other => panic!("expected a validation error, got {other:?}"),
            }
        }
    }

    /// A vocabulary whose two subtypes are the same is structurally broken,
    /// not merely odd: every name in it passes the character check, so nothing
    /// else would catch it, and the paired query would silently return a
    /// self-join instead of an error.
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

    /// Validation must cover every field, not just the first — a spec that is
    /// valid except for its last field must still be refused.
    #[test]
    fn validates_every_field() {
        let mut s = spec();
        s.settled_at_key = "not an identifier";

        let err = EventVocabulary::new(s).expect_err("an invalid last field must be rejected");
        match err {
            CoreError::ValidationError { field, .. } => assert_eq!(field, "settled_at_key"),
            other => panic!("expected a validation error, got {other:?}"),
        }
    }
}
