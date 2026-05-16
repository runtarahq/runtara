//! Integration tests: exercise Azure Shared Key signing and Service SAS
//! generation against a real Azurite instance running in Docker.
//!
//! These tests spin up a fresh Azurite container per test via testcontainers.
//! They prove that our canonical-string formats are byte-accurate against a
//! real Azure-compatible endpoint — unit tests can only verify our internal
//! invariants.
//!
//! Requires Docker. If Docker is not running, the tests will fail at the
//! container start step with a clear error.

use std::collections::HashMap;
use std::time::Duration;

use runtara_connections::auth::{azure_sas, azure_signing};
use testcontainers::core::{ContainerPort, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};

/// Azurite's well-known emulator credentials (publicly documented).
const ACCOUNT: &str = "devstoreaccount1";
const KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
const BLOB_PORT: u16 = 10000;

struct AzuriteFixture {
    /// Container handle — dropping it stops Azurite.
    _container: testcontainers::ContainerAsync<GenericImage>,
    /// Base URL for the emulator account (path-style — Azurite always is).
    /// Includes the account name segment.
    pub base_url: String,
    pub client: reqwest::Client,
}

impl AzuriteFixture {
    async fn start() -> Self {
        // Azurite emits "Azurite Blob service successfully listens on …" on stdout.
        let container = GenericImage::new("mcr.microsoft.com/azure-storage/azurite", "latest")
            .with_exposed_port(ContainerPort::Tcp(BLOB_PORT))
            .with_wait_for(WaitFor::message_on_stdout(
                "Azurite Blob service successfully listens",
            ))
            .with_cmd([
                "azurite-blob".to_string(),
                "--blobHost".to_string(),
                "0.0.0.0".to_string(),
                "--skipApiVersionCheck".to_string(),
            ])
            .start()
            .await
            .expect("failed to start Azurite container — is Docker running?");
        let port = container
            .get_host_port_ipv4(BLOB_PORT.tcp())
            .await
            .expect("get mapped port");
        let base_url = format!("http://127.0.0.1:{}/{}", port, ACCOUNT);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap();
        Self {
            _container: container,
            base_url,
            client,
        }
    }

    /// Build, sign, and send a Shared Key authenticated request. Returns the response.
    async fn send_signed(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        extra_headers: &[(&str, &str)],
    ) -> reqwest::Response {
        let url_str = format!("{}{}", self.base_url, path);
        let url = url::Url::parse(&url_str).expect("parse url");

        let mut headers: HashMap<String, String> = extra_headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();

        azure_signing::sign_request_shared_key(method, &url, &mut headers, body, ACCOUNT, KEY)
            .expect("sign request");

        let mut request = self
            .client
            .request(method.parse().unwrap(), &url_str)
            .body(body.to_vec());
        for (k, v) in &headers {
            request = request.header(k, v);
        }
        request.send().await.expect("send request")
    }
}

#[tokio::test]
async fn shared_key_full_blob_lifecycle_against_azurite() {
    let fixture = AzuriteFixture::start().await;

    // 1. Create a container
    let create = fixture
        .send_signed("PUT", "/lifecycle?restype=container", b"", &[])
        .await;
    assert!(
        create.status().is_success() || create.status().as_u16() == 409,
        "create container: {} body={}",
        create.status(),
        create.text().await.unwrap_or_default()
    );

    // 2. Upload a blob
    let body = b"hello from azurite";
    let put = fixture
        .send_signed(
            "PUT",
            "/lifecycle/hello.txt",
            body,
            &[
                ("Content-Type", "text/plain"),
                ("x-ms-blob-type", "BlockBlob"),
            ],
        )
        .await;
    assert!(
        put.status().is_success(),
        "put blob: {} body={}",
        put.status(),
        put.text().await.unwrap_or_default()
    );

    // 3. List blobs and confirm ours is there
    let list = fixture
        .send_signed("GET", "/lifecycle?restype=container&comp=list", b"", &[])
        .await;
    let list_status = list.status();
    let list_body = list.text().await.unwrap_or_default();
    assert!(
        list_status.is_success(),
        "list blobs: {} body={}",
        list_status,
        list_body
    );
    assert!(
        list_body.contains("<Name>hello.txt</Name>"),
        "list output missing uploaded blob: {}",
        list_body
    );

    // 4. Download and verify
    let get = fixture
        .send_signed("GET", "/lifecycle/hello.txt", b"", &[])
        .await;
    assert!(get.status().is_success(), "get blob: {}", get.status());
    let downloaded = get.bytes().await.expect("read body");
    assert_eq!(downloaded.as_ref(), body);

    // 5. HEAD for metadata
    let head = fixture
        .send_signed("HEAD", "/lifecycle/hello.txt", b"", &[])
        .await;
    assert!(head.status().is_success(), "head blob: {}", head.status());
    assert_eq!(
        head.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some(body.len().to_string().as_str())
    );

    // 6. Delete the blob
    let del = fixture
        .send_signed("DELETE", "/lifecycle/hello.txt", b"", &[])
        .await;
    assert!(
        del.status().is_success() || del.status().as_u16() == 202,
        "delete blob: {}",
        del.status()
    );

    // 7. Delete the container
    let del_c = fixture
        .send_signed("DELETE", "/lifecycle?restype=container", b"", &[])
        .await;
    assert!(
        del_c.status().is_success() || del_c.status().as_u16() == 202,
        "delete container: {}",
        del_c.status()
    );
}

