// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared operation implementations used by both backends.
//!
//! Each submodule hosts a family of operations (instances, checkpoints,
//! events, signals, sleep, step summaries, retention) and exposes a
//! `macro_rules!` macro that expands to concrete `impl` blocks against a
//! given backend type + pool type + dialect type. The shared body composes
//! SQL via [`crate::persistence::dialect::Dialect`], binds, executes, and
//! routes errors/rows through [`crate::persistence::common::error`] and
//! [`crate::persistence::common::row`].
//!
//! Phase 1 (SYN-394) creates the module structure; subsequent phases
//! migrate operations family-by-family (see the SYN-394 plan for ordering).
//! Until each family is migrated, the existing inline implementations in
//! [`crate::persistence::postgres`] and [`crate::persistence::sqlite`]
//! remain authoritative.

pub mod instances;
pub mod sleep;
// pub mod checkpoints;     // Phase 3
// pub mod signals;         // Phase 3
// pub mod events;          // Phase 4
// pub mod step_summaries;  // Phase 4
// pub mod retention;       // Phase 5

pub(crate) use instances::impl_instance_ops;
pub(crate) use sleep::impl_sleep_ops;

#[cfg(test)]
pub mod parity_harness;
