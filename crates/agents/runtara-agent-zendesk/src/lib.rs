// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Zendesk Support integration agent — WebAssembly Component.
//!
//! Every capability, input and output type lives in `generated.rs`, rendered by
//! `scripts/connector_gen` from `connector.yaml` and `spec.slice.json`. Change
//! the operation list in `connector.yaml` and regenerate instead of editing it.
//! Vendor quirks the spec cannot express go into their own hand-written module.
#![allow(clippy::result_large_err)]

pub mod kit;

mod generated;
pub use generated::*;