#[tokio::test]
async fn shared_key_signs_query_params_correctly_with_prefix_filter() {
    // This guards against canonical-resource regressions with sorted/joined query params.
    let fixture = AzuriteFixture::start().await;

    fixture
        .send_signed("PUT", "/prefixtest?restype=container", b"", &[])
        .await;
    for name in [
        "a.txt",
        "b.txt",
        "reports/2026-05-16.csv",
        "reports/older.csv",
    ] {
        fixture
            .send_signed(
                "PUT",
                &format!("/prefixtest/{}", name),
                name.as_bytes(),
                &[
                    ("Content-Type", "text/plain"),
                    ("x-ms-blob-type", "BlockBlob"),
                ],
            )
            .await;
    }

    let resp = fixture
        .send_signed(
            "GET",
            "/prefixtest?restype=container&comp=list&prefix=reports/",
            b"",
            &[],
        )
        .await;
    assert!(
        resp.status().is_success(),
        "list with prefix: {}",
        resp.status()
    );
    let xml = resp.text().await.unwrap();
    assert!(
        xml.contains("<Name>reports/2026-05-16.csv</Name>"),
        "{}",
        xml
    );
    assert!(xml.contains("<Name>reports/older.csv</Name>"), "{}", xml);
    assert!(!xml.contains("<Name>a.txt</Name>"), "{}", xml);
}

#[tokio::test]
async fn service_sas_url_grants_anonymous_read() {
    let fixture = AzuriteFixture::start().await;

    // Seed a blob using shared key auth.
    fixture
        .send_signed("PUT", "/sasread?restype=container", b"", &[])
        .await;
    let body = b"sas-protected payload";
    let put = fixture
        .send_signed(
            "PUT",
            "/sasread/secret.txt",
            body,
            &[
                ("Content-Type", "text/plain"),
                ("x-ms-blob-type", "BlockBlob"),
            ],
        )
        .await;
    assert!(put.status().is_success(), "seed blob: {}", put.status());

    // Generate a read-only SAS URL.
    let sas_url = azure_sas::generate_blob_sas_url(
        &fixture.base_url,
        ACCOUNT,
        KEY,
        "sasread",
        "secret.txt",
        "r",
        600,
        None,
    )
    .expect("generate SAS URL");

    // Fetch via SAS — note: no Authorization header.
    let response = fixture
        .client
        .get(&sas_url)
        .send()
        .await
        .expect("send sas request");
    assert!(
        response.status().is_success(),
        "GET via SAS URL: {} body={}",
        response.status(),
        response.text().await.unwrap_or_default()
    );
    let downloaded = response.bytes().await.expect("read sas body");
    assert_eq!(downloaded.as_ref(), body);
}

#[tokio::test]
async fn service_sas_url_with_write_permission_supports_upload() {
    let fixture = AzuriteFixture::start().await;

    // Create the container with shared key auth (SAS write permission alone
    // doesn't include container creation).
    fixture
        .send_signed("PUT", "/sasupload?restype=container", b"", &[])
        .await;

    // Generate a write SAS for a specific blob.
    let sas_url = azure_sas::generate_blob_sas_url(
        &fixture.base_url,
        ACCOUNT,
        KEY,
        "sasupload",
        "via-sas.txt",
        "cw",
        600,
        Some("text/plain"),
    )
    .expect("generate write SAS");

    let body = b"uploaded via SAS";
    let response = fixture
        .client
        .put(&sas_url)
        .header("Content-Type", "text/plain")
        .header("x-ms-blob-type", "BlockBlob")
        .body(body.to_vec())
        .send()
        .await
        .expect("send upload via SAS");
    assert!(
        response.status().is_success(),
        "PUT via SAS URL: {} body={}",
        response.status(),
        response.text().await.unwrap_or_default()
    );

    // Round-trip: read it back with shared key signing to confirm it was stored.
    let get = fixture
        .send_signed("GET", "/sasupload/via-sas.txt", b"", &[])
        .await;
    assert!(get.status().is_success(), "read back: {}", get.status());
    let got = get.bytes().await.unwrap();
    assert_eq!(got.as_ref(), body);
}
