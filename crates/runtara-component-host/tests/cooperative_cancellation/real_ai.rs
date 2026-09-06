//! Built AI Agents use the production component linker and a local proxy stub.
//! Cancellation must release I/O before acknowledgement and leave the same
//! Agent instance usable, including when a capability normally catches errors.
use super::real_agent::{cancel_and_reuse, compose_agent, invoke_named_agent, read_proxy, respond};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection(provider: &str) -> Value {
    json!({"connection_id":"fixture-connection","integration_id":if provider == "openai" {"openai_api_key"} else {"aws_credentials"},"parameters":{}})
}

fn input(provider: &str, capability: &str) -> Value {
    let mut input = json!({"_connection":connection(provider),"provider":provider,
        "prompt":"before","user_prompt":"before","timeout_ms":300_000});
    match capability {
        "embed-text" => {
            input["texts"] = json!(["before", "must not start after cancellation"]);
        }
        "summarize-memory" => {
            input["max_messages"] = json!(1);
            input["state"] = json!({"chat_history":[{"role":"user","content":"old"},{"role":"user","content":"recent"}],"iterations":2,"tool_call_log":[]});
        }
        _ => {}
    }
    input
}

fn request_body(request: &Value) -> anyhow::Result<Value> {
    if let Some(raw) = request["body_raw"].as_str() {
        Ok(serde_json::from_slice(&BASE64.decode(raw)?)?)
    } else {
        Ok(request["body"].clone())
    }
}

fn assert_connection(request: &Value) {
    assert_eq!(request["connection_id"], "fixture-connection");
    assert_eq!(request["method"], "POST");
    assert!(request["headers"].get("Authorization").is_none());
    assert!(request["headers"].get("X-Runtara-Connection-Id").is_none());
}

