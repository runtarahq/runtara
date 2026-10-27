// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Content-addressed storage for runtara's artifacts.
//!
//! An artifact is addressed by the SHA-256 of its bytes ([`Digest`]), so a
//! stored artifact never changes under its address and two writers of the
//! same bytes write the same thing. Rows that name an artifact (a workflow
//! image, later an agent release) record its digest, never a path: where the
//! bytes live is the store's business. [`FilesystemArtifactStore`] is the
//! default; an object-storage store implements the same trait.
//!
//! An artifact's record is a [`Manifest`] under a key (`agents/<id>/<version>.json`,
//! `components/<name>/<version>.json`, `agents/index/<release>.json`): its
//! metadata and the digest of its content, written after the content, so the
//! manifest is the commit point and the two cannot drift.

use std::fmt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt;

/// The SHA-256 of an artifact's bytes, as 64 lowercase hex characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest(String);

impl Digest {
    /// The digest of these bytes.
    pub fn of(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }

    /// A digest from its hex form, refusing anything that is not 64 lowercase
    /// hex characters: a digest also names a file, so it must not carry a path.
    pub fn parse(hex: &str) -> Result<Self, ArtifactError> {
        if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(Self(hex.to_owned()))
        } else {
            Err(ArtifactError::InvalidDigest(hex.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl serde::Serialize for Digest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Digest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = <String as serde::Deserialize>::deserialize(deserializer)?;
        Digest::parse(&hex).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("not a sha256 digest: {0:?}")]
    InvalidDigest(String),
    #[error("not a manifest key: {0:?}")]
    InvalidKey(String),
    #[error("manifest {key} names content {content} that is not stored")]
    MissingContent { key: String, content: Digest },
    #[error("manifest {key} is malformed: {message}")]
    InvalidManifest { key: String, message: String },
    #[error("object store: {0}")]
    Store(String),
    #[error("release: {0}")]
    Release(String),
    #[error("artifact store I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// An artifact's record: the digest of its content and its metadata (an
/// agent's `.meta.json`), kept as the exact JSON text it was written with, so
/// anything hashed over the metadata hashes the same bytes it was built from.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub content: Digest,
    pub metadata: Box<serde_json::value::RawValue>,
}

impl Manifest {
    /// A manifest for `content` with `metadata`, which must be JSON.
    pub fn new(content: Digest, metadata: &[u8]) -> Result<Self, serde_json::Error> {
        let text = std::str::from_utf8(metadata).map_err(serde::de::Error::custom)?;
        Ok(Self {
            content,
            metadata: serde_json::value::RawValue::from_string(text.to_owned())?,
        })
    }

    /// The metadata's exact bytes.
    pub fn metadata_bytes(&self) -> &[u8] {
        self.metadata.get().as_bytes()
    }
}

impl PartialEq for Manifest {
    fn eq(&self, other: &Self) -> bool {
        self.content == other.content && self.metadata.get() == other.metadata.get()
    }
}

/// A manifest key: `/`-separated segments of lowercase letters, digits, `.`,
/// `_` and `-`, ending in `.json`, never `.` or `..`. Keys name files and
/// object keys, so nothing else gets in.
pub fn validate_manifest_key(key: &str) -> Result<(), ArtifactError> {
    let valid = key.ends_with(".json")
        && key.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
        });
    if valid {
        Ok(())
    } else {
        Err(ArtifactError::InvalidKey(key.to_owned()))
    }
}

