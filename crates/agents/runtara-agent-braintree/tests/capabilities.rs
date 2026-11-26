// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The generated GraphQL capabilities: their metadata, the request each input
//! builds (the operation and its variables), and that an answer of
//! Braintree's shape reads into the output types without losing a field.
//! Braintree's schema has no response examples, so the answers here are
//! written from its reference and the sandbox's answers.

use std::collections::HashMap;

use runtara_agent_braintree::{
    AcceptDisputeInput, AcceptDisputeResult, AddPackageTrackingInput, AddPackageTrackingResult,
    AuthorizePaymentMethodInput, AuthorizePaymentMethodResult, CaptureTransactionInput,
    CaptureTransactionResult, ChargePaymentMethodInput, ChargePaymentMethodResult,
    CreateCustomerInput, CreateCustomerResult, DeletePaymentMethodFromVaultInput,
    DeletePaymentMethodFromVaultResult, GetTransactionInput, GetTransactionResult, PingInput,
    RefundTransactionInput, RefundTransactionResult, ReverseTransactionInput,
    ReverseTransactionResult, SandboxSettleTransactionInput, SandboxSettleTransactionResult,
    SearchCustomersInput, SearchCustomersResult, SearchDisputesInput, SearchDisputesResult,
    SearchTransactionsInput, SearchTransactionsResult, VaultPaymentMethodInput,
    VaultPaymentMethodResult, kit,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const ENDPOINT: &str = "/graphql";

fn answer(body: Value) -> runtara_http::HttpResponse {
    runtara_http::HttpResponse {
        status: 200,
        body: serde_json::to_vec(&body).unwrap(),
        headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
    }
}

/// Paths present in `original` but missing after the typed round trip.
fn lost_paths(original: &Value, round_tripped: &Value, at: &str, lost: &mut Vec<String>) {
    match (original, round_tripped) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let path = format!("{at}.{key}");
                match b.get(key) {
                    Some(other) => lost_paths(value, other, &path, lost),
                    None if !value.is_null() => lost.push(path),
                    None => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                lost_paths(x, y, &format!("{at}[{i}]"), lost);
            }
        }
        _ => {}
    }
}

fn input<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

/// `data.<field>` of `body` reads into `T` through the capability's request,
/// and writes back without dropping a field.
fn round_trip<T: DeserializeOwned + Serialize>(request: kit::Request, field: &str, value: Value) {
    let call = request.graphql_call().unwrap();
    assert_eq!(call.field, field);
    let body = json!({"data": {field: value}, "extensions": {"requestId": "4b2e…"}});
    let typed: T = kit::decode_graphql(&answer(body.clone()), call, ENDPOINT, "BRAINTREE")
        .unwrap_or_else(|e| panic!("{field}: the answer does not read: {}", e.message));
    let mut lost = Vec::new();
    lost_paths(
        &body["data"][field],
        &serde_json::to_value(&typed).unwrap(),
        field,
        &mut lost,
    );
    assert!(lost.is_empty(), "{field}: fields dropped: {lost:?}");
}

/// A transaction as the operations' `TransactionFields` select it.
fn transaction(status: &str) -> Value {
    json!({
        "id": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo",
        "legacyId": "kewsajrj",
        "status": status,
        "orderId": "SO-1001",
        "createdAt": "2026-11-26T12:00:00.000000Z",
        "merchantAccountId": "syncmyorders",
        "amount": {"value": "10.00", "currencyCode": "PLN"},
        "processorAuthorizationResponse": {
            "legacyCode": "1000",
            "message": "Approved",
            "cvvResponse": "MATCHES",
            "avsPostalCodeResponse": "MATCHES",
            "avsStreetAddressResponse": "NOT_VERIFIED",
        },
        "paymentMethodSnapshot": {
            "__typename": "CreditCardDetails",
            "brandCode": "VISA",
            "last4": "1881",
            "expirationMonth": "12",
            "expirationYear": "2030",
        },
        "customer": null,
    })
}

#[test]
fn metadata_lists_every_operation() {
    let info = runtara_agent_braintree::agent_info();
    assert_eq!(info.id, "braintree");
    assert_eq!(info.integration_ids, ["braintree_api_keys"]);
    // openapi-generator orders operations by operationId.
    let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "accept-dispute",
            "add-package-tracking",
            "authorize-payment-method",
            "capture-transaction",
            "charge-payment-method",
            "create-customer",
            "delete-payment-method-from-vault",
            "get-transaction",
            "ping",
            "refund-transaction",
            "reverse-transaction",
            "sandbox-settle-transaction",
            "search-customers",
            "search-disputes",
            "search-transactions",
            "vault-payment-method",
        ]
    );
    // Braintree limits requests: every step gets a rate-limited one's retries.
    assert!(info.capabilities.iter().all(|c| c.rate_limited));
    // Queries are POSTs too, but only mutations change anything.
    let reads: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| !c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        reads,
        [
            "get-transaction",
            "ping",
            "search-customers",
            "search-disputes",
            "search-transactions",
        ]
    );
}

