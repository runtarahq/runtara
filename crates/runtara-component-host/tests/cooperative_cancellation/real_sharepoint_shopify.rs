//! Shared HTTP cancellation through macro-generated real Agent exports.
//! Local provider only: no provider accounts or credentials. Distinct tests cover
//! request families and multi-request control flow rather than duplicating every
//! Shopify capability that delegates to the same GraphQL helper.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_outbound_limited, respond, run_cancellation_fixture,
};
use super::*;
use crate::outbound_fixture::FixtureContext;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

#[derive(Clone)]
struct Request {
    method: &'static str,
    url: String,
    connection: bool,
    timeout: u64,
    body: Option<Vec<u8>>,
    query_contains: Option<&'static str>,
    range: Option<String>,
    response: Value,
}
impl Request {
    fn graph(method: &'static str, path: &str, body: Option<Value>, response: Value) -> Self {
        Self {
            method,
            url: format!("https://graph.microsoft.com/v1.0{path}"),
            connection: true,
            timeout: 30_000,
            body: body.map(|v| serde_json::to_vec(&v).unwrap()),
            query_contains: None,
            range: None,
            response: envelope(200, response),
        }
    }
    fn graphql(query: &'static str, variables: Value, response: Value) -> Self {
        Self {
            method: "POST",
            url: "/admin/api/2026-10/graphql.json".into(),
            connection: true,
            timeout: 60_000,
            body: Some(serde_json::to_vec(&variables).unwrap()),
            query_contains: Some(query),
            range: None,
            response: envelope(200, response),
        }
    }
    async fn check(&self, socket: &mut tokio::net::TcpStream) -> anyhow::Result<()> {
        let request = read_outbound_limited(socket, 8 * 1024 * 1024).await?;
        assert_eq!(request["method"], self.method);
        assert_eq!(request["url"], self.url);
        assert_eq!(request["timeout_ms"], self.timeout);
        assert_eq!(
            request["connection_id"],
            if self.connection {
                json!("fixture-connection")
            } else {
                Value::Null
            }
        );
        assert!(
            request["headers"]
                .as_object()
                .unwrap()
                .keys()
                .all(|k| !k.eq_ignore_ascii_case("authorization"))
        );
        if let Some(expected) = &self.body {
            let body = BASE64.decode(request["body_raw"].as_str().unwrap())?;
            if let Some(query) = self.query_contains {
                let body: Value = serde_json::from_slice(&body)?;
                assert!(body["query"].as_str().unwrap().contains(query));
                assert_eq!(
                    body["variables"],
                    serde_json::from_slice::<Value>(expected)?
                );
            } else if let Ok(expected) = serde_json::from_slice::<Value>(expected) {
                assert_eq!(serde_json::from_slice::<Value>(&body)?, expected);
            } else {
                assert_eq!(body, *expected);
            }
        } else {
            assert!(request["body_raw"].is_null());
        }
        if let Some(range) = &self.range {
            assert_eq!(request["headers"]["Content-Range"], *range);
        }
        Ok(())
    }
}
fn envelope(status: u16, body: Value) -> Value {
    json!({"status":status,"headers":{},"body":body})
}
fn item() -> Value {
    json!({"id":"42","name":"fixture.txt","file":{"mimeType":"text/plain"}})
}
struct Case {
    agent: &'static str,
    capability: &'static str,
    input: Value,
    requests: Vec<Request>,
    expected: Value,
}
fn case(
    agent: &'static str,
    capability: &'static str,
    mut input: Value,
    requests: Vec<Request>,
    expected: Value,
) -> Case {
    input["_connection"] = json!({"connection_id":"fixture-connection", "integration_id": if agent=="sharepoint" {"microsoft_entra_client_credentials"} else {"shopify"}, "parameters":{}});
    Case {
        agent,
        capability,
        input,
        requests,
        expected,
    }
}
fn fresh(agent: &'static str) -> Case {
    if agent == "sharepoint" {
        case(
            agent,
            "sharepoint-get-item",
            json!({"drive_id":"drive","item_id":"fresh"}),
            vec![Request::graph(
                "GET",
                "/drives/drive/items/fresh",
                None,
                item(),
            )],
            json!({"item":{"id":"42","name":"fixture.txt"}}),
        )
    } else {
        case(
            agent,
            "add-tags",
            json!({"id":"fresh","tags":["fixture"]}),
            vec![Request::graphql(
                "tagsAdd",
                json!({"id":"fresh","tags":["fixture"]}),
                json!({"data":{"tagsAdd":{"node":{"id":"fresh"},"userErrors":[]}}}),
            )],
            json!({"node":{"id":"fresh"}}),
        )
    }
}
fn sharepoint_cases() -> Vec<Case> {
    let mut download = Request::graph(
        "GET",
        "/drives/drive/items/42/content",
        None,
        json!("hello"),
    );
    download.timeout = 120_000;
    download.response = json!({"status":200,"headers":{},"body_raw":BASE64.encode(b"hello")});
    let mut upload = Request::graph(
        "PUT",
        "/drives/drive/root:/fixture.txt:/content",
        None,
        item(),
    );
    upload.timeout = 60_000;
    upload.body = Some(b"hello".to_vec());
    let mut copy = Request::graph(
        "POST",
        "/drives/drive/items/42/copy",
        Some(json!({"parentReference":{"id":"destination"}})),
        Value::Null,
    );
    copy.response =
        json!({"status":202,"headers":{"Location":"https://monitor.invalid/copy"},"body":null});
    let mut monitor = Request::graph(
        "GET",
        "",
        None,
        json!({"status":"inProgress","percentageComplete":50}),
    );
    monitor.url = "https://monitor.invalid/copy".into();
    monitor.connection = false;
    let metadata = Request::graph("GET", "/drives/drive/items/42", None, item());
    let mut missing_metadata = metadata.clone();
    missing_metadata.response = envelope(404, json!({"error":{"message":"missing"}}));
    vec![
        fresh("sharepoint"),
        case(
            "sharepoint",
            "sharepoint-list-children",
            json!({"drive_id":"drive","page_token":"/drives/drive/root/children?$skiptoken=page2","page_size":99}),
            vec![Request::graph(
                "GET",
                "/drives/drive/root/children?$skiptoken=page2",
                None,
                json!({"value":[],"@odata.nextLink":"https://graph.microsoft.com/v1.0/drives/drive/root/children?$skiptoken=page3"}),
            )],
            json!({"count":0,"next_page_token":"/drives/drive/root/children?$skiptoken=page3"}),
        ),
        case(
            "sharepoint",
            "sharepoint-create-folder",
            json!({"drive_id":"drive","folder_name":"folder"}),
            vec![Request::graph(
                "POST",
                "/drives/drive/root/children",
                Some(
                    json!({"name":"folder","folder":{},"@microsoft.graph.conflictBehavior":"rename"}),
                ),
                item(),
            )],
            json!({"item":{"id":"42"}}),
        ),
        case(
            "sharepoint",
            "sharepoint-move-item",
            json!({"drive_id":"drive","item_id":"42","new_name":"renamed.txt"}),
            vec![Request::graph(
                "PATCH",
                "/drives/drive/items/42",
                Some(json!({"name":"renamed.txt"})),
                item(),
            )],
            json!({"item":{"id":"42"}}),
        ),
        case(
            "sharepoint",
            "sharepoint-delete-item",
            json!({"drive_id":"drive","item_id":"42"}),
            vec![Request {
                response: envelope(204, json!("")),
                ..metadata.clone().with_method("DELETE")
            }],
            json!({"success":true}),
        ),
        case(
            "sharepoint",
            "sharepoint-upload-file",
            json!({"drive_id":"drive","filename":"fixture.txt","content":"hello","is_base64":false}),
            vec![upload],
            json!({"item":{"id":"42"}}),
        ),
        case(
            "sharepoint",
            "sharepoint-copy-item",
            json!({"drive_id":"drive","item_id":"42","destination_parent_id":"destination"}),
            vec![copy],
            json!({"monitor_url":"https://monitor.invalid/copy"}),
        ),
        case(
            "sharepoint",
            "sharepoint-get-copy-status",
            json!({"monitor_url":"https://monitor.invalid/copy"}),
            vec![monitor],
            json!({"status":"inProgress","percentage_complete":50.0}),
        ),
        case(
            "sharepoint",
            "sharepoint-download-file",
            json!({"drive_id":"drive","item_id":"42","as_text":true}),
            vec![metadata, download.clone()],
            json!({"content":"hello","filename":"fixture.txt"}),
        ),
        case(
            "sharepoint",
            "sharepoint-download-file",
            json!({"drive_id":"drive","item_id":"42","as_text":true}),
            vec![missing_metadata, download],
            json!({"content":"hello","filename":null}),
        ),
    ]
}
impl Request {
    fn with_method(mut self, method: &'static str) -> Self {
        self.method = method;
        self
    }
}
fn large_upload() -> Case {
    // Exercise the existing 4 MiB boundary; this fixture does not certify the
    // provider's chunk-alignment requirements (tracked separately in the audit).
    let chunk = 4 * 1024 * 1024;
    let total = chunk + 17;
    let session = Request::graph(
        "POST",
        "/drives/drive/root:/large.txt:/createUploadSession",
        Some(json!({"item":{}})),
        json!({"uploadUrl":"https://upload.invalid/session"}),
    );
    let part = |start: usize, end: usize, status| Request {
        method: "PUT",
        url: "https://upload.invalid/session".into(),
        connection: false,
        timeout: 120_000,
        body: Some(vec![b'x'; end - start]),
        query_contains: None,
        range: Some(format!("bytes {start}-{}/{total}", end - 1)),
        response: envelope(
            status,
            if status == 202 {
                json!({"nextExpectedRanges":[format!("{end}-")]})
            } else {
                item()
            },
        ),
    };
    case(
        "sharepoint",
        "sharepoint-upload-file-large",
        json!({"drive_id":"drive","filename":"large.txt","content":"x".repeat(total),"is_base64":false}),
        vec![session, part(0, chunk, 202), part(chunk, total, 201)],
        json!({"item":{"id":"42"}}),
    )
}
fn shopify_cases() -> Vec<Case> {
    // Three requests in a row: the product's media, taking it off the product,
    // then adding the new image.
    let images = case(
        "shopify",
        "replace-product-images",
        json!({"productId":"42","images":[{"url":"https://image.invalid/new.png","alt":"new image"}]}),
        vec![
            Request::graphql(
                "product",
                json!({"id":"42","mediaFirst":250}),
                json!({"data":{"product":{"id":"42","media":{"nodes":[{"id":"old-image"}],"pageInfo":{"hasNextPage":false}}}}}),
            ),
            Request::graphql(
                "fileUpdate",
                json!({"files":[{"id":"old-image","referencesToRemove":["42"]}]}),
                json!({"data":{"fileUpdate":{"files":[{"id":"old-image"}],"userErrors":[]}}}),
            ),
            Request::graphql(
                "productUpdate",
                json!({"id":"42","media":[{"originalSource":"https://image.invalid/new.png","mediaContentType":"IMAGE","alt":"new image"}]}),
                json!({"data":{"productUpdate":{"product":{"id":"42"},"userErrors":[]}}}),
            ),
        ],
        json!({"product":{"id":"42"},"removedMediaIds":["old-image"],"added":1}),
    );
    // One request per product; a product Shopify refuses does not stop the rest.
    let bulk = |first_fails| {
        let requests = (1..=3).map(|id| Request::graphql("productVariantsBulkUpdate", json!({"productId":format!("product-{id}"),"variants":[{"id":format!("variant-{id}"),"price":"12.5"}]}),
            if first_fails && id==1 {json!({"data":{"productVariantsBulkUpdate":{"productVariants":null,"userErrors":[{"field":["price"],"message":"invalid"}]}}})}
            else {json!({"data":{"productVariantsBulkUpdate":{"productVariants":[{"id":format!("variant-{id}")}],"userErrors":[]}}})})).collect();
        case(
            "shopify",
            "bulk-update-variants",
            json!({"products":(1..=3).map(|id| json!({"productId":format!("product-{id}"),"variants":[{"id":format!("variant-{id}"),"price":"12.5"}]})).collect::<Vec<_>>()}),
            requests,
            json!({"succeeded":if first_fails {2} else {3},"failed":if first_fails {1} else {0}}),
        )
    };
    vec![fresh("shopify"), images, bulk(false), bulk(true)]
}
fn contains(actual: &Value, expected: &Value) {
    if let Value::Object(fields) = expected {
        for (key, value) in fields {
            assert!(actual.get(key).is_some(), "missing {key}: {actual}");
            contains(&actual[key], value);
        }
    } else {
        assert_eq!(actual, expected);
    }
}
async fn cancellation(cases: Vec<Case>, partial: bool) -> anyhow::Result<()> {
    for case in cases {
        for pending in 0..case.requests.len() {
            let fresh = fresh(case.agent);
            let bytes = compose_agent(
                case.agent,
                case.capability,
                &serde_json::to_vec(&case.input)?,
                fresh.capability,
                &serde_json::to_vec(&fresh.input)?,
            )?;
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let upstream = format!("http://{}/upstream", listener.local_addr()?);
            let started = Arc::new(Notify::new());
            let cleaned = Arc::new(Notify::new());
            let requests = case.requests.clone();
            let expected = fresh.expected.clone();
            let server = tokio::spawn({
                let started = started.clone();
                let cleaned = cleaned.clone();
                async move {
                    for (index, request) in requests.iter().enumerate().take(pending + 1) {
                        let (mut socket, _) = listener.accept().await?;
                        request.check(&mut socket).await?;
                        if index < pending {
                            respond(&mut socket, request.response.clone()).await?;
                            continue;
                        }
                        if partial {
                            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 400\r\nConnection: close\r\n\r\n{").await?;
                        }
                        started.notify_one();
                        match socket.read(&mut [0]).await {
                            Ok(0) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                            other => {
                                anyhow::bail!("pending HTTP request was not closed: {other:?}")
                            }
                        }
                        cleaned.notify_one();
                    }
                    // A caught provider error or dropped request must never advance
                    // the cancelled invocation. Only this fresh call may issue I/O.
                    let (mut socket, _) = listener.accept().await?;
                    fresh.requests[0].check(&mut socket).await?;
                    respond(&mut socket, fresh.requests[0].response.clone()).await
                }
            });
            let result = run_cancellation_fixture(
                bytes,
                FixtureContext::with_upstream("fixture-tenant", upstream),
                started,
                cleaned,
                server,
            )
            .await?;
            contains(&result, &expected);
        }
    }
    Ok(())
}
async fn normal(case: Case) -> anyhow::Result<Result<Vec<u8>, runtara_component_host::ErrorInfo>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream = format!("http://{}/upstream", listener.local_addr()?);
    let mut server = tokio::spawn(async move {
        for request in case.requests {
            let (mut socket, _) = listener.accept().await?;
            request.check(&mut socket).await?;
            respond(&mut socket, request.response).await?;
        }
        anyhow::Ok(())
    });
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        invoke_named_agent(
            case.agent,
            FixtureContext::with_upstream("fixture-tenant", upstream),
            case.capability,
            serde_json::to_vec(&case.input)?,
        ),
    )
    .await;
    let result = match result {
        Ok(Ok(value)) => value,
        other => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("invocation failed: {other:?}");
        }
    };
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(value) => value??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            return Err(error.into());
        }
    }
    Ok(result)
}
#[tokio::test]
async fn sharepoint_headers_cancel_all_request_families() -> anyhow::Result<()> {
    cancellation(sharepoint_cases(), false).await
}
#[tokio::test]
async fn sharepoint_body_cancel_all_request_families() -> anyhow::Result<()> {
    cancellation(sharepoint_cases(), true).await
}
#[tokio::test]
async fn sharepoint_cancel_each_upload_stage() -> anyhow::Result<()> {
    for partial in [false, true] {
        cancellation(vec![large_upload()], partial).await?;
    }
    Ok(())
}
#[tokio::test]
async fn shopify_headers_cancel_mutation_chains_and_bulk_error_continuations() -> anyhow::Result<()>
{
    cancellation(shopify_cases(), false).await
}
#[tokio::test]
async fn shopify_body_cancel_mutation_chains_and_bulk_error_continuations() -> anyhow::Result<()> {
    cancellation(shopify_cases(), true).await
}
#[tokio::test]
async fn shared_macro_preserves_successful_provider_behavior() -> anyhow::Result<()> {
    for case in sharepoint_cases()
        .into_iter()
        .chain(shopify_cases())
        .chain([large_upload()])
    {
        let expected = case.expected.clone();
        let result = normal(case)
            .await?
            .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
        contains(&serde_json::from_slice(&result)?, &expected);
    }
    Ok(())
}
#[tokio::test]
async fn shared_macro_preserves_provider_errors() -> anyhow::Result<()> {
    for status in [400, 401, 429, 503] {
        let mut case = fresh("sharepoint");
        case.requests[0].response = envelope(status, json!({"error":{"message":"fixture error"}}));
        let error = normal(case).await?.unwrap_err();
        assert_eq!(error.retryable, status == 429 || status == 503);
        assert!(error.code.contains("HTTP_"));
    }
    // Shopify's fresh case is a mutation: a 5xx may have come after Shopify
    // carried it out, so it is not sent again.
    for (status, code, retryable) in [
        (400, "SHOPIFY_REQUEST_FAILED", false),
        (401, "SHOPIFY_UNAUTHORIZED", false),
        (429, "SHOPIFY_RATE_LIMITED", true),
        (503, "SHOPIFY_OUTCOME_UNKNOWN", false),
    ] {
        let mut case = fresh("shopify");
        case.requests[0].response = envelope(status, json!({"error":{"message":"fixture error"}}));
        let error = normal(case).await?.unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.retryable),
            (code, retryable),
            "{status}"
        );
    }
    let mut case = fresh("shopify");
    case.requests[0].response = envelope(200, json!({"errors":[{"message":"fixture error"}]}));
    assert_eq!(
        normal(case).await?.unwrap_err().code,
        "SHOPIFY_GRAPHQL_ERROR"
    );
    Ok(())
}
/// Fulfilling by SKU sends one fulfillment per location. Once one went
/// through, a later failure must not leave the step to be retried: a retry
/// would fulfill the first location's units again.
#[tokio::test]
async fn shopify_fulfill_by_sku_is_not_retried_after_a_location_was_fulfilled() -> anyhow::Result<()>
{
    let order = |id: &str, location: &str, line: &str, sku: &str| {
        json!({"id":id,"status":"OPEN","assignedLocation":{"name":location,"location":{"id":location}},
            "supportedActions":[{"action":"CREATE_FULFILLMENT"}],
            "lineItems":{"nodes":[{"id":line,"sku":sku,"remainingQuantity":2,"totalQuantity":2}],"pageInfo":{"hasNextPage":false}}})
    };
    let fulfill = |order: &str, line: &str, quantity: i64, answer: Value| {
        Request::graphql(
            "fulfillmentCreate",
            json!({"lineItemsByFulfillmentOrder":[{"fulfillmentOrderId":order,"fulfillmentOrderLineItems":[{"id":line,"quantity":quantity}]}]}),
            json!({"data":{"fulfillmentCreate":answer}}),
        )
    };
    let case = case(
        "shopify",
        "fulfill-by-sku",
        json!({"orderId":"order-1","items":[{"sku":"HAT","quantity":2},{"sku":"SCARF","quantity":1}]}),
        vec![
            Request::graphql(
                "fulfillmentOrders",
                json!({"orderId":"order-1","first":4,"lineItemsFirst":100}),
                json!({"data":{"order":{"id":"order-1","fulfillmentOrders":{"nodes":[
                    order("fo-east","loc-east","east-1","HAT"),
                    order("fo-west","loc-west","west-1","SCARF"),
                ],"pageInfo":{"hasNextPage":false}}}}}),
            ),
            fulfill(
                "fo-east",
                "east-1",
                2,
                json!({"fulfillment":{"id":"fulfillment-east"},"userErrors":[]}),
            ),
            fulfill(
                "fo-west",
                "west-1",
                1,
                json!({"fulfillment":null,"userErrors":[{"field":["fulfillment"],"message":"refused"}]}),
            ),
        ],
        Value::Null,
    );
    let error = normal(case).await?.unwrap_err();
    assert_eq!(error.code, "SHOPIFY_PARTIALLY_FULFILLED");
    assert!(!error.retryable);
    assert!(error.message.contains("refused"), "{}", error.message);
    Ok(())
}
#[tokio::test]
async fn shared_macro_preserves_unknown_capability_and_malformed_input() -> anyhow::Result<()> {
    for agent in ["sharepoint", "shopify"] {
        for (capability, input, code) in [
            ("unknown", b"{}".to_vec(), "UNKNOWN_CAPABILITY"),
            (
                fresh(agent).capability,
                b"{".to_vec(),
                "INPUT_DESERIALIZATION_ERROR",
            ),
            (
                fresh(agent).capability,
                b"{}".to_vec(),
                "INPUT_DESERIALIZATION_ERROR",
            ),
        ] {
            let result = invoke_named_agent(agent, FixtureContext::public(), capability, input)
                .await?
                .unwrap_err();
            assert_eq!(result.code, code);
            assert!(!result.retryable);
        }
    }
    Ok(())
}