/// Where artifacts are kept, by digest.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Store these bytes and return their digest. Storing bytes that are
    /// already stored is a no-op, and a reader never sees a partial artifact.
    async fn put(&self, bytes: &[u8]) -> Result<Digest, ArtifactError>;

    /// Whether the artifact is stored.
    async fn exists(&self, digest: &Digest) -> Result<bool, ArtifactError>;

    /// The artifact's bytes, or `None` when it is not stored.
    async fn get(&self, digest: &Digest) -> Result<Option<Vec<u8>>, ArtifactError>;

    /// A local file holding the artifact, for readers that need a path (the
    /// embedded runner's precompiler), or `None` when it is not stored. A
    /// store that keeps artifacts elsewhere fetches it into a local cache.
    /// The path holds exactly the digest's bytes for as long as the artifact
    /// is stored, so it is safe to cache by.
    async fn local_path(&self, digest: &Digest) -> Result<Option<PathBuf>, ArtifactError>;

    /// Remove the artifact; `false` when it was not stored. The caller owns
    /// knowing that nothing references it any more.
    async fn delete(&self, digest: &Digest) -> Result<bool, ArtifactError>;

    /// A URL that fetches the artifact's content with no credentials, valid
    /// for `ttl`, for a reader that must not hold the store's (a fleet node).
    /// `None` when the store cannot sign URLs (a filesystem) or the artifact is
    /// not stored. The reader still verifies the bytes against the digest.
    async fn presigned_get(
        &self,
        digest: &Digest,
        ttl: std::time::Duration,
    ) -> Result<Option<String>, ArtifactError> {
        let _ = (digest, ttl);
        Ok(None)
    }

    /// Record `manifest` under `key`, replacing what was there. Refused when
    /// its content is not stored: content first, then the manifest.
    async fn put_manifest(&self, key: &str, manifest: &Manifest) -> Result<(), ArtifactError>;

    /// The manifest under `key`, or `None`. A manifest whose content is not
    /// stored is refused, never half-loaded.
    async fn get_manifest(&self, key: &str) -> Result<Option<Manifest>, ArtifactError>;

    /// The keys of every manifest under `prefix` (a `/`-terminated key
    /// prefix, or empty for all), sorted.
    async fn list_manifests(&self, prefix: &str) -> Result<Vec<String>, ArtifactError>;

    /// Remove the manifest under `key`; `false` when there was none. Its
    /// content stays: other records may name it.
    async fn delete_manifest(&self, key: &str) -> Result<bool, ArtifactError>;
}

/// Artifacts as files under a root directory: `blobs/sha256/<digest>`.
#[derive(Clone, Debug)]
pub struct FilesystemArtifactStore {
    root: PathBuf,
}

impl FilesystemArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn blobs(&self) -> PathBuf {
        self.root.join("blobs").join("sha256")
    }

    fn blob(&self, digest: &Digest) -> PathBuf {
        self.blobs().join(digest.as_str())
    }

    fn manifest(&self, key: &str) -> Result<PathBuf, ArtifactError> {
        validate_manifest_key(key)?;
        Ok(self.root.join("manifests").join(key))
    }
}

/// Write `bytes` to `path` whole or not at all: beside it, then renamed.
async fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), ArtifactError> {
    let dir = path.parent().expect("artifact paths have a parent");
    tokio::fs::create_dir_all(dir).await?;
    let temporary = dir.join(format!(".upload-{}", uuid::Uuid::new_v4()));
    let written = async {
        let mut file = tokio::fs::File::create(&temporary).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&temporary, path).await
    }
    .await;
    if let Err(error) = written {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    Ok(())
}

#[async_trait]
impl ArtifactStore for FilesystemArtifactStore {
    async fn put(&self, bytes: &[u8]) -> Result<Digest, ArtifactError> {
        let digest = Digest::of(bytes);
        let path = self.blob(&digest);
        if tokio::fs::try_exists(&path).await? {
            return Ok(digest);
        }
        // A racing writer of the same bytes renames identical content over it.
        write_atomically(&path, bytes).await?;
        Ok(digest)
    }

    async fn exists(&self, digest: &Digest) -> Result<bool, ArtifactError> {
        Ok(tokio::fs::try_exists(self.blob(digest)).await?)
    }

