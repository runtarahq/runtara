//! Real provider components, with no internal HTTP service or external network.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use runtara_agent_trusted::{TrustedContext, error};
use runtara_component_host::trusted::TrustedCredentials;
use runtara_component_host::{ComponentDispatcherService, TestCapabilityRequest};
use serde_json::json;

const CAP: &str = "storage-generate-presigned-url";
const NOW: i64 = 1_700_000_000_000;

#[derive(Default)]
struct Credentials {
    calls: AtomicUsize,
    per_agent: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}
#[async_trait::async_trait]
impl TrustedCredentials for Credentials {
    async fn resolve(
        &self,
        tenant: &str,
        agent: &str,
        connection: &str,
        allowed: &[String],
    ) -> Result<TrustedContext, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self
            .per_agent
            .lock()
            .unwrap()
            .entry(agent.to_owned())
            .or_default() += 1;
        if tenant != "tenant-a" {
            return Err(error("DENIED", "Connection unavailable"));
        }
        let (integration, credentials) = match connection {
            "s3" => (
                "s3_compatible",
                json!({"base_url":"https://storage.example.test", "access_key_id":"test-access", "secret_access_key":"synthetic-secret-never-return", "region":"us-east-1"}),
            ),
            "azure" => (
                "azure_blob_storage",
                json!({"base_url":"https://test.blob.core.windows.net", "account_name":"test", "account_key":"c3ludGhldGljLXNlY3JldA=="}),
            ),
            _ => return Err(error("DENIED", "Connection unavailable")),
        };
        if !allowed.iter().any(|s| s == integration) {
            return Err(error("DENIED", "Connection unavailable"));
        }
        assert!(matches!(agent, "s3-storage" | "azure-blob-storage"));
        Ok(TrustedContext {
            integration_id: integration.into(),
            credentials,
            now_ms: NOW,
        })
    }
}
async fn dispatcher() -> anyhow::Result<(
    tempfile::TempDir,
    ComponentDispatcherService,
    Arc<Credentials>,
)> {
    let credentials = Arc::new(Credentials::default());
    let (bundle, dispatcher) = dispatcher_with_credentials(credentials.clone()).await?;
    Ok((bundle, dispatcher, credentials))
}

