//! Storage components use the production linker against local HTTP stubs.
//! These tests do not contact object storage, an SSH server or a real signer.
use super::real_agent::{
    compose_agent, invoke_named_agent, request_headers, respond, run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection(agent: &str) -> Value {
    json!({"connection_id":"fixture-connection","integration_id":match agent {"s3-storage"=>"s3_compatible","azure-blob-storage"=>"azure_blob_storage",_=>"sftp"},"parameters":{}})
}
fn input(agent: &str) -> Value {
    json!({"_connection":connection(agent),"bucket":"bucket","key":"dir/file name.txt","content":"aGVsbG8=","is_base64":"true","source_bucket":"source","source_key":"source.txt","destination_bucket":"bucket","destination_key":"dir/file name.txt","operation":"download","expires_in_seconds":"123","content_type":"text/plain","path":"/before.txt"})
}

async fn request(socket: &mut tokio::net::TcpStream) -> anyhow::Result<(String, Value)> {
    let headers = String::from_utf8(request_headers(socket).await?)?;
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("x-org-id: fixture-tenant\r\n")
    );
    let length: usize = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .trim()
        .parse()?;
    anyhow::ensure!(length < 16_384, "unexpected fixture request size");
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await?;
    Ok((
        headers.lines().next().unwrap().to_owned(),
        serde_json::from_slice(&body)?,
    ))
}

