//! Both resolver ABIs use the native service, host tenant and per-run cache.
use super::*;
use test_support::{Ticker, bounded, spec};

#[derive(Default)]
struct Resolver {
    calls: std::sync::Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl crate::ConnectionResolverHost for Resolver {
    async fn describe(&self, tenant: &str, connection: String) -> Result<Vec<u8>, String> {
        self.calls.lock().unwrap().push((tenant.into(), connection));
        Ok(b"[1]".to_vec())
    }
    async fn resolve_resource(
        &self,
        tenant: &str,
        connection: String,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        assert_eq!(request, b"{}");
        self.calls.lock().unwrap().push((tenant.into(), connection));
        Ok(b"[1]".to_vec())
    }
}

fn component(version: &str, asynchronous: &str) -> String {
    include_str!("connection_resolver_test.wat")
        .replace("{{VERSION}}", version)
        .replace("{{ASYNC}}", asynchronous)
}

#[tokio::test]
async fn native_resolvers_preserve_both_abis_and_per_run_caches() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let resolver = Arc::new(Resolver::default());
    executor.set_connection_resolver(resolver.clone())?;
    for (version, asynchronous) in [("0.1.0", ""), ("0.2.0", "async")] {
        let wasm = Component::new(&engine, component(version, asynchronous))?;
        let prepared = executor.prepare_precompiled(wasm).await?;
        for tenant in ["tenant-a", "tenant-b"] {
            let mut run = spec();
            run.trusted_tenant = Some(tenant.into());
            // Guest environment cannot provide authority or select an HTTP backend.
            run.env.insert("RUNTARA_TENANT_ID".into(), "spoofed".into());
            run.env
                .insert("CONNECTION_SERVICE_URL".into(), "http://127.0.0.1:1".into());
            let result =
                bounded(executor.execute_invoke(prepared.instance_pre(), run, b"{}".to_vec()))
                    .await;
            assert!(
                matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == b"[1]"),
                "{version}: {:?}",
                result.exit
            );
        }
    }
    // The guest calls each operation twice per run; only one of each reaches
    // the backend. Neither tenant nor subsequent runs reuse another run's cache.
    let calls = resolver.calls.lock().unwrap();
    assert_eq!(calls.len(), 8);
    for (i, (tenant, connection)) in calls.iter().enumerate() {
        assert_eq!(tenant, if i % 4 < 2 { "tenant-a" } else { "tenant-b" });
        assert_eq!(connection, "conn");
    }
    Ok(())
}

#[tokio::test]
async fn guest_environment_cannot_enable_connection_resolution() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let resolver = Arc::new(Resolver::default());
    executor.set_connection_resolver(resolver.clone())?;
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, component("0.2.0", "async"))?)
        .await?;
    let mut run = spec();
    run.env.insert("RUNTARA_TENANT_ID".into(), "spoofed".into());
    let result =
        bounded(executor.execute_invoke(prepared.instance_pre(), run, b"{}".to_vec())).await;
    assert!(
        matches!(result.exit, InvokeExit::Trapped { ref reason } if reason.contains("authoritative tenant")),
        "{:?}",
        result.exit
    );
    assert!(resolver.calls.lock().unwrap().is_empty());
    Ok(())
}
