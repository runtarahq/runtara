//! Sealed storage keys against real storage services: Azurite and RustFS
//! check every signature, so a request seal signs, and a URL seal presigns,
//! is accepted only if it is right.
//!
//! The credentials are the emulators' own well-known ones.

use std::sync::Arc;
use std::time::Duration;

use runtara_connections::sealed::{
    Allowances, BoundedResponse, MemoryKeySource, SealedCredentials, is_sealed, policy,
};
use serde_json::{Value, json};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};

const TENANT: &str = "acme";
const CONNECTION: &str = "6f1c5f0e-2a55-4b1c-9d0a-0d6a6a2f8f10";
const AZURE_ACCOUNT: &str = "devstoreaccount1";
const AZURE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
const S3_USER: &str = "runtara-test-access";
const S3_KEY: &str = "runtara-synthetic-test-secret";

/// seal in this process, allowed to reach a storage service on this machine
/// over plain HTTP.
fn sealed_for(host: &str, port: u16) -> SealedCredentials {
    SealedCredentials::in_process(
        Arc::new(MemoryKeySource::default()),
        &[7_u8; 32],
        Allowances {
            private_hosts: vec![format!("{host}:{port}")],
            plain_http_hosts: vec![host.to_owned()],
            ..Default::default()
        },
    )
    .unwrap()
}

/// A sealed storage connection, and what requests through seal need.
struct Storage {
    sealed: SealedCredentials,
    integration_id: &'static str,
    stored: Value,
}

impl Storage {
    async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> BoundedResponse {
        self.sealed
            .send_bounded(
                TENANT,
                CONNECTION,
                self.integration_id,
                &self.stored,
                method,
                url,
                headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
                body.map(<[u8]>::to_vec),
                1024 * 1024,
            )
            .await
            .unwrap_or_else(|error| panic!("{method} {url}: {error:?}"))
    }

    async fn presign(&self, method: &str, url: &str) -> String {
        self.sealed
            .presign(
                TENANT,
                CONNECTION,
                self.integration_id,
                &self.stored,
                method,
                url,
                900,
            )
            .await
            .unwrap_or_else(|error| panic!("presign {method}: {error}"))
            .to_string()
    }

    /// An object is uploaded, downloaded and deleted with URLs alone: no
    /// seal, and no credential but the URL.
    async fn presigned_roundtrip(&self, object_url: &str, upload_headers: &[(&str, &str)]) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let mut upload = client
            .put(self.presign("PUT", object_url).await)
            .body("sealed fixture");
        for (name, value) in upload_headers {
            upload = upload.header(*name, *value);
        }
        let response = upload.send().await.unwrap();
        assert!(
            response.status().is_success(),
            "upload: {}",
            response.status()
        );

        let download = self.presign("GET", object_url).await;
        let response = client.get(&download).send().await.unwrap();
        assert!(
            response.status().is_success(),
            "download: {}",
            response.status()
        );
        assert_eq!(response.text().await.unwrap(), "sealed fixture");

        // A URL that is changed is refused.
        let other = download.replacen("my%20file.txt", "other.txt", 1);
        assert_ne!(other, download);
        let response = client.get(other).send().await.unwrap();
        assert!(
            !response.status().is_success(),
            "a changed URL was accepted"
        );

