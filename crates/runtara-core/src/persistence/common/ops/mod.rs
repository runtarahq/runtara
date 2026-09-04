// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Operation implementations for the persistence backend.
//!
//! Each submodule hosts a family of operations (instances, checkpoints,
//! events, signals, sleep, paired records, retention) and exposes a
//! `macro_rules!` macro that expands to concrete `impl` blocks against a
//! given backend type + pool type + dialect type. The shared body composes
//! SQL via [`crate::persistence::dialect::Dialect`], binds, executes, and
//! routes errors/rows through [`crate::persistence::common::error`] and
//! [`crate::persistence::common::row`].
//!
//! The macro indirection dates from when there were two backends. With one
//! left it buys nothing; expanding it into plain `impl` blocks is a separate
//! cleanup.

pub mod checkpoints;
pub mod events;
pub mod instances;
pub mod paired_records;
pub mod retention;
pub mod signals;
pub mod sleep;

pub(crate) use checkpoints::impl_checkpoint_ops;
pub(crate) use events::impl_event_ops;
pub(crate) use instances::impl_instance_ops;
pub(crate) use paired_records::impl_paired_record_ops;
pub(crate) use retention::impl_retention_ops;
pub(crate) use signals::impl_signal_ops;
pub(crate) use sleep::impl_sleep_ops;

#[cfg(all(test, feature = "db-integration-tests"))]
pub mod postgres_conformance;
