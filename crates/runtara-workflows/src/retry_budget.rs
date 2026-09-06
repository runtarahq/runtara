// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The retry counter includes the initial attempt in its u32 domain.

use runtara_dsl::Step;

pub(crate) const MAX_RETRIES: u32 = u32::MAX - 1;

pub(crate) fn step_max_retries(step: &Step) -> Option<u32> {
    match step {
        Step::Agent(step) => step.max_retries,
        Step::EmbedWorkflow(step) => step.max_retries,
        Step::Split(step) => step.config.as_ref().and_then(|config| config.max_retries),
        Step::AiAgent(step) => step.config.as_ref().and_then(|config| config.max_retries),
        _ => None,
    }
}

pub(crate) fn retry_count_message(max_retries: u64) -> String {
    format!(
        "maxRetries {max_retries} exceeds {MAX_RETRIES}; the initial attempt must also fit in the u32 attempt counter"
    )
}
