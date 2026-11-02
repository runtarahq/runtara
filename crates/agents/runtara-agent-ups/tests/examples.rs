// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! UPS publishes no response examples in its specs, so these responses are
//! written in the shape UPS returns them; each must deserialize into the
//! generated output type and survive a round trip without losing a field.
//! UPS wraps request bodies (`{"ShipmentRequest": {...}}`); the wrapped fields
//! are the capability inputs, and each input builds the exact request the
//! capability sends. The connection sends `transId` and `transactionSrc`, and
//! the API version is pinned in the path, so neither is an input.

use runtara_agent_ups::{
    GetSingleTrackResponseUsingGetInput, LabelRecoveryInput, LabelrecoveryResponseWrapper,
    ShipResponseWrapper, ShipmentInput, TrackApiResponse, VoidShipmentInput,
    VoidshipmentResponseWrapper,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

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

fn check<T: DeserializeOwned + Serialize>(name: &str, original: Value) {
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("{name}: response does not deserialize: {e}"));
    let round_tripped = serde_json::to_value(&typed).unwrap();
    let mut lost = Vec::new();
    lost_paths(&original, &round_tripped, name, &mut lost);
    assert!(
        lost.is_empty(),
        "{name}: fields dropped by the typed output: {lost:?}"
    );
}

#[test]
fn track_response() {
    check::<TrackApiResponse>(
        "Track",
        json!({ "trackResponse": { "shipment": [{
            "inquiryNumber": "1Z023E2X0214323462",
            "package": [{
                "trackingNumber": "1Z023E2X0214323462",
                "deliveryDate": [{ "type": "DEL", "date": "20230104" }],
                "deliveryTime": { "startTime": "", "endTime": "130000", "type": "DEL" },
                "activity": [{
                    "location": { "address": {
                        "city": "ATLANTA", "stateProvince": "GA", "countryCode": "US",
                        "postalCode": "30303", "country": "US"
                    }, "slic": "3021" },
                    "status": {
                        "type": "D", "description": "DELIVERED", "code": "KB",
                        "statusCode": "011"
                    },
                    "date": "20230104",
                    "time": "130000",
                    "gmtDate": "20230104",
                    "gmtOffset": "-05:00",
                    "gmtTime": "18:00:00"
                }],
                "currentStatus": { "description": "Delivered", "code": "011" },
                "packageCount": 1,
                "weight": { "unitOfMeasurement": "LBS", "weight": "1.00" },
                "service": { "code": "518", "levelCode": "003", "description": "UPS Ground" },
                "referenceNumber": [{ "type": "SHIPMENT", "number": "PO-1234" }]
            }],
            "userRelation": ["SHIPPER"],
            "warnings": [{ "code": "TW0001", "message": "Tracking Information Not Found" }]
        }]}}),
    );
}

#[test]
fn shipment_response() {
    check::<ShipResponseWrapper>(
        "Shipment",
        json!({ "ShipmentResponse": {
            "Response": {
                "ResponseStatus": { "Code": "1", "Description": "Success" },
                "Alert": [{ "Code": "129001", "Description": "Additional Handling has automatically been set on Package 1." }],
                "TransactionReference": { "CustomerContext": "order-1001" }
            },
            "ShipmentResults": {
                "ShipmentCharges": {
                    "TransportationCharges": { "CurrencyCode": "USD", "MonetaryValue": "13.50" },
                    "ServiceOptionsCharges": { "CurrencyCode": "USD", "MonetaryValue": "0.00" },
                    "TotalCharges": { "CurrencyCode": "USD", "MonetaryValue": "13.50" }
                },
                "BillingWeight": {
                    "UnitOfMeasurement": { "Code": "LBS", "Description": "Pounds" },
                    "Weight": "2.0"
                },
                "ShipmentIdentificationNumber": "1ZXXXXXXXXXXXXXXXX",
                "PackageResults": [{
                    "TrackingNumber": "1ZXXXXXXXXXXXXXXXX",
                    "ServiceOptionsCharges": { "CurrencyCode": "USD", "MonetaryValue": "0.00" },
                    "ShippingLabel": {
                        "ImageFormat": { "Code": "GIF", "Description": "GIF" },
                        "GraphicImage": "R0lGODlhAQABAAAAACw="
                    }
                }]
            }
        }}),
    );
}

