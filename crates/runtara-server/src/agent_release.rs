// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The platform components this server runs: a release in the artifact store.
//!
//! At boot the components directory (`RUNTARA_AGENT_COMPONENTS_DIR`), when
//! set, is published into the store as this server's release; the agent
//! catalog, the dispatcher and the entitlement allowlist then load from that
//! release's index rather than from a directory listing. Without a directory,
//! the server runs the release the store already holds.

use std::path::Path;

use runtara_environment::artifacts::ArtifactStore;
use runtara_environment::artifacts::release::{self, ReleaseIndex};

/// The release this server runs: `RUNTARA_AGENT_RELEASE`, or the server's
/// version.
pub fn release_name() -> String {
    std::env::var("RUNTARA_AGENT_RELEASE")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

/// Publish `components_dir` as `release_name` when given, then return the
/// release's index; `None` when there is no directory and the store holds no
/// such release.
pub async fn prepare(
    store: &dyn ArtifactStore,
    components_dir: Option<&Path>,
    release_name: &str,
) -> Result<Option<ReleaseIndex>, String> {
    match components_dir {
        Some(dir) => release::publish_dir(store, dir, release_name)
            .await
            .map(Some)
            .map_err(|error| {
                format!(
                    "publish the components at {} as release {release_name}: {error}",
                    dir.display()
                )
            }),
        None => release::load(store, release_name)
            .await
            .map_err(|error| format!("load release {release_name}: {error}")),
    }
}
