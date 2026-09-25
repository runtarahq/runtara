//! Provider acceptance of URLs produced by the actual restricted WASM exports.
//! Container credentials below are synthetic and exist only for each test.
use super::*;
use anyhow::Context;
use std::collections::HashMap;
use testcontainers::{
    GenericBuildableImage, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::{AsyncBuilder, AsyncRunner},
};

const S3_USER: &str = "runtara-test-access";
const S3_KEY: &str = "runtara-synthetic-test-secret";
const AZURE_ACCOUNT: &str = "devstoreaccount1";
const AZURE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

struct EmulatorCredentials {
    integration: &'static str,
    fields: serde_json::Value,
}
#[async_trait::async_trait]
impl TrustedCredentials for EmulatorCredentials {
    async fn resolve(
        &self,
        tenant: &str,
        _: &str,
        connection: &str,
        allowed: &[String],
    ) -> Result<TrustedContext, String> {
        assert_eq!(tenant, "tenant-a");
        assert_eq!(connection, "emulator");
        assert!(allowed.iter().any(|t| t == self.integration));
        Ok(TrustedContext {
            integration_id: self.integration.into(),
            credentials: self.fields.clone(),
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
    }
}

async fn signed_url(
    dispatcher: &ComponentDispatcherService,
    agent: &str,
    operation: &str,
) -> anyhow::Result<String> {
    let result = dispatcher.test_capability(TestCapabilityRequest {
        tenant_id: "tenant-a".into(), agent_id: agent.into(), capability_id: CAP.into(), connection: None,
        input: json!({"bucket":"trusted-presign", "key":"folder/my file.txt", "operation":operation,
            "expires_in_seconds":900, "content_type":"text/plain", "_connection":{"connection_id":"emulator"}}),
    }).await?;
    assert!(result.success, "{:?}", result.error);
    let output = result.output.unwrap();
    assert_eq!(output["success"], true, "{output}");
    Ok(output["url"].as_str().unwrap().into())
}

async fn roundtrip(
    dispatcher: &ComponentDispatcherService,
    agent: &str,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    let upload = signed_url(dispatcher, agent, "upload").await?;
    // Verify that the URL addresses the requested key, not a double-encoded key.
    assert!(
        url::Url::parse(&upload)?
            .path()
            .ends_with("/folder/my%20file.txt")
    );
    let response = client
        .put(upload)
        .header("Content-Type", "text/plain")
        .header("x-ms-blob-type", "BlockBlob")
        .body("trusted fixture")
        .send()
        .await?;
    assert!(
        response.status().is_success(),
        "upload: {} {}",
        response.status(),
        response.text().await?
    );
    let download = signed_url(dispatcher, agent, "download").await?;
    let response = client.get(&download).send().await?;
    assert!(
        response.status().is_success(),
        "download: {} {}",
        response.status(),
        response.text().await?
    );
    assert_eq!(response.text().await?, "trusted fixture");
    let delete = signed_url(dispatcher, agent, "delete").await?;
    let response = client.delete(delete).send().await?;
    assert!(
        response.status().is_success(),
        "delete: {} {}",
        response.status(),
        response.text().await?
    );
    assert_eq!(
        client.get(download).send().await?.status(),
        reqwest::StatusCode::NOT_FOUND
    );
    Ok(())
}

async fn wait_for_minio_bucket(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> anyhow::Result<()> {
    // MinIO's health probe can succeed before its S3 API accepts writes. Use
    // bucket creation as the readiness boundary; never retry signing failures
    // or the actual presigned-URL operations that this fixture is testing.
    tokio::time::timeout(timeout, async {
        loop {
            let response = request
                .try_clone()
                .context("bucket creation request must be replayable")?
                .send()
                .await?;
            if response.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE {
                response.error_for_status()?;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("MinIO did not become ready for bucket creation before the deadline")?
}

#[tokio::test]
async fn minio_bucket_readiness_retries_only_startup_503s() -> anyhow::Result<()> {
    for final_status in [
        reqwest::StatusCode::OK,
        reqwest::StatusCode::FORBIDDEN,
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let app = axum::Router::new().route(
            "/bucket",
            axum::routing::put(move || {
                let attempt = observed.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt < 2 {
                        reqwest::StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        final_status
                    }
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let result = wait_for_minio_bucket(
            reqwest::Client::new().put(format!("http://{address}/bucket")),
            Duration::from_secs(5),
        )
        .await;
        server.abort();
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        if final_status.is_success() {
            result?;
        } else {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<reqwest::Error>()
                    .unwrap()
                    .status(),
                Some(final_status)
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn minio_bucket_readiness_has_a_deadline() -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = axum::Router::new().route(
        "/bucket",
        axum::routing::put(|| async { reqwest::StatusCode::SERVICE_UNAVAILABLE }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let result = wait_for_minio_bucket(
        reqwest::Client::new().put(format!("http://{address}/bucket")),
        Duration::from_millis(250),
    )
    .await;
    server.abort();
    assert!(result.unwrap_err().is::<tokio::time::error::Elapsed>());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wasm_presigned_urls_work_against_minio() -> anyhow::Result<()> {
    // Upstream registry images can disappear independently of the release.
    // Build the pinned source; Docker caches it for subsequent test runs.
    let image = GenericBuildableImage::new("runtara-test-minio", "2025-09-07")
        .with_dockerfile_string(include_str!("minio.Dockerfile"))
        .build_image()
        .await?;
    let container = image
        .with_exposed_port(9000.tcp())
        .with_env_var("MINIO_ROOT_USER", S3_USER)
        .with_env_var("MINIO_ROOT_PASSWORD", S3_KEY)
        .with_cmd(["server", "/data"])
        .start()
        .await?;
    let base = format!(
        "http://{}:{}",
        container.get_host().await?,
        container.get_host_port_ipv4(9000).await?
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if client
                .get(format!("{base}/minio/health/ready"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    // Provision only the bucket with normal request authentication; all object
    // operations below use URLs from the actual WASM signer.
    let url = url::Url::parse(&format!("{base}/trusted-presign"))?;
    let mut headers = HashMap::new();
    runtara_connections::auth::aws_signing::sign_request_v4(
        "PUT",
        &url,
        &mut headers,
        b"",
        S3_USER,
        S3_KEY,
        "us-east-1",
        "s3",
        None,
    );
    let mut request = client.put(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    wait_for_minio_bucket(request, Duration::from_secs(30)).await?;
    let (_bundle, dispatcher) = dispatcher_with_credentials(Arc::new(EmulatorCredentials {
        integration: "s3_compatible", fields: json!({"base_url":base, "access_key_id":S3_USER, "secret_access_key":S3_KEY, "region":"us-east-1"}),
    })).await?;
    roundtrip(&dispatcher, "s3-storage", &client).await
}

#[tokio::test(flavor = "multi_thread")]
async fn wasm_sas_urls_work_against_azurite() -> anyhow::Result<()> {
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
        .await?;
    let base = format!(
        "http://{}:{}/{AZURE_ACCOUNT}",
        container.get_host().await?,
        container.get_host_port_ipv4(10000).await?
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = url::Url::parse(&format!("{base}/trusted-presign?restype=container"))?;
    let mut headers = HashMap::new();
    runtara_connections::auth::azure_signing::sign_request_shared_key(
        "PUT",
        &url,
        &mut headers,
        b"",
        AZURE_ACCOUNT,
        AZURE_KEY,
    )
    .map_err(anyhow::Error::msg)?;
    let mut request = client.put(url).body("");
    for (key, value) in headers {
        request = request.header(key, value);
    }
    request.send().await?.error_for_status()?;
    let (_bundle, dispatcher) = dispatcher_with_credentials(Arc::new(EmulatorCredentials {
        integration: "azure_blob_storage",
        fields: json!({"base_url":base, "account_name":AZURE_ACCOUNT, "account_key":AZURE_KEY}),
    }))
    .await?;
    roundtrip(&dispatcher, "azure-blob-storage", &client).await
}
