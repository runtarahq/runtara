// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner module - instance execution backends.
//!
//! This module is moved from runtara-core.

pub mod mock;
pub mod native;
pub mod oci;
mod traits;
pub mod wasm;

pub use mock::MockRunner;
pub use native::{NativeRunner, NativeRunnerConfig};
pub use traits::*;
pub use wasm::{WasmRunner, WasmRunnerConfig};
