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

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("not a sha256 digest: {0:?}")]
    InvalidDigest(String),
    #[error("artifact store I/O: {0}")]
    Io(#[from] std::io::Error),
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
}

#[async_trait]
impl ArtifactStore for FilesystemArtifactStore {
    async fn put(&self, bytes: &[u8]) -> Result<Digest, ArtifactError> {
        let digest = Digest::of(bytes);
        let path = self.blob(&digest);
        if tokio::fs::try_exists(&path).await? {
            return Ok(digest);
        }
        let blobs = self.blobs();
        tokio::fs::create_dir_all(&blobs).await?;
        // Written beside its final name and renamed into place, so a reader
        // sees the whole artifact or none. A racing writer of the same bytes
        // renames identical content over it.
        let temporary = blobs.join(format!(".upload-{}", uuid::Uuid::new_v4()));
        let written = async {
            let mut file = tokio::fs::File::create(&temporary).await?;
            file.write_all(bytes).await?;
            file.sync_all().await?;
            tokio::fs::rename(&temporary, &path).await
        }
        .await;
        if let Err(error) = written {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error.into());
        }
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
}

#[cfg(feature = "test-support")]
pub mod test_support {
    //! A store over files that already exist, for tests that write a
    //! component somewhere and launch it from there.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{ArtifactError, ArtifactStore, Digest, async_trait};

    /// Artifacts as the files they were added from, by the digest of the
    /// bytes each held when added.
    #[derive(Debug, Default)]
    pub struct FileMapStore {
        files: Mutex<HashMap<Digest, PathBuf>>,
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