#[test]
fn variables_are_the_inputs() {
    let info = runtara_agent_braintree::agent_info();
    let inputs = |id: &str| -> Vec<(String, bool)> {
        info.capabilities
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .inputs
            .iter()
            .map(|f| (f.name.clone(), f.required))
            .collect()
    };
    let charge = inputs("charge-payment-method");
    for (name, required) in [
        ("paymentMethodId", true),
        ("amount", true),
        ("orderId", false),
        ("merchantAccountId", false),
        ("apiRequestKey", false),
    ] {
        assert!(
            charge.contains(&(name.into(), required)),
            "{name}: {charge:?}"
        );
    }
    assert_eq!(charge.len(), 5, "{charge:?}");
    assert_eq!(inputs("ping"), []);
    let first = info
        .capabilities
        .iter()
        .find(|c| c.id == "search-transactions")
        .and_then(|c| c.inputs.iter().find(|f| f.name == "first"))
        .unwrap();
    assert_eq!(first.default_value, Some(json!(50)));
    let key = info
        .capabilities
        .iter()
        .find(|c| c.id == "charge-payment-method")
        .and_then(|c| c.inputs.iter().find(|f| f.name == "apiRequestKey"))
        .unwrap();
    assert_eq!(key.display_name.as_deref(), Some("Idempotency key"));
}

#[test]
fn a_charge_sends_its_operation_and_variables() {
    let request = input::<ChargePaymentMethodInput>(json!({
        "paymentMethodId": "fake-valid-nonce",
        "amount": "10.00",
        "orderId": "SO-1001",
        "apiRequestKey": "so-1001-charge",
    }))
    .request()
    .unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), ENDPOINT);
    let call = request.graphql_call().unwrap();
    assert_eq!((call.field, call.read), ("chargePaymentMethod", false));
    let body = request.body_json().unwrap();
    let document = body["query"].as_str().unwrap();
    assert!(
        document.starts_with("mutation chargePaymentMethod($paymentMethodId: ID!, $amount: Amount!, $orderId: String, $merchantAccountId: ID, $apiRequestKey: String)"),
        "{document}"
    );
    // The fragment travels with the operation.
    assert!(
        document.contains("fragment TransactionFields on Transaction"),
        "{document}"
    );
    // Unset inputs are left out: Braintree uses the default merchant account.
    assert_eq!(
        body["variables"],
        json!({
            "paymentMethodId": "fake-valid-nonce",
            "amount": "10.00",
            "orderId": "SO-1001",
            "apiRequestKey": "so-1001-charge",
        })
    );
}

#[test]
fn a_search_sends_only_the_filters_given() {
    let request = input::<SearchTransactionsInput>(json!({"orderId": "SO-1001"}))
        .request()
        .unwrap();
    let call = request.graphql_call().unwrap();
    assert_eq!((call.field, call.read), ("transactions", true));
    assert_eq!(
        request.body_json().unwrap()["variables"],
        json!({"orderId": "SO-1001"})
    );
    let request = input::<SearchTransactionsInput>(json!({
        "statuses": ["SETTLED", "SETTLING"],
        "first": 10,
        "after": "Y3Vyc29y",
    }))
    .request()
    .unwrap();
    assert_eq!(
        request.body_json().unwrap()["variables"],
        json!({"statuses": ["SETTLED", "SETTLING"], "first": 10, "after": "Y3Vyc29y"})
    );
}

#[test]
fn ping_answers_pong() {
    let request = input::<PingInput>(json!({})).request().unwrap();
    let call = request.graphql_call().unwrap();
    assert_eq!((call.field, call.read), ("ping", true));
    let pong: String = kit::decode_graphql(
        &answer(json!({"data": {"ping": "pong"}})),
        call,
        ENDPOINT,
        "BRAINTREE",
    )
    .unwrap();
    assert_eq!(pong, "pong");
}

