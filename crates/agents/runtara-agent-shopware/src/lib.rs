// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shopware integration agent — WebAssembly Component.
//!
//! Every generated capability, input and output type lives in `generated.rs`,
//! rendered by `scripts/connector_gen` from `connector.yaml` and
//! `spec.slice.json`. Change the operation list in `connector.yaml` and
//! regenerate instead of editing it. Vendor quirks the spec cannot express go
//! into `kit.rs`; the search of any record is written by hand in `custom.rs`.
#![allow(clippy::result_large_err)]

pub mod custom;
pub mod kit;

mod generated;
pub use custom::*;
pub use generated::*;