    async fn get(&self, digest: &Digest) -> Result<Option<Vec<u8>>, ArtifactError> {
        match tokio::fs::read(self.blob(digest)).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn local_path(&self, digest: &Digest) -> Result<Option<PathBuf>, ArtifactError> {
        let path = self.blob(digest);
        Ok(tokio::fs::try_exists(&path).await?.then_some(path))
    }

    async fn delete(&self, digest: &Digest) -> Result<bool, ArtifactError> {
        match tokio::fs::remove_file(self.blob(digest)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    async fn put_manifest(&self, key: &str, manifest: &Manifest) -> Result<(), ArtifactError> {
        let path = self.manifest(key)?;
        if !self.exists(&manifest.content).await? {
            return Err(ArtifactError::MissingContent {
                key: key.to_owned(),
                content: manifest.content.clone(),
            });
        }
        let bytes = serde_json::to_vec(manifest).expect("a manifest serializes");
        write_atomically(&path, &bytes).await
    }

    async fn get_manifest(&self, key: &str) -> Result<Option<Manifest>, ArtifactError> {
        let bytes = match tokio::fs::read(self.manifest(key)?).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|error| ArtifactError::InvalidManifest {
                key: key.to_owned(),
                message: error.to_string(),
            })?;
        if !self.exists(&manifest.content).await? {
            return Err(ArtifactError::MissingContent {
                key: key.to_owned(),
                content: manifest.content,
            });
        }
        Ok(Some(manifest))
    }

    async fn list_manifests(&self, prefix: &str) -> Result<Vec<String>, ArtifactError> {
        let root = self.root.join("manifests");
        let mut keys = Vec::new();
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if entry.file_type().await?.is_dir() {
                    pending.push(path);
                    continue;
                }
                let Ok(relative) = path.strip_prefix(&root) else {
                    continue;
                };
                let key = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                // Uploads in flight are not keys.
                if key.starts_with(prefix) && validate_manifest_key(&key).is_ok() {
                    keys.push(key);
                }
            }
        }
        keys.sort();
        Ok(keys)
    }

    async fn delete_manifest(&self, key: &str) -> Result<bool, ArtifactError> {
        match tokio::fs::remove_file(self.manifest(key)?).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(feature = "object-store")]
pub mod object;
pub mod release;

#[cfg(feature = "test-support")]
pub mod test_support {
    //! A store over files that already exist, for tests that write a
    //! component somewhere and launch it from there.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{ArtifactError, ArtifactStore, Digest, Manifest, async_trait};

    /// Artifacts as the files they were added from, by the digest of the
    /// bytes each held when added.
    #[derive(Debug, Default)]
    pub struct FileMapStore {
        files: Mutex<HashMap<Digest, PathBuf>>,
        manifests: Mutex<HashMap<String, Manifest>>,
    }

    impl FileMapStore {
        pub fn new() -> Self {
            Self::default()
        }

        /// The digest of the file at `path`, which the store now serves from
        /// there. A path with no file gets an address with nothing stored
        /// under it, for tests of a missing artifact.
        pub fn add(&self, path: &Path) -> Digest {
            let Ok(bytes) = std::fs::read(path) else {
                return Digest::of(path.as_os_str().as_encoded_bytes());
            };
            let digest = Digest::of(&bytes);
            self.files
                .lock()
                .unwrap()
                .insert(digest.clone(), path.to_owned());
            digest
        }

        fn path(&self, digest: &Digest) -> Option<PathBuf> {
            self.files.lock().unwrap().get(digest).cloned()
        }
    }

    #[async_trait]
    impl ArtifactStore for FileMapStore {
        async fn put(&self, _bytes: &[u8]) -> Result<Digest, ArtifactError> {
            unimplemented!("FileMapStore serves files added with add()")
        }

        async fn exists(&self, digest: &Digest) -> Result<bool, ArtifactError> {
            Ok(self.path(digest).is_some_and(|path| path.is_file()))
        }

        async fn get(&self, digest: &Digest) -> Result<Option<Vec<u8>>, ArtifactError> {
            match self.path(digest) {
                Some(path) => Ok(std::fs::read(path).ok()),
                None => Ok(None),
            }
        }

        async fn local_path(&self, digest: &Digest) -> Result<Option<PathBuf>, ArtifactError> {
            Ok(self.path(digest).filter(|path| path.is_file()))
        }

        async fn delete(&self, digest: &Digest) -> Result<bool, ArtifactError> {
            Ok(self.files.lock().unwrap().remove(digest).is_some())
        }

        async fn put_manifest(&self, key: &str, manifest: &Manifest) -> Result<(), ArtifactError> {
            super::validate_manifest_key(key)?;
            self.manifests
                .lock()
                .unwrap()
                .insert(key.to_owned(), manifest.clone());
            Ok(())
        }

        async fn get_manifest(&self, key: &str) -> Result<Option<Manifest>, ArtifactError> {
            super::validate_manifest_key(key)?;
            Ok(self.manifests.lock().unwrap().get(key).cloned())
        }