#[test]
fn void_shipment_response() {
    check::<VoidshipmentResponseWrapper>(
        "VoidShipment",
        json!({ "VoidShipmentResponse": {
            "Response": {
                "ResponseStatus": { "Code": "1", "Description": "Success" },
                "TransactionReference": { "CustomerContext": "order-1001" }
            },
            "SummaryResult": { "Status": { "Code": "1", "Description": "Voided" } }
        }}),
    );
}

#[test]
fn label_recovery_response() {
    check::<LabelrecoveryResponseWrapper>(
        "LabelRecovery",
        json!({ "LabelRecoveryResponse": {
            "Response": { "ResponseStatus": { "Code": "1", "Description": "Success" } },
            "ShipmentIdentificationNumber": "1ZXXXXXXXXXXXXXXXX",
            "LabelResults": [{
                "TrackingNumber": "1ZXXXXXXXXXXXXXXXX",
                "LabelImage": {
                    "LabelImageFormat": { "Code": "GIF" },
                    "GraphicImage": "R0lGODlhAQABAAAAACw="
                }
            }]
        }}),
    );
}

#[test]
fn metadata_lists_every_operation() {
    // openapi-generator groups operations by tag, then orders them by operationId.
    let info = runtara_agent_ups::agent_info();
    let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "label-recovery",
            "shipment",
            "void-shipment",
            "get-single-track-response-using-get",
        ]
    );
    let writes: Vec<&str> = info
        .capabilities
        .iter()
        .filter(|c| c.has_side_effects)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(writes, ["label-recovery", "shipment", "void-shipment"]);
}

#[test]
fn agent_uses_the_ups_oauth_connection() {
    let info = runtara_agent_ups::agent_info();
    assert_eq!(info.integration_ids, ["ups_client_credentials"]);
}

fn input_names(capability: &str) -> Vec<String> {
    let info = runtara_agent_ups::agent_info();
    info.capabilities
        .iter()
        .find(|c| c.id == capability)
        .unwrap_or_else(|| panic!("{capability} missing"))
        .inputs
        .iter()
        .map(|f| f.name.clone())
        .collect()
}

#[test]
fn connection_headers_and_the_api_version_are_not_inputs() {
    let info = runtara_agent_ups::agent_info();
    for capability in &info.capabilities {
        let inputs = input_names(&capability.id);
        for name in ["transId", "transactionSrc", "version"] {
            assert!(
                !inputs.iter().any(|i| i == name),
                "{}: {inputs:?}",
                capability.id
            );
        }
    }
}

#[test]
fn enveloped_body_fields_are_capability_inputs() {
    let shipment = input_names("shipment");
    for name in ["Request", "Shipment", "LabelSpecification"] {
        assert!(shipment.iter().any(|i| i == name), "{shipment:?}");
    }
    assert!(
        !shipment.iter().any(|i| i == "ShipmentRequest"),
        "{shipment:?}"
    );
    let recovery = input_names("label-recovery");
    for name in ["Request", "TrackingNumber", "LabelSpecification"] {
        assert!(recovery.iter().any(|i| i == name), "{recovery:?}");
    }
}