async fn cancellation(
    agent: &str,
    provider: &str,
    capability: &str,
    partial: bool,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    // Reuse the same Agent via a request distinguishable from any continuation
    // of the cancelled invocation (especially Bedrock's embedding batch).
    let (next_capability, next_input, response, expected) = match agent {
        "openai" => (
            "openai-moderate-content",
            json!({"_connection":connection(provider),"input":"after"}),
            json!({"results":[],"model":"fixture"}),
            json!({"results":[],"model":"fixture"}),
        ),
        "bedrock" => (
            "bedrock-list-models",
            json!({"_connection":connection(provider)}),
            json!({"modelSummaries":[]}),
            json!({"model_summaries":[]}),
        ),
        "ai-tools" => (
            "text-completion",
            json!({"_connection":connection("openai"),"provider":"openai","prompt":"after"}),
            json!({"model":"fixture","choices":[{"message":{"role":"assistant","content":"after"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
            json!({"text":"after","model":"fixture","finish_reason":"stop","usage":{"promptTokens":2,"completionTokens":1,"totalTokens":3}}),
        ),
        _ => unreachable!(),
    };
    let bytes = compose_agent(
        agent,
        capability,
        &serde_json::to_vec(&input(provider, capability))?,
        next_capability,
        &serde_json::to_vec(&next_input)?,
    )?;
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let agent = agent.to_owned();
    let provider = provider.to_owned();
    let capability = capability.to_owned();
    let server =
        tokio::spawn({
            let started = started.clone();
            let cleaned = cleaned.clone();
            async move {
                let (mut socket, _) = listener.accept().await?;
                let request = read_proxy(&mut socket).await?;
                assert_connection(&request);
                let body = request_body(&request)?;
                let shared = matches!(
                    capability.as_str(),
                    "chat-completion" | "chat-turn" | "summarize-memory"
                );
                if shared {
                    assert_eq!(request["ai_provider"], provider);
                    if provider == "openai" {
                        assert_eq!(request["url"], "/v1/chat/completions");
                        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
                    } else {
                        assert!(request["url"].as_str().unwrap().ends_with("/converse"));
                        assert!(!body["messages"].as_array().unwrap().is_empty());
                    }
                    if capability != "summarize-memory" {
                        assert_eq!(request["timeout_ms"], 300_000);
                    }
                } else if capability == "embed-text" {
                    if provider == "bedrock" {
                        assert_eq!(body["inputText"], "before");
                    } else {
                        assert_eq!(
                            body["input"],
                            json!(["before", "must not start after cancellation"])
                        );
                    }
                } else {
                    assert!(request["url"].as_str().unwrap().starts_with(
                        if provider == "openai" {
                            "https://api.openai.com/"
                        } else if agent == "ai-tools" {
                            "/model/"
                        } else {
                            "https://bedrock-runtime.amazonaws.com/"
                        }
                    ));
                }
                if partial {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 300\r\nConnection: close\r\n\r\n{",
                        )
                        .await?;
                }
                started.notify_one();
                match socket.read(&mut [0]).await {
                    Ok(0) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => anyhow::bail!("pending AI request not closed: {other:?}"),
                }
                cleaned.notify_one();

                let (mut socket, _) = listener.accept().await?;
                let request = read_proxy(&mut socket).await?;
                assert_eq!(request["connection_id"], "fixture-connection");
                match agent.as_str() {
                    "openai" => {
                        assert_eq!(request["url"], "https://api.openai.com/v1/moderations");
                        assert_eq!(request_body(&request)?["input"], "after");
                    }
                    "bedrock" => {
                        assert_eq!(request["method"], "GET");
                        assert_eq!(
                            request["url"],
                            "https://bedrock.amazonaws.com/foundation-models"
                        );
                    }
                    "ai-tools" => {
                        assert_eq!(request["url"], "https://api.openai.com/v1/chat/completions");
                        assert_eq!(request_body(&request)?["messages"][0]["content"], "after");
                    }
                    _ => unreachable!(),
                }
                respond(
                    &mut socket,
                    json!({"status":200,"headers":{},"body":response}),
                )
                .await
            }
        });
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cancel_and_reuse(
            bytes,
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            started,
            cleaned,
        ),
    )
    .await;
    let output = match result {
        Ok(Ok(output)) => output,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("AI cancellation/reuse failed: {other:?}");
        }
    };
    let mut server = server;
    match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        Ok(result) => result??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            return Err(error.into());
        }
    }
    assert_eq!(output, expected);
    Ok(())
}

#[tokio::test]
async fn provider_agents_cancel_headers_and_body_and_reuse() -> anyhow::Result<()> {
    for provider in ["openai", "bedrock"] {
        for partial in [false, true] {
            cancellation(provider, provider, "text-completion", partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn ai_tools_text_helpers_cancel_headers_and_body() -> anyhow::Result<()> {
    for provider in ["openai", "bedrock"] {
        for partial in [false, true] {
            cancellation("ai-tools", provider, "text-completion", partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn shared_providers_cancel_chat_completion_and_turn() -> anyhow::Result<()> {
    for provider in ["openai", "bedrock"] {
        for capability in ["chat-completion", "chat-turn"] {
            for partial in [false, true] {
                cancellation("ai-tools", provider, capability, partial).await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn summary_cancellation_does_not_return_fallback_state() -> anyhow::Result<()> {
    for provider in ["openai", "bedrock"] {
        for partial in [false, true] {
            cancellation("ai-tools", provider, "summarize-memory", partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn embedding_cancellation_stops_the_remaining_batch() -> anyhow::Result<()> {
    for provider in ["openai", "bedrock"] {
        for partial in [false, true] {
            cancellation("ai-tools", provider, "embed-text", partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn ai_async_dispatch_keeps_validation_and_connection_errors() -> anyhow::Result<()> {
    for (agent, missing_connection) in [
        ("openai", "OPENAI_MISSING_CONNECTION"),
        ("bedrock", "BEDROCK_MISSING_CONNECTION"),
        ("ai-tools", "AI_TOOLS_MISSING_CONNECTION"),
    ] {
        for (capability, input, code) in [
            ("missing", "{}", "UNKNOWN_CAPABILITY"),
            ("text-completion", "{", "INPUT_DESERIALIZATION_ERROR"),
            (
                "text-completion",
                r#"{"prompt":"fixture"}"#,
                missing_connection,
            ),
        ] {
            let error = invoke_named_agent(
                agent,
                CallContext::placeholder_for_metadata(),
                capability,
                input.as_bytes().to_vec(),
            )
            .await?
            .unwrap_err();
            assert_eq!(error.code, code, "{agent}");
            assert!(!error.retryable);
        }
    }
    Ok(())
}

async fn exchange(
    agent: &str,
    provider: &str,
    capability: &str,
    response: Value,
) -> anyhow::Result<(Value, Result<Value, runtara_component_host::ErrorInfo>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let request = read_proxy(&mut socket).await?;
        assert_connection(&request);
        respond(&mut socket, response).await?;
        anyhow::Ok(request)
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        invoke_named_agent(
            agent,
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            capability,
            serde_json::to_vec(&input(provider, capability))?,
        ),
    )
    .await;
    let result = match result {
        Ok(Ok(result)) => result,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("AI invocation failed: {other:?}");
        }
    };
    let mut server = server;
    let request = match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        Ok(result) => result??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            return Err(error.into());
        }
    };
    Ok((
        request,
        result.map(|bytes| serde_json::from_slice(&bytes).expect("Agent returned JSON")),
    ))
}

#[tokio::test]
async fn async_provider_helpers_preserve_http_error_contracts() -> anyhow::Result<()> {
    for (agent, provider) in [
        ("openai", "openai"),
        ("bedrock", "bedrock"),
        ("ai-tools", "openai"),
        ("ai-tools", "bedrock"),
    ] {
        for (status, code, retryable) in [
            (429, "HTTP_429", true),
            (403, "HTTP_4XX", false),
            (503, "HTTP_5XX", true),
        ] {
            let (_, result) = exchange(
                agent,
                provider,
                "text-completion",
                json!({"status":status,"headers":{"retry-after":"2"},"body":{"error":"fixture"}}),
            )
            .await?;
            let error = result.unwrap_err();
            assert_eq!(error.code, code, "{agent}/{provider}/{status}");
            assert_eq!(error.retryable, retryable);
            // AI-tools preserves retry hints for any error; provider-specific
            // Agents attach them only to 429. This migration keeps both contracts.
            assert_eq!(
                error.retry_after_ms,
                if status == 429 || agent == "ai-tools" {
                    Some(2000)
                } else {
                    None
                }
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn async_shared_providers_preserve_text_tools_usage_and_failures() -> anyhow::Result<()> {
    for provider in ["openai", "bedrock"] {
        let body = if provider == "openai" {
            json!({"choices":[{"message":{"content":"answer","tool_calls":[{"id":"call-1","function":{"name":"echo","arguments":"{\"value\":42}"}}]}}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}})
        } else {
            json!({"output":{"message":{"content":[{"text":"answer"},{"toolUse":{"toolUseId":"call-1","name":"echo","input":{"value":42}}}]}},"usage":{"inputTokens":2,"outputTokens":1,"totalTokens":3}})
        };
        let (request, result) = exchange(
            "ai-tools",
            provider,
            "chat-completion",
            json!({"status":200,"headers":{},"body":body}),
        )
        .await?;
        assert_eq!(request["ai_provider"], provider);
        assert_eq!(request["timeout_ms"], 300_000);
        assert_eq!(
            result.unwrap(),
            json!({"choice":[{"text":"answer"},{"id":"call-1","function":{"name":"echo","arguments":{"value":42}}}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}})
        );

        for body in [
            json!({"status":429,"headers":{"retry-after":"2"},"body":{"error":"fixture"}}),
            json!({"status":200,"headers":{},"body":{}}),
        ] {
            let (_, result) = exchange("ai-tools", provider, "chat-completion", body).await?;
            let error = result.unwrap_err();
            assert_eq!(error.code, "AI_CHAT_COMPLETION_FAILED");
            assert!(error.retryable);
            assert_eq!(error.retry_after_ms, None);
        }
        // Ordinary provider failure still produces the documented summary
        // fallback; the cancellation tests above must never take this branch.
        let (_, result) = exchange(
            "ai-tools",
            provider,
            "summarize-memory",
            json!({"status":503,"headers":{},"body":{}}),
        )
        .await?;
        let output = result.unwrap();
        let history = output["state"]["chat_history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0].to_string().contains("[Summary unavailable]"));
        assert_eq!(history[1], json!({"role":"user","content":"recent"}));
        assert_eq!(output["state"]["iterations"], 2);
    }
    Ok(())
}
