//! Built Agents + normal composition + production host I/O. The parent and
//! signal source are fixtures; emitted DSL proofs live in runtara-workflows.
use super::*;
use crate::outbound_fixture::FixtureContext;
use runtara_component_host::HostState;
use serde_json::Value;
use std::path::PathBuf;
use wac_graph::{CompositionGraph, EncodeOptions, types::Package};

pub(super) fn agent_path(agent_id: &str) -> anyhow::Result<PathBuf> {
    let directory = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release")
        });
    let agent = directory.join(format!("runtara_agent_{}.wasm", agent_id.replace('-', "_")));
    anyhow::ensure!(
        agent.exists(),
        "run scripts/build-agent-components.sh first"
    );
    Ok(agent)
}

fn compose(input: &[u8], second: &[u8]) -> anyhow::Result<Vec<u8>> {
    compose_agent("http", "http-request", input, "http-request", second)
}

pub(super) fn compose_agent(
    agent_id: &str,
    capability: &str,
    input: &[u8],
    second_capability: &str,
    second: &[u8],
) -> anyhow::Result<Vec<u8>> {
    compose_agent_with_parent(
        include_str!("http-parent.wat"),
        agent_id,
        capability,
        input,
        second_capability,
        second,
    )
}

pub(super) fn compose_agent_with_parent(
    parent: &str,
    agent_id: &str,
    capability: &str,
    input: &[u8],
    second_capability: &str,
    second: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let bytes = std::fs::read(agent_path(agent_id)?)?;
    compose_agent_bytes(
        parent,
        agent_id,
        capability,
        input,
        second_capability,
        second,
        &bytes,
    )
}

pub(super) fn compose_agent_bytes(
    parent: &str,
    agent_id: &str,
    capability: &str,
    input: &[u8],
    second_capability: &str,
    second: &[u8],
    bytes: &[u8],
) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        input.len() <= 16 * 1024 * 1024
            && second.len() <= 24576
            && capability.len() <= 512
            && second_capability.len() <= 512,
        "fixture input exceeds its bounded memory layout"
    );
    let second_offset = 8192.max((2048 + input.len() + 15) & !15);
    let heap = 32768.max((second_offset + second.len() + 15) & !15);
    let pages = 2.max((heap + 65536).div_ceil(65536));
    let mut has_callback_lift = false;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::ComponentCanonicalSection(section) = payload? {
            for function in section {
                if let wasmparser::CanonicalFunction::Lift { options, .. } = function? {
                    has_callback_lift |= options
                        .iter()
                        .any(|option| matches!(option, wasmparser::CanonicalOption::Callback(_)));
                }
            }
        }
    }
    // This catches an obviously stale synchronous artifact before a fixture
    // waits for cancellation. Runtime cancellation/reuse remains the real proof.
    anyhow::ensure!(
        has_callback_lift,
        "{} has no callback lift; rebuild this worktree with scripts/build-agent-components.sh in its own CARGO_TARGET_DIR and point RUNTARA_AGENT_COMPONENTS_DIR at that output",
        agent_id
    );
    let escape = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("\\{byte:02x}"))
            .collect::<String>()
    };
    let parent = parent
        .replace("{{AGENT}}", agent_id)
        .replace("{{SECOND_INPUT_OFFSET}}", &second_offset.to_string())
        .replace("{{HEAP}}", &heap.to_string())
        .replace("{{PAGES}}", &pages.to_string())
        .replace("{{CAPABILITY}}", &escape(capability.as_bytes()))
        .replace("{{CAPABILITY_LEN}}", &capability.len().to_string())
        .replace(
            "{{SECOND_CAPABILITY}}",
            &escape(second_capability.as_bytes()),
        )
        .replace(
            "{{SECOND_CAPABILITY_LEN}}",
            &second_capability.len().to_string(),
        )
        .replace("{{INPUT}}", &escape(input))
        .replace("{{INPUT_LEN}}", &input.len().to_string())
        .replace("{{SECOND_INPUT}}", &escape(second))
        .replace("{{SECOND_INPUT_LEN}}", &second.len().to_string());
    let mut graph = CompositionGraph::new();
    let package = Package::from_bytes(
        "test:parent",
        None,
        wat::parse_str(parent)?,
        graph.types_mut(),
    )?;
    let socket = graph.register_package(package)?;
    let package = Package::from_bytes("test:http", None, bytes.to_vec(), graph.types_mut())?;
    let plug = graph.register_package(package)?;
    wac_graph::plug(&mut graph, vec![plug], socket)?;
    Ok(graph.encode(EncodeOptions::default())?)
}

