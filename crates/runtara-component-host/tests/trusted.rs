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

#[derive(Default)]
struct Credentials {
    calls: AtomicUsize,
    per_agent: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}
impl Credentials {
    fn count(&self, agent: &str) -> usize {
        self.per_agent
            .lock()
            .unwrap()
            .get(agent)
            .copied()
            .unwrap_or(0)
    }
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
    let env = DispatcherEnv {
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

/// Child scopes whose authoritative tenant is `tenant-a`; the guest-visible
/// environment claims another tenant, which must be ignored.
struct Scopes;
impl runtara_component_host::InvocationScopeFactory for Scopes {
    fn prepare_child(
        &self,
        _: &runtara_component_host::execution_host::StartRequest,
    ) -> Result<
        runtara_component_host::ChildInvocationScope,
        runtara_component_host::execution_host::ExecutionError,
    > {
        Ok(runtara_component_host::ChildInvocationScope {
            lifecycle: None,
            execution: None,
            make_spec: Box::new(|_| {
                Ok(runtara_component_host::WorkflowRunSpec {
                    trusted_instance: None,
                    trusted_tenant: Some("tenant-a".into()),
                    env: std::collections::HashMap::from([(
                        "RUNTARA_TENANT_ID".into(),
                        "forged".into(),
                    )]),
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

#[tokio::test(flavor = "multi_thread")]
async fn scoped_child_presigning_keeps_root_authority_and_exact_artifact_version()
-> anyhow::Result<()> {
    use runtara_component_host::execution_host::{
        Entry, InvocationContext, InvocationLauncher, StartRequest,
    };
    use runtara_component_host::{PreparedInvocationLauncher, WorkflowExecutor};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use wasmtime::component::Component;

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
    let package = |root_pin: Option<&str>, child_digest: String| -> anyhow::Result<_> {
        let import = root_pin
            .map(|pin| format!("(import \"{pin}\" (instance))"))
            .unwrap_or_default();
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
    // Rejections are checked by their guard, so an unrelated failure (a bad
    // fixture, catalog preparation) cannot pass for a pin-binding decision.
    let rejection = |result: anyhow::Result<runtara_component_host::PreparedWorkflow>| match result
    {
        Ok(_) => panic!("package must be rejected"),
        Err(error) => format!("{error:#}"),
    };
    // No root pin, or a root pin that binds other bytes than the child's:
    // the trusted child has no approved pin at all.
    for (root_pin, child_digest) in [(None, digest.clone()), (Some(&pin), "0".repeat(64))] {
        let error = rejection(
            executor
                .prepare_precompiled_package(package(root_pin.map(String::as_str), child_digest)?)
                .await,
        );
        assert!(
            error.contains("isolated trusted caller has no approved artifact pin"),
            "{error}"
        );
    }
    // A child's own pin must also be one of the root's.
    let foreign_pin = runtara_dsl::agent_meta::trusted_artifact_import(
        "s3-storage",
        &"a".repeat(64),
        &"b".repeat(64),
    );
    let pinned_child = Component::new(
        &engine,
        format!(r#"(component (import "{foreign_pin}" (instance)))"#),
    )?;
    let mut foreign = package(Some(&pin), digest.clone())?;
    foreign.artifacts = BTreeMap::from([("1".repeat(64), pinned_child)]);
    foreign.bindings[0].artifact = "1".repeat(64);
    let error = rejection(executor.prepare_precompiled_package(foreign).await);
    assert!(
        error.contains("isolated trusted dependency is not pinned by root"),
        "{error}"
    );
    // A root that imports the trusted executor must pin what it calls.
    let unpinned_root = Component::new(
        &engine,
        format!(
            r#"(component (import "{}" (instance)))"#,
            runtara_agent_trusted::EXECUTOR_INTERFACE
        ),
    )?;
    let error = rejection(executor.prepare_precompiled(unpinned_root).await);
    assert!(
        error.contains("workflow trusted executor import has no artifact pins"),
        "{error}"
    );
    assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);
    let tasks =
        runtara_component_host::isolated_tasks::IsolatedTasks::new(engine.clone(), 2, 1024 * 1024)
            .unwrap();
    let sign = |prepared: runtara_component_host::PreparedWorkflow| {
        let launcher = PreparedInvocationLauncher::new(
            executor.clone(),
            prepared.child_catalog().unwrap().clone(),
            Arc::new(Scopes),
        )
        .unwrap();
        let invocation = launcher.prepare(StartRequest {
            binding: "s3".into(), entry: Entry::Capability(CAP.into()),
            context: InvocationContext { path: "sign".into(), attempt: 1 },
            input: serde_json::to_vec(&json!({"bucket":"uploads", "key":"report.csv", "operation":"download", "_connection":{"connection_id":"s3"}})).unwrap(),
        }).unwrap();
        tasks
            .spawn_managed(invocation.run, invocation.cleanup, invocation.lifecycle)
            .unwrap()
    };

    // A package compiled against another version of the built-in (changed
    // metadata, or older bytes) still links. Its child's trusted call fails
    // with TRUSTED_VERSION_REQUIRED before any credential is resolved.
    let stale_metadata =
        runtara_dsl::agent_meta::trusted_artifact_import("s3-storage", &digest, &"f".repeat(64));
    let old_bytes = "e".repeat(64);
    let stale_bytes =
        runtara_dsl::agent_meta::trusted_artifact_import("s3-storage", &old_bytes, &"f".repeat(64));
    for (root_pin, child_digest) in [
        (&stale_metadata, digest.clone()),
        (&stale_bytes, old_bytes.clone()),
    ] {
        let prepared = executor
            .prepare_precompiled_package(package(Some(root_pin), child_digest)?)
            .await?;
        let id = sign(prepared);
        let result = tokio::time::timeout(Duration::from_secs(15), tasks.join(id))
            .await?
            .unwrap();
        let outcome = format!("{:?}", result.outcome());
        assert!(
            outcome.contains("TRUSTED_VERSION_REQUIRED"),
            "{root_pin}: {outcome}"
        );
        tasks.release(id).await.unwrap();
    }
    assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);

    let prepared = executor
        .prepare_precompiled_package(package(Some(&pin), digest)?)
        .await?;
    let id = sign(prepared);
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

/// The per-call version check is per agent. One package pins the installed
/// version of one trusted built-in and a stale version of the other (an
/// operator upgraded only that one): it still links, the installed agent's
/// call signs, and the stale agent's call fails with TRUSTED_VERSION_REQUIRED
/// without resolving that agent's credentials. Both directions are checked,
/// so a stale Azure pin fails exactly like a stale S3 one.
#[tokio::test(flavor = "multi_thread")]
async fn one_stale_trusted_pin_fails_only_its_own_agent_calls() -> anyhow::Result<()> {
    use runtara_component_host::execution_host::{
        Entry, InvocationContext, InvocationLauncher, StartRequest,
    };
    use runtara_component_host::{PreparedInvocationLauncher, WorkflowExecutor};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use wasmtime::component::Component;

    // (agent, component file stem, binding / connection id, signed-URL marker)
    const AGENTS: [(&str, &str, &str, &str); 2] = [
        ("s3-storage", "s3_storage", "s3", "X-Amz-Signature="),
        ("azure-blob-storage", "azure_blob_storage", "azure", "sig="),
    ];
    let (bundle, dispatcher, credentials) = dispatcher().await?;
    let engine = runtara_component_host::build_engine(&Default::default())?;
    let executor = Arc::new(WorkflowExecutor::new(engine.clone())?);
    executor.set_trusted_executor(dispatcher.trusted_executor())?;
    let installed: std::collections::BTreeSet<String> = dispatcher
        .trusted_executor()
        .artifact_pins()
        .map(str::to_owned)
        .collect();
    // Per agent: component, its digest, its installed pin, and a pin for
    // another version of it (same bytes, other metadata: what an upgrade
    // that only touched the sidecar produces).
    let mut members = Vec::new();
    for (agent, stem, binding, marker) in AGENTS {
        let wasm = std::fs::read(bundle.path().join(format!("runtara_agent_{stem}.wasm")))?;
        let meta = std::fs::read(
            bundle
                .path()
                .join(format!("runtara_agent_{stem}.meta.json")),
        )?;
        let digest = format!("{:x}", Sha256::digest(&wasm));
        let current = runtara_dsl::agent_meta::trusted_artifact_import(
            agent,
            &digest,
            &format!("{:x}", Sha256::digest(&meta)),
        );
        assert!(installed.contains(&current), "{agent}: {installed:?}");
        let stale =
            runtara_dsl::agent_meta::trusted_artifact_import(agent, &digest, &"f".repeat(64));
        assert!(!installed.contains(&stale));
        let component = Component::new(&engine, &wasm)?;
        members.push((agent, binding, marker, digest, current, stale, component));
    }
    let tasks =
        runtara_component_host::isolated_tasks::IsolatedTasks::new(engine.clone(), 2, 1024 * 1024)
            .unwrap();

    for stale_index in 0..members.len() {
        // The root pins one agent's installed version and the other's stale one.
        let pins: Vec<&str> = members
            .iter()
            .enumerate()
            .map(|(i, (_, _, _, _, current, stale, _))| {
                if i == stale_index {
                    stale.as_str()
                } else {
                    current.as_str()
                }
            })
            .collect();
        let imports: String = pins
            .iter()
            .map(|pin| format!("(import \"{pin}\" (instance))"))
            .collect();
        let root = Component::new(
            &engine,
            format!(
                r#"(component {imports}
            (core module $m (func (export "run") (result i32) i32.const 0))
            (core instance $m (instantiate $m))
            (func $run (result (result)) (canon lift (core func $m "run")))
            (instance $api (export "run" (func $run)))
            (export "wasi:cli/run@0.2.3" (instance $api)))"#
            ),
        )?;
        let package = runtara_component_host::precompile::CompiledWorkflowPackage {
            root,
            artifacts: members
                .iter()
                .map(|(_, _, _, digest, _, _, component)| (digest.clone(), component.clone()))
                .collect::<BTreeMap<_, _>>(),
            bindings: members
                .iter()
                .map(|(agent, binding, _, digest, _, _, _)| {
                    runtara_workflow_wit::isolation_package::Binding {
                        id: (*binding).into(),
                        artifact: digest.clone(),
                        interface: format!("runtara:agent-{agent}/capabilities@0.4.0"),
                    }
                })
                .collect(),
            invocations: None,
        };
        let prepared = executor
            .prepare_precompiled_package(package)
            .await
            .expect("a stale pin must not fail linking the package");
        let launcher = PreparedInvocationLauncher::new(
            executor.clone(),
            prepared.child_catalog().unwrap().clone(),
            Arc::new(Scopes),
        )?;
        let sign = |binding: &str| {
            let invocation = launcher
                .prepare(StartRequest {
                    binding: binding.into(),
                    entry: Entry::Capability(CAP.into()),
                    context: InvocationContext {
                        path: format!("sign-{binding}"),
                        attempt: 1,
                    },
                    input: serde_json::to_vec(&json!({"bucket":"uploads", "key":"report.csv",
                        "operation":"download", "_connection":{"connection_id":binding}}))
                    .unwrap(),
                })
                .unwrap();
            tasks
                .spawn_managed(invocation.run, invocation.cleanup, invocation.lifecycle)
                .unwrap()
        };
        // The stale agent is called first, then the installed one, then the
        // stale one again: an installed call in between does not unlock it.
        let stale = &members[stale_index];
        let current = &members[1 - stale_index];
        let before = (credentials.count(stale.0), credentials.count(current.0));
        for (member, expect_signed) in [(stale, false), (current, true), (stale, false)] {
            let (agent, binding, marker, ..) = member;
            let id = sign(binding);
            let result = tokio::time::timeout(Duration::from_secs(15), tasks.join(id))
                .await?
                .unwrap();
            if expect_signed {
                let runtara_component_host::InvokeExit::Completed(output) = result.outcome() else {
                    panic!("installed {agent}: {:?}", result.outcome())
                };
                let output = String::from_utf8_lossy(output);
                assert!(output.contains(*marker), "installed {agent}: {output}");
                assert!(!output.contains("synthetic-secret"));
            } else {
                let outcome = format!("{:?}", result.outcome());
                assert!(
                    outcome.contains("TRUSTED_VERSION_REQUIRED"),
                    "stale {agent}: {outcome}"
                );
                assert!(!outcome.contains("synthetic-secret"));
            }
            tasks.release(id).await.unwrap();
        }
        assert_eq!(
            credentials.count(stale.0),
            before.0,
            "the stale {} pin never resolves credentials",
            stale.0
        );
        assert_eq!(
            credentials.count(current.0),
            before.1 + 1,
            "the installed {} call resolves credentials exactly once",
            current.0
        );
    }
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
