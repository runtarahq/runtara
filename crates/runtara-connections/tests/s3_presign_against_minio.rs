//! Storage-server acceptance for the pure signer shared by native compatibility
//! and isolated WASM execution. Uses only synthetic, per-container credentials.
use runtara_agent_trusted::{TrustedContext, presign};
use std::time::Duration;
use testcontainers::{GenericImage, ImageExt, core::IntoContainerPort, runners::AsyncRunner};

#[tokio::test]
async fn presigned_upload_download_and_delete_work_against_minio() {
    let container = GenericImage::new("minio/minio", "latest")
        .with_exposed_port(9000.tcp())
        .with_env_var("MINIO_ROOT_USER", "runtara-test-access")
        .with_env_var("MINIO_ROOT_PASSWORD", "runtara-synthetic-test-secret")
        .with_cmd(["server", "/data"])
        .start()
        .await
        .expect("isolated MinIO container");
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(9000).await.unwrap();
    let base = format!("http://{host}:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
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
    .await
    .expect("MinIO ready");
    let context = TrustedContext {
        integration_id: "s3_compatible".into(),
        now_ms: chrono::Utc::now().timestamp_millis(),
        credentials: serde_json::json!({"base_url":base, "access_key_id":"runtara-test-access", "secret_access_key":"runtara-synthetic-test-secret", "region":"us-east-1"}),
    };
    let bucket = presign(&context, "PUT", "/trusted-presign", 900, None).unwrap();
    assert!(
        client
            .put(bucket.url)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let path = "/trusted-presign/folder/my file.txt";
    let upload = presign(&context, "PUT", path, 900, Some("text/plain")).unwrap();
    assert!(
        client
            .put(upload.url)
            .header("Content-Type", "text/plain")
            .body("trusted fixture")
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let download = presign(&context, "GET", path, 900, None).unwrap();
    let response = client.get(download.url).send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!(response.text().await.unwrap(), "trusted fixture");
    let delete = presign(&context, "DELETE", path, 900, None).unwrap();
    assert!(
        client
            .delete(delete.url)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
}
