//! Explicit native backends for loopback provider fixtures. Test metadata travels
//! in a diagnostic header so fixtures can assert typed request fields while the
//! HTTP wire carries the original method/body and the provider's raw response.
//! Production uses NativeOutboundHttp and never emits this diagnostic header.
#![allow(dead_code)]
use base64::{Engine as _, engine::general_purpose::STANDARD};
use runtara_component_host::outbound_http::{
    self, Destination, OutboundContext, OutboundError, OutboundHttpHost, RequestOptions, Response,
};
use runtara_component_host::{CallContext, HostState};
use serde_json::json;
use std::sync::Arc;

pub struct PublicHttp {
    client: reqwest::Client,
    upstream: Option<String>,
    allow_loopback_connections: bool,
}

impl Default for PublicHttp {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            upstream: None,
            allow_loopback_connections: false,
        }
    }
}

impl PublicHttp {
    /// Explicitly authorize connection fixtures whose URLs already name a local provider.
    pub fn with_loopback_connections() -> Self {
        Self {
            allow_loopback_connections: true,
            ..Default::default()
        }
    }

    pub fn with_upstream(upstream: impl Into<String>) -> Self {
        Self {
            upstream: Some(upstream.into()),
            ..Default::default()
        }
    }
}

/// Recover typed call metadata for fixture assertions, adding the raw wire body.
/// This value is never sent as the HTTP request body.
pub fn captured_request(headers: &str, body: &[u8]) -> serde_json::Value {
    let encoded = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("x-test-outbound"))
        .expect("request used the injected service")
        .1
        .trim();
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();
    assert_eq!(
        headers.split_whitespace().next().unwrap(),
        metadata["method"].as_str().unwrap()
    );
    metadata["body"] = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    metadata["body_raw"] = STANDARD.encode(body).into();
    metadata
}

/// Convert a scripted response into an ordinary provider HTTP response.
pub fn response_bytes(fixture: &serde_json::Value) -> Vec<u8> {
    let status = fixture["status"].as_u64().expect("fixture status");
    let body = if let Some(raw) = fixture["body_raw"].as_str() {
        STANDARD.decode(raw).unwrap()
    } else {
        serde_json::to_vec(&fixture["body"]).unwrap()
    };
    let headers = fixture["headers"]
        .as_object()
        .map(|headers| {
            headers
                .iter()
                .filter(|(name, _)| !name.eq_ignore_ascii_case("content-length"))
                .map(|(name, value)| format!("{name}: {}\r\n", value.as_str().unwrap()))
                .collect::<String>()
        })
        .unwrap_or_default();
    let mut bytes = format!(
        "HTTP/1.1 {status} Fixture\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(&body);
    bytes
}

/// Test-owned identity and explicit upstream routing, never guest environment.
pub struct FixtureContext {
    context: CallContext,
    upstream: Option<String>,
}
impl FixtureContext {
    pub fn public() -> Self {
        Self {
            context: CallContext::for_test("fixture-tenant", ""),
            upstream: None,
        }
    }
    pub fn with_upstream(
        tenant: impl Into<String>,
        upstream: impl Into<String>,
        core: impl Into<String>,
    ) -> Self {
        let upstream = upstream.into();
        Self {
            context: CallContext::for_test(tenant, core),
            upstream: (!upstream.is_empty()).then_some(upstream),
        }
    }
    pub fn into_state(self) -> HostState {
        HostState::new(Arc::new(self.context)).with_outbound_http(Arc::new(PublicHttp {
            upstream: self.upstream,
            ..Default::default()
        }))
    }
}

#[async_trait::async_trait]
impl OutboundHttpHost for PublicHttp {
    async fn request(
        &self,
        context: &OutboundContext,
        request: RequestOptions,
    ) -> Result<Response, OutboundError> {
        assert!(!context.tenant_id.is_empty());
        let (url, connection, endpoint, endpoint_ref, provider, service) = match request.destination
        {
            Destination::Public(url) => (url, None, None, None, None, None),
            Destination::Connection(c) => (
                c.url,
                Some(c.connection_id),
                c.endpoint,
                c.endpoint_ref,
                c.ai_provider,
                c.aws_service,
            ),
        };
        if connection.is_some() && self.upstream.is_none() && !self.allow_loopback_connections {
            return Err(outbound_http::error(
                "FIXTURE_CONNECTION_UNAVAILABLE",
                "Connection fixture needs an explicit upstream",
            ));
        }
        let target = self.upstream.as_deref().unwrap_or(&url);
        let target_url = reqwest::Url::parse(target).expect("fixture target URL");
        assert!(
            matches!(
                target_url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            ),
            "fixture network must stay on loopback"
        );
        let metadata = json!({
            "tenant":context.tenant_id,"instance":context.instance_id,
            "url":url,"connection_id":connection,"endpoint":endpoint,"endpoint_ref":endpoint_ref,
            "ai_provider":provider,"aws_service":service,"method":request.method,
            "headers":request.headers.iter().cloned().collect::<std::collections::HashMap<_,_>>(),
            "timeout_ms":request.timeout_ms,"max_response_bytes":request.max_response_bytes,"body_present":request.body.is_some(),
        });
        let transport_error =
            |_| outbound_http::error("HTTP_TRANSPORT_ERROR", "Fixture upstream request failed");
        let mut builder = self
            .client
            .request(request.method.parse().unwrap(), target)
            .header(
                "x-test-outbound",
                STANDARD.encode(serde_json::to_vec(&metadata).unwrap()),
            );
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let mut response = builder.send().await.map_err(transport_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if body.len().saturating_add(chunk.len()) > outbound_http::MAX_RESPONSE_BYTES {
                return Err(outbound_http::error(
                    "HTTP_TOO_LARGE",
                    "Fixture response exceeds byte limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}