#[test]
fn transactions_read_into_their_types() {
    let get = input::<GetTransactionInput>(json!({"id": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo"}))
        .request()
        .unwrap();
    let mut node = transaction("SETTLED");
    node["statusHistory"] = json!([
        {"status": "AUTHORIZED", "timestamp": "2026-11-26T12:00:00.000000Z",
         "amount": {"value": "10.00", "currencyCode": "PLN"}},
        {"status": "SETTLED", "timestamp": "2026-11-26T12:01:00.000000Z",
         "amount": {"value": "10.00", "currencyCode": "PLN"}},
    ]);
    node["refunds"] = json!([{
        "id": "cmVmdW5kXzE", "status": "SUBMITTED_FOR_SETTLEMENT",
        "createdAt": "2026-11-26T12:02:00.000000Z",
        "amount": {"value": "4.00", "currencyCode": "PLN"},
    }]);
    node["customer"] = json!({"id": "Y3VzdG9tZXJfMQ", "email": "buyer@example.com"});
    round_trip::<GetTransactionResult>(get, "node", node);

    let search = input::<SearchTransactionsInput>(json!({}))
        .request()
        .unwrap();
    let mut paypal = transaction("SETTLING");
    paypal["paymentMethodSnapshot"] = json!({
        "__typename": "PayPalTransactionDetails",
        "authorizationId": null,
        "captureId": "5TY05013RG002845M",
        "payer": {"email": "payer@example.com", "payerId": "PAYER123"},
    });
    let mut card = transaction("SUBMITTED_FOR_SETTLEMENT");
    card["paymentMethodSnapshot"] = json!({
        "__typename": "CreditCardTransactionDetails",
        "creditCard": {"brandCode": "MASTERCARD", "last4": "4444",
                       "expirationMonth": "01", "expirationYear": "2031"},
    });
    round_trip::<SearchTransactionsResult>(
        search,
        "transactions",
        json!({
            "pageInfo": {"hasNextPage": true, "endCursor": "Y3Vyc29y"},
            "edges": [{"node": paypal}, {"node": card}],
        }),
    );
}

#[test]
fn payments_read_into_their_types() {
    let charge = input::<ChargePaymentMethodInput>(json!({
        "paymentMethodId": "fake-valid-nonce", "amount": "10.00",
    }))
    .request()
    .unwrap();
    round_trip::<ChargePaymentMethodResult>(
        charge,
        "chargePaymentMethod",
        json!({"transaction": transaction("SUBMITTED_FOR_SETTLEMENT")}),
    );
    let authorize = input::<AuthorizePaymentMethodInput>(json!({
        "paymentMethodId": "fake-valid-nonce", "amount": "10.00",
    }))
    .request()
    .unwrap();
    round_trip::<AuthorizePaymentMethodResult>(
        authorize,
        "authorizePaymentMethod",
        json!({"transaction": transaction("AUTHORIZED")}),
    );
    let capture = input::<CaptureTransactionInput>(json!({
        "transactionId": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "amount": "6.00",
    }))
    .request()
    .unwrap();
    assert_eq!(
        capture.body_json().unwrap()["variables"],
        json!({"transactionId": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "amount": "6.00"})
    );
    round_trip::<CaptureTransactionResult>(
        capture,
        "captureTransaction",
        json!({"transaction": transaction("SUBMITTED_FOR_SETTLEMENT")}),
    );
    let refund = input::<RefundTransactionInput>(json!({
        "transactionId": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "amount": "4.00",
    }))
    .request()
    .unwrap();
    round_trip::<RefundTransactionResult>(
        refund,
        "refundTransaction",
        json!({"refund": {
            "id": "cmVmdW5kXzE", "legacyId": "refund1", "status": "SUBMITTED_FOR_SETTLEMENT",
            "orderId": "SO-1001", "createdAt": "2026-11-26T12:02:00.000000Z",
            "amount": {"value": "4.00", "currencyCode": "PLN"},
            "processorAuthorizationResponse": {"legacyCode": "1002", "message": "Processed"},
            "refundedTransaction": {"id": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "status": "SETTLED"},
        }}),
    );
    for reversal in [
        json!({"__typename": "Transaction", "id": "dHJhbnNhY3Rpb25fMg", "legacyId": "t2",
               "status": "VOIDED", "amount": {"value": "10.00", "currencyCode": "PLN"}}),
        json!({"__typename": "Refund", "id": "cmVmdW5kXzI", "legacyId": "r2",
               "status": "SUBMITTED_FOR_SETTLEMENT",
               "amount": {"value": "10.00", "currencyCode": "PLN"},
               "processorAuthorizationResponse": {"legacyCode": "1002", "message": "Processed"}}),
    ] {
        let reverse =
            input::<ReverseTransactionInput>(json!({"transactionId": "dHJhbnNhY3Rpb25fMg"}))
                .request()
                .unwrap();
        round_trip::<ReverseTransactionResult>(
            reverse,
            "reverseTransaction",
            json!({ "reversal": reversal }),
        );
    }
    let settle = input::<SandboxSettleTransactionInput>(json!({
        "transactionId": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo",
    }))
    .request()
    .unwrap();
    round_trip::<SandboxSettleTransactionResult>(
        settle,
        "sandboxSettleTransaction",
        json!({"transaction": {"id": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "status": "SETTLED"}}),
    );
    let tracking = input::<AddPackageTrackingInput>(json!({
        "transactionId": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo",
        "trackingNumber": "1Z999AA10123456784",
        "carrier": "UPS",
    }))
    .request()
    .unwrap();
    round_trip::<AddPackageTrackingResult>(
        tracking,
        "createTransactionPackageTracking",
        json!({
            "packageTracker": {"id": "cGFja2FnZQ", "trackingNumber": "1Z999AA10123456784",
                               "carrier": "UPS", "paypalTrackerId": "TRK-1"},
            "transaction": {"id": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "status": "SETTLED"},
        }),
    );
}

#[test]
fn customers_the_vault_and_disputes_read_into_their_types() {
    let create = input::<CreateCustomerInput>(json!({
        "email": "buyer@example.com", "firstName": "Ada",
    }))
    .request()
    .unwrap();
    assert_eq!(
        create.body_json().unwrap()["variables"],
        json!({"email": "buyer@example.com", "firstName": "Ada"})
    );
    let customer = json!({
        "id": "Y3VzdG9tZXJfMQ", "legacyId": "1", "email": "buyer@example.com",
        "firstName": "Ada", "lastName": "Lovelace", "company": "SyncMyOrders",
        "phoneNumber": "+48 600 000 000", "createdAt": "2026-11-26T12:00:00.000000Z",
    });
    round_trip::<CreateCustomerResult>(
        create,
        "createCustomer",
        json!({ "customer": customer.clone() }),
    );
    let search = input::<SearchCustomersInput>(json!({"email": "buyer@example.com"}))
        .request()
        .unwrap();
    let mut found = customer;
    found["defaultPaymentMethod"] = json!({"id": "cGF5bWVudG1ldGhvZF8x"});
    round_trip::<SearchCustomersResult>(
        search,
        "customers",
        json!({
            "pageInfo": {"hasNextPage": false, "endCursor": null},
            "edges": [{"node": found}],
        }),
    );
    let vault = input::<VaultPaymentMethodInput>(json!({
        "paymentMethodId": "fake-valid-nonce", "customerId": "Y3VzdG9tZXJfMQ",
    }))
    .request()
    .unwrap();
    round_trip::<VaultPaymentMethodResult>(
        vault,
        "vaultPaymentMethod",
        json!({"paymentMethod": {
            "id": "cGF5bWVudG1ldGhvZF8x", "legacyId": "abc123", "usage": "MULTI_USE",
            "createdAt": "2026-11-26T12:00:00.000000Z", "customer": {"id": "Y3VzdG9tZXJfMQ"},
        },
        "verification": {
            "id": "dmVyaWZpY2F0aW9uXzE", "status": "VERIFIED", "gatewayRejectionReason": null,
            "processorResponse": {"legacyCode": "1000", "message": "Approved"},
        }}),
    );
    let delete = input::<DeletePaymentMethodFromVaultInput>(json!({
        "paymentMethodId": "cGF5bWVudG1ldGhvZF8x",
    }))
    .request()
    .unwrap();
    round_trip::<DeletePaymentMethodFromVaultResult>(
        delete,
        "deletePaymentMethodFromVault",
        json!({"clientMutationId": null}),
    );
    let disputes = input::<SearchDisputesInput>(json!({"statuses": ["OPEN"]}))
        .request()
        .unwrap();
    round_trip::<SearchDisputesResult>(
        disputes,
        "disputes",
        json!({
            "pageInfo": {"hasNextPage": false, "endCursor": null},
            "edges": [{"node": {
                "id": "ZGlzcHV0ZV8x", "legacyId": "d1", "status": "OPEN", "type": "CHARGEBACK",
                "caseNumber": "CB123", "referenceNumber": "REF1", "receivedDate": "2026-11-20",
                "replyByDate": "2026-12-04", "createdAt": "2026-11-20T09:00:00.000000Z",
                "amountDisputed": {"value": "10.00", "currencyCode": "PLN"},
                "transaction": {"id": "dHJhbnNhY3Rpb25fa2V3c2Fqcmo", "orderId": "SO-1001"},
            }}],
        }),
    );
    let accept = input::<AcceptDisputeInput>(json!({"disputeId": "ZGlzcHV0ZV8x"}))
        .request()
        .unwrap();
    round_trip::<AcceptDisputeResult>(
        accept,
        "acceptDispute",
        json!({"dispute": {"id": "ZGlzcHV0ZV8x", "legacyId": "d1", "status": "ACCEPTED"}}),
    );
}
