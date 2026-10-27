// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The artifact store on the [`object_store`] crate: S3-compatible storage,
//! Azure Blob, GCS or the local filesystem, with the filesystem store's
//! layout (`blobs/sha256/<digest>`, `manifests/<key>`), so a bucket and a
//! directory hold the same thing.
//!
//! Content is verified against its digest on every read, so a bucket can
//! never hand back bytes other than the ones a record pins. Readers that need
//! a file get one from a local cache of verified content.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::TryStreamExt;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};

use crate::{ArtifactError, ArtifactStore, Digest, Manifest, validate_manifest_key};

/// Artifacts in an object store, with a local cache of verified content.
#[derive(Debug)]
pub struct ObjectArtifactStore {
    store: Arc<dyn ObjectStore>,
    cache: PathBuf,
}

/// The environment variables `from_url` reads options from: the standard
/// ones of each provider (`AWS_ENDPOINT`, `AWS_ACCESS_KEY_ID`, ...).
const OPTION_PREFIXES: [&str; 3] = ["AWS_", "AZURE_", "GOOGLE_"];

fn store_error(error: object_store::Error) -> ArtifactError {
    ArtifactError::Store(error.to_string())
}

fn is_not_found(error: &object_store::Error) -> bool {
    matches!(error, object_store::Error::NotFound { .. })
}

impl ObjectArtifactStore {
    /// A store over `store`, caching content for local readers in `cache`.
    pub fn new(store: Arc<dyn ObjectStore>, cache: impl Into<PathBuf>) -> Self {
        Self {
            store,
            cache: cache.into(),
        }
    }

    /// A store at `url` (`s3://bucket/prefix`, `gs://...`, `az://...`,
    /// `file:///...`), configured from the provider's environment variables.
    pub fn from_url(url: &str, cache: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let options = std::env::vars().filter_map(|(name, value)| {
            OPTION_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
                .then(|| (name.to_ascii_lowercase(), value))
        });
        Self::from_url_with_options(url, options, cache)
    }

