// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The engine's side of the wire: a router that answers the API's calls by
//! asking the engine in this process.
//!
//! Who may connect is not decided here. The listener this router is served on
//! accepts the API role's certificate and no other
//! ([`super::remote::serve_api_role`]), so what a request says of its caller
//! is the API's word, as it was when both were one process.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;

use super::wire;
use super::{Engine, Served};

/// The largest operation the engine reads. A run's inputs are the bulk of
/// one, and are held to the same size as a request's body.
const OPERATION_LIMIT: usize = 96 * 1024 * 1024;

/// Answers the API role's calls with `engine`.
pub fn router(engine: Arc<dyn Engine>) -> axum::Router {
    axum::Router::new().fallback(move |request: Request| {
        let engine = engine.clone();
        async move { answer(engine, request).await }
    })
}

async fn answer(engine: Arc<dyn Engine>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if let Some(name) = path.strip_prefix(wire::OPERATIONS) {
        if request.method() != Method::POST {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let Ok(body) = axum::body::to_bytes(request.into_body(), OPERATION_LIMIT).await else {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        };
        return match wire::answer(engine.as_ref(), name, &body).await {
            None => StatusCode::NOT_FOUND.into_response(),
            Some(Err(reason)) => (StatusCode::BAD_REQUEST, reason).into_response(),
            Some(Ok(answer)) => (
                [(header::CONTENT_TYPE, "application/json")],
                Body::from(answer),
            )
                .into_response(),
        };
    }
    if let Some(rest) = path.strip_prefix(wire::SERVED) {
        let (part, own) = rest.split_once('/').unwrap_or((rest, ""));
        let Some(part) = Served::named(part) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(request) = as_it_arrived(request, own) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        return engine.serve(part, request).await;
    }
    StatusCode::NOT_FOUND.into_response()
}

/// The request the API was sent, from the one it handed over: its own path,
/// and what the API established about it put back where this process's
/// handlers look for it.
fn as_it_arrived(mut request: Request, own_path: &str) -> Option<Request> {
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    *request.uri_mut() = format!("/{own_path}{query}").parse().ok()?;
    // How this listener routed it here is no part of the request.
    request
        .extensions_mut()
        .remove::<axum::extract::OriginalUri>();

    let carried = carried(request.headers_mut())?;
    if let Some(caller) = carried.caller {
        request.extensions_mut().insert(caller);
    }
    if let Some(tenant) = carried.requested_tenant {
        request
            .extensions_mut()
            .insert(crate::middleware::tenant_path::RequestedTenant(tenant));
    }
    if let Some(source) = carried.event_source {
        request.extensions_mut().insert(source);
    }
    Some(request)
}

struct Carried {
    caller: Option<crate::auth::AuthContext>,
    requested_tenant: Option<String>,
    event_source: Option<crate::product_events::EventSource>,
}

/// Takes what the API established out of the headers. `None` if a header that
/// is there cannot be read: a request is not answered as if it said less.
fn carried(headers: &mut HeaderMap) -> Option<Carried> {
    let caller = match headers.remove(wire::CALLER) {
        None => None,
        Some(value) => {
            let json = base64::engine::general_purpose::STANDARD
                .decode(value.as_bytes())
                .ok()?;
            Some(serde_json::from_slice(&json).ok()?)
        }
    };
    let requested_tenant = match headers.remove(wire::REQUESTED_TENANT) {
        None => None,
        Some(value) => Some(value.to_str().ok()?.to_owned()),
    };
    let event_source = match headers.remove(wire::EVENT_SOURCE) {
        None => None,
        Some(value) => Some(
            serde_json::from_value(serde_json::Value::String(value.to_str().ok()?.to_owned()))
                .ok()?,
        ),
    };
    Some(Carried {
        caller,
        requested_tenant,
        event_source,
    })
}
