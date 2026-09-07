//! Built Stripe component against local proxy fixtures only. Provider-side
//! subscription cancellation is a normal API call, distinct from task cancellation.
use super::real_agent::{
    compose_agent, invoke_named_agent, read_proxy, respond, run_cancellation_fixture,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use runtara_component_host::CallContext;
use serde_json::{Value, json};

fn connection() -> Value {
    json!({"connection_id":"fixture-connection","integration_id":"stripe_api_key","parameters":{}})
}

struct Case {
    capability: &'static str,
    method: &'static str,
    path: &'static str,
    input: Value,
    encoded: &'static str,
    output_key: &'static str,
}

fn cases() -> Vec<Case> {
    [
        ("list-customers", "GET", "/customers", json!({"limit":7,"starting_after":"cus_before","email":"a+b@example.invalid"}), "limit=7&starting_after=cus_before&email=a%2Bb%40example.invalid", "list"),
        ("get-customer", "GET", "/customers/cus_fixture", json!({"customer_id":"cus_fixture"}), "", "customer"),
        ("create-customer", "POST", "/customers", json!({"name":"A & B","email":"a+b@example.invalid","metadata":{"kind":"fixture","count":2}}), "name=A%20%26%20B&email=a%2Bb%40example.invalid&metadata%5Bkind%5D=fixture&metadata%5Bcount%5D=2", "customer"),
        ("update-customer", "POST", "/customers/cus_fixture", json!({"customer_id":"cus_fixture","description":"updated / fixture"}), "description=updated%20%2F%20fixture", "customer"),
        ("list-products", "GET", "/products", json!({"active":"true","limit":3}), "active=true&limit=3", "list"),
        ("get-product", "GET", "/products/prod_fixture", json!({"product_id":"prod_fixture"}), "", "product"),
        ("create-product", "POST", "/products", json!({"name":"fixture product","active":"false"}), "name=fixture%20product&active=false", "product"),
        ("list-prices", "GET", "/prices", json!({"product":"prod_fixture","active":"true"}), "product=prod_fixture&active=true", "list"),
        ("create-price", "POST", "/prices", json!({"product":"prod_fixture","unit_amount":1234,"currency":"usd","recurring_interval":"month"}), "product=prod_fixture&unit_amount=1234&currency=usd&recurring%5Binterval%5D=month", "price"),
        ("create-payment-intent", "POST", "/payment_intents", json!({"amount":1234,"currency":"usd","customer":"cus_fixture","payment_method_types":"card, link","receipt_email":"a+b@example.invalid"}), "amount=1234&currency=usd&customer=cus_fixture&payment_method_types%5B0%5D=card&payment_method_types%5B1%5D=link&receipt_email=a%2Bb%40example.invalid", "payment_intent"),
        ("get-payment-intent", "GET", "/payment_intents/pi_fixture", json!({"payment_intent_id":"pi_fixture"}), "", "payment_intent"),
        ("list-payment-intents", "GET", "/payment_intents", json!({"customer":"cus_fixture","starting_after":"pi_before"}), "customer=cus_fixture&starting_after=pi_before", "list"),
        ("create-invoice", "POST", "/invoices", json!({"customer":"cus_fixture","collection_method":"send_invoice","days_until_due":14}), "customer=cus_fixture&collection_method=send_invoice&days_until_due=14", "invoice"),
        ("get-invoice", "GET", "/invoices/in_fixture", json!({"invoice_id":"in_fixture"}), "", "invoice"),
        ("list-invoices", "GET", "/invoices", json!({"customer":"cus_fixture","status":"draft"}), "customer=cus_fixture&status=draft", "list"),
        ("finalize-invoice", "POST", "/invoices/in_fixture/finalize", json!({"invoice_id":"in_fixture"}), "", "invoice"),
        ("send-invoice", "POST", "/invoices/in_fixture/send", json!({"invoice_id":"in_fixture"}), "", "invoice"),
        ("create-subscription", "POST", "/subscriptions", json!({"customer":"cus_fixture","price":"price_fixture","quantity":2,"trial_period_days":7}), "customer=cus_fixture&items%5B0%5D%5Bprice%5D=price_fixture&items%5B0%5D%5Bquantity%5D=2&trial_period_days=7", "subscription"),
        ("get-subscription", "GET", "/subscriptions/sub_fixture", json!({"subscription_id":"sub_fixture"}), "", "subscription"),
        ("list-subscriptions", "GET", "/subscriptions", json!({"customer":"cus_fixture","status":"active"}), "customer=cus_fixture&status=active", "list"),
        ("cancel-subscription", "DELETE", "/subscriptions/sub_fixture", json!({"subscription_id":"sub_fixture"}), "", "subscription"),
        ("cancel-subscription", "POST", "/subscriptions/sub_fixture", json!({"subscription_id":"sub_fixture","cancel_at_period_end":"true"}), "cancel_at_period_end=true", "subscription"),
        ("create-refund", "POST", "/refunds", json!({"payment_intent":"pi_fixture","amount":200,"reason":"requested_by_customer"}), "payment_intent=pi_fixture&amount=200&reason=requested_by_customer", "refund"),
        ("get-refund", "GET", "/refunds/re_fixture", json!({"refund_id":"re_fixture"}), "", "refund"),
        ("get-balance", "GET", "/balance", json!({}), "", "balance"),
        ("list-charges", "GET", "/charges", json!({"customer":"cus_fixture","payment_intent":"pi_fixture"}), "customer=cus_fixture&payment_intent=pi_fixture", "list"),
        ("get-charge", "GET", "/charges/ch_fixture", json!({"charge_id":"ch_fixture"}), "", "charge"),
    ].into_iter().map(|(capability,method,path,mut input,encoded,output_key)| {
        input["_connection"] = connection();
        Case { capability, method, path, input, encoded, output_key }
    }).collect()
}

fn sorted_fields(encoded: &str) -> Vec<&str> {
    let mut fields: Vec<_> = encoded
        .split('&')
        .filter(|field| !field.is_empty())
        .collect();
    fields.sort_unstable();
    fields
}

async fn request(socket: &mut tokio::net::TcpStream, case: &Case) -> anyhow::Result<()> {
    let envelope = read_proxy(socket).await?;
    assert_eq!(envelope["method"], case.method);
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
    let url = envelope["url"].as_str().unwrap();
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    assert_eq!(path, format!("/v1{}", case.path));
    let encoded = if case.method == "POST" {
        assert!(query.is_empty());
        assert_eq!(
            envelope["headers"]["Content-Type"],
            "application/x-www-form-urlencoded"
        );
        String::from_utf8(BASE64.decode(envelope["body_raw"].as_str().unwrap())?)?
    } else {
        assert!(envelope["body_raw"].is_null());
        query.to_owned()
    };
    assert_eq!(
        sorted_fields(&encoded),
        sorted_fields(case.encoded),
        "{} {}",
        case.capability,
        case.method
    );
    Ok(())
}

fn fresh_case() -> Case {
    cases()
        .into_iter()
        .find(|case| case.capability == "get-balance")
        .unwrap()
}

async fn cancellation(partial: bool) -> anyhow::Result<()> {
    for case in cases() {
        let fresh = fresh_case();
        let bytes = compose_agent(
            "stripe",
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
                    other => anyhow::bail!("Stripe wait was not closed: {other:?}"),
                }
                cleaned.notify_one();
                // Task cancellation must not synthesize a refund, subscription
                // change, compensating request or automatic retry.
                let (mut socket, _) = listener.accept().await?;
                request(&mut socket, &fresh).await?;
                respond(&mut socket, json!({"status":200,"headers":{},"body":{"available":[{"amount":1234,"currency":"usd"}]}})).await
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
            json!({"balance":{"available":[{"amount":1234,"currency":"usd"}]}})
        );
    }
    Ok(())
}