pub(super) async fn request_headers(socket: &mut tokio::net::TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(socket.read_u8().await?);
        anyhow::ensure!(bytes.len() <= 8192, "unexpected request size");
    }
    Ok(bytes)
}

async fn cancel_real_http(body_wait: bool) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let input = serde_json::to_vec(
        &serde_json::json!({"url":format!("{base}/pending"), "timeout_ms":120000}),
    )?;
    let second = serde_json::to_vec(
        &serde_json::json!({"url":format!("{base}/ok"), "response_type":"text"}),
    )?;
    let bytes = compose(&input, &second)?;
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            let (mut socket, _) = listener.accept().await?;
            let request = request_headers(&mut socket).await?;
            anyhow::ensure!(request.starts_with(b"GET /pending "), "wrong first request");
            if body_wait {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx",
                    )
                    .await?;
            }
            started.notify_one();
            let mut byte = [0];
            match socket.read(&mut byte).await {
                Ok(0) => {}
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => anyhow::bail!("request was not closed: {other:?}"),
            }
            cleaned.notify_one();
            let (mut socket, _) = listener.accept().await?;
            let request = request_headers(&mut socket).await?;
            anyhow::ensure!(request.starts_with(b"GET /ok "), "wrong second request");
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await?;
            Ok::<_, anyhow::Error>(())
        }
    });
    let run = async {
        let output = cancel_and_reuse(bytes, FixtureContext::public(), started, cleaned).await?;
        assert_eq!(output["status_code"], 200);
        assert_eq!(output["body"], "ok");
        assert_eq!(output["success"], true);
        Ok::<_, anyhow::Error>(())
    };
    let result = tokio::time::timeout(Duration::from_secs(15), run).await;
    if !matches!(result, Ok(Ok(()))) {
        server.abort();
        let _ = server.await;
        return result?;
    }
    let mut server = server;
    let result = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
    match result {
        Ok(result) => result?,
        Err(error) => {
            server.abort();
            let _ = server.await;
            Err(error.into())
        }
    }
}

pub(super) async fn cancel_and_reuse(
    bytes: Vec<u8>,
    context: FixtureContext,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
) -> anyhow::Result<serde_json::Value> {
    cancel_and_reuse_with_resolver(
        bytes,
        context,
        started,
        cleaned,
        Arc::new(super::real_mcp::McpResolver::default()),
    )
    .await
}

async fn cancel_and_reuse_with_resolver(
    bytes: Vec<u8>,
    context: FixtureContext,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
    resolver: Arc<dyn runtara_component_host::ConnectionResolverHost>,
) -> anyhow::Result<Value> {
    let state = context.into_state().with_connection_resolver(resolver);
    cancel_and_reuse_with_state(bytes, state, started, cleaned).await
}

pub(super) async fn cancel_and_reuse_with_state(
    bytes: Vec<u8>,
    state: HostState,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
) -> anyhow::Result<Value> {
    let sibling_started = Arc::new(Notify::new());
    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let component = Component::new(&engine, bytes)?;
    let mut linker = runtara_component_host::build_linker(&engine)?;
    linker.root().func_wrap_concurrent("sibling", {
        let sibling_started = sibling_started.clone();
        move |_, (): ()| {
            let sibling_started = sibling_started.clone();
            let cleaned = cleaned.clone();
            Box::pin(async move {
                sibling_started.notify_one();
                cleaned.notified().await;
                Ok((7u32,))
            })
        }
    })?;
    linker
        .root()
        .func_wrap_concurrent("signal", move |_, (): ()| {
            let started = started.clone();
            let sibling_started = sibling_started.clone();
            Box::pin(async move {
                started.notified().await;
                sibling_started.notified().await;
                Ok(())
            })
        })?;
    let mut store = Store::new(&engine, state);
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let run = instance.get_typed_func::<(), (Vec<u8>,)>(&mut store, "run")?;
    let (output,) = run.call_async(&mut store, ()).await?;
    let output: serde_json::Value = serde_json::from_slice(&output)?;
    Ok(output)
}

