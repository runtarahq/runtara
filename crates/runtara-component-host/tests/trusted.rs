//! Real provider components, with no internal HTTP service or external network.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use runtara_agent_trusted::{TrustedContext, error};
use runtara_component_host::trusted::TrustedCredentials;
use runtara_component_host::{ComponentDispatcherService, DispatcherEnv, TestCapabilityRequest};
use serde_json::json;

const CAP: &str = "storage-generate-presigned-url";
const NOW: i64 = 1_700_000_000_000;

struct Credentials {
    calls: AtomicUsize,
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
    let credentials = Arc::new(Credentials {
        calls: AtomicUsize::new(0),
    });
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
    let env = DispatcherEnv {
        proxy_url: "http://127.0.0.1:1".into(),
        object_model_url: "http://127.0.0.1:1".into(),
        core_http_url: "http://127.0.0.1:1".into(),
    };
    let dispatcher = ComponentDispatcherService::from_dir(bundle.path(), env).await?;
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
async fn scoped_child_presigning_keeps_root_authority_and_exact_artifact_version()
-> anyhow::Result<()> {
    use runtara_component_host::execution_host::{
        Entry, ExecutionError, InvocationContext, InvocationLauncher, StartRequest,
    };
    use runtara_component_host::{
        ChildInvocationScope, InvocationScopeFactory, PreparedInvocationLauncher, WorkflowExecutor,
        WorkflowRunSpec,
    };
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, HashMap};
    use wasmtime::component::Component;

    struct Scopes;
    impl InvocationScopeFactory for Scopes {
        fn prepare_child(&self, _: &StartRequest) -> Result<ChildInvocationScope, ExecutionError> {
            Ok(ChildInvocationScope {
                lifecycle: None,
                execution: None,
                make_spec: Box::new(|_| {
                    Ok(WorkflowRunSpec {
                        trusted_tenant: Some("tenant-a".into()),
                        env: HashMap::from([("RUNTARA_TENANT_ID".into(), "forged".into())]),
                        stderr: None,
                        timeout: Duration::from_secs(10),
                        cancel: None,
                        limits: Default::default(),
                        runtime: None,
                    }
                    .into())
                }),
            })
        }
    }
    let (bundle, dispatcher, credentials) = dispatcher().await?;
    let engine = runtara_component_host::build_engine(&Default::default())?;
    let executor = Arc::new(WorkflowExecutor::new(engine.clone())?);
    executor.set_trusted_executor(dispatcher.trusted_executor())?;
    let bytes = std::fs::read(bundle.path().join("runtara_agent_s3_storage.wasm"))?;
    let meta = std::fs::read(bundle.path().join("runtara_agent_s3_storage.meta.json"))?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let pin = runtara_dsl::agent_meta::trusted_artifact_import(
        "s3-storage",
        &digest,
        &format!("{:x}", Sha256::digest(meta)),
    );
    let child = Component::new(&engine, &bytes)?;
    let package = |root_pin: bool, child_digest: String| -> anyhow::Result<_> {
        let import = if root_pin {
            format!("(import \"{pin}\" (instance))")
        } else {
            String::new()
        };
        Ok(
            runtara_component_host::precompile::CompiledWorkflowPackage {
                root: Component::new(
                    &engine,
                    format!(
                        r#"(component {import}
                (core module $m (func (export "run") (result i32) i32.const 0))
                (core instance $m (instantiate $m))
                (func $run (result (result)) (canon lift (core func $m "run")))
                (instance $api (export "run" (func $run)))
                (export "wasi:cli/run@0.2.3" (instance $api)))"#
                    ),
                )?,
                artifacts: BTreeMap::from([(child_digest.clone(), child.clone())]),
                bindings: vec![runtara_workflow_wit::isolation_package::Binding {
                    id: "s3".into(),
                    artifact: child_digest,
                    interface: "runtara:agent-s3-storage/capabilities@0.4.0".into(),
                }],
                invocations: None,
            },
        )
    };
    assert!(
        executor
            .prepare_precompiled_package(package(false, digest.clone())?)
            .await
            .is_err()
    );
    assert!(
        executor
            .prepare_precompiled_package(package(true, "0".repeat(64))?)
            .await
            .is_err()
    );
    assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);
    let prepared = executor
        .prepare_precompiled_package(package(true, digest)?)
        .await?;
    let launcher = PreparedInvocationLauncher::new(
        executor,
        prepared.child_catalog().unwrap().clone(),
        Arc::new(Scopes),
    )?;
    let invocation = launcher.prepare(StartRequest {
        binding: "s3".into(), entry: Entry::Capability(CAP.into()),
        context: InvocationContext { path: "sign".into(), attempt: 1 },
        input: serde_json::to_vec(&json!({"bucket":"uploads", "key":"report.csv", "operation":"download", "_connection":{"connection_id":"s3"}}))?,
    }).unwrap();
    let tasks =
        runtara_component_host::isolated_tasks::IsolatedTasks::new(engine, 2, 1024 * 1024).unwrap();
    let id = tasks
        .spawn_managed(invocation.run, invocation.cleanup, invocation.lifecycle)
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), tasks.join(id))
        .await?
        .unwrap();
    let runtara_component_host::InvokeExit::Completed(output) = result.outcome() else {
        panic!("{:?}", result.outcome())
    };
    assert!(String::from_utf8_lossy(output).contains("X-Amz-Signature="));
    assert!(!String::from_utf8_lossy(output).contains("synthetic-secret"));
    assert_eq!(credentials.calls.load(Ordering::SeqCst), 1);
    tasks.release(id).await.unwrap();
    tasks.shutdown().await.unwrap();
    Ok(())
}

#[path = "trusted/emulators.rs"]
mod emulators;

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
            Credentials {
                calls: AtomicUsize::new(0),
            }
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
