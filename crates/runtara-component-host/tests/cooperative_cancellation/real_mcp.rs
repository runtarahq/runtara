//! Standard cancellation during MCP connection lookup and the three-request
//! handshake. All services are local stubs; no remote tool is invoked.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_proxy, request_headers, respond,
    run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn input(after: bool, lookup: bool) -> Value {
    let parameters = if lookup {
        json!({})
    } else {
        json!({"url":"https://mcp.invalid/rpc","tool_scope":["echo"],"extra_headers":{"X-Fixture":"present"}})
    };
    json!({"query":"echo","limit":"5","tool_name":"echo","args":{"phase":if after {"after"} else {"before"}},"_connection":{"connection_id":"fixture-connection","integration_id":"mcp","parameters":parameters}})
}

async fn request(
    socket: &mut tokio::net::TcpStream,
    stage: usize,
    method: &str,
    after: bool,
) -> anyhow::Result<()> {
    let request = read_proxy(socket).await?;
    assert_eq!(request["url"], "https://mcp.invalid/rpc");
    assert_eq!(request["connection_id"], "fixture-connection");
    assert_eq!(request["headers"]["X-Fixture"], "present");
    assert_eq!(
        request["headers"]["Accept"],
        "application/json, text/event-stream"
    );
    if stage == 1 {
        assert!(request["headers"].get("Mcp-Session-Id").is_none());
    } else {
        assert_eq!(
            request["headers"]["Mcp-Session-Id"],
            if after {
                "after-session"
            } else {
                "before-session"
            }
        );
    }
    let body = &request["body"];
    assert_eq!(
        body["method"],
        match stage {
            1 => "initialize",
            2 => "notifications/initialized",
            _ => method,
        }
    );
    if stage == 1 {
        assert_eq!(body["params"]["protocolVersion"], "2025-03-26");
    }
    if stage == 3 && method == "tools/call" {
        assert_eq!(body["params"]["name"], "echo");
        assert_eq!(
            body["params"]["arguments"]["phase"],
            if after { "after" } else { "before" }
        );
    }
    Ok(())
}

async fn reply(
    socket: &mut tokio::net::TcpStream,
    stage: usize,
    after: bool,
) -> anyhow::Result<()> {
    let result = if stage == 1 {
        json!({"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}})
    } else {
        json!({"content":[{"type":"text","text":"after"}],"isError":false})
    };
    let sse = format!(
        "event: message\ndata: {}\n\n",
        json!({"jsonrpc":"2.0","id":if stage==1 {1} else {2},"result":result})
    );
    respond(socket,json!({"status":if stage==2 {202} else {200},"headers":{"mcp-session-id":if after {"after-session"} else {"before-session"},"content-type":"text/event-stream"},"body_raw":BASE64.encode(if stage==2 {b"".as_slice()} else {sse.as_bytes()})})).await
}

async fn cancellation(method: &'static str, blocked: usize, partial: bool) -> anyhow::Result<()> {
    let capability = if method == "tools/list" {
        "mcp-tool-search"
    } else {
        "mcp-tool-invoke"
    };
    let bytes = compose_agent(
        "mcp",
        capability,
        &serde_json::to_vec(&input(false, blocked == 0))?,
        "mcp-tool-invoke",
        &serde_json::to_vec(&input(true, false))?,
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            for stage in (if blocked == 0 { 0 } else { 1 })..=blocked {
                let (mut socket, _) = listener.accept().await?;
                if stage == 0 {
                    let headers = request_headers(&mut socket).await?;
                    assert!(headers.starts_with(b"GET /fixture-tenant/fixture-connection "));
                } else {
                    request(&mut socket, stage, method, false).await?;
                }
                if stage < blocked {
                    reply(&mut socket, stage, false).await?;
                } else {
                    if partial {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 300\r\nConnection: close\r\n\r\n{").await?;
                    }
                    started.notify_one();
                    match socket.read(&mut [0]).await {
                        Ok(0) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                        other => anyhow::bail!("MCP request not closed: {other:?}"),
                    }
                    cleaned.notify_one();
                }
            }
            // Fresh invocation must initialize a fresh session; the cancelled
            // one must neither notify initialization nor send its tool request.
            for stage in 1..=3 {
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, stage, "tools/call", true).await?;
                reply(&mut socket, stage, true).await?;
            }
            anyhow::Ok(())
        }
    });
    let mut context = CallContext::for_test("fixture-tenant", format!("{base}/proxy"), "", "", "");
    context.connection_service_url = Some(base);
    let output = run_cancellation_fixture(bytes, context, started, cleaned, server).await?;
    assert_eq!(
        output,
        json!({"text":"after","content":[{"type":"text","text":"after"}],"is_error":false})
    );
    Ok(())
}