#[tokio::test]
async fn built_http_agent_cancels_pending_headers_and_can_be_reused() -> anyhow::Result<()> {
    cancel_real_http(false).await
}

#[tokio::test]
async fn built_http_agent_cancels_after_partial_response_and_can_be_reused() -> anyhow::Result<()> {
    cancel_real_http(true).await
}

async fn invoke_agent(
    context: FixtureContext,
    capability: &str,
    input: Vec<u8>,
) -> anyhow::Result<Result<Vec<u8>, runtara_component_host::ErrorInfo>> {
    invoke_named_agent("http", context, capability, input).await
}

pub(super) async fn invoke_named_agent(
    agent_id: &str,
    context: FixtureContext,
    capability: &str,
    input: Vec<u8>,
) -> anyhow::Result<Result<Vec<u8>, runtara_component_host::ErrorInfo>> {
    let state = context
        .into_state()
        .with_connection_resolver(Arc::new(super::real_mcp::McpResolver::default()));
    invoke_named_agent_with_state(agent_id, state, capability, input).await
}

pub(super) async fn invoke_named_agent_with_state(
    agent_id: &str,
    state: HostState,
    capability: &str,
    input: Vec<u8>,
) -> anyhow::Result<Result<Vec<u8>, runtara_component_host::ErrorInfo>> {
    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let component = Component::from_file(&engine, agent_path(agent_id)?)?;
    let linker = runtara_component_host::build_linker(&engine)?;
    let mut store = Store::new(&engine, state);
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let interface = instance
        .get_export_index(
            &mut store,
            None,
            &format!("runtara:agent-{agent_id}/capabilities@0.4.0"),
        )
        .unwrap();
    let export = instance
        .get_export_index(&mut store, Some(&interface), "invoke")
        .unwrap();
    type Output = (Result<Vec<u8>, runtara_component_host::ErrorInfo>,);
    let invoke = instance.get_typed_func::<(String, Vec<u8>), Output>(&mut store, export)?;
    let (result,) = invoke
        .call_async(&mut store, (capability.into(), input))
        .await?;
    Ok(result)
}

#[tokio::test]
async fn async_http_export_preserves_input_and_dispatch_errors() -> anyhow::Result<()> {
    for (capability, input, code) in [
        ("missing", "{}", "UNKNOWN_CAPABILITY"),
        // JSON decoding has always preceded dispatch, including unknown names.
        ("missing", "{", "INPUT_DESERIALIZATION_ERROR"),
        (
            "http-request",
            "{\"method\":\"invalid\",\"url\":\"https://unused.invalid\"}",
            "INPUT_DESERIALIZATION_ERROR",
        ),
    ] {
        let result = invoke_agent(
            FixtureContext::public(),
            capability,
            input.as_bytes().to_vec(),
        )
        .await?;
        let error = result.expect_err("invalid inputs must fail before any HTTP request");
        assert_eq!(error.code, code);
        assert_eq!(error.category, "permanent");
        assert_eq!(error.severity, "error");
        assert!(!error.retryable);
    }
    Ok(())
}

#[tokio::test]
async fn async_http_preserves_coercion_host_context_and_error_response() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream = format!("http://{}/upstream", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let body = read_outbound(&mut socket).await?;
        assert_eq!(body["connection_id"], "fixture-connection");
        assert_eq!(body["endpoint"], "reports");
        assert_eq!(body["url"], "https://service.invalid/item?q=hello%20world");
        assert_eq!(body["timeout_ms"], 1000);
        assert_eq!(body["headers"]["X-Fixture"], "input");
        assert!(body["headers"].get("X-Runtara-Connection-Id").is_none());
        respond(
            &mut socket,
            serde_json::json!({"status":503,"headers":{"x-fixture":"response"},"body_raw":"bm8="}),
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    });
    let input = serde_json::to_vec(&serde_json::json!({
        "url":"https://service.invalid/item",
        "query_parameters":{"q":"hello world"},
        "headers":{"X-Fixture":"input"},
        "timeout_ms":"1000", "fail_on_error":"false", "response_type":"text",
        "connection_endpoint":"reports",
        "_connection":{"connection_id":"fixture-connection", "integration_id":"http", "parameters":{}}
    }))?;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        invoke_agent(
            FixtureContext::with_upstream("fixture-tenant", upstream, ""),
            "http-request",
            input,
        ),
    )
    .await;
    if !matches!(result, Ok(Ok(Ok(_)))) {
        server.abort();
        let _ = server.await;
        anyhow::bail!("outbound request failed: {result:?}");
    }
    let output = result??.unwrap();
    let output: serde_json::Value = serde_json::from_slice(&output)?;
    assert_eq!(output["status_code"], 503);
    assert_eq!(output["body"], "no");
    assert_eq!(output["headers"]["x-fixture"], "response");
    assert_eq!(output["success"], false);
    server.await??;
    Ok(())
}

