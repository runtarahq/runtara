//! Actual messaging components; every request goes to a local proxy fixture.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_proxy, respond, run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn input(agent: &str, after: bool) -> Value {
    if agent == "mailgun" {
        json!({"to":"fixture@example.invalid","subject":if after {"after"} else {"before"},"text":"fixture","tags":"first,second","_connection":{"connection_id":"fixture-connection","integration_id":"mailgun","parameters":{"domain":"example.invalid"}}})
    } else {
        json!({"target":"fixture-ref","conversation_id":"19:fixture@thread","text":if after {"after".into()} else {"x".repeat(4001)},"card":{"type":"AdaptiveCard","body":[]},"timeout_ms":"30000","_connection":{"connection_id":"fixture-connection","integration_id":"teams_bot","parameters":{}}})
    }
}

async fn cancellation(agent: &'static str, blocked: usize, partial: bool) -> anyhow::Result<()> {
    let capability = if agent == "mailgun" {
        "send-email"
    } else {
        "send-message"
    };
    let bytes = compose_agent(
        agent,
        capability,
        &serde_json::to_vec(&input(agent, false))?,
        capability,
        &serde_json::to_vec(&input(agent, true))?,
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            for stage in 1..=blocked {
                let (mut socket, _) = listener.accept().await?;
                let request = read_proxy(&mut socket).await?;
                assert_eq!(request["connection_id"], "fixture-connection");
                let body = BASE64.decode(request["body_raw"].as_str().unwrap())?;
                if agent == "mailgun" {
                    assert_eq!(request["url"], "/v3/example.invalid/messages");
                    let body = String::from_utf8(body)?;
                    assert!(body.contains("subject=before"));
                    assert!(body.contains("from=noreply%40example.invalid"));
                    assert!(body.contains("o%3Atag=first&o%3Atag=second"));
                } else {
                    assert_eq!(
                        request["url"],
                        "/v3/conversations/19%3Afixture%40thread/activities"
                    );
                    assert_eq!(request["endpoint_ref"], "fixture-ref");
                    assert_eq!(request["timeout_ms"], 30000);
                    let body: Value = serde_json::from_slice(&body)?;
                    assert_eq!(
                        body["text"].as_str().unwrap().len(),
                        if stage == 1 { 4000 } else { 1 }
                    );
                    assert_eq!(body["inputHint"], "acceptingInput");
                    assert_eq!(body.get("attachments").is_some(), stage == 1);
                }
                if stage < blocked {
                    respond(
                        &mut socket,
                        json!({"status":200,"headers":{},"body":{"id":"first"}}),
                    )
                    .await?;
                } else {
                    if partial {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{").await?;
                    }
                    started.notify_one();
                    match socket.read(&mut [0]).await {
                        Ok(0) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                        other => anyhow::bail!("pending send not closed: {other:?}"),
                    }
                    cleaned.notify_one();
                }
            }
            let (mut socket, _) = listener.accept().await?;
            let request = read_proxy(&mut socket).await?;
            let body = BASE64.decode(request["body_raw"].as_str().unwrap())?;
            if agent == "mailgun" {
                assert!(String::from_utf8(body)?.contains("subject=after"));
            } else {
                assert_eq!(serde_json::from_slice::<Value>(&body)?["text"], "after");
            }
            respond(
                &mut socket,
                json!({"status":200,"headers":{},"body":{"id":"after","message":"queued"}}),
            )
            .await
        }
    });
    let output = run_cancellation_fixture(
        bytes,
        CallContext::for_test("fixture-tenant", proxy, "", "", ""),
        started,
        cleaned,
        server,
    )
    .await?;
    if agent == "mailgun" {
        assert_eq!(output, json!({"id":"after","message":"queued"}));
    } else {
        assert_eq!(
            output,
            json!({"ok":true,"conversation_id":"19:fixture@thread","activity_id":"after","activity_ids":["after"]})
        );
    }
    Ok(())
}

#[tokio::test]
async fn mailgun_cancel_closes_headers_and_body_and_reuses_agent() -> anyhow::Result<()> {
    for partial in [false, true] {
        cancellation("mailgun", 1, partial).await?;
    }
    Ok(())
}
#[tokio::test]
async fn teams_cancel_stops_chunking_and_reuses_agent() -> anyhow::Result<()> {
    for stage in 1..=2 {
        for partial in [false, true] {
            cancellation("teams", stage, partial).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn messaging_async_exports_preserve_error_classification() -> anyhow::Result<()> {
    for (agent, capability) in [("mailgun", "send-email"), ("teams", "send-message")] {
        for status in [429, 403, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let proxy = format!("http://{}/proxy", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                read_proxy(&mut socket).await?;
                respond(&mut socket, json!({"status":status,"headers":{"retry-after":"2"},"body":{"error":{"code":"fixture","message":"fixture"}}})).await
            });
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                invoke_named_agent(
                    agent,
                    CallContext::for_test("fixture-tenant", proxy, "", "", ""),
                    capability,
                    serde_json::to_vec(&input(agent, true))?,
                ),
            )
            .await;
            let error = match result {
                Ok(Ok(Err(error))) => error,
                other => {
                    server.abort();
                    let _ = server.await;
                    anyhow::bail!("expected send error: {other:?}");
                }
            };
            server.await??;
            let code = match (agent, status) {
                ("mailgun", 403) => "MAILGUN_UNAUTHORIZED",
                ("mailgun", _) => "MAILGUN_UPSTREAM_ERROR",
                (_, 429) => "TEAMS_RATE_LIMITED",
                (_, 403) => "TEAMS_PERMISSION_ERROR",
                _ => "TEAMS_SERVER_ERROR",
            };
            assert_eq!(error.code, code);
            assert_eq!(error.retryable, status != 403);
            assert_eq!(
                error.retry_after_ms,
                if status == 429 || agent == "teams" {
                    Some(2000)
                } else {
                    None
                }
            );
        }
    }
    Ok(())
}