#[tokio::test]
async fn every_builtin_uses_shared_callback_export_and_error_contract() -> anyhow::Result<()> {
    for agent in [
        "ai-tools",
        "azure-blob-storage",
        "bedrock",
        "compression",
        "crypto",
        "csv",
        "datetime",
        "http",
        "hubspot",
        "mailgun",
        "mcp",
        "object-model",
        "openai",
        "quickbooks",
        "s3-storage",
        "sharepoint",
        "shopify",
        "slack",
        "sqs",
        "stripe",
        "teams",
        "text",
        "transform",
        "utils",
        "xlsx",
        "xml",
    ] {
        // Composition validates actual callback lifts, not just async WIT types.
        compose_agent(agent, "unknown", b"{}", "unknown", b"{}")?;
        for (input, code) in [
            (b"{}".to_vec(), "UNKNOWN_CAPABILITY"),
            (b"{".to_vec(), "INPUT_DESERIALIZATION_ERROR"),
        ] {
            let error = invoke_named_agent(agent, FixtureContext::public(), "unknown", input)
                .await?
                .unwrap_err();
            assert_eq!(error.code, code, "{agent}");
            assert!(!error.retryable);
        }
    }
    Ok(())
}
#[tokio::test]
async fn synchronous_capabilities_keep_results_and_datetime_input_normalization()
-> anyhow::Result<()> {
    // The existing datetime decoder maps empty/whitespace to null. That
    // reaches typed validation, which rejects null; it does not apply defaults.
    for input in [b"".to_vec(), b" \t\n".to_vec()] {
        let error = invoke_named_agent(
            "datetime",
            FixtureContext::public(),
            "get-current-date",
            input,
        )
        .await?
        .unwrap_err();
        assert_eq!(error.code, "INPUT_DESERIALIZATION_ERROR");
        assert!(error.message.contains("invalid type: null"));
    }
    let result = invoke_named_agent(
        "datetime",
        FixtureContext::public(),
        "get-current-date",
        b"{}".to_vec(),
    )
    .await?
    .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
    let value: Value = serde_json::from_slice(&result)?;
    assert!(value.as_str().is_some_and(|s| s.len() >= 10), "{value}");
    let result = invoke_named_agent(
        "utils",
        FixtureContext::public(),
        "random-double",
        b"{}".to_vec(),
    )
    .await?
    .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
    let value: Value = serde_json::from_slice(&result)?;
    assert!(
        value
            .as_f64()
            .is_some_and(|value| (0.0..1.0).contains(&value)),
        "{value}"
    );
    Ok(())
}