pub(super) async fn read_outbound(socket: &mut tokio::net::TcpStream) -> anyhow::Result<Value> {
    read_outbound_limited(socket, 16_384).await
}

pub(super) async fn read_outbound_limited(
    socket: &mut tokio::net::TcpStream,
    max_bytes: usize,
) -> anyhow::Result<Value> {
    use base64::Engine as _;
    let headers = String::from_utf8(request_headers(socket).await?)?;
    let header = |name: &str| {
        headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    };
    let metadata = base64::engine::general_purpose::STANDARD
        .decode(header("x-test-outbound").expect("native fixture metadata"))?;
    let mut request: Value = serde_json::from_slice(&metadata)?;
    assert_eq!(request["tenant"], "fixture-tenant");
    assert_eq!(
        headers.split_whitespace().next().unwrap(),
        request["method"].as_str().unwrap()
    );
    let length: usize = header("content-length").unwrap_or("0").parse()?;
    anyhow::ensure!(length < max_bytes, "unexpected provider body size");
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).await?;
    if request["body_present"] == true {
        request["body_raw"] = base64::engine::general_purpose::STANDARD
            .encode(&bytes)
            .into();
        request["body"] = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    }
    Ok(request)
}

pub(super) async fn respond(
    socket: &mut tokio::net::TcpStream,
    fixture: Value,
) -> anyhow::Result<()> {
    use base64::Engine as _;
    let status = fixture["status"].as_u64().unwrap_or(200);
    let body = if let Some(raw) = fixture["body_raw"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(raw) {
            Ok(bytes) => bytes,
            Err(_) => {
                // A malformed transport fixture now corrupts HTTP framing,
                // since base64 no longer exists on the production wire.
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: invalid\r\n\r\n")
                    .await?;
                return Ok(());
            }
        }
    } else if fixture.get("status").is_none() {
        serde_json::to_vec(&fixture)?
    } else if fixture["body"].is_null() {
        vec![]
    } else {
        serde_json::to_vec(&fixture["body"])?
    };
    let mut headers = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(values) = fixture["headers"].as_object() {
        for (name, value) in values {
            headers.push_str(&format!("{name}: {}\r\n", value.as_str().unwrap()));
        }
    }
    headers.push_str("\r\n");
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(&body).await?;
    Ok(())
}

/// Join the standard cancellation proof and its owned local endpoint, with
/// bounded failure cleanup so a broken Agent cannot leave a fixture running.
pub(super) async fn run_cancellation_fixture(
    bytes: Vec<u8>,
    context: FixtureContext,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<Value> {
    run_cancellation_fixture_with_resolver(
        bytes,
        context,
        started,
        cleaned,
        server,
        Arc::new(super::real_mcp::McpResolver::default()),
    )
    .await
}

pub(super) async fn run_cancellation_fixture_with_resolver(
    bytes: Vec<u8>,
    context: FixtureContext,
    started: Arc<Notify>,
    cleaned: Arc<Notify>,
    mut server: tokio::task::JoinHandle<anyhow::Result<()>>,
    resolver: Arc<dyn runtara_component_host::ConnectionResolverHost>,
) -> anyhow::Result<Value> {
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cancel_and_reuse_with_resolver(bytes, context, started, cleaned, resolver),
    )
    .await;
    let output = match result {
        Ok(Ok(output)) => output,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("cancellation/reuse failed: {other:?}");
        }
    };
    match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        Ok(result) => result??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            return Err(error.into());
        }
    }
    Ok(output)
}