async fn raw_response(
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

async fn wait_closed(
    socket: &mut tokio::net::TcpStream,
    started: &Notify,
    cleaned: &Notify,
    partial: bool,
) -> anyhow::Result<()> {
    if partial {
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 400\r\nConnection: close\r\n\r\n{")
            .await?;
    }
    started.notify_one();
    match socket.read(&mut [0]).await {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => anyhow::bail!("storage request not closed: {other:?}"),
    }
    cleaned.notify_one();
    Ok(())
}

async fn cloud_cancel(
    agent: &'static str,
    capability: &'static str,
    blocked: usize,
    partial: bool,
    canonical_proxy: bool,
) -> anyhow::Result<()> {
    let bytes = compose_agent(
        agent,
        capability,
        &serde_json::to_vec(&input(agent))?,
        "storage-list-buckets",
        &serde_json::to_vec(&json!({"_connection":connection(agent)}))?,
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let root = if canonical_proxy {
        "/api/internal/proxy"
    } else {
        "/proxy"
    };
    let proxy = format!("http://{}{root}", listener.local_addr()?);
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            for stage in 1..=blocked {
                let (mut socket, _) = listener.accept().await?;
                let (line, body) = request(&mut socket).await?;
                assert_eq!(body["connection_id"], "fixture-connection");
                if capability == "storage-generate-presigned-url" {
                    assert_eq!(
                        line,
                        if canonical_proxy {
                            "POST /api/internal/presign HTTP/1.1"
                        } else {
                            "POST /proxy/presign HTTP/1.1"
                        }
                    );
                    assert_eq!(body["method"], "GET");
                    assert_eq!(body["path"], "/bucket/dir/file name.txt");
                    assert_eq!(body["expires_in_seconds"], 123);
                    assert_eq!(body["content_type"], "text/plain");
                } else {
                    assert_eq!(line, format!("POST {root} HTTP/1.1"));
                    assert_eq!(body["url"], "/bucket/dir/file%20name.txt");
                    let method = match capability {
                        "storage-download-file" if stage == 1 => "HEAD",
                        "storage-download-file" => "GET",
                        "storage-delete-file" => "DELETE",
                        _ => "PUT",
                    };
                    assert_eq!(body["method"], method);
                    if capability == "storage-upload-file" {
                        assert_eq!(body["body_raw"], "aGVsbG8=");
                        if agent == "azure-blob-storage" {
                            assert_eq!(body["headers"]["x-ms-blob-type"], "BlockBlob");
                        }
                    }
                    if capability == "storage-copy-file" {
                        assert_eq!(
                            body["headers"][if agent == "s3-storage" {
                                "x-amz-copy-source"
                            } else {
                                "x-ms-copy-source"
                            }],
                            "/source/source.txt"
                        );
                    }
                }
                if stage < blocked {
                    respond(
                        &mut socket,
                        json!({"status":200,"headers":{"content-type":"text/plain"},"body_raw":""}),
                    )
                    .await?;
                } else {
                    wait_closed(&mut socket, &started, &cleaned, partial).await?;
                }
            }
            // A cancelled download HEAD must not fall through its ordinary-error
            // fallback and issue GET. Only the fresh list call may arrive next.
            let (mut socket, _) = listener.accept().await?;
            let (line, body) = request(&mut socket).await?;
            assert_eq!(line, format!("POST {root} HTTP/1.1"));
            assert_eq!(body["method"], "GET");
            assert_eq!(
                body["url"],
                if agent == "s3-storage" {
                    "/"
                } else {
                    "/?comp=list"
                }
            );
            let xml = if agent == "s3-storage" {
                "<ListAllMyBucketsResult><Buckets><Bucket><Name>after</Name></Bucket></Buckets></ListAllMyBucketsResult>"
            } else {
                "<EnumerationResults><Containers><Container><Name>after</Name></Container></Containers></EnumerationResults>"
            };
            respond(
                &mut socket,
                json!({"status":200,"headers":{},"body_raw":BASE64.encode(xml)}),
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
    assert_eq!(output["success"], true);
    assert_eq!(output["buckets"][0]["name"], "after");
    Ok(())
}

#[tokio::test]
async fn object_storage_cancel_stops_upload_copy_and_delete() -> anyhow::Result<()> {
    for agent in ["s3-storage", "azure-blob-storage"] {
        for capability in [
            "storage-upload-file",
            "storage-copy-file",
            "storage-delete-file",
        ] {
            for partial in [false, true] {
                cloud_cancel(agent, capability, 1, partial, false).await?;
            }
        }
    }
    Ok(())
}
#[tokio::test]
async fn object_storage_cancel_stops_both_download_requests_without_fallback() -> anyhow::Result<()>
{
    for agent in ["s3-storage", "azure-blob-storage"] {
        for stage in 1..=2 {
            for partial in [false, true] {
                cloud_cancel(agent, "storage-download-file", stage, partial, false).await?;
            }
        }
    }
    Ok(())
}
#[tokio::test]
async fn presign_cancel_stops_headers_and_body_for_both_proxy_url_shapes() -> anyhow::Result<()> {
    for agent in ["s3-storage", "azure-blob-storage"] {
        for canonical in [false, true] {
            for partial in [false, true] {
                cloud_cancel(
                    agent,
                    "storage-generate-presigned-url",
                    1,
                    partial,
                    canonical,
                )
                .await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn sftp_wrapper_cancel_stops_every_capability_and_reuses_instance() -> anyhow::Result<()> {
    for capability in [
        "sftp-list-files",
        "sftp-download-file",
        "sftp-upload-file",
        "sftp-delete-file",
    ] {
        for partial in [false, true] {
            let bytes = compose_agent(
                "sftp",
                capability,
                &serde_json::to_vec(&input("sftp"))?,
                "sftp-download-file",
                &serde_json::to_vec(
                    &json!({"_connection":connection("sftp"),"path":"/after.txt"}),
                )?,
            )?;
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("http://{}/agent", listener.local_addr()?);
            let started = Arc::new(Notify::new());
            let cleaned = Arc::new(Notify::new());
            let server = tokio::spawn({
                let started = started.clone();
                let cleaned = cleaned.clone();
                async move {
                    let (mut socket, _) = listener.accept().await?;
                    let (line, body) = request(&mut socket).await?;
                    assert_eq!(line, format!("POST /agent/sftp/{capability} HTTP/1.1"));
                    assert_eq!(body["_connection"], connection("sftp"));
                    assert_eq!(body["path"], "/before.txt");
                    wait_closed(&mut socket, &started, &cleaned, partial).await?;
                    let (mut socket, _) = listener.accept().await?;
                    let (line, body) = request(&mut socket).await?;
                    assert_eq!(line, "POST /agent/sftp/sftp-download-file HTTP/1.1");
                    assert_eq!(body["path"], "/after.txt");
                    assert_eq!(body["response_format"], "text");
                    raw_response(&mut socket, 200, br#"{"success":true,"output":"after"}"#).await
                }
            });
            let output = run_cancellation_fixture(
                bytes,
                CallContext::for_test("fixture-tenant", "http://127.0.0.1:1/unused", base, "", ""),
                started,
                cleaned,
                server,
            )
            .await?;
            assert_eq!(output, json!("after"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn presign_preserves_success_and_soft_failure_shapes() -> anyhow::Result<()> {
    for agent in ["s3-storage", "azure-blob-storage"] {
        for (status, response) in [
            (
                200,
                json!({"url":"https://storage.invalid/fixture","expires_in_seconds":123}),
            ),
            (403, json!({"error":"fixture"})),
            (200, json!({"expires_in_seconds":123})),
        ] {
            let expected_success = status == 200 && response.get("url").is_some();
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let proxy = format!("http://{}/api/internal/proxy", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                let (line, body) = request(&mut socket).await?;
                assert_eq!(line, "POST /api/internal/presign HTTP/1.1");
                assert_eq!(body["connection_id"], "fixture-connection");
                raw_response(&mut socket, status, &serde_json::to_vec(&response)?).await
            });
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                invoke_named_agent(
                    agent,
                    CallContext::for_test("fixture-tenant", proxy, "", "", ""),
                    "storage-generate-presigned-url",
                    serde_json::to_vec(&input(agent))?,
                ),
            )
            .await;
            let output = match result {
                Ok(Ok(Ok(output))) => serde_json::from_slice::<Value>(&output)?,
                other => {
                    server.abort();
                    let _ = server.await;
                    anyhow::bail!("expected presign output: {other:?}");
                }
            };
            server.await??;
            assert_eq!(output["success"], expected_success);
            if expected_success {
                assert_eq!(output["url"], "https://storage.invalid/fixture");
                assert_eq!(output["expires_in_seconds"], 123);
            } else {
                assert!(output["error"].as_str().is_some());
                assert!(output["url"].is_null());
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn object_storage_preserves_delete_statuses_and_download_head_fallback() -> anyhow::Result<()>
{
    for agent in ["s3-storage", "azure-blob-storage"] {
        for capability in ["storage-delete-file", "storage-download-file"] {
            for status in [200, 202, 204, 403, 404, 503] {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let proxy = format!("http://{}/proxy", listener.local_addr()?);
                let server = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await?;
                    let (_, body) = request(&mut socket).await?;
                    assert_eq!(body["url"], "/bucket/dir/file%20name.txt");
                    if capability == "storage-download-file" {
                        assert_eq!(body["method"], "HEAD");
                        // Ordinary HEAD failure still permits GET. Component
                        // cancellation in the other test must never take this fallback.
                        respond(
                            &mut socket,
                            json!({"status":403,"headers":{},"body_raw":""}),
                        )
                        .await?;
                        (socket, _) = listener.accept().await?;
                        let (_, body) = request(&mut socket).await?;
                        assert_eq!(body["method"], "GET");
                    } else {
                        assert_eq!(body["method"], "DELETE");
                    }
                    let payload = if status == 200 {
                        "hello"
                    } else {
                        "<Error><Code>Fixture</Code><Message>fixture rejection</Message></Error>"
                    };
                    respond(
                        &mut socket,
                        json!({"status":status,"headers":{},"body_raw":BASE64.encode(payload)}),
                    )
                    .await
                });
                let result = tokio::time::timeout(
                    Duration::from_secs(10),
                    invoke_named_agent(
                        agent,
                        CallContext::for_test("fixture-tenant", proxy, "", "", ""),
                        capability,
                        serde_json::to_vec(&input(agent))?,
                    ),
                )
                .await;
                let output = match result {
                    Ok(Ok(Ok(output))) => serde_json::from_slice::<Value>(&output)?,
                    other => {
                        server.abort();
                        let _ = server.await;
                        anyhow::bail!("expected storage soft result: {other:?}");
                    }
                };
                server.await??;
                let success = if capability == "storage-download-file" {
                    status == 200
                } else if agent == "s3-storage" {
                    matches!(status, 200 | 204 | 404)
                } else {
                    matches!(status, 202 | 404)
                };
                assert_eq!(output["success"], success, "{agent}/{capability}/{status}");
                if !success {
                    assert!(output["error"].as_str().is_some());
                }
                if capability == "storage-download-file" && success {
                    assert_eq!(output["content"], BASE64.encode("hello"));
                    assert_eq!(output["size"], 5);
                    assert!(output["content_type"].is_null());
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn sftp_wrapper_preserves_http_envelope_and_output_errors() -> anyhow::Result<()> {
    for (status, body, code) in [
        (503, "unavailable", "SFTP_NATIVE_AGENT_HTTP_503"),
        (200, "not json", "SFTP_NATIVE_AGENT_PARSE_ERROR"),
        (
            200,
            r#"{"success":false,"error":"fixture rejection"}"#,
            "SFTP_NATIVE_AGENT_ERROR",
        ),
        (
            200,
            r#"{"success":true,"output":{}}"#,
            "SFTP_OUTPUT_DESERIALIZATION_ERROR",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}/agent", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let (line, _) = request(&mut socket).await?;
            assert_eq!(line, "POST /agent/sftp/sftp-download-file HTTP/1.1");
            raw_response(&mut socket, status, body.as_bytes()).await
        });
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            invoke_named_agent(
                "sftp",
                CallContext::for_test("fixture-tenant", "", base, "", ""),
                "sftp-download-file",
                serde_json::to_vec(&input("sftp"))?,
            ),
        )
        .await;
        let error = match result {
            Ok(Ok(Err(error))) => error,
            other => {
                server.abort();
                let _ = server.await;
                anyhow::bail!("expected native wrapper error: {other:?}");
            }
        };
        server.await??;
        assert_eq!(error.code, code);
        assert!(!error.retryable);
    }
    Ok(())
}
