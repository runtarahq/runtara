// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Outbound HTTP over the workflow API (`POST /v1/http`), through the guest
//! SDK. Connection credentials are resolved by the host from the opaque
//! connection id; nothing secret crosses into the guest. Dropping the async
//! call's future cancels the request.

use runtara_workflow_api::{ConnectionTarget, OutboundDestination, OutboundMeta};
use runtara_workflow_sdk::Outbound;

use crate::{Body, HttpError, HttpResponse, RequestBuilder};

/// The request as the workflow API carries it: metadata and a raw body.
fn encode(request: RequestBuilder) -> Result<(OutboundMeta, Option<Vec<u8>>), HttpError> {
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
        Some(connection_id) => OutboundDestination::Connection(ConnectionTarget {
            connection_id,
            url,
            endpoint: request.endpoint,
            endpoint_ref: request.endpoint_ref,
            ai_provider: request.ai_provider,
            aws_service: request.aws_service,
        }),
        None => OutboundDestination::Public(url),
    };
    Ok((
        OutboundMeta {
            method: request.method,
            destination,
            headers,
            timeout_ms: request
                .timeout
                .map(|timeout| timeout.as_millis().min(u64::MAX as u128) as u64),
            max_response_bytes: request.max_response_bytes,
            has_body: body.is_some(),
        },
        body,
    ))
}

/// The answer as agents see it: every upstream status, and host
/// connection/policy failures with a status, are responses; the rest are
/// transport errors.
fn decode(
    outbound: Result<Outbound, runtara_workflow_sdk::Error>,
) -> Result<HttpResponse, HttpError> {
    match outbound {
        Ok(Outbound::Reply {
            status,
            headers,
            body,
        }) => Ok(HttpResponse {
            status,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value))
                .collect(),
            body,
        }),
        Ok(Outbound::Failed { failure, body }) => match failure.status {
            // Preserve agent-facing status/error classification for host
            // connection/policy failures, without a transport envelope.
            Some(status) => {
                let mut headers = std::collections::HashMap::new();
                if let Some(delay) = failure.retry_after_ms {
                    headers.insert("retry-after".into(), delay.div_ceil(1000).to_string());
                    headers.insert("retry-after-ms".into(), delay.to_string());
                }
                Ok(HttpResponse {
                    status,
                    headers,
                    body,
                })
            }
            None => Err(HttpError::Transport(format!(
                "{}: {}",
                failure.code, failure.message
            ))),
        },
        Err(runtara_workflow_sdk::Error::Api(error)) => Err(HttpError::Transport(format!(
            "{}: {}",
            error.code.as_str(),
            error.message
        ))),
        Err(runtara_workflow_sdk::Error::Transport(message)) => Err(HttpError::Transport(message)),
    }
}

/// Run the request to completion. Agents call this from synchronous
/// capability code inside their `async` invoke, where waiting is allowed.
pub(crate) fn execute(request: RequestBuilder) -> Result<HttpResponse, HttpError> {
    #[cfg(target_arch = "wasm32")]
    return wit_bindgen::block_on(execute_async(request));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = request;
        Err(HttpError::Transport(
            "the workflow API is only reachable from a wasm32 guest".into(),
        ))
    }
}

pub(crate) async fn execute_async(request: RequestBuilder) -> Result<HttpResponse, HttpError> {
    let (meta, body) = encode(request)?;
    decode(runtara_workflow_sdk::http(&meta, body).await)
}

#[cfg(test)]
mod tests {
    use runtara_workflow_api::{ApiError, ErrorCode, OutboundFailure};

    use super::*;

    #[test]
    fn public_signed_url_and_body_presence_are_preserved() -> Result<(), HttpError> {
        let url = "https://provider.invalid/file?sig=a%2Fb%2Bc&x=1&x=2";
        let (meta, empty) = encode(RequestBuilder::new("POST", url).body_bytes(&[]))?;
        assert!(matches!(meta.destination, OutboundDestination::Public(value) if value == url));
        assert_eq!(empty, Some(vec![]));
        let (_, absent) = encode(RequestBuilder::new("GET", url))?;
        assert_eq!(absent, None);
        let (_, binary) = encode(RequestBuilder::new("PUT", url).body_bytes(&[0, 255, 128]))?;
        assert_eq!(binary, Some(vec![0, 255, 128]));
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
        let (meta, body) = encode(request)?;
        let OutboundDestination::Connection(connection) = meta.destination else {
            panic!("connection")
        };
        assert_eq!(connection.connection_id, "opaque-id");
        assert_eq!(connection.url, "/items?q=a%20b");
        assert_eq!(connection.endpoint.as_deref(), Some("files"));
        assert_eq!(connection.endpoint_ref.as_deref(), Some("opaque-ref"));
        assert_eq!(connection.ai_provider.as_deref(), Some("openai"));
        assert_eq!(connection.aws_service.as_deref(), Some("s3"));
        assert_eq!(body.as_deref(), Some(br#"{"text":"hi"}"#.as_slice()));
        assert!(
            meta.headers
                .contains(&("content-type".into(), "application/json".into()))
        );
        assert_eq!(meta.max_response_bytes, Some(1024));
        Ok(())
    }

    #[test]
    fn host_errors_and_upstream_statuses_have_explicit_adaptation() {
        let response = decode(Ok(Outbound::Reply {
            status: 429,
            headers: vec![
                ("Retry-After".into(), "1".into()),
                ("X-Test".into(), "first".into()),
                ("x-test".into(), "last".into()),
            ],
            body: vec![0, 255],
        }))
        .unwrap();
        assert_eq!(response.status, 429);
        assert_eq!(response.body, [0, 255]);
        assert_eq!(response.header("X-Test"), Some("last"));
        let response = decode(Ok(Outbound::Failed {
            failure: OutboundFailure {
                code: "CONNECTION_NOT_FOUND".into(),
                message: "Missing connection".into(),
                status: Some(404),
                retry_after_ms: None,
            },
            body: b"missing".to_vec(),
        }))
        .unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(response.body, b"missing");
        let failed = decode(Ok(Outbound::Failed {
            failure: OutboundFailure {
                code: "HTTP_DEADLINE_EXCEEDED".into(),
                message: "HTTP request timeout".into(),
                status: None,
                retry_after_ms: None,
            },
            body: vec![],
        }));
        assert!(
            matches!(failed, Err(HttpError::Transport(message)) if message.contains("timeout"))
        );
        let refused = decode(Err(runtara_workflow_sdk::Error::Api(ApiError::new(
            ErrorCode::Fenced,
            "lost the lease",
        ))));
        assert!(
            matches!(refused, Err(HttpError::Transport(message)) if message == "fenced: lost the lease")
        );
    }
}
