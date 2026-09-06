//! The internal object-model transport remains direct HTTP, using connection
//! and tenant context. These fixtures never connect to a database.
use super::real_agent::{cancel_and_reuse, compose_agent, invoke_named_agent, request_headers};
use super::*;
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection() -> Value {
    json!({"connection_id":"fixture-connection","integration_id":"object_model","parameters":{}})
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Query,
    Execute,
    Load,
    LoadMissingSchema,
    SaveNew,
    SaveExisting,
}

impl Operation {
    fn capability(self) -> &'static str {
        match self {
            Self::Query => "query-sql",
            Self::Execute => "execute-sql",
            Self::Load | Self::LoadMissingSchema => "load-memory",
            Self::SaveNew | Self::SaveExisting => "save-memory",
        }
    }
    fn stages(self) -> usize {
        match self {
            Self::Query | Self::Execute => 1,
            Self::Load => 2,
            _ => 3,
        }
    }
    fn input(self) -> Value {
        json!({"_connection":connection(),"sql":if matches!(self, Self::Execute) {"UPDATE fixture SET value = 1"} else {"SELECT 1"},"params":[],"conversation_id":"conversation","messages":[{"role":"user","content":"before"}]})
    }
    fn stage(self, index: usize) -> (&'static str, &'static str, Value) {
        match (self, index) {
            (Self::Query, _) => ("POST", "/sql/query", json!({})),
            (Self::Execute, _) => ("POST", "/sql/execute", json!({})),
            (Self::LoadMissingSchema, 1) => (
                "GET",
                "/schemas/ai_conversation_memory",
                json!({"success":false}),
            ),
            (_, 1) => (
                "GET",
                "/schemas/ai_conversation_memory",
                json!({"success":true,"schema":{}}),
            ),
            (Self::LoadMissingSchema, 2) => ("POST", "/schemas", json!({"success":true})),
            (Self::LoadMissingSchema, 3) => ("POST", "/instances/query", json!({})),
            (Self::SaveExisting, 2) => (
                "POST",
                "/instances/query",
                json!({"success":true,"instances":[{"id":"existing"}]}),
            ),
            (_, 2) => (
                "POST",
                "/instances/query",
                json!({"success":true,"instances":[]}),
            ),
            (Self::SaveExisting, 3) => (
                "PUT",
                "/instances/ai_conversation_memory/existing",
                json!({}),
            ),
            (Self::SaveNew, 3) => ("POST", "/instances", json!({})),
            _ => unreachable!(),
        }
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> anyhow::Result<(String, Value)> {
    let headers = String::from_utf8(request_headers(socket).await?)?;
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("x-org-id: fixture-tenant\r\n")
    );
    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
    let line = headers.lines().next().unwrap().to_owned();
    let length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, length)| length.trim().parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    anyhow::ensure!(length < 16_384, "unexpected fixture request size");
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).await?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    if !body.is_null() {
        assert_eq!(body["connectionId"], "fixture-connection");
    }
    Ok((line, body))
}

async fn respond(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    body: &[u8],
) -> anyhow::Result<()> {
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    socket.write_all(body).await?;
    Ok(())
}

async fn cancellation(operation: Operation, blocked: usize, partial: bool) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let next = json!({"_connection":connection(),"sql":"SELECT 42","params":[]});
    let bytes = compose_agent(
        "object-model",
        operation.capability(),
        &serde_json::to_vec(&operation.input())?,
        "query-sql",
        &serde_json::to_vec(&next)?,
    )?;
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            for stage in 1..=blocked {
                let (mut socket, _) = listener.accept().await?;
                let (line, body) = read_request(&mut socket).await?;
                let (method, path, reply) = operation.stage(stage);
                assert_eq!(
                    line,
                    format!("{method} {path}?connectionId=fixture-connection HTTP/1.1"),
                    "{operation:?} stage {stage}"
                );
                if matches!(operation, Operation::Query | Operation::Execute) {
                    assert_eq!(body["sql"], operation.input()["sql"]);
                }
                if stage < blocked {
                    respond(&mut socket, 200, &serde_json::to_vec(&reply)?).await?;
                } else {
                    if partial {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{").await?;
                    }
                    started.notify_one();
                    match socket.read(&mut [0]).await {
                        Ok(0) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                        other => anyhow::bail!("request not closed: {other:?}"),
                    }
                    cleaned.notify_one();
                }
            }
            // Any remaining schema/query/write stage would arrive here and fail.
            let (mut socket, _) = listener.accept().await?;
            let (line, body) = read_request(&mut socket).await?;
            assert_eq!(
                line,
                "POST /sql/query?connectionId=fixture-connection HTTP/1.1"
            );
            assert_eq!(body["sql"], "SELECT 42");
            respond(
                &mut socket,
                200,
                br#"{"success":true,"rows":[{"value":42}],"rowCount":1}"#,
            )
            .await
        }
    });
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cancel_and_reuse(
            bytes,
            CallContext::for_test(
                "fixture-tenant",
                "http://127.0.0.1:1/proxy-must-not-be-used",
                "",
                base,
                "",
            ),
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
            anyhow::bail!("{operation:?}/{blocked}/{partial}: {other:?}");
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
    assert_eq!(
        output,
        json!({"success":true,"rows":[{"value":42}],"row_count":1,"error":null})
    );
    Ok(())
}

#[tokio::test]
async fn sql_query_and_execute_cancel_headers_and_partial_body() -> anyhow::Result<()> {
    for operation in [Operation::Query, Operation::Execute] {
        for partial in [false, true] {
            cancellation(operation, 1, partial).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn memory_load_cancels_schema_lookup_creation_and_query() -> anyhow::Result<()> {
    for operation in [Operation::Load, Operation::LoadMissingSchema] {
        for stage in 1..=operation.stages() {
            for partial in [false, true] {
                cancellation(operation, stage, partial).await?;
            }
        }
    }
    Ok(())
}
#[tokio::test]
async fn memory_save_cancels_before_or_during_create_and_update() -> anyhow::Result<()> {
    for operation in [Operation::SaveNew, Operation::SaveExisting] {
        for stage in 1..=operation.stages() {
            for partial in [false, true] {
                cancellation(operation, stage, partial).await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn sql_read_and_write_keep_distinct_retry_contracts() -> anyhow::Result<()> {
    for capability in ["query-sql", "execute-sql"] {
        for status in [429, 503, 413] {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("http://{}", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                read_request(&mut socket).await?;
                respond(&mut socket, status, b"fixture failure").await
            });
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                invoke_named_agent(
                    "object-model",
                    CallContext::for_test(
                        "fixture-tenant",
                        "http://127.0.0.1:1/proxy-must-not-be-used",
                        "",
                        base,
                        "",
                    ),
                    capability,
                    serde_json::to_vec(&Operation::Query.input())?,
                ),
            )
            .await;
            let error = match result {
                Ok(Ok(Err(error))) => error,
                other => {
                    server.abort();
                    let _ = server.await;
                    anyhow::bail!("expected SQL error: {other:?}");
                }
            };
            server.await??;
            assert_eq!(
                error.code,
                if status == 413 {
                    "OBJECT_MODEL_PAYLOAD_TOO_LARGE"
                } else {
                    "OBJECT_MODEL_UPSTREAM_ERROR"
                }
            );
            assert_eq!(
                error.retryable,
                status == 429 || (status == 503 && capability == "query-sql")
            );
        }
    }
    Ok(())
}
