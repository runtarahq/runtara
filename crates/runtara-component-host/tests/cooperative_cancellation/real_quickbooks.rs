//! Built QuickBooks component against local proxy fixtures; no Intuit account
//! or real accounting data is used.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_proxy, respond, run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection() -> Value {
    json!({"connection_id":"fixture-connection","integration_id":"quickbooks_online","parameters":{}})
}

struct Case {
    capability: &'static str,
    input: Value,
    method: &'static str,
    path: &'static str,
    body: Value,
}

fn cases() -> Vec<Case> {
    [
        ("query", json!({"query":"SELECT * FROM Customer","minor_version":"70"}), "GET", "/query?query=SELECT%20%2A%20FROM%20Customer&minorversion=70", Value::Null),
        ("read", json!({"entity":"Customer","id":"42 / fixture"}), "GET", "/customer/42%20%2F%20fixture?minorversion=75", Value::Null),
        ("create", json!({"entity":"Customer","body":{"DisplayName":"fixture"}}), "POST", "/customer?minorversion=75", json!({"DisplayName":"fixture"})),
        ("update", json!({"entity":"Customer","id":"42","sync_token":"3","body":{"Id":"wrong","SyncToken":"wrong","DisplayName":"fixture"}}), "POST", "/customer?minorversion=75", json!({"Id":"42","SyncToken":"3","DisplayName":"fixture","sparse":true})),
        ("update", json!({"entity":"Customer","id":"42","sync_token":"3","sparse":false,"body":{"DisplayName":"fixture"}}), "POST", "/customer?minorversion=75", json!({"Id":"42","SyncToken":"3","DisplayName":"fixture"})),
        ("delete", json!({"entity":"Invoice","id":"42","sync_token":"3"}), "POST", "/invoice?operation=delete&minorversion=75", json!({"Id":"42","SyncToken":"3"})),
        ("report", json!({"report_name":"ProfitAndLoss","params":{"start_date":"2026-01-01","end_date":"2026-02-01","summarize_column_by":"Month"}}), "GET", "/reports/ProfitAndLoss?minorversion=75&end_date=2026-02-01&start_date=2026-01-01&summarize_column_by=Month", Value::Null),
        ("cdc", json!({"entities":["Customer","Invoice"],"changed_since":"2026-01-01T00:00:00+00:00"}), "GET", "/cdc?entities=Customer%2CInvoice&changedSince=2026-01-01T00%3A00%3A00%2B00%3A00&minorversion=75", Value::Null),
    ].into_iter().map(|(capability,mut input,method,path,body)| {
        input["_connection"] = connection();
        Case { capability, input, method, path, body }
    }).collect()
}

async fn request(socket: &mut tokio::net::TcpStream, case: &Case) -> anyhow::Result<()> {
    let envelope = read_proxy(socket).await?;
    assert_eq!(envelope["method"], case.method);
    assert_eq!(envelope["url"], case.path);
    assert_eq!(envelope["connection_id"], "fixture-connection");
    assert_eq!(envelope["timeout_ms"], 30_000);
    assert_eq!(envelope["headers"]["Accept"], "application/json");
    assert!(
        envelope["headers"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.eq_ignore_ascii_case("authorization"))
    );
    if case.method == "POST" {
        assert_eq!(envelope["headers"]["Content-Type"], "application/json");
        let body = BASE64.decode(envelope["body_raw"].as_str().unwrap())?;
        assert_eq!(serde_json::from_slice::<Value>(&body)?, case.body);
    } else {
        assert!(envelope["body_raw"].is_null());
    }
    Ok(())
}

fn fresh_case() -> Case {
    cases()
        .into_iter()
        .find(|case| case.capability == "read")
        .unwrap()
}

async fn cancellation(partial: bool) -> anyhow::Result<()> {
    for case in cases() {
        let fresh = fresh_case();
        let bytes = compose_agent(
            "quickbooks",
            case.capability,
            &serde_json::to_vec(&case.input)?,
            fresh.capability,
            &serde_json::to_vec(&fresh.input)?,
        )?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = format!("http://{}/proxy", listener.local_addr()?);
        let started = Arc::new(Notify::new());
        let cleaned = Arc::new(Notify::new());
        let server = tokio::spawn({
            let started = started.clone();
            let cleaned = cleaned.clone();
            async move {
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, &case).await?;
                if partial {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 400\r\nConnection: close\r\n\r\n{",
                        )
                        .await?;
                }
                started.notify_one();
                match socket.read(&mut [0]).await {
                    Ok(0) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => anyhow::bail!("QuickBooks wait was not closed: {other:?}"),
                }
                cleaned.notify_one();
                // Cancellation must not retry a write, refresh its SyncToken,
                // delete an entity or synthesize a compensating request.
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, &fresh).await?;
                respond(&mut socket, json!({"status":200,"headers":{},"body":{"Customer":{"Id":"42","SyncToken":"4","DisplayName":"fresh"}}})).await
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
        assert_eq!(
            output,
            json!({"entity":"Customer","id":"42","sync_token":"4","object":{"Id":"42","SyncToken":"4","DisplayName":"fresh"}})
        );
    }
    Ok(())
}

#[tokio::test]
async fn quickbooks_cancel_closes_pending_headers_for_every_capability() -> anyhow::Result<()> {
    cancellation(false).await
}

#[tokio::test]
async fn quickbooks_cancel_closes_partial_body_for_every_capability() -> anyhow::Result<()> {
    cancellation(true).await
}

async fn invoke_response(
    case: Case,
    envelope: Value,
) -> anyhow::Result<Result<Vec<u8>, runtara_component_host::ErrorInfo>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}/proxy", listener.local_addr()?);
    let capability = case.capability;
    let input = serde_json::to_vec(&case.input)?;
    let mut server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        request(&mut socket, &case).await?;
        respond(&mut socket, envelope).await
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        invoke_named_agent(
            "quickbooks",
            CallContext::for_test("fixture-tenant", proxy, "", "", ""),
            capability,
            input,
        ),
    )
    .await;
    let result = match result {
        Ok(Ok(result)) => result,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("QuickBooks invocation failed: {other:?}");
        }
    };
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(joined) => joined??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("QuickBooks proxy fixture did not finish: {error}; result={result:?}");
        }
    }
    Ok(result)
}

