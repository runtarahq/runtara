// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! PostgreSQL implementation of [`runtara_core::persistence::Persistence`].
//!
//! Core owns the contract and knows nothing about SQL; this crate owns the
//! schema, the queries and the driver. Anything that assumes a relational store
//! — placeholders, enum casts, `ON CONFLICT`, `SKIP LOCKED`, the migrations —
//! lives here.
//!
//! Correctness against the contract is proven by
//! `runtara_core::persistence::conformance`, which this crate runs under its
//! `db-integration-tests` feature and which Core also runs against its
//! in-memory backend. Two implementations, one suite.

#![deny(missing_docs)]

/// Database migrations for the Postgres schema.
pub mod migrations;

/// Row decoding for Core's record types.
pub mod rows;

/// PostgreSQL encodings of execution domain values.
pub mod encoding;

mod backend;
mod dialect;
mod ops_common;

pub use backend::{PostgresPersistence, load_latest_checkpoint};
