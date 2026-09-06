//! Real Slack WASM bindings, with every request confined to a local proxy stub.
use super::real_agent::{cancel_and_reuse, compose_agent, invoke_named_agent, request_headers};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection() -> Value {
    json!({"connection_id":"fixture-connection","integration_id":"slack_bot","parameters":{}})
}
fn message(text: &str) -> Value {
    json!({"channel":"C-fixture","text":text,"unfurl_links":"false","_connection":connection()})
}

async fn read_proxy(socket: &mut tokio::net::TcpStream) -> anyhow::Result<Value> {
    let headers = String::from_utf8(request_headers(socket).await?)?;
    anyhow::ensure!(
        headers.starts_with("POST /proxy "),
        "request bypassed the local proxy"
    );
    anyhow::ensure!(
        headers
            .to_lowercase()
            .contains("x-org-id: fixture-tenant\r\n"),
        "missing tenant context"
    );
    let length: usize = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .trim()
        .parse()?;
    anyhow::ensure!(length < 16_384, "unexpected request size");
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn respond(socket: &mut tokio::net::TcpStream, envelope: Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(&envelope)?;
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .as_bytes(),
        )
        .await?;
    socket.write_all(&bytes).await?;
    Ok(())
}

async fn cancellation(
    upload: bool,
    blocked_stage: usize,
    partial_body: bool,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    let (capability, input) = if upload {
        (
            "upload-file",
            json!({"channel":"C-fixture","filename":"fixture.txt","content":"aGVsbG8=","_connection":connection()}),
        )
    } else {
        ("send-message", message("before"))
    };
    let bytes = compose_agent(
        "slack",
        capability,
        &serde_json::to_vec(&input)?,
        "send-message",
        &serde_json::to_vec(&message("after"))?,
    )?;
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            for stage in 1..=blocked_stage {
                let (mut socket, _) = listener.accept().await?;
                let request = read_proxy(&mut socket).await?;
                let expected_url = match (upload, stage) {
                    (false, _) => "https://slack.com/api/chat.postMessage",
                    (true, 1) => "https://slack.com/api/files.getUploadURLExternal",
                    (true, 2) => "https://upload.invalid/blob",
                    (true, 3) => "https://slack.com/api/files.completeUploadExternal",
                    _ => unreachable!(),
                };
                assert_eq!(request["url"], expected_url);
                assert_eq!(request["method"], "POST");
                assert!(request["headers"].get("X-Runtara-Connection-Id").is_none());
                if upload && stage == 2 {
                    assert!(
                        request["connection_id"].is_null(),
                        "presigned upload gained credential injection"
                    );
                    assert_eq!(request["body_raw"], "aGVsbG8=");
                } else {
                    assert_eq!(request["connection_id"], "fixture-connection");
                }
                if stage < blocked_stage {
                    respond(&mut socket, json!({"status":200,"headers":{},"body":{"ok":true,"upload_url":"https://upload.invalid/blob","file_id":"F-fixture"}})).await?;
                    continue;
                }
                if partial_body {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{",
                        )
                        .await?;
                }
                started.notify_one();
                match socket.read(&mut [0]).await {
                    Ok(0) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => anyhow::bail!("pending request not closed: {other:?}"),
                }
                cleaned.notify_one();
            }
            // The cancelled invocation cannot advance to another upload stage.
            // Only the parent's fresh call in the same Agent instance may arrive.
            let (mut socket, _) = listener.accept().await?;
            let request = read_proxy(&mut socket).await?;
            assert_eq!(request["url"], "https://slack.com/api/chat.postMessage");
            let body: Value =
                serde_json::from_slice(&BASE64.decode(request["body_raw"].as_str().unwrap())?)?;
            assert_eq!(body["text"], "after");
            assert_eq!(body["unfurl_links"], false, "macro coercion was bypassed");
            respond(&mut socket, json!({"status":200,"headers":{},"body":{"ok":true,"channel":"C-fixture","ts":"42"}})).await?;
            anyhow::Ok(())
        }
    });
    let run = tokio::time::timeout(
        Duration::from_secs(15),
        cancel_and_reuse(
            bytes,
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            started,
            cleaned,
        ),
    )
    .await;
    let output = match run {
        Ok(Ok(output)) => output,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("cancellation/reuse failed: {other:?}");
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
    assert_eq!(output, json!({"ok":true,"channel":"C-fixture","ts":"42"}));
    Ok(())
}

#[tokio::test]
async fn slack_cancels_pending_headers_and_reuses_the_same_agent() -> anyhow::Result<()> {
    cancellation(false, 1, false).await
}
#[tokio::test]
async fn slack_cancels_partial_proxy_response_and_reuses_the_same_agent() -> anyhow::Result<()> {
    cancellation(false, 1, true).await
}
#[tokio::test]
async fn slack_upload_cancels_before_obtaining_upload_url() -> anyhow::Result<()> {
    cancellation(true, 1, false).await
}
#[tokio::test]
async fn slack_upload_cancels_while_uploading_without_finalizing() -> anyhow::Result<()> {
    cancellation(true, 2, false).await
}
#[tokio::test]
async fn slack_upload_cancels_while_finalizing() -> anyhow::Result<()> {
    cancellation(true, 3, false).await
}

#[tokio::test]
async fn slack_async_dispatch_preserves_validation_and_connection_errors() -> anyhow::Result<()> {
    for (capability, input, code) in [
        ("missing", "{}", "UNKNOWN_CAPABILITY"),
        ("missing", "{", "INPUT_DESERIALIZATION_ERROR"),
        ("send-message", "{}", "INPUT_DESERIALIZATION_ERROR"),
        (
            "send-message",
            r#"{"channel":"C-fixture","text":"fixture"}"#,
            "SLACK_MISSING_CONNECTION",
        ),
    ] {
        let error = invoke_named_agent(
            "slack",
            CallContext::placeholder_for_metadata(),
            capability,
            input.as_bytes().to_vec(),
        )
        .await?
        .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.category, "permanent");
        assert!(!error.retryable);
    }
    Ok(())
}

#[tokio::test]
async fn slack_async_dispatch_preserves_retry_and_slack_error_contracts() -> anyhow::Result<()> {
    for (status, body, code, retryable, retry_after) in [
        (
            429,
            json!({"ok":false,"error":"ratelimited"}),
            "SLACK_RATE_LIMITED",
            true,
            Some(2000),
        ),
        (
            200,
            json!({"ok":false,"error":"already_reacted"}),
            "SLACK_ALREADY_REACTED",
            false,
            None,
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = format!("http://{}/proxy", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let request = read_proxy(&mut socket).await?;
            assert_eq!(request["url"], "https://slack.com/api/reactions.add");
            assert_eq!(request["connection_id"], "fixture-connection");
            respond(
                &mut socket,
                json!({"status":status,"headers":{"retry-after":"2"},"body":body}),
            )
            .await
        });
        let input = serde_json::to_vec(
            &json!({"channel":"C-fixture","timestamp":"42","name":"eyes","_connection":connection()}),
        )?;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            invoke_named_agent(
                "slack",
                CallContext::for_test("fixture-tenant", proxy, "", "", ""),
                "add-reaction",
                input,
            ),
        )
        .await;
        if !matches!(result, Ok(Ok(Err(_)))) {
            server.abort();
            let _ = server.await;
            anyhow::bail!("expected Slack error: {result:?}");
        }
        let error = result??.unwrap_err();
        server.await??;
        assert_eq!(error.code, code);
        assert_eq!(error.retryable, retryable);
        assert_eq!(error.retry_after_ms, retry_after);
    }
    Ok(())
}
