// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Typed outbound HTTP import. Both lowering modes share the same contract;
//! the async lowering also propagates guest task cancellation to the host.

use crate::{Body, HttpError, HttpResponse, RequestBuilder};

#[allow(warnings)]
mod bindings {
    wit_bindgen::generate!({
        path: "../runtara-workflow-wit/wit/outbound-http",
        world: "outbound-http-client",
        async: false,
    });
}

#[allow(warnings)]
mod async_bindings {
    wit_bindgen::generate!({
        path: "../runtara-workflow-wit/wit/outbound-http",
        world: "outbound-http-client",
        async: true,
        type_section_suffix: "async",
    });
}

// The two generated modules have distinct Rust types for the same WIT records.
macro_rules! encode_request {
    ($request:expr, $contract:path) => {{
        use $contract as contract;
        let request = $request;
        let mut headers = request.headers;
        let body = match request.body {
            Some(Body::Json(value)) => {
                if !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                {
                    headers.push(("content-type".into(), "application/json".into()));
                }
                Some(serde_json::to_vec(&value)?)
            }
            Some(Body::Bytes(bytes)) => Some(bytes),
            None => None,
        };
        let url = crate::build_url_with_query(&request.url, &request.query_params);
        let destination = match request.connection_id {
            Some(connection_id) => {
                contract::Destination::Connection(contract::ConnectionDestination {
                    connection_id,
                    url,
                    endpoint: request.endpoint,
                    endpoint_ref: request.endpoint_ref,
                    ai_provider: request.ai_provider,
                    aws_service: request.aws_service,
                })
            }
            None => contract::Destination::Public(url),
        };
        contract::RequestOptions {
            destination,
            method: request.method,
            headers,
            body,
            timeout_ms: request
                .timeout
                .map(|timeout| timeout.as_millis().min(u64::MAX as u128) as u64),
            max_response_bytes: request.max_response_bytes,
        }
    }};
}

macro_rules! decode_response {
    ($response:expr) => {
        match $response {
            Ok(response) => Ok(HttpResponse {
                status: response.status,
                headers: response
                    .headers
                    .into_iter()
                    .map(|(name, value)| (name.to_ascii_lowercase(), value))
                    .collect(),
                body: response.body,
            }),
            Err(error) => match error.status {
                // Preserve agent-facing status/error classification for host
                // connection/policy failures, without a transport envelope.
                Some(status) => {
                    let mut headers = std::collections::HashMap::new();
                    if let Some(delay) = error.retry_after_ms {
                        headers.insert("retry-after".into(), delay.div_ceil(1000).to_string());
                        headers.insert("retry-after-ms".into(), delay.to_string());
                    }
                    Ok(HttpResponse {
                        status,
                        headers,
                        body: error.body,
                    })
                }
                None => Err(HttpError::Transport(format!(
                    "{}: {}",
                    error.code, error.message
                ))),
            },
        }
    };
}

pub(crate) fn execute(request: RequestBuilder) -> Result<HttpResponse, HttpError> {
    let input = encode_request!(request, bindings::runtara::outbound_http::client);
    decode_response!(bindings::runtara::outbound_http::client::request(&input))
}

pub(crate) async fn execute_async(request: RequestBuilder) -> Result<HttpResponse, HttpError> {
    let input = encode_request!(request, async_bindings::runtara::outbound_http::client);
    decode_response!(async_bindings::runtara::outbound_http::client::request(input).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bindings::runtara::outbound_http::client as contract;

    #[test]
    fn public_signed_url_and_body_presence_are_preserved() -> Result<(), HttpError> {
        let url = "https://provider.invalid/file?sig=a%2Fb%2Bc&x=1&x=2";
        let empty = encode_request!(
            RequestBuilder::new("POST", url).body_bytes(&[]),
            bindings::runtara::outbound_http::client
        );
        assert!(matches!(empty.destination, contract::Destination::Public(value) if value == url));
        assert_eq!(empty.body, Some(vec![]));
        let absent = encode_request!(
            RequestBuilder::new("GET", url),
            bindings::runtara::outbound_http::client
        );
        assert_eq!(absent.body, None);
        let binary = encode_request!(
            RequestBuilder::new("PUT", url).body_bytes(&[0, 255, 128]),
            bindings::runtara::outbound_http::client
        );
        assert_eq!(binary.body, Some(vec![0, 255, 128]));
        Ok(())
    }

    #[test]
    fn json_is_serialized_once_and_controls_are_separate_from_headers() -> Result<(), HttpError> {
        let request = RequestBuilder::new("POST", "/items")
            .connection_id("opaque-id")
            .endpoint("files")
            .endpoint_ref("opaque-ref")
            .ai_provider("openai")
            .aws_service("s3")
            .max_response_bytes(1024)
            .header("X-Runtara-Connection-Id", "forged-id")
            .body_json(&serde_json::json!({"text":"hi"}))
            .query("q", "a b");
        let request = encode_request!(request, bindings::runtara::outbound_http::client);
        let contract::Destination::Connection(connection) = request.destination else {
            panic!("connection")
        };
        assert_eq!(connection.connection_id, "opaque-id");
        assert_eq!(connection.url, "/items?q=a%20b");
        assert_eq!(connection.endpoint.as_deref(), Some("files"));
        assert_eq!(connection.endpoint_ref.as_deref(), Some("opaque-ref"));
        assert_eq!(connection.ai_provider.as_deref(), Some("openai"));
        assert_eq!(connection.aws_service.as_deref(), Some("s3"));
        assert_eq!(
            request.body.as_deref(),
            Some(br#"{"text":"hi"}"#.as_slice())
        );
        assert!(
            request
                .headers
                .contains(&("content-type".into(), "application/json".into()))
        );
        assert_eq!(request.max_response_bytes, Some(1024));
        Ok(())
    }

    #[test]
    fn host_errors_and_upstream_statuses_have_explicit_adaptation() {
        let response: Result<contract::Response, contract::OutboundError> =
            Ok(contract::Response {
                status: 429,
                headers: vec![
                    ("Retry-After".into(), "1".into()),
                    ("X-Test".into(), "first".into()),
                    ("x-test".into(), "last".into()),
                ],
                body: vec![0, 255],
            });
        let response = decode_response!(response).unwrap();
        assert_eq!(response.status, 429);
        assert_eq!(response.body, [0, 255]);
        assert_eq!(response.header("X-Test"), Some("last"));
        let response: Result<contract::Response, contract::OutboundError> =
            Err(contract::OutboundError {
                code: "CONNECTION_NOT_FOUND".into(),
                message: "Missing connection".into(),
                status: Some(404),
                body: b"missing".to_vec(),
                retry_after_ms: None,
            });
        let response = decode_response!(response).unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(response.body, b"missing");
        let response: Result<contract::Response, contract::OutboundError> =
            Err(contract::OutboundError {
                code: "HTTP_DEADLINE_EXCEEDED".into(),
                message: "HTTP request timeout".into(),
                status: None,
                body: vec![],
                retry_after_ms: None,
            });
        assert!(
            matches!(decode_response!(response), Err(HttpError::Transport(message)) if message.contains("timeout"))
        );
    }
}
