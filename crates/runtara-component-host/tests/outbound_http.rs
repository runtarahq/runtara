//! Real components use typed outbound calls without any internal HTTP listener.
mod common;

use runtara_component_host::outbound_http::{
    self, Destination, OutboundContext, OutboundError, OutboundHttpHost, RequestOptions, Response,
};
use runtara_component_host::{
    ComponentDispatcherService, DispatcherEnv, ResolvedConnection, TestCapabilityRequest,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Service {
    calls: Mutex<Vec<(OutboundContext, RequestOptions)>>,
}

#[async_trait::async_trait]
impl OutboundHttpHost for Service {
    async fn request(
        &self,
        context: &OutboundContext,
        request: RequestOptions,
    ) -> Result<Response, OutboundError> {
        self.calls
            .lock()
            .unwrap()
            .push((context.clone(), request.clone()));
        let url = match &request.destination {
            Destination::Public(url) => url,
            Destination::Connection(connection) => &connection.url,
        };
        if url.ends_with("/timeout") {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        if url.ends_with("/missing") {
            return Err(OutboundError {
                code: "CONNECTION_NOT_FOUND".into(),
                message: "Connection not found".into(),
                status: Some(404),
                body: br#"{"code":"CONNECTION_NOT_FOUND"}"#.to_vec(),
                retry_after_ms: None,
            });
        }
        let mut headers = vec![
            ("Content-Type".into(), "application/json".into()),
            ("X-Test".into(), "first".into()),
            ("x-test".into(), "last".into()),
        ];
        let body = if url.ends_with("/chat/completions") {
            json!({"id":"completion", "model":"fixture", "choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"total_tokens":3}})
        } else if url.is_empty() {
            let rpc: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
            headers.push(("Mcp-Session-Id".into(), "session".into()));
            match rpc["method"].as_str().unwrap() {
                "initialize" => {
                    json!({"jsonrpc":"2.0","id":rpc["id"],"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}})
                }
                "notifications/initialized" => {
                    return Ok(Response {
                        status: 202,
                        headers,
                        body: vec![],
                    });
                }
                "tools/call" => {
                    json!({"jsonrpc":"2.0","id":rpc["id"],"result":{"content":[{"type":"text","text":"done"}],"isError":false}})
                }
                other => panic!("unexpected MCP method: {other}"),
            }
        } else {
            json!({"ok":true})
        };
        Ok(Response {
            status: 200,
            headers,
            body: serde_json::to_vec(&body).unwrap(),
        })
    }
}

struct Resolver;
#[async_trait::async_trait]
impl runtara_component_host::ConnectionResolverHost for Resolver {
    async fn describe(&self, tenant: &str, connection: String) -> Result<Vec<u8>, String> {
        assert_eq!(tenant, "host-tenant");
        assert_eq!(connection, "opaque-connection");
        Ok(br#"{"integrationId":"mcp","metadata":{"tool_scope":["echo"]}}"#.to_vec())
    }
    async fn resolve_resource(&self, _: &str, _: String, _: Vec<u8>) -> Result<Vec<u8>, String> {
        Err("unsupported resource".into())
    }
}

async fn dispatcher(service: Option<Arc<Service>>) -> anyhow::Result<ComponentDispatcherService> {
    let bundle = tempfile::tempdir()?;
    for agent in ["http", "openai", "mcp"] {
        for extension in ["wasm", "meta.json"] {
            let name = format!("runtara_agent_{agent}.{extension}");
            std::fs::copy(common::bundle_dir().join(&name), bundle.path().join(name))?;
        }
    }
    let dispatcher = ComponentDispatcherService::from_dir(
        bundle.path(),
        DispatcherEnv {
            core_http_url: String::new(),
        },
    )
    .await?;
    dispatcher.set_connection_resolver(Arc::new(Resolver))?;
    if let Some(service) = service {
        dispatcher.set_outbound_http(service)?;
    }
    Ok(dispatcher)
}

fn call(agent: &str, capability: &str, input: Value, connection: bool) -> TestCapabilityRequest {
    TestCapabilityRequest {
        tenant_id: "host-tenant".into(),
        agent_id: agent.into(),
        capability_id: capability.into(),
        input,
        connection: connection.then(|| ResolvedConnection {
            connection_id: "opaque-connection".into(),
            integration_id: agent.into(),
            connection_subtype: None,
            parameters: json!({}),
            rate_limit_config: None,
        }),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn components_preserve_typed_fields_identity_and_public_urls() -> anyhow::Result<()> {
    let service = Arc::new(Service::default());
    let dispatcher = dispatcher(Some(service.clone())).await?;
    let result = dispatcher.test_capability(call("http", "http-request", json!({
        "url":"/echo", "method":"POST", "body":{"message":"hello"}, "connection_endpoint":"alternate",
        "headers":{"X-Org-Id":"forged-tenant","X-Runtara-Connection-Id":"forged-connection"}
    }), true)).await?;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output.as_ref().unwrap()["headers"]["x-test"], "last");
    let signed_url = "https://provider.invalid/file?signature=a%2Fb%2Bz&x=1&x=2";
    let result = dispatcher
        .test_capability(call(
            "http",
            "http-request",
            json!({"url":signed_url}),
            false,
        ))
        .await?;
    assert!(result.success, "{:?}", result.error);
    let result = dispatcher
        .test_capability(call(
            "openai",
            "openai-chat-completion",
            json!({"model":"fixture", "messages":[{"role":"user","content":"hi"}]}),
            true,
        ))
        .await?;
    assert!(result.success, "{:?}", result.error);
    let result = dispatcher
        .test_capability(call(
            "mcp",
            "mcp-tool-invoke",
            json!({"tool_name":"echo", "args":{"value":1}}),
            true,
        ))
        .await?;
    assert!(result.success, "{:?}", result.error);
    let calls = service.calls.lock().unwrap();
    assert_eq!(calls.len(), 6);
    assert!(calls.iter().all(|(context, _)| context.tenant_id == "host-tenant" && context.instance_id.is_none()));
    let Destination::Connection(connection) = &calls[0].1.destination else {
        panic!("connection destination")
    };
    assert_eq!(connection.connection_id, "opaque-connection");
    assert_eq!(connection.endpoint.as_deref(), Some("alternate"));
    assert_eq!(
        serde_json::from_slice::<Value>(calls[0].1.body.as_deref().unwrap())?,
        json!({"message":"hello"})
    );
    assert!(matches!(&calls[1].1.destination, Destination::Public(url) if url == signed_url));
    let Destination::Connection(connection) = &calls[2].1.destination else {
        panic!("provider connection")
    };
    assert_eq!(connection.connection_id, "opaque-connection");
    assert_eq!(
        calls[5]
            .1
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("mcp-session-id"))
            .unwrap()
            .1,
        "session"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn deadline_and_host_errors_preserve_agent_behavior_and_allow_reuse() -> anyhow::Result<()> {
    let service = Arc::new(Service::default());
    let dispatcher = dispatcher(Some(service)).await?;
    let result = dispatcher
        .test_capability(call(
            "http",
            "http-request",
            json!({"url":"https://provider.invalid/timeout", "timeout_ms":20}),
            false,
        ))
        .await?;
    assert_eq!(result.error.unwrap().code, "NETWORK_ERROR");
    let result = dispatcher
        .test_capability(call(
            "http",
            "http-request",
            json!({"url":"/missing"}),
            true,
        ))
        .await?;
    assert_eq!(result.error.unwrap().code, "HTTP_4XX");
    let result = dispatcher
        .test_capability(call("http", "http-request", json!({"url":"/ok"}), true))
        .await?;
    assert!(result.success, "{:?}", result.error);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_service_has_no_direct_network_fallback() -> anyhow::Result<()> {
    let dispatcher = dispatcher(None).await?;
    let result = dispatcher
        .test_capability(call(
            "http",
            "http-request",
            json!({"url":"https://provider.invalid/"}),
            false,
        ))
        .await?;
    assert_eq!(result.error.unwrap().code, "NETWORK_ERROR");
    assert_eq!(outbound_http::DEFAULT_TIMEOUT.as_secs(), 30);
    Ok(())
}
