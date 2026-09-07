//! Built HubSpot component against local proxy fixtures; no CRM account is used.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_proxy, respond, run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection() -> Value {
    json!({"connection_id":"fixture-connection","integration_id":"hubspot_private_app","parameters":{}})
}

struct Case {
    capability: &'static str,
    input: Value,
    method: &'static str,
    path: &'static str,
    body: Value,
    output_key: &'static str,
}

fn cases() -> Vec<Case> {
    [
        ("list-business-units", json!({"user_id": "7"}), "GET", "/business-units/v3/business-units/user/7", json!(null), "results"),
        ("list-object-properties", json!({"object_type": "contacts", "archived": false, "data_sensitivity": "sensitive"}), "GET", "/crm/v3/properties/contacts?archived=false&dataSensitivity=sensitive", json!(null), "results"),
        ("get-object-property", json!({"object_type": "contacts", "property_name": "email", "archived": true}), "GET", "/crm/v3/properties/contacts/email?archived=true", json!(null), "property"),
        ("list-contacts", json!({"limit": 2, "after": "page +", "properties": "name,email"}), "GET", "/crm/v3/objects/contacts?limit=2&after=page%20%2B&properties=name%2Cemail", json!(null), "page"),
        ("get-contact", json!({"contact_id": "42", "properties": "name,email", "id_property": "email"}), "GET", "/crm/v3/objects/contacts/42?properties=name%2Cemail&idProperty=email", json!(null), "contact"),
        ("create-contact", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "POST", "/crm/v3/objects/contacts", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "contact"),
        ("update-contact", json!({"contact_id": "42", "properties": {"name": "fixture", "note": "a + / €"}}), "PATCH", "/crm/v3/objects/contacts/42", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "contact"),
        ("delete-contact", json!({"contact_id": "42"}), "DELETE", "/crm/v3/objects/contacts/42", json!(null), "success"),
        ("search-contacts", json!({"filter_groups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "POST", "/crm/v3/objects/contacts/search", json!({"filterGroups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "search"),
        ("list-companies", json!({"limit": 2, "after": "page +", "properties": "name,email"}), "GET", "/crm/v3/objects/companies?limit=2&after=page%20%2B&properties=name%2Cemail", json!(null), "page"),
        ("get-company", json!({"company_id": "42", "properties": "name,email"}), "GET", "/crm/v3/objects/companies/42?properties=name%2Cemail", json!(null), "company"),
        ("create-company", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "POST", "/crm/v3/objects/companies", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "company"),
        ("update-company", json!({"company_id": "42", "properties": {"name": "fixture", "note": "a + / €"}}), "PATCH", "/crm/v3/objects/companies/42", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "company"),
        ("delete-company", json!({"company_id": "42"}), "DELETE", "/crm/v3/objects/companies/42", json!(null), "success"),
        ("search-companies", json!({"filter_groups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "POST", "/crm/v3/objects/companies/search", json!({"filterGroups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "search"),
        ("list-deals", json!({"limit": 2, "after": "page +", "properties": "name,email"}), "GET", "/crm/v3/objects/deals?limit=2&after=page%20%2B&properties=name%2Cemail", json!(null), "page"),
        ("get-deal", json!({"deal_id": "42", "properties": "name,email"}), "GET", "/crm/v3/objects/deals/42?properties=name%2Cemail", json!(null), "deal"),
        ("create-deal", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "POST", "/crm/v3/objects/deals", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "deal"),
        ("update-deal", json!({"deal_id": "42", "properties": {"name": "fixture", "note": "a + / €"}}), "PATCH", "/crm/v3/objects/deals/42", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "deal"),
        ("delete-deal", json!({"deal_id": "42"}), "DELETE", "/crm/v3/objects/deals/42", json!(null), "success"),
        ("search-deals", json!({"filter_groups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "POST", "/crm/v3/objects/deals/search", json!({"filterGroups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "search"),
        ("list-quotes", json!({"limit": 2, "after": "page +", "properties": "name,email"}), "GET", "/crm/v3/objects/quotes?limit=2&after=page%20%2B&properties=name%2Cemail", json!(null), "page"),
        ("get-quote", json!({"quote_id": "42", "properties": "name,email"}), "GET", "/crm/v3/objects/quotes/42?properties=name%2Cemail", json!(null), "quote"),
        ("create-quote", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "POST", "/crm/v3/objects/quotes", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "quote"),
        ("update-quote", json!({"quote_id": "42", "properties": {"name": "fixture", "note": "a + / €"}}), "PATCH", "/crm/v3/objects/quotes/42", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "quote"),
        ("delete-quote", json!({"quote_id": "42"}), "DELETE", "/crm/v3/objects/quotes/42", json!(null), "success"),
        ("search-quotes", json!({"filter_groups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "POST", "/crm/v3/objects/quotes/search", json!({"filterGroups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "search"),
        ("list-line-items", json!({"limit": 2, "after": "page +", "properties": "name,email"}), "GET", "/crm/v3/objects/line_items?limit=2&after=page%20%2B&properties=name%2Cemail", json!(null), "page"),
        ("get-line-item", json!({"line_item_id": "42", "properties": "name,email", "properties_with_history": "price,name", "associations": "deals"}), "GET", "/crm/v3/objects/line_items/42?properties=name%2Cemail&propertiesWithHistory=price%2Cname&associations=deals", json!(null), "line_item"),
        ("create-line-item", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "POST", "/crm/v3/objects/line_items", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "line_item"),
        ("update-line-item", json!({"line_item_id": "42", "properties": {"name": "fixture", "note": "a + / €"}}), "PATCH", "/crm/v3/objects/line_items/42", json!({"properties": {"name": "fixture", "note": "a + / €"}}), "line_item"),
        ("delete-line-item", json!({"line_item_id": "42"}), "DELETE", "/crm/v3/objects/line_items/42", json!(null), "success"),
        ("search-line-items", json!({"filter_groups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "POST", "/crm/v3/objects/line_items/search", json!({"filterGroups": [{"filters": [{"propertyName": "name", "operator": "EQ", "value": "fixture"}]}], "query": "fixture", "properties": ["name"], "limit": 2, "after": "99", "sorts": ["name"]}), "search"),
        ("list-owners", json!({"limit": 2, "after": "page +", "email": "fixture@example.invalid"}), "GET", "/crm/v3/owners/?limit=2&after=page%20%2B&email=fixture%40example.invalid", json!(null), "page"),
        ("get-owner", json!({"owner_id": "42"}), "GET", "/crm/v3/owners/42", json!(null), "owner"),
        ("list-pipelines", json!({}), "GET", "/crm/v3/pipelines/deals", json!(null), "results"),
        ("get-pipeline", json!({"pipeline_id": "42", "object_type": "tickets"}), "GET", "/crm/v3/pipelines/tickets/42", json!(null), "pipeline"),
        ("create-association", json!({"from_object_type": "contacts", "from_object_id": "42", "to_object_type": "companies", "to_object_id": "9", "association_type": "1"}), "PUT", "/crm/v4/objects/contacts/42/associations/companies/9", json!([{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 1}]), "result"),
        ("list-associations", json!({"from_object_type": "contacts", "from_object_id": "42", "to_object_type": "companies"}), "GET", "/crm/v4/objects/contacts/42/associations/companies", json!(null), "page"),
        ("list-webhook-subscriptions", json!({"app_id": "7"}), "GET", "/webhooks/2026-03/7/subscriptions", json!(null), "subscriptions"),
        ("create-webhook-subscription", json!({"app_id": "7", "event_type": "contact.propertyChange", "property_name": "email", "object_type_id": "0-1", "event_type_name": "fixture"}), "POST", "/webhooks/2026-03/7/subscriptions", json!({"eventType": "contact.propertyChange", "active": false, "propertyName": "email", "objectTypeId": "0-1", "eventTypeName": "fixture"}), "subscription"),
        ("update-webhook-subscription", json!({"app_id": "7", "subscription_id": "9", "active": true}), "PUT", "/webhooks/2026-03/7/subscriptions/9", json!({"active": true}), "subscription"),
        ("delete-webhook-subscription", json!({"app_id": "7", "subscription_id": "9"}), "DELETE", "/webhooks/2026-03/7/subscriptions/9", json!(null), "success"),
    ].into_iter().map(|(capability, mut input, method, path, body, output_key)| {
        input["_connection"] = connection();
        Case { capability, input, method, path, body, output_key }
    }).collect()
}

async fn request(socket: &mut tokio::net::TcpStream, case: &Case) -> anyhow::Result<()> {
    let envelope = read_proxy(socket).await?;
    assert_eq!(envelope["method"], case.method, "{}", case.capability);
    // Query field order is unspecified because the Agent builds a HashMap.
    let expected = format!("https://api.hubapi.com{}", case.path);
    let parts = |url: &str| {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        let mut fields: Vec<String> = query.split('&').map(str::to_owned).collect();
        fields.sort();
        (path.to_string(), fields)
    };
    assert_eq!(parts(envelope["url"].as_str().unwrap()), parts(&expected));
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
    if matches!(case.method, "POST" | "PATCH" | "PUT") {
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
        .find(|case| case.capability == "get-contact")
        .unwrap()
}
async fn cancellation(partial: bool) -> anyhow::Result<()> {
    for case in cases() {
        let fresh = fresh_case();
        let bytes = compose_agent(
            "hubspot",
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
                    other => anyhow::bail!("HubSpot wait was not closed: {other:?}"),
                }
                cleaned.notify_one();
                // The next request must belong to the explicit fresh invocation.
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, &fresh).await?;
                respond(&mut socket, json!({"status":200,"headers":{},"body":{"id":"42","properties":{"name":"fresh"}}})).await
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
            json!({"contact":{"id":"42","properties":{"name":"fresh"}}})
        );
    }
    Ok(())
}

#[tokio::test]
async fn hubspot_cancel_closes_pending_headers_for_every_capability() -> anyhow::Result<()> {
    cancellation(false).await
}

#[tokio::test]
async fn hubspot_cancel_closes_partial_body_for_every_capability() -> anyhow::Result<()> {
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
            "hubspot",
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
            anyhow::bail!("HubSpot invocation failed: {other:?}");
        }
    };
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(joined) => joined??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("HubSpot proxy fixture did not finish: {error}; result={result:?}");
        }
    }
    Ok(result)
}

#[tokio::test]
async fn hubspot_success_preserves_crm_search_paging_associations_and_webhooks()
-> anyhow::Result<()> {
    for case in cases() {
        let body =
            json!({"id":"42","results":[{"id":"42"}],"paging":{"next":{"after":"99"}},"total":1});
        let expected = match case.output_key {
            "success" => json!({"success":true}),
            "results" => json!({"results":body["results"]}),
            "page" => json!({"results":body["results"],"paging":body["paging"]}),
            "search" => json!({"results":body["results"],"paging":body["paging"],"total":1}),
            key => json!({key:body}),
        };
        let bytes = invoke_response(case, json!({"status":200,"headers":{},"body":body}))
            .await?
            .expect("success");
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
    }
    for (capability, expected) in [
        ("create-association", json!({"result":null})),
        ("delete-contact", json!({"success":true})),
    ] {
        let case = cases()
            .into_iter()
            .find(|c| c.capability == capability)
            .unwrap();
        let bytes = invoke_response(case, json!({"status":204,"headers":{},"body_raw":""}))
            .await?
            .expect("empty success");
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn hubspot_provider_errors_preserve_retry_contracts_for_every_http_method()
-> anyhow::Result<()> {
    for method in ["GET", "POST", "PATCH", "PUT", "DELETE"] {
        for (status, code, retryable) in [
            (400, "HUBSPOT_REQUEST_FAILED", false),
            (401, "HUBSPOT_UNAUTHORIZED", false),
            (403, "HUBSPOT_UNAUTHORIZED", false),
            (429, "HUBSPOT_UPSTREAM_ERROR", true),
            (503, "HUBSPOT_UPSTREAM_ERROR", true),
        ] {
            let case = cases().into_iter().find(|c| c.method == method).unwrap();
            let path = case.path.split('?').next().unwrap();
            let error = invoke_response(case,json!({"status":status,"headers":{"Retry-After":"3","retry-after-ms":"7"},"body_raw":BASE64.encode("€".repeat(200))})).await?.expect_err("provider rejection");
            assert_eq!(error.code, code);
            assert_eq!(error.retryable, retryable);
            assert_eq!(
                error.retry_after_ms,
                if status == 429 { Some(7) } else { None }
            );
            let attributes: Value = serde_json::from_str(&error.attributes.unwrap())?;
            assert_eq!(attributes["path"], path);
            assert_eq!(attributes["status_code"], status.to_string());
            assert_eq!(attributes["body"], format!("{}…", "€".repeat(170)));
        }
    }
    let error = invoke_response(
        fresh_case(),
        json!({"status":429,"headers":{"Retry-After":u64::MAX.to_string()},"body":"limited"}),
    )
    .await?
    .expect_err("overflow is ignored");
    assert!(error.retryable);
    assert_eq!(error.retry_after_ms, None);
    Ok(())
}

#[tokio::test]
async fn hubspot_transport_parse_and_validation_failures_stay_distinct() -> anyhow::Result<()> {
    for (body_raw, code, retryable) in [
        ("!invalid-base64!", "HUBSPOT_NETWORK_ERROR", true),
        ("eA==", "HUBSPOT_RESPONSE_PARSE_ERROR", false),
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
            "get-contact",
            json!({"contact_id":"42"}),
            "HUBSPOT_MISSING_CONNECTION",
        ),
        ("does-not-exist", json!({}), "UNKNOWN_CAPABILITY"),
    ] {
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            invoke_named_agent(
                "hubspot",
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

#[test]
fn hubspot_fixtures_cover_every_published_capability() -> anyhow::Result<()> {
    let path = super::real_agent::agent_path("hubspot")?.with_extension("meta.json");
    let metadata: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let published = metadata["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cap| cap["id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let cases = cases();
    let tested = cases
        .iter()
        .map(|case| case.capability)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(tested.len(), cases.len(), "duplicate fixture capability");
    assert_eq!(tested, published);
    Ok(())
}
