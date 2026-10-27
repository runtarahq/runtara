// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A release of platform components in the store: its agents
//! (`runtara_agent_*.wasm` with their `.meta.json`) and shared workflow
//! components (the stdlib, the api component, the run shim), each one record
//! of content and metadata, listed by an index pinned by digest.
//!
//! - `agents/<id>/<record>.json` and `components/<name>/<record>.json`: a
//!   [`Manifest`] of the `.wasm`'s digest and the `.meta.json`'s exact bytes.
//!   `<record>` is the digest of the pair, so a key never changes meaning.
//! - `agents/index/<release>.json`: a manifest whose content is the
//!   [`ReleaseIndex`] document.

use std::path::Path;

use crate::{ArtifactError, ArtifactStore, Digest, Manifest, validate_manifest_key};

/// One component of a release: its id, the key of its record and the digest
/// of its content, which the record must name.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseEntry {
    /// An agent's canonical (kebab) id, or a shared component's file stem.
    pub id: String,
    pub manifest: String,
    pub content: Digest,
}

/// What a release holds.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseIndex {
    pub release: String,
    /// Sorted by id.
    pub agents: Vec<ReleaseEntry>,
    /// Sorted by id.
    pub components: Vec<ReleaseEntry>,
}

/// A component read back from the store: its bytes and its metadata's exact
/// bytes.
#[derive(Clone, Debug)]
pub struct ReleasedComponent {
    pub id: String,
    pub wasm: Vec<u8>,
    pub manifest: Manifest,
}

const AGENT_PREFIX: &str = "runtara_agent_";

/// The key of a release's index.
pub fn index_key(release: &str) -> Result<String, ArtifactError> {
    let key = format!("agents/index/{release}.json");
    validate_manifest_key(&key)?;
    Ok(key)
}

fn release_error(message: impl Into<String>) -> ArtifactError {
    ArtifactError::Release(message.into())
}

/// Store every component in `dir` as `release`, and record its index. An
/// agent without its `.meta.json` is refused; a `.wasm` that is neither an
/// agent nor has a `.meta.json` is not a component and is skipped.
pub async fn publish_dir(
    store: &dyn ArtifactStore,
    dir: &Path,
    release: &str,
) -> Result<ReleaseIndex, ArtifactError> {
    let index_key = index_key(release)?;
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        files.push(entry.path());
    }
    files.sort();

    let mut agents = Vec::new();
    let mut components = Vec::new();
    for path in files {
        if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let agent = stem
            .strip_prefix(AGENT_PREFIX)
            .map(|id| id.replace('_', "-"));
        let meta_path = path.with_extension("meta.json");
        let meta = match tokio::fs::read(&meta_path).await {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && agent.is_none() => {
                continue;
            }
            Err(error) => {
                return Err(release_error(format!(
                    "{}: no metadata at {}: {error}",
                    path.display(),
                    meta_path.display()
                )));
            }
        };
        let content = store.put(&tokio::fs::read(&path).await?).await?;
        let manifest = Manifest::new(content.clone(), &meta).map_err(|error| {
            release_error(format!(
                "{}: metadata is not JSON: {error}",
                meta_path.display()
            ))
        })?;
        let mut record = content.as_str().as_bytes().to_vec();
        record.push(b'\n');
        record.extend_from_slice(&meta);
        let record = Digest::of(&record);
        let (id, kind, list) = match agent {
            Some(id) => (id, "agents", &mut agents),
            None => (stem.to_owned(), "components", &mut components),
        };
        let key = format!("{kind}/{id}/{record}.json");
        store.put_manifest(&key, &manifest).await?;
        list.push(ReleaseEntry {
            id,
            manifest: key,
            content,
        });
    }
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    components.sort_by(|a, b| a.id.cmp(&b.id));

    let index = ReleaseIndex {
        release: release.to_owned(),
        agents,
        components,
    };
    let document = serde_json::to_vec(&index).expect("a release index serializes");
    let content = store.put(&document).await?;
    let metadata = serde_json::to_vec(&serde_json::json!({ "release": release }))
        .expect("release metadata serializes");
    let manifest = Manifest::new(content, &metadata).expect("release metadata is JSON");
    store.put_manifest(&index_key, &manifest).await?;
    Ok(index)
}

/// The index of `release`, or `None` when the store holds no such release.
pub async fn load(
    store: &dyn ArtifactStore,
    release: &str,
) -> Result<Option<ReleaseIndex>, ArtifactError> {
    let Some(manifest) = store.get_manifest(&index_key(release)?).await? else {
        return Ok(None);
    };
    let document = read_verified(store, &manifest.content).await?;
    let index: ReleaseIndex = serde_json::from_slice(&document)
        .map_err(|error| release_error(format!("release {release}: malformed index: {error}")))?;
    if index.release != release {
        return Err(release_error(format!(
            "release {release}: index names release {}",
            index.release
        )));
    }
    Ok(Some(index))
}