        async fn list_manifests(&self, prefix: &str) -> Result<Vec<String>, ArtifactError> {
            let mut keys: Vec<String> = self
                .manifests
                .lock()
                .unwrap()
                .keys()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect();
            keys.sort();
            Ok(keys)
        }

        async fn delete_manifest(&self, key: &str) -> Result<bool, ArtifactError> {
            Ok(self.manifests.lock().unwrap().remove(key).is_some())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_sha256_hex_and_nothing_else() {
        let digest = Digest::of(b"abc");
        assert_eq!(
            digest.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Digest::parse(digest.as_str()).unwrap(), digest);
        for bad in [
            "",
            "../etc/passwd",
            &digest.as_str().to_uppercase(),
            &digest.as_str()[1..],
        ] {
            assert!(Digest::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn manifest_keys_are_relative_json_paths() {
        for good in [
            "agents/http/1.2.0.json",
            "agents/index/9.0.1.json",
            "components/stdlib/a_b-c.json",
        ] {
            validate_manifest_key(good).unwrap();
        }
        for bad in [
            "",
            "agents/../x.json",
            "/abs.json",
            "agents//x.json",
            "agents/x",
            "Agents/x.json",
            "agents/./x.json",
        ] {
            assert!(validate_manifest_key(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_manifest_is_committed_after_its_content_and_never_half_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let store = FilesystemArtifactStore::new(dir.path());
        let meta = b"{\n  \"id\": \"http\"\n}";
        let missing = Manifest::new(Digest::of(b"agent"), meta).unwrap();
        assert!(matches!(
            store.put_manifest("agents/http/1.json", &missing).await,
            Err(ArtifactError::MissingContent { .. })
        ));
        assert_eq!(
            store.get_manifest("agents/http/1.json").await.unwrap(),
            None
        );

        store.put(b"agent").await.unwrap();
        store
            .put_manifest("agents/http/1.json", &missing)
            .await
            .unwrap();
        let read = store
            .get_manifest("agents/http/1.json")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read, missing);
        assert_eq!(
            read.metadata_bytes(),
            meta,
            "metadata keeps its exact bytes"
        );

        store.put(b"other").await.unwrap();
        let other = Manifest::new(Digest::of(b"other"), b"{}").unwrap();
        store
            .put_manifest("agents/zeta/2.json", &other)
            .await
            .unwrap();
        assert_eq!(
            store.list_manifests("agents/").await.unwrap(),
            ["agents/http/1.json", "agents/zeta/2.json"]
        );
        assert_eq!(
            store.list_manifests("agents/zeta/").await.unwrap(),
            ["agents/zeta/2.json"]
        );
        assert!(store.delete_manifest("agents/zeta/2.json").await.unwrap());
        assert!(!store.delete_manifest("agents/zeta/2.json").await.unwrap());
        assert!(store.exists(&other.content).await.unwrap(), "content stays");

        // Content lost from under its manifest: refused, not served.
        store.delete(&missing.content).await.unwrap();
        assert!(matches!(
            store.get_manifest("agents/http/1.json").await,
            Err(ArtifactError::MissingContent { .. })
        ));
    }

    #[tokio::test]
    async fn artifacts_are_stored_by_their_digest() {
        let dir = tempfile::tempdir().unwrap();
        let store = FilesystemArtifactStore::new(dir.path());
        let digest = store.put(b"workflow").await.unwrap();
        assert_eq!(digest, Digest::of(b"workflow"));
        assert_eq!(store.put(b"workflow").await.unwrap(), digest);
        assert!(store.exists(&digest).await.unwrap());
        assert_eq!(store.get(&digest).await.unwrap().unwrap(), b"workflow");
        let path = store.local_path(&digest).await.unwrap().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"workflow");

        assert!(store.delete(&digest).await.unwrap());
        assert!(!store.delete(&digest).await.unwrap());
        assert!(!store.exists(&digest).await.unwrap());
        assert_eq!(store.get(&digest).await.unwrap(), None);
        assert_eq!(store.local_path(&digest).await.unwrap(), None);
        // No upload leftovers.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("blobs/sha256"))
            .unwrap()
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }
}