#[test]
fn track_sends_the_number_in_the_path() {
    let input: GetSingleTrackResponseUsingGetInput = serde_json::from_value(json!({
        "inquiryNumber": "1Z023E2X0214323462",
        "returnMilestones": "true",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/api/track/v1/details/1Z023E2X0214323462");
    assert_eq!(
        request.query_pairs(),
        [("returnMilestones".to_string(), "true".to_string())]
    );
    assert_eq!(request.body_json(), None);
}

#[test]
fn shipment_wraps_its_fields_in_the_shipment_request() {
    let shipment = json!({
        "Description": "Order 1001",
        "Shipper": {
            "Name": "Acme",
            "ShipperNumber": "A1B2C3",
            "Address": { "AddressLine": ["1 Main St"], "City": "Atlanta",
                         "StateProvinceCode": "GA", "PostalCode": "30303", "CountryCode": "US" }
        },
        "ShipTo": {
            "Name": "Jane Doe",
            "Address": { "AddressLine": ["2 Elm St"], "City": "Timonium",
                         "StateProvinceCode": "MD", "PostalCode": "21093", "CountryCode": "US" }
        },
        "PaymentInformation": { "ShipmentCharge": [{ "Type": "01", "BillShipper": { "AccountNumber": "A1B2C3" } }] },
        "Service": { "Code": "03" },
        "Package": [{
            "Packaging": { "Code": "02" },
            "PackageWeight": { "UnitOfMeasurement": { "Code": "LBS" }, "Weight": "2" }
        }]
    });
    let input: ShipmentInput = serde_json::from_value(json!({
        "Request": { "RequestOption": "nonvalidate" },
        "Shipment": shipment,
        "LabelSpecification": { "LabelImageFormat": { "Code": "GIF" } },
        "additionaladdressvalidation": "city",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/shipments/v2409/ship");
    assert_eq!(
        request.query_pairs(),
        [(
            "additionaladdressvalidation".to_string(),
            "city".to_string()
        )]
    );
    assert_eq!(
        request.body_json(),
        Some(json!({ "ShipmentRequest": {
            "LabelSpecification": { "LabelImageFormat": { "Code": "GIF" } },
            "Request": { "RequestOption": "nonvalidate" },
            "Shipment": shipment,
        }}))
    );
}

#[test]
fn void_shipment_deletes_by_shipment_id() {
    let input: VoidShipmentInput = serde_json::from_value(json!({
        "shipmentidentificationnumber": "1ZISDE016691676846",
        "trackingnumber": "1ZISDE016691609089",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "DELETE");
    assert_eq!(
        request.path(),
        "/api/shipments/v2409/void/cancel/1ZISDE016691676846"
    );
    assert_eq!(
        request.query_pairs(),
        [(
            "trackingnumber".to_string(),
            "1ZISDE016691609089".to_string()
        )]
    );
    assert_eq!(request.body_json(), None);
}

#[test]
fn label_recovery_needs_only_a_tracking_number() {
    // The spec also requires TrackingNumbers (Roadie) and ReferenceValues;
    // connector.yaml makes them optional, as UPS's own descriptions say.
    let input: LabelRecoveryInput = serde_json::from_value(json!({
        "Request": { "RequestOption": "Non_Validate" },
        "TrackingNumber": "1Z12345E8791315509",
        "LabelSpecification": { "LabelImageFormat": { "Code": "GIF" } },
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/labels/v1/recovery");
    assert_eq!(
        request.body_json(),
        Some(json!({ "LabelRecoveryRequest": {
            "LabelSpecification": { "LabelImageFormat": { "Code": "GIF" } },
            "Request": { "RequestOption": "Non_Validate" },
            "TrackingNumber": "1Z12345E8791315509",
        }}))
    );
}

#[test]
fn inputs_carry_labels() {
    let info = runtara_agent_ups::agent_info();
    let label = |capability: &str, name: &str| {
        info.capabilities
            .iter()
            .find(|c| c.id == capability)
            .and_then(|c| c.inputs.iter().find(|f| f.name == name))
            .and_then(|f| f.display_name.clone())
            .unwrap_or_else(|| panic!("{capability}.{name} has no label"))
    };
    assert_eq!(
        label("void-shipment", "shipmentidentificationnumber"),
        "Shipment Identification Number"
    );
    assert_eq!(label("void-shipment", "trackingnumber"), "Tracking Number");
    assert_eq!(
        label("get-single-track-response-using-get", "inquiryNumber"),
        "Inquiry Number"
    );
}