/// Read one component of a release: its record must name the content the
/// index pinned, and the content must be the bytes of that digest.
pub async fn read(
    store: &dyn ArtifactStore,
    entry: &ReleaseEntry,
) -> Result<ReleasedComponent, ArtifactError> {
    let manifest = store
        .get_manifest(&entry.manifest)
        .await?
        .ok_or_else(|| release_error(format!("{}: no record at {}", entry.id, entry.manifest)))?;
    if manifest.content != entry.content {
        return Err(release_error(format!(
            "{}: record names {} but the release pinned {}",
            entry.id, manifest.content, entry.content
        )));
    }
    let wasm = read_verified(store, &manifest.content).await?;
    Ok(ReleasedComponent {
        id: entry.id.clone(),
        wasm,
        manifest,
    })
}

/// The bytes of `digest`, checked against it.
async fn read_verified(
    store: &dyn ArtifactStore,
    digest: &Digest,
) -> Result<Vec<u8>, ArtifactError> {
    let bytes = store
        .get(digest)
        .await?
        .ok_or_else(|| release_error(format!("content {digest} is not stored")))?;
    if Digest::of(&bytes) != *digest {
        return Err(release_error(format!(
            "content {digest} does not match its digest"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilesystemArtifactStore;

    fn bundle(dir: &Path) {
        std::fs::write(dir.join("runtara_agent_azure_blob.wasm"), b"azure").unwrap();
        std::fs::write(
            dir.join("runtara_agent_azure_blob.meta.json"),
            b"{\n  \"id\": \"azure-blob\"\n}",
        )
        .unwrap();
        std::fs::write(dir.join("runtara_agent_http.wasm"), b"http").unwrap();
        std::fs::write(
            dir.join("runtara_agent_http.meta.json"),
            b"{\"id\":\"http\"}",
        )
        .unwrap();
        std::fs::write(dir.join("runtara_workflow_stdlib.wasm"), b"stdlib").unwrap();
        std::fs::write(
            dir.join("runtara_workflow_stdlib.meta.json"),
            b"{\"kind\":\"shared\"}",
        )
        .unwrap();
        std::fs::write(dir.join("scratch.wasm"), b"not a component").unwrap();
    }

    #[tokio::test]
    async fn a_release_is_published_and_read_back_whole() {
        let bundle_dir = tempfile::tempdir().unwrap();
        bundle(bundle_dir.path());
        let store_dir = tempfile::tempdir().unwrap();
        let store = FilesystemArtifactStore::new(store_dir.path());

        let published = publish_dir(&store, bundle_dir.path(), "9.0.1")
            .await
            .unwrap();
        let ids =
            |entries: &[ReleaseEntry]| entries.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&published.agents), ["azure-blob", "http"]);
        assert_eq!(ids(&published.components), ["runtara_workflow_stdlib"]);

        let loaded = load(&store, "9.0.1").await.unwrap().unwrap();
        assert_eq!(loaded, published);
        let azure = read(&store, &loaded.agents[0]).await.unwrap();
        assert_eq!(azure.wasm, b"azure");
        assert_eq!(
            azure.manifest.metadata_bytes(),
            b"{\n  \"id\": \"azure-blob\"\n}",
            "metadata keeps the bytes it was built with"
        );
        assert_eq!(load(&store, "9.0.2").await.unwrap(), None);

        // Republishing the same bundle writes the same records.
        assert_eq!(
            publish_dir(&store, bundle_dir.path(), "9.0.1")
                .await
                .unwrap(),
            published
        );
    }

    #[tokio::test]
    async fn an_agent_without_metadata_is_refused() {
        let bundle_dir = tempfile::tempdir().unwrap();
        std::fs::write(bundle_dir.path().join("runtara_agent_http.wasm"), b"http").unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store = FilesystemArtifactStore::new(store_dir.path());
        assert!(publish_dir(&store, bundle_dir.path(), "1").await.is_err());
        assert_eq!(
            load(&store, "1").await.unwrap(),
            None,
            "no index for a refused release"
        );
    }

    #[tokio::test]
    async fn a_record_that_does_not_match_its_pin_is_refused() {
        let bundle_dir = tempfile::tempdir().unwrap();
        bundle(bundle_dir.path());
        let store_dir = tempfile::tempdir().unwrap();
        let store = FilesystemArtifactStore::new(store_dir.path());
        let index = publish_dir(&store, bundle_dir.path(), "1").await.unwrap();
        let mut pinned = index.agents[1].clone();
        pinned.content = Digest::of(b"something else");
        assert!(read(&store, &pinned).await.is_err());

        // Bytes changed under their address: refused on read.
        let blob = store_dir
            .path()
            .join("blobs/sha256")
            .join(index.agents[1].content.as_str());
        std::fs::write(blob, b"tampered").unwrap();
        assert!(read(&store, &index.agents[1]).await.is_err());
    }
}