#[tokio::test]
async fn quickbooks_success_preserves_tokens_query_report_and_cdc_outputs() -> anyhow::Result<()> {
    for case in cases() {
        let object = json!({"Id":"42","SyncToken":"4","DisplayName":"fixture"});
        let (body, expected) = match case.capability {
            "query" => {
                let qr = json!({"Customer":[object],"startPosition":1,"maxResults":1});
                (
                    json!({"QueryResponse":qr}),
                    json!({"items":[object],"count":1,"query_response":qr}),
                )
            }
            "delete" => {
                let object = json!({"Id":"42","status":"Deleted"});
                (
                    json!({"Invoice":object}),
                    json!({"id":"42","status":"Deleted","object":object}),
                )
            }
            "report" => {
                let report =
                    json!({"Header":{"ReportName":"ProfitAndLoss"},"Columns":{},"Rows":{}});
                (report.clone(), json!({"report":report}))
            }
            "cdc" => {
                let raw = json!({"CDCResponse":[{"QueryResponse":[{"Customer":[object,{"Id":"43","status":"Deleted"}],"maxResults":2}]}]});
                (
                    raw.clone(),
                    json!({"changes":{"Customer":{"changed":[object],"deleted":["43"]}},"raw":raw}),
                )
            }
            _ => (
                json!({"Customer":object}),
                json!({"entity":"Customer","id":"42","sync_token":"4","object":object}),
            ),
        };
        let bytes = invoke_response(case, json!({"status":200,"headers":{},"body":body}))
            .await?
            .expect("successful response");
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
    }
    let bytes = invoke_response(
        fresh_case(),
        json!({"status":204,"headers":{},"body_raw":""}),
    )
    .await?
    .expect("existing empty-body behavior");
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"entity":"Customer","id":"","sync_token":"","object":null})
    );
    Ok(())
}

#[tokio::test]
async fn quickbooks_provider_errors_preserve_retry_contracts_for_reads_and_writes()
-> anyhow::Result<()> {
    for method in ["GET", "POST"] {
        for (status, code, retryable) in [
            (400, "QUICKBOOKS_REQUEST_FAILED", false),
            (401, "QUICKBOOKS_UNAUTHORIZED", false),
            (403, "QUICKBOOKS_UNAUTHORIZED", false),
            (429, "QUICKBOOKS_UPSTREAM_ERROR", true),
            (503, "QUICKBOOKS_UPSTREAM_ERROR", true),
        ] {
            let case = cases()
                .into_iter()
                .find(|case| case.method == method)
                .unwrap();
            let path = case.path;
            let error = invoke_response(case, json!({"status":status,"headers":{},"body":{"Fault":{"Error":[{"Message":"fixture rejection"}]}}})).await?.expect_err("provider error");
            assert_eq!(error.code, code);
            assert_eq!(error.retryable, retryable);
            assert_eq!(
                error.category,
                if retryable { "transient" } else { "permanent" }
            );
            let attributes: Value = serde_json::from_str(&error.attributes.unwrap())?;
            assert_eq!(attributes["path"], path);
            assert_eq!(attributes["status_code"], status.to_string());
        }
    }
    Ok(())
}

#[tokio::test]
async fn quickbooks_transport_parse_and_validation_failures_stay_distinct() -> anyhow::Result<()> {
    for (body_raw, code, retryable) in [
        ("!invalid-base64!", "QUICKBOOKS_NETWORK_ERROR", true),
        ("eA==", "QUICKBOOKS_RESPONSE_PARSE_ERROR", false),
    ] {
        let error = invoke_response(
            fresh_case(),
            json!({"status":200,"headers":{},"body_raw":body_raw}),
        )
        .await?
        .expect_err("invalid response");
        assert_eq!(error.code, code);
        assert_eq!(error.retryable, retryable);
    }
    for (capability, input, code) in [
        (
            "read",
            json!({"entity":"Customer","id":"42"}),
            "QUICKBOOKS_MISSING_CONNECTION",
        ),
        (
            "cdc",
            json!({"_connection":connection(),"entities":[],"changed_since":"2026-01-01"}),
            "QUICKBOOKS_CDC_NO_ENTITIES",
        ),
    ] {
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            invoke_named_agent(
                "quickbooks",
                CallContext::for_test("fixture-tenant", "http://127.0.0.1:1/unused", "", "", ""),
                capability,
                serde_json::to_vec(&input)?,
            ),
        )
        .await??
        .expect_err("validation before I/O");
        assert_eq!(error.code, code);
        assert!(!error.retryable);
    }
    Ok(())
}
