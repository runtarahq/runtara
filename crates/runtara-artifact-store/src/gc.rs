// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Finding what nothing in the store references any more (SYN-777): the
//! mark half of a mark-and-sweep.
//!
//! Retained are every release whose index the store holds, and every
//! manifest that is not a release's record (a published workflow-agent's,
//! say). A release record no index names, and a blob no retained manifest
//! names, may go once it is older than the grace, so a publish in flight
//! (content first, then its record, then its index) is never swept. Blobs a
//! workflow image names are the caller's to keep: [`plan`] lists the
//! unreferenced ones, and the caller deletes each under its own lock.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use crate::release::ReleaseIndex;
use crate::{ArtifactError, ArtifactStore, Digest};

/// What [`plan`] found unreferenced and older than the grace.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Release records no retained index names.
    pub records: Vec<String>,
    /// Blobs no retained manifest names; images may still name them.
    pub blobs: Vec<Digest>,
    /// The index documents of the releases the store holds: their
    /// materialized views (`releases/<digest>`) stay.
    pub releases: Vec<Digest>,
}

const INDEX_PREFIX: &str = "agents/index/";

fn is_release_record(key: &str) -> bool {
    (key.starts_with("agents/") && !key.starts_with(INDEX_PREFIX)) || key.starts_with("components/")
}

fn older_than(at: SystemTime, now: SystemTime, grace: Duration) -> bool {
    now.duration_since(at).is_ok_and(|age| age > grace)
}

/// Marks what the store's retained manifests reference, and lists what is
/// left that is older than `grace` at `now`.
pub async fn plan(
    store: &dyn ArtifactStore,
    grace: Duration,
    now: SystemTime,
) -> Result<Plan, ArtifactError> {
    let keys = store.list_manifests("").await?;
    let mut marked: HashSet<Digest> = HashSet::new();
    let mut named_records: HashSet<String> = HashSet::new();
    let mut releases = Vec::new();

    // Releases: each index's document, and the records it names.
    for key in keys.iter().filter(|key| key.starts_with(INDEX_PREFIX)) {
        let Some(manifest) = store.get_manifest(key).await? else {
            continue;
        };
        let document =
            store
                .get(&manifest.content)
                .await?
                .ok_or_else(|| ArtifactError::MissingContent {
                    key: key.clone(),
                    content: manifest.content.clone(),
                })?;
        let index: ReleaseIndex =
            serde_json::from_slice(&document).map_err(|error| ArtifactError::InvalidManifest {
                key: key.clone(),
                message: error.to_string(),
            })?;
        marked.insert(manifest.content.clone());
        releases.push(manifest.content);
        for entry in index.agents.iter().chain(&index.components) {
            named_records.insert(entry.manifest.clone());
            marked.insert(entry.content.clone());
        }
    }

    let mut plan = Plan {
        releases,
        ..Plan::default()
    };
    for key in keys.iter().filter(|key| !key.starts_with(INDEX_PREFIX)) {
        if is_release_record(key) && !named_records.contains(key) {
            let modified = store.manifest_modified(key).await?;
            if modified.is_some_and(|at| older_than(at, now, grace)) {
                plan.records.push(key.clone());
                continue;
            }
        }
        // Retained (or too young to judge): its content stays.
        match store.get_manifest(key).await {
            Ok(Some(manifest)) => {
                marked.insert(manifest.content);
            }
            Ok(None) | Err(ArtifactError::MissingContent { .. }) => {}
            Err(error) => return Err(error),
        }
    }

    for (digest, stored) in store.list_blobs().await? {
        if !marked.contains(&digest) && older_than(stored, now, grace) {
            plan.blobs.push(digest);
        }
    }
    plan.records.sort();
    plan.blobs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilesystemArtifactStore;
    use crate::release::publish_dir;

    async fn bundle(dir: &std::path::Path, description: &str, extra: bool) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        let meta = format!(r#"{{"id":"echo","description":"{description}"}}"#);
        tokio::fs::write(dir.join("runtara_agent_echo.wasm"), b"\0asm echo")
            .await
            .unwrap();
        tokio::fs::write(dir.join("runtara_agent_echo.meta.json"), meta)
            .await
            .unwrap();
        tokio::fs::write(dir.join("runtara_workflow_stdlib.wasm"), b"\0asm stdlib")
            .await
            .unwrap();
        tokio::fs::write(dir.join("runtara_workflow_stdlib.meta.json"), "{}")
            .await
            .unwrap();
        if extra {
            tokio::fs::write(dir.join("runtara_agent_old.wasm"), b"\0asm old only")
                .await
                .unwrap();
            tokio::fs::write(dir.join("runtara_agent_old.meta.json"), r#"{"id":"old"}"#)
                .await
                .unwrap();
        }
    }

    /// Two releases under one name (a dev rebuild): the replaced one's
    /// unique record, content and index document become collectable once
    /// older than the grace; what the current release shares stays.
    #[tokio::test]
    async fn a_replaced_release_is_collected_and_shared_content_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = FilesystemArtifactStore::new(dir.path().join("store"));
        bundle(&dir.path().join("a"), "first", true).await;
        let first = publish_dir(&store, &dir.path().join("a"), "dev")
            .await
            .unwrap();
        bundle(&dir.path().join("b"), "second", false).await;
        let second = publish_dir(&store, &dir.path().join("b"), "dev")
            .await
            .unwrap();
        let workflow_agent = store.put(b"\0asm a workflow-agent").await.unwrap();
        store
            .put_manifest(
                "tenants/00/workflow-agents/x.json",
                &crate::Manifest::new(workflow_agent.clone(), b"{}").unwrap(),
            )
            .await
            .unwrap();

        // Within the grace nothing goes.
        let now = SystemTime::now();
        let young = plan(&store, Duration::from_secs(3600), now).await.unwrap();
        assert!(
            young.records.is_empty() && young.blobs.is_empty(),
            "{young:?}"
        );
        assert_eq!(young.releases, vec![second.digest()], "the current release");

        let later = now + Duration::from_secs(7200);
        let found = plan(&store, Duration::from_secs(3600), later)
            .await
            .unwrap();
        let first_only: Vec<String> = first
            .agents
            .iter()
            .filter(|entry| !second.agents.contains(entry))
            .map(|entry| entry.manifest.clone())
            .collect();
        assert_eq!(found.records, {
            let mut records = first_only;
            records.sort();
            records
        });
        let old = first
            .agents
            .iter()
            .find(|entry| entry.id == "old")
            .unwrap()
            .content
            .clone();
        assert!(
            found.blobs.contains(&old),
            "the replaced release's own content"
        );
        assert!(found.blobs.contains(&first.digest()), "its index document");
        let stdlib = &second.components[0].content;
        assert!(!found.blobs.contains(stdlib), "shared content stays");
        assert!(!found.blobs.contains(&second.digest()));
        assert!(
            !found.blobs.contains(&workflow_agent),
            "a workflow-agent's content stays"
        );
    }
}
