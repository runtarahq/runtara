// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workflow event vocabulary, stated once.
//!
//! `runtara-core` stores and pairs events without knowing what any of them
//! mean: which subtypes open and close a step, which payload key correlates
//! two events into one step, which key carries a failed agent's error. Those
//! names belong to the workflow DSL and to the guest code that emits the
//! events — `runtara-workflow-stdlib` and the direct-WASM compiler in
//! `runtara-workflows` — so they are declared here, in the crate that knows
//! about steps, and handed to the kernel per query.
//!
//! This module is the single place the two halves agree. If the guest ever
//! renames one of these keys, this is the file that changes, and the compiler
//! points at every call site.
//!
//! # These names are a wire contract
//!
//! They match what a *compiled workflow* emits, not just what the current
//! stdlib source says. A rename here without a matching rename in the guest
//! silently un-pairs every step: starts and ends stop joining, and every step
//! reads as still running. Workflow history is short-lived (retention wipes it
//! on the order of days), so historical rows are not the hazard — runs already
//! in flight at deploy are.

use std::sync::LazyLock;

use runtara_core::persistence::{EventVocabulary, EventVocabularySpec};

/// The subtypes and payload keys a compiled workflow's step events use.
///
/// Built once: the names are fixed at compile time and validation cannot fail,
/// but it is still validation rather than an unchecked construction, so a
/// future edit that breaks the identifier rule fails loudly at first use
/// rather than producing malformed SQL.
static WORKFLOW_STEPS: LazyLock<EventVocabulary> = LazyLock::new(|| {
    EventVocabulary::new(EventVocabularySpec {
        start_subtype: "step_debug_start",
        end_subtype: "step_debug_end",
        correlation_key: "step_id",
        kind_key: "step_type",
        label_key: "step_name",
        inputs_key: "inputs",
        outputs_key: "outputs",
        error_key: "error",
        // A WASM agent's generated code reports failure by setting this flag
        // inside its output rather than by populating `error`. It is a codegen
        // return convention, a layer below the DSL, and the only reason the
        // kernel needs to know about the output object's *interior* at all.
        error_flag_key: "_error",
        launched_at_key: "launched_at_ms",
        settled_at_key: "settled_at_ms",
    })
    .expect("the workflow step vocabulary is a fixed set of valid identifiers")
});

/// The workflow step vocabulary.
pub fn workflow_steps() -> &'static EventVocabulary {
    &WORKFLOW_STEPS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names are a wire contract with compiled workflow guests, so they
    /// are pinned here rather than left to read off the constructor. A change
    /// to any of them must be a deliberate edit to this test too.
    #[test]
    fn the_vocabulary_matches_what_compiled_workflows_emit() {
        let vocab = workflow_steps();

        assert_eq!(vocab.start_subtype(), "step_debug_start");
        assert_eq!(vocab.end_subtype(), "step_debug_end");
        assert_eq!(vocab.correlation_key(), "step_id");
        assert_eq!(vocab.kind_key(), "step_type");
        assert_eq!(vocab.label_key(), "step_name");
        assert_eq!(vocab.inputs_key(), "inputs");
        assert_eq!(vocab.outputs_key(), "outputs");
        assert_eq!(vocab.error_key(), "error");
        assert_eq!(vocab.error_flag_key(), "_error");
        assert_eq!(vocab.launched_at_key(), "launched_at_ms");
        assert_eq!(vocab.settled_at_key(), "settled_at_ms");
    }
}
