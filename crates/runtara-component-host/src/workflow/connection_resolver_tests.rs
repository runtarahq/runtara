//! Exercise both resolver ABI versions through the production linker and HTTP cache.
use super::*;
use test_support::{Ticker, bounded, spec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn component(version: &str, asynchronous: &str) -> String {
    include_str!("connection_resolver_test.wat")
        .replace("{{VERSION}}", version)
        .replace("{{ASYNC}}", asynchronous)
}

#[tokio::test]
async fn legacy_and_async_resolvers_preserve_results_and_per_run_caches() -> anyhow::Result<()> {
    let engine = crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })?;
    let _ticker = Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    for (version, asynchronous) in [("0.1.0", ""), ("0.2.0", "async")] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            for operation in ["metadata", "resources"] {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    let count = stream.read(&mut buffer).await?;
                    anyhow::ensure!(count > 0, "request closed before headers");
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request)?;
                let method = if operation == "metadata" {
                    "GET"
                } else {
                    "POST"
                };
                anyhow::ensure!(
                    request.starts_with(&format!("{method} /fixture/conn/{operation} "))
                );
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n[1]",
                    )
                    .await?;
            }
            anyhow::Ok(())
        });
        let wasm = Component::new(&engine, component(version, asynchronous))?;
        let prepared = executor.prepare_precompiled(wasm).await?;
        let mut spec = spec();
        spec.env.insert("CONNECTION_SERVICE_URL".into(), url);
        spec.env
            .insert("RUNTARA_TENANT_ID".into(), "fixture".into());
        let result =
            bounded(executor.execute_invoke(prepared.instance_pre(), spec, b"{}".to_vec())).await;
        assert!(
            matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == b"[1]"),
            "{version}: {:?}",
            result.exit
        );
        bounded(server).await??;
    }
    Ok(())
}
