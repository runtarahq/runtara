// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What an engine shares with the other engines on its databases and Valkey
//! (docs/multiple-engines-plan.md).
//!
//! - [`members`]: who the engines are, a row each in the runtime database.
//! - [`singleton`]: a lease in Valkey, for a job no two engines should run
//!   at once.
//!
//! No engine is special: a job that must run once at a time holds a lease
//! and loses it by not renewing it; everything else is claimed row by row.

use std::sync::OnceLock;

pub mod members;
pub mod singleton;

static ENGINE_ID: OnceLock<String> = OnceLock::new();

/// This engine's id, made once per process: `engine-<uuid>`.
pub fn engine_id() -> &'static str {
    ENGINE_ID.get_or_init(|| format!("engine-{}", uuid::Uuid::new_v4().simple()))
}

/// The release this engine runs: engines on one database run one release,
/// unless an upgrade is rolling (`RUNTARA_ROLLING_UPGRADE`).
pub fn release() -> String {
    format!("{}+{}", env!("BUILD_VERSION"), env!("BUILD_COMMIT"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_engine_has_one_id_for_its_life() {
        assert_eq!(super::engine_id(), super::engine_id());
        assert!(super::engine_id().starts_with("engine-"));
    }
}
