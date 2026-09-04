// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared persistence helpers consumed by the persistence backend.
//!
//! Contains:
//! - [`error`] — error-mapping helpers (e.g. `rows_affected == 0 → InstanceNotFound`
//!   and `CheckpointSaveFailed` wrapping) shared by every operation family.
//! - [`row`] — result-marshaling helpers for records that aren't served by
//!   `#[sqlx(FromRow)]` auto-derive (currently `PairedRecordSummary`).
//! - [`filters`] — shared predicates and bind ordering for list/count filters.
//! - [`ops`] — macro-generated operation implementations for
//!   [`super::postgres::PostgresPersistence`].

pub mod error;
pub mod filters;
pub mod ops;
pub mod row;