#[tokio::test]
async fn mcp_cancel_interrupts_connection_lookup_headers_and_body() -> anyhow::Result<()> {
    for partial in [false, true] {
        cancellation("tools/call", 0, partial).await?;
    }
    Ok(())
}
#[tokio::test]
async fn mcp_cancel_stops_each_search_handshake_stage() -> anyhow::Result<()> {
    for stage in 1..=3 {
        for partial in [false, true] {
            cancellation("tools/list", stage, partial).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn mcp_cancel_stops_each_tool_handshake_stage() -> anyhow::Result<()> {
    for stage in 1..=3 {
        for partial in [false, true] {
            cancellation("tools/call", stage, partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn mcp_async_dispatch_keeps_scope_and_protocol_errors() -> anyhow::Result<()> {
    let mut forbidden = input(false, false);
    forbidden["tool_name"] = "forbidden".into();
    let error = invoke_named_agent(
        "mcp",
        CallContext::placeholder_for_metadata(),
        "mcp-tool-invoke",
        serde_json::to_vec(&forbidden)?,
    )
    .await?
    .unwrap_err();
    assert_eq!(error.code, "MCP_TOOL_OUT_OF_SCOPE");
    assert!(!error.retryable);
    for server_error in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = format!("http://{}/proxy", listener.local_addr()?);
        let server = tokio::spawn(async move {
            for stage in 1..=if server_error { 3 } else { 1 } {
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, stage, "tools/call", false).await?;
                if !server_error {
                    respond(&mut socket, json!({"status":503,"headers":{},"body":{}})).await?;
                } else if stage < 3 {
                    reply(&mut socket, stage, false).await?;
                } else {
                    respond(&mut socket,json!({"status":200,"headers":{},"body":{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"fixture"}}})).await?;
                }
            }
            anyhow::Ok(())
        });
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            invoke_named_agent(
                "mcp",
                CallContext::for_test("fixture-tenant", proxy, "", "", ""),
                "mcp-tool-invoke",
                serde_json::to_vec(&input(false, false))?,
            ),
        )
        .await;
        let error = match result {
            Ok(Ok(Err(error))) => error,
            other => {
                server.abort();
                let _ = server.await;
                anyhow::bail!("expected MCP error: {other:?}");
            }
        };
        server.await??;
        assert_eq!(
            error.code,
            if server_error {
                "MCP_SERVER_ERROR"
            } else {
                "MCP_HTTP_ERROR"
            }
        );
        assert_eq!(error.retryable, !server_error);
    }
    Ok(())
}

#[tokio::test]
async fn mcp_async_search_preserves_tool_scope_and_schema() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    let server = tokio::spawn(async move {
        for stage in 1..=3 {
            let (mut socket, _) = listener.accept().await?;
            request(&mut socket, stage, "tools/list", false).await?;
            if stage < 3 {
                reply(&mut socket, stage, false).await?;
            } else {
                respond(&mut socket, json!({"status":200,"headers":{},"body":{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"echo a fixture","inputSchema":{"type":"object"}},{"name":"forbidden","description":"echo forbidden","inputSchema":{}}]}}})).await?;
            }
        }
        anyhow::Ok(())
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        invoke_named_agent(
            "mcp",
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            "mcp-tool-search",
            serde_json::to_vec(&input(false, false))?,
        ),
    )
    .await;
    let output = match result {
        Ok(Ok(Ok(output))) => serde_json::from_slice::<Value>(&output)?,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("expected tool list: {other:?}");
        }
    };
    server.await??;
    assert_eq!(output["total_available"], 2);
    assert_eq!(output["tools"].as_array().unwrap().len(), 1);
    assert_eq!(output["tools"][0]["name"], "echo");
    assert_eq!(output["tools"][0]["inputSchema"], json!({"type":"object"}));
    Ok(())
}