        let response = client
            .delete(self.presign("DELETE", object_url).await)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "delete: {}",
            response.status()
        );
        assert_eq!(client.get(download).send().await.unwrap().status(), 404);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_azure_key_signs_and_presigns_against_azurite() {
    let container = GenericImage::new("mcr.microsoft.com/azure-storage/azurite", "3.35.0")
        .with_exposed_port(10000.tcp())
        .with_wait_for(WaitFor::message_on_stdout(
            "Azurite Blob service successfully listens",
        ))
        .with_cmd([
            "azurite-blob",
            "--blobHost",
            "0.0.0.0",
            "--skipApiVersionCheck",
        ])
        .start()
        .await
        .expect("required Docker Azurite container must start");
    let host = container.get_host().await.unwrap().to_string();
    let port = container.get_host_port_ipv4(10000).await.unwrap();
    // Azurite listens on every address; seal is told the literal one.
    let host = if host == "localhost" {
        "127.0.0.1".to_owned()
    } else {
        host
    };
    let endpoint = format!("http://{host}:{port}");

    let sealed = sealed_for(&host, port);
    let stored = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "azure_blob_storage",
            &json!({
                "account_name": AZURE_ACCOUNT,
                "account_key": AZURE_KEY,
                "endpoint_override": endpoint,
            }),
            None,
        )
        .await
        .unwrap()
        .expect("azure_blob_storage is sealed");
    assert!(is_sealed(&stored["account_key"]));
    let storage = Storage {
        sealed,
        integration_id: "azure_blob_storage",
        stored,
    };
    let base = format!("{endpoint}/{AZURE_ACCOUNT}");

    // Requests seal signs: a container, a blob, a read, a copy.
    let created = storage
        .send(
            "PUT",
            &format!("{base}/sealed?restype=container"),
            &[],
            None,
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "{:?}",
        String::from_utf8_lossy(&created.body)
    );
    let put = storage
        .send(
            "PUT",
            &format!("{base}/sealed/signed.txt"),
            &[
                ("content-type", "text/plain"),
                ("x-ms-blob-type", "BlockBlob"),
            ],
            Some(b"signed by seal"),
        )
        .await;
    assert_eq!(put.status, 201, "{:?}", String::from_utf8_lossy(&put.body));
    let got = storage
        .send("GET", &format!("{base}/sealed/signed.txt"), &[], None)
        .await;
    assert_eq!(
        (got.status, got.body.as_slice()),
        (200, &b"signed by seal"[..])
    );
    let copied = storage
        .send(
            "PUT",
            &format!("{base}/sealed/copy.txt"),
            &[("x-ms-copy-source", &format!("{base}/sealed/signed.txt"))],
            None,
        )
        .await;
    assert_eq!(
        copied.status,
        202,
        "{:?}",
        String::from_utf8_lossy(&copied.body)
    );
    let listed = storage
        .send(
            "GET",
            &format!("{base}/sealed?restype=container&comp=list"),
            &[],
            None,
        )
        .await;
    assert_eq!(listed.status, 200);
    assert!(String::from_utf8_lossy(&listed.body).contains("copy.txt"));

    // URLs seal presigns.
    storage
        .presigned_roundtrip(
            &format!("{base}/sealed/folder/my%20file.txt"),
            &[
                ("x-ms-blob-type", "BlockBlob"),
                ("content-type", "text/plain"),
            ],
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_s3_key_signs_and_presigns_against_rustfs() {
    // seal before it let a whole endpoint copy between buckets leaves these
    // connections readable; there is then nothing sealed to check.
    if !policy::seal_copies_from_any_bucket() {
        eprintln!("skipped: this build's seal does not seal s3_compatible connections");
        return;
    }
    let container = GenericImage::new("rustfs/rustfs", "1.0.4")
        .with_exposed_port(9000.tcp())
        .with_env_var("RUSTFS_ACCESS_KEY", S3_USER)
        .with_env_var("RUSTFS_SECRET_KEY", S3_KEY)
        .start()
        .await
        .expect("required Docker RustFS container must start");
    let host = container.get_host().await.unwrap().to_string();
    let port = container.get_host_port_ipv4(9000).await.unwrap();
    let host = if host == "localhost" {
        "127.0.0.1".to_owned()
    } else {
        host
    };
    let endpoint = format!("http://{host}:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while !client
            .get(format!("{endpoint}/health/ready"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("RustFS must become ready");

    let sealed = sealed_for(&host, port);
    let stored = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "s3_compatible",
            &json!({
                "endpoint": endpoint,
                "access_key_id": S3_USER,
                "secret_access_key": S3_KEY,
                "region": "us-east-1",
            }),
            None,
        )
        .await
        .unwrap()
        .expect("s3_compatible is sealed");
    assert!(is_sealed(&stored["secret_access_key"]));
    let storage = Storage {
        sealed,
        integration_id: "s3_compatible",
        stored,
    };

    // The service can answer 503 for a moment after it reports ready.
    for bucket in ["sealed", "sealed-other"] {
        let mut status = 0;
        for _ in 0..100 {
            status = storage
                .send("PUT", &format!("{endpoint}/{bucket}"), &[], None)
                .await
                .status;
            if status != 503 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(status, 200, "creating {bucket}");
    }
    let put = storage
        .send(
            "PUT",
            &format!("{endpoint}/sealed-other/signed.txt"),
            &[("content-type", "text/plain")],
            Some(b"signed by seal"),
        )
        .await;
    assert_eq!(put.status, 200, "{:?}", String::from_utf8_lossy(&put.body));
    // A copy from another bucket, which the whole endpoint's key may read.
    let copied = storage
        .send(
            "PUT",
            &format!("{endpoint}/sealed/copy.txt"),
            &[("x-amz-copy-source", "/sealed-other/signed.txt")],
            None,
        )
        .await;
    assert_eq!(
        copied.status,
        200,
        "{:?}",
        String::from_utf8_lossy(&copied.body)
    );
    let got = storage
        .send("GET", &format!("{endpoint}/sealed/copy.txt"), &[], None)
        .await;
    assert_eq!(
        (got.status, got.body.as_slice()),
        (200, &b"signed by seal"[..])
    );
    let listed = storage
        .send(
            "GET",
            &format!("{endpoint}/sealed?list-type=2&prefix=co"),
            &[],
            None,
        )
        .await;
    assert_eq!(listed.status, 200);
    assert!(String::from_utf8_lossy(&listed.body).contains("copy.txt"));

    storage
        .presigned_roundtrip(&format!("{endpoint}/sealed/folder/my%20file.txt"), &[])
        .await;
}
