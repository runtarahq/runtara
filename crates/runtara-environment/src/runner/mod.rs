// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner module - instance execution backends.
//!
//! This module is moved from runtara-core.

mod common;
pub mod embedded;
pub mod mock;
mod traits;
pub mod wasm;

pub use embedded::EmbeddedWasmRunner;
pub use mock::MockRunner;
pub use traits::*;
pub use wasm::{WasmRunner, WasmRunnerConfig};
