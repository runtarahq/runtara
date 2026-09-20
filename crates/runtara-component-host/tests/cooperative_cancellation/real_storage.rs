//! Storage components use the production linker against local HTTP stubs.
//! These tests do not contact object storage or a real signer.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_outbound, respond, run_cancellation_fixture,
};
use super::*;
use crate::outbound_fixture::FixtureContext;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

fn connection(agent: &str) -> Value {
    json!({"connection_id":"fixture-connection","integration_id":match agent {"s3-storage"=>"s3_compatible","azure-blob-storage"=>"azure_blob_storage",_=>unreachable!()},"parameters":{}})
}
fn input(agent: &str) -> Value {
    json!({"_connection":connection(agent),"bucket":"bucket","key":"dir/file name.txt","content":"aGVsbG8=","is_base64":"true","source_bucket":"source","source_key":"source.txt","destination_bucket":"bucket","destination_key":"dir/file name.txt","operation":"download","expires_in_seconds":"123","content_type":"text/plain"})
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
) -> anyhow::Result<()> {
    let bytes = compose_agent(
        agent,
        capability,
        &serde_json::to_vec(&input(agent))?,
        "storage-list-buckets",
        &serde_json::to_vec(&json!({"_connection":connection(agent)}))?,
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream = format!("http://{}/upstream", listener.local_addr()?);
    let started = Arc::new(Notify::new());
    let cleaned = Arc::new(Notify::new());
    let server = tokio::spawn({
        let started = started.clone();
        let cleaned = cleaned.clone();
        async move {
            for stage in 1..=blocked {
                let (mut socket, _) = listener.accept().await?;
                let body = read_outbound(&mut socket).await?;
                assert_eq!(body["connection_id"], "fixture-connection");
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
            let body = read_outbound(&mut socket).await?;
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
        FixtureContext::with_upstream("fixture-tenant", upstream, ""),
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
                cloud_cancel(agent, capability, 1, partial).await?;
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
                cloud_cancel(agent, "storage-download-file", stage, partial).await?;
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
                let upstream = format!("http://{}/upstream", listener.local_addr()?);
                let server = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await?;
                    let body = read_outbound(&mut socket).await?;
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
                        let body = read_outbound(&mut socket).await?;
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
                        FixtureContext::with_upstream("fixture-tenant", upstream, ""),
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
