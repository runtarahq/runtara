// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The object-storage artifact store against a real S3-compatible service:
//! a MinIO container started for the test (needs docker and the
//! `minio/minio` image).

use std::process::Command;

use runtara_artifact_store::object::ObjectArtifactStore;
use runtara_artifact_store::{ArtifactStore, Digest, Manifest, release};

/// Test-only credentials of the throwaway container.
const USER: &str = "runtara-test";
const SECRET: &str = "runtara-test-secret";
const BUCKET: &str = "runtara-artifacts";

/// A MinIO container with one bucket, removed when dropped.
struct Minio {
    container: String,
    endpoint: String,
}

impl Minio {
    fn start() -> Self {
        let output = Command::new("docker")
            .args(["run", "-d", "--rm", "-p", "127.0.0.1::9000"])
            .args(["-e", &format!("MINIO_ROOT_USER={USER}")])
            .args(["-e", &format!("MINIO_ROOT_PASSWORD={SECRET}")])
            .args(["minio/minio:latest", "server", "/data"])
            .output()
            .expect("docker is required for the object-store integration tests");
        assert!(output.status.success(), "docker run: {output:?}");
        let container = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        let port = Command::new("docker")
            .args(["port", &container, "9000/tcp"])
            .output()
            .unwrap();
        let mapped = String::from_utf8(port.stdout).unwrap();
        let address = mapped
            .lines()
            .next()
            .expect("a mapped port")
            .trim()
            .to_owned();
        let minio = Self {
            endpoint: format!("http://{address}"),
            container,
        };
        // Ready once the bucket can be made.
        let mut made = false;
        for _ in 0..60 {
            let status = Command::new("docker")
                .args(["exec", &minio.container, "sh", "-c"])
                .arg(format!(
                    "mc alias set local http://127.0.0.1:9000 {USER} {SECRET} >/dev/null 2>&1 \
                     && mc mb --ignore-existing local/{BUCKET} >/dev/null 2>&1"
                ))
                .status()
                .unwrap();
            if status.success() {
                made = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        assert!(made, "MinIO did not come up");
        minio
    }

    fn store(&self, prefix: &str, cache: &std::path::Path) -> ObjectArtifactStore {
        ObjectArtifactStore::from_url_with_options(
            &format!("s3://{BUCKET}/{prefix}"),
            [
                ("aws_endpoint", self.endpoint.as_str()),
                ("aws_allow_http", "true"),
                ("aws_region", "us-east-1"),
                ("aws_access_key_id", USER),
                ("aws_secret_access_key", SECRET),
            ]
            .map(|(key, value)| (key.to_owned(), value.to_owned())),
            cache,
        )
        .expect("an S3 artifact store")
    }
}

impl Drop for Minio {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output();
    }
}

#[tokio::test]
async fn a_bucket_holds_content_records_and_releases() {
    let minio = Minio::start();
    let cache = tempfile::tempdir().unwrap();
    let store = minio.store("tenant-a", cache.path());

    let digest = store.put(b"workflow").await.unwrap();
    assert_eq!(digest, Digest::of(b"workflow"));
    assert!(store.exists(&digest).await.unwrap());
    assert_eq!(store.get(&digest).await.unwrap().unwrap(), b"workflow");
    let local = store.local_path(&digest).await.unwrap().unwrap();
    assert_eq!(std::fs::read(local).unwrap(), b"workflow");
    assert_eq!(store.get(&Digest::of(b"absent")).await.unwrap(), None);

    // A record is refused before its content, and never served without it.
    let meta = b"{\n  \"id\": \"http\"\n}";
    let orphan = Manifest::new(Digest::of(b"agent"), meta).unwrap();
    assert!(
        store
            .put_manifest("agents/http/1.json", &orphan)
            .await
            .is_err()
    );
    store.put(b"agent").await.unwrap();
    store
        .put_manifest("agents/http/1.json", &orphan)
        .await
        .unwrap();
    let read = store
        .get_manifest("agents/http/1.json")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.metadata_bytes(), meta);
    assert_eq!(
        store.list_manifests("agents/").await.unwrap(),
        ["agents/http/1.json"]
    );

    // A prefix is its own store.
    let other = minio.store("tenant-b", cache.path());
    assert!(other.list_manifests("").await.unwrap().is_empty());

    // A release published to the bucket loads back whole.
    let bundle = tempfile::tempdir().unwrap();
    std::fs::write(bundle.path().join("runtara_agent_http.wasm"), b"http").unwrap();
    std::fs::write(
        bundle.path().join("runtara_agent_http.meta.json"),
        b"{\"id\":\"http\"}",
    )
    .unwrap();
    let published = release::publish_dir(&store, bundle.path(), "9.0.1")
        .await
        .unwrap();
    let loaded = release::load(&store, "9.0.1").await.unwrap().unwrap();
    assert_eq!(loaded, published);
    let view = cache.path().join("view");
    release::materialize(&store, &loaded, &view).await.unwrap();
    assert_eq!(
        std::fs::read(view.join("runtara_agent_http.wasm")).unwrap(),
        b"http"
    );

    // A presigned URL fetches the content with no credentials, until it expires.
    let url = store
        .presigned_get(&digest, std::time::Duration::from_secs(60))
        .await
        .unwrap()
        .expect("an S3 store signs URLs");
    assert!(url.contains("/tenant-a/blobs/sha256/"), "{url}");
    let fetched = reqwest::get(&url).await.unwrap();
    assert!(fetched.status().is_success(), "{}", fetched.status());
    assert_eq!(Digest::of(&fetched.bytes().await.unwrap()), digest);
    let short = store
        .presigned_get(&digest, std::time::Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        reqwest::get(&short).await.unwrap().status().as_u16(),
        403,
        "expired"
    );
    assert_eq!(
        store
            .presigned_get(&Digest::of(b"absent"), std::time::Duration::from_secs(60))
            .await
            .unwrap(),
        None
    );

    assert!(store.delete_manifest("agents/http/1.json").await.unwrap());
    assert!(store.delete(&digest).await.unwrap());
    assert!(!store.exists(&digest).await.unwrap());
}
