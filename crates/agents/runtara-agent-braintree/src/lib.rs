// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Braintree GraphQL API integration agent — WebAssembly Component.
//!
//! Every capability, input and output type lives in `generated.rs`, rendered by
//! `scripts/connector_gen` from `connector.yaml`, the GraphQL operations in
//! `operations/` and `spec.slice.json`. Change an operation or the list in
//! `connector.yaml` and regenerate instead of editing it.
#![allow(clippy::result_large_err)]

pub mod kit;

mod generated;
pub use generated::*;