#[tokio::test]
async fn stripe_cancel_closes_pending_headers_for_every_capability() -> anyhow::Result<()> {
    cancellation(false).await
}

#[tokio::test]
async fn stripe_cancel_closes_partial_body_for_every_capability() -> anyhow::Result<()> {
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
            "stripe",
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
            anyhow::bail!("Stripe invocation failed: {other:?}");
        }
    };
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(joined) => joined??,
        Err(error) => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("Stripe proxy fixture did not finish: {error}; result={result:?}");
        }
    }
    Ok(result)
}

#[tokio::test]
async fn stripe_success_preserves_objects_pagination_and_subscription_cancel_modes()
-> anyhow::Result<()> {
    let body = json!({"id":"fixture-response","data":[{"id":"fixture-item"}],"has_more":true,"metadata":{"kind":"fixture"},"cancel_at_period_end":true});
    for case in cases() {
        let expected = if case.output_key == "list" {
            json!({"data":body["data"],"has_more":true})
        } else {
            json!({case.output_key:body})
        };
        let bytes = invoke_response(case, json!({"status":200,"headers":{},"body":body}))
            .await?
            .expect("successful Stripe response");
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn stripe_errors_preserve_categories_and_retry_after_for_all_http_methods()
-> anyhow::Result<()> {
    for method in ["GET", "POST", "DELETE"] {
        for (status, code, retryable) in [
            (400, "STRIPE_REQUEST_FAILED", false),
            (401, "STRIPE_UNAUTHORIZED", false),
            (403, "STRIPE_UNAUTHORIZED", false),
            (429, "STRIPE_RATE_LIMITED", true),
            (503, "STRIPE_UPSTREAM_ERROR", true),
        ] {
            let case = cases()
                .into_iter()
                .find(|case| case.method == method)
                .unwrap();
            let path = case.path;
            let error = invoke_response(case, json!({"status":status,"headers":{"retry-after":"3"},"body":{"error":{"message":"fixture rejection"}}})).await?.expect_err("provider error");
            assert_eq!(error.code, code);
            assert_eq!(error.retryable, retryable);
            assert_eq!(
                error.category,
                if retryable { "transient" } else { "permanent" }
            );
            assert_eq!(
                error.retry_after_ms,
                if status == 429 { Some(3000) } else { None }
            );
            let attributes: Value = serde_json::from_str(&error.attributes.unwrap())?;
            assert_eq!(attributes["path"], path);
            assert_eq!(attributes["status_code"], status.to_string());
        }
    }
    let error = invoke_response(
        fresh_case(),
        json!({"status":429,"headers":{"retry-after-ms":"17","retry-after":"3"},"body":{}}),
    )
    .await?
    .expect_err("rate limit");
    assert_eq!(error.retry_after_ms, Some(17));
    Ok(())
}

#[tokio::test]
async fn stripe_transport_response_and_missing_connection_errors_stay_distinct()
-> anyhow::Result<()> {
    for (body_raw, code, retryable) in [
        ("!invalid-base64!", "STRIPE_NETWORK_ERROR", true),
        ("eA==", "STRIPE_RESPONSE_PARSE_ERROR", false),
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
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        invoke_named_agent(
            "stripe",
            CallContext::for_test("fixture-tenant", "http://127.0.0.1:1/unused", "", "", ""),
            "get-balance",
            b"{}".to_vec(),
        ),
    )
    .await??
    .expect_err("missing connection must fail before I/O");
    assert_eq!(error.code, "STRIPE_MISSING_CONNECTION");
    assert_eq!(error.category, "permanent");
    assert!(!error.retryable);
    Ok(())
}