    /// A store at `url` configured with `options` (`aws_endpoint`,
    /// `aws_access_key_id`, ...), as the provider's builder names them.
    pub fn from_url_with_options(
        url: &str,
        options: impl IntoIterator<Item = (String, String)>,
        cache: impl Into<PathBuf>,
    ) -> Result<Self, ArtifactError> {
        // TLS for the cloud providers is rustls on ring; the process picks
        // its provider once, and ring is the workspace's.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let parsed = url::Url::parse(url).map_err(|error| {
            ArtifactError::Store(format!("artifact store URL {url:?}: {error}"))
        })?;
        let (store, prefix) =
            object_store::parse_url_opts(&parsed, options).map_err(store_error)?;
        let store: Arc<dyn ObjectStore> = if prefix.as_ref().is_empty() {
            Arc::from(store)
        } else {
            Arc::new(object_store::prefix::PrefixStore::new(store, prefix))
        };
        Ok(Self::new(store, cache))
    }

    fn blob(digest: &Digest) -> Path {
        Path::from(format!("blobs/sha256/{digest}"))
    }

    fn manifest(key: &str) -> Result<Path, ArtifactError> {
        validate_manifest_key(key)?;
        Ok(Path::from(format!("manifests/{key}")))
    }

    fn cached(&self, digest: &Digest) -> PathBuf {
        self.cache
            .join("blobs")
            .join("sha256")
            .join(digest.as_str())
    }

    async fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, ArtifactError> {
        match self.store.get(path).await {
            Ok(result) => Ok(Some(result.bytes().await.map_err(store_error)?.to_vec())),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    async fn present(&self, path: &Path) -> Result<bool, ArtifactError> {
        match self.store.head(path).await {
            Ok(_) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(store_error(error)),
        }
    }
}

/// Write `bytes` to `path` whole or not at all.
async fn write_atomically(path: &FsPath, bytes: &[u8]) -> Result<(), ArtifactError> {
    let dir = path.parent().expect("cache paths have a parent");
    tokio::fs::create_dir_all(dir).await?;
    let temporary = dir.join(format!(".download-{}", uuid::Uuid::new_v4()));
    if let Err(error) = async {
        tokio::fs::write(&temporary, bytes).await?;
        tokio::fs::rename(&temporary, path).await
    }
    .await
    {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    Ok(())
}

#[async_trait]
impl ArtifactStore for ObjectArtifactStore {
    async fn put(&self, bytes: &[u8]) -> Result<Digest, ArtifactError> {
        let digest = Digest::of(bytes);
        let path = Self::blob(&digest);
        if !self.present(&path).await? {
            // An object store's put is atomic: readers see all of it or none.
            self.store
                .put(&path, PutPayload::from(bytes.to_vec()))
                .await
                .map_err(store_error)?;
        }
        Ok(digest)
    }

    async fn exists(&self, digest: &Digest) -> Result<bool, ArtifactError> {
        self.present(&Self::blob(digest)).await
    }

    async fn get(&self, digest: &Digest) -> Result<Option<Vec<u8>>, ArtifactError> {
        let Some(bytes) = self.read(&Self::blob(digest)).await? else {
            return Ok(None);
        };
        if Digest::of(&bytes) != *digest {
            return Err(ArtifactError::Store(format!(
                "content {digest} does not match its digest"
            )));
        }
        Ok(Some(bytes))
    }

    async fn local_path(&self, digest: &Digest) -> Result<Option<PathBuf>, ArtifactError> {
        let path = self.cached(digest);
        if tokio::fs::try_exists(&path).await? {
            return Ok(Some(path));
        }
        let Some(bytes) = self.get(digest).await? else {
            return Ok(None);
        };
        write_atomically(&path, &bytes).await?;
        Ok(Some(path))
    }

    async fn delete(&self, digest: &Digest) -> Result<bool, ArtifactError> {
        let path = Self::blob(digest);
        let present = self.present(&path).await?;
        if present {
            self.store.delete(&path).await.map_err(store_error)?;
        }
        match tokio::fs::remove_file(self.cached(digest)).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error.into());
            }
            _ => {}
        }
        Ok(present)
    }

    async fn put_manifest(&self, key: &str, manifest: &Manifest) -> Result<(), ArtifactError> {
        let path = Self::manifest(key)?;
        if !self.exists(&manifest.content).await? {
            return Err(ArtifactError::MissingContent {
                key: key.to_owned(),
                content: manifest.content.clone(),
            });
        }
        let bytes = serde_json::to_vec(manifest).expect("a manifest serializes");
        self.store
            .put(&path, PutPayload::from(bytes))
            .await
            .map_err(store_error)?;
        Ok(())
    }

    async fn get_manifest(&self, key: &str) -> Result<Option<Manifest>, ArtifactError> {
        let Some(bytes) = self.read(&Self::manifest(key)?).await? else {
            return Ok(None);
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
        // `list` matches whole path segments, so list the deepest directory
        // the prefix names and filter the rest.
        let directory = match prefix.rfind('/') {
            Some(end) => format!("manifests/{}", &prefix[..end]),
            None => "manifests".to_owned(),
        };
        let objects: Vec<_> = self
            .store
            .list(Some(&Path::from(directory)))
            .try_collect()
            .await
            .map_err(store_error)?;
        let mut keys: Vec<String> = objects
            .into_iter()
            .filter_map(|object| {
                object
                    .location
                    .as_ref()
                    .strip_prefix("manifests/")
                    .map(str::to_owned)
            })
            .filter(|key| key.starts_with(prefix) && validate_manifest_key(key).is_ok())
            .collect();
        keys.sort();
        Ok(keys)
    }

    async fn delete_manifest(&self, key: &str) -> Result<bool, ArtifactError> {
        let path = Self::manifest(key)?;
        if !self.present(&path).await? {
            return Ok(false);
        }
        self.store.delete(&path).await.map_err(store_error)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The object store over a local directory holds exactly the filesystem
    /// store's layout: either reads what the other wrote.
    #[tokio::test]
    async fn a_local_object_store_shares_the_filesystem_layout() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let url = url::Url::from_directory_path(root.path()).unwrap();
        let objects = ObjectArtifactStore::from_url(url.as_str(), cache.path()).unwrap();
        let files = crate::FilesystemArtifactStore::new(root.path());

        let digest = objects.put(b"workflow").await.unwrap();
        assert_eq!(files.get(&digest).await.unwrap().unwrap(), b"workflow");
        let manifest = Manifest::new(digest.clone(), b"{\"id\": \"http\"}").unwrap();
        objects
            .put_manifest("agents/http/1.json", &manifest)
            .await
            .unwrap();
        assert_eq!(
            files.get_manifest("agents/http/1.json").await.unwrap(),
            Some(manifest)
        );

        let other = files.put(b"other").await.unwrap();
        files
            .put_manifest(
                "agents/zeta/2.json",
                &Manifest::new(other.clone(), b"{}").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            objects.list_manifests("agents/").await.unwrap(),
            ["agents/http/1.json", "agents/zeta/2.json"]
        );
        assert_eq!(
            objects.list_manifests("agents/z").await.unwrap(),
            ["agents/zeta/2.json"]
        );
        let local = objects.local_path(&other).await.unwrap().unwrap();
        assert!(local.starts_with(cache.path()));
        assert_eq!(std::fs::read(local).unwrap(), b"other");

        // Bytes changed under their address are refused, not served.
        std::fs::write(
            root.path().join("blobs/sha256").join(digest.as_str()),
            b"tampered",
        )
        .unwrap();
        assert!(objects.get(&digest).await.is_err());

        assert!(objects.delete_manifest("agents/zeta/2.json").await.unwrap());
        assert!(!objects.delete_manifest("agents/zeta/2.json").await.unwrap());
        assert!(objects.delete(&other).await.unwrap());
        assert!(!objects.exists(&other).await.unwrap());
    }
}