async fn dispatcher_with_credentials(
    credentials: Arc<dyn TrustedCredentials>,
) -> anyhow::Result<(tempfile::TempDir, ComponentDispatcherService)> {
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release");
    let bundle = tempfile::tempdir()?;
    for agent in ["s3_storage", "azure_blob_storage"] {
        for suffix in ["wasm", "meta.json"] {
            let file = format!("runtara_agent_{agent}.{suffix}");
            std::fs::copy(source.join(&file), bundle.path().join(file))?;
        }
    }
    let dispatcher = ComponentDispatcherService::from_dir(bundle.path()).await?;
    dispatcher
        .trusted_executor()
        .set_credentials(credentials.clone())?;
    Ok((bundle, dispatcher))
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_invocation_forwards_to_isolated_signer_without_http() -> anyhow::Result<()> {
    let (_bundle, dispatcher, credentials) = dispatcher().await?;
    for (agent, connection, expected) in [
        ("s3-storage", "s3", "X-Amz-Signature="),
        ("azure-blob-storage", "azure", "sig="),
    ] {
        assert!(
            dispatcher
                .agent_info_of(agent)
                .unwrap()
                .capabilities
                .iter()
                .any(|c| c.id == CAP && c.trusted)
        );
        let result = dispatcher.test_capability(TestCapabilityRequest {
            tenant_id: "tenant-a".into(), agent_id: agent.into(), capability_id: CAP.into(),
            input: json!({"bucket":"uploads", "key":"report.csv", "operation":"download", "expires_in_seconds":900,
                "_connection":{"connection_id":connection,"integration_id":"forged", "parameters":{"secret_access_key":"attacker"}}}), connection: None,
        }).await?;
        assert!(result.success, "{:?}", result.error);
        let output = result.output.unwrap();
        assert_eq!(output["success"], true, "{output}");
        assert_eq!(output["expires_in_seconds"], 900);
        assert!(output["url"].as_str().unwrap().contains(expected));
        assert!(!output.to_string().contains("synthetic-secret"));
        assert!(!output.to_string().contains("attacker"));
        if agent == "s3-storage" {
            assert!(output["url"].as_str().unwrap().contains("20231114T221320Z"));
        }
    }
    assert_eq!(credentials.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unapproved_targets_and_expired_calls_never_resolve_credentials() -> anyhow::Result<()> {
    let (_bundle, dispatcher, credentials) = dispatcher().await?;
    let executor = dispatcher.trusted_executor();
    for (agent, cap) in [("tenant-agent", CAP), ("s3-storage", "storage-upload-file")] {
        let result = executor
            .invoke(
                "tenant-a",
                agent,
                cap,
                "s3",
                b"{}".to_vec(),
                tokio::time::Instant::now() + Duration::from_secs(10),
            )
            .await;
        assert!(result.unwrap_err().contains("TRUSTED_CAPABILITY_DENIED"));
    }
    let result = executor
        .invoke(
            "tenant-a",
            "s3-storage",
            CAP,
            "s3",
            b"{}".to_vec(),
            tokio::time::Instant::now() - Duration::from_secs(1),
        )
        .await;
    assert!(result.unwrap_err().contains("TRUSTED_TIMEOUT"));
    for (tenant, connection) in [("", "s3"), ("tenant-a", "")] {
        let result = executor
            .invoke(
                tenant,
                "s3-storage",
                CAP,
                connection,
                b"{}".to_vec(),
                tokio::time::Instant::now() + Duration::from_secs(10),
            )
            .await;
        assert!(result.unwrap_err().contains("TRUSTED_CONNECTION_REQUIRED"));
    }
    assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tenant_and_type_are_authoritative_and_guest_context_is_ignored() -> anyhow::Result<()> {
    let (_bundle, dispatcher, _) = dispatcher().await?;
    for (tenant, connection) in [("tenant-b", "s3"), ("tenant-a", "azure")] {
        let result = dispatcher.test_capability(TestCapabilityRequest {
            tenant_id: tenant.into(), agent_id: "s3-storage".into(), capability_id: CAP.into(),
            input: json!({"bucket":"uploads", "key":"a", "operation":"download", "trusted":true,
                "_connection":{"connection_id":connection,"integration_id":"s3_compatible"},
                "context":{"integration_id":"s3_compatible","credentials":{"secret_access_key":"attacker"}}}), connection: None,
        }).await?;
        assert!(!result.success);
        assert_eq!(result.error.unwrap().code, "DENIED");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wasm_signatures_cover_operations_encoding_and_expiry() -> anyhow::Result<()> {
    let (_bundle, dispatcher, _) = dispatcher().await?;
    for (agent, connection, _integration) in [
        ("s3-storage", "s3", "s3_compatible"),
        ("azure-blob-storage", "azure", "azure_blob_storage"),
    ] {
        for (operation, permission) in [("download", "r"), ("upload", "cw"), ("delete", "d")] {
            let result = dispatcher.test_capability(TestCapabilityRequest {
                tenant_id: "tenant-a".into(), agent_id: agent.into(), capability_id: CAP.into(), connection: None,
                input: json!({"bucket":"uploads", "key":"folder/my file.csv", "operation":operation,
                    "expires_in_seconds":u64::MAX, "content_type":"text/csv", "_connection":{"connection_id":connection}}),
            }).await?;
            assert!(result.success, "{:?}", result.error);
            let output = result.output.unwrap();
            let url = url::Url::parse(output["url"].as_str().unwrap())?;
            assert_eq!(url.path(), "/uploads/folder/my%20file.csv");
            let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
            if agent == "s3-storage" {
                assert_eq!(query["X-Amz-Expires"], "604800");
                assert_eq!(query["X-Amz-Date"], "20231114T221320Z");
                assert_eq!(query["X-Amz-Signature"].len(), 64);
            } else {
                assert_eq!(query["sp"], permission);
                assert_eq!(query["se"], "2023-11-21T22:13:20Z");
                assert_eq!(query["rsct"], "text/csv");
                assert!(!query["sig"].is_empty());
            }
            assert_eq!(output["expires_in_seconds"], 604800);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_during_credential_resolution_drops_work_and_allows_reuse()
-> anyhow::Result<()> {
    struct BlockingCredentials {
        started: tokio::sync::Notify,
        dropped: Arc<AtomicUsize>,
        calls: AtomicUsize,
    }
    struct Pending(Arc<AtomicUsize>);
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl TrustedCredentials for BlockingCredentials {
        async fn resolve(
            &self,
            tenant: &str,
            agent: &str,
            connection: &str,
            allowed: &[String],
        ) -> Result<TrustedContext, String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let _pending = Pending(self.dropped.clone());
                self.started.notify_one();
                std::future::pending::<()>().await;
            }
            Credentials::default()
                .resolve(tenant, agent, connection, allowed)
                .await
        }
    }
    let credentials = Arc::new(BlockingCredentials {
        started: tokio::sync::Notify::new(),
        dropped: Arc::new(AtomicUsize::new(0)),
        calls: AtomicUsize::new(0),
    });
    let (_bundle, dispatcher) = dispatcher_with_credentials(credentials.clone()).await?;
    let executor = dispatcher.trusted_executor();
    let first = executor.clone();
    let input =
        serde_json::to_vec(&json!({"bucket":"uploads", "key":"file.txt", "operation":"download"}))?;
    let first_input = input.clone();
    let task = tokio::spawn(async move {
        first
            .invoke(
                "tenant-a",
                "s3-storage",
                CAP,
                "s3",
                first_input,
                tokio::time::Instant::now() + Duration::from_secs(30),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), credentials.started.notified()).await?;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(credentials.dropped.load(Ordering::SeqCst), 1);
    let output = executor
        .invoke(
            "tenant-a",
            "s3-storage",
            CAP,
            "s3",
            input,
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .map_err(anyhow::Error::msg)?;
    assert!(String::from_utf8_lossy(&output).contains("X-Amz-Signature="));
    Ok(())
}
