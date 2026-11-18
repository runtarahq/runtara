// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The engine's side of the wire: a router that answers the API's calls by
//! asking the engine in this process.
//!
//! Who may connect is not decided here: the listener this router is served on
//! accepts the API role's certificate and no other
//! ([`super::remote::serve_api_role`]).
//!
//! Who a call is for is decided here, and not by the API. A call comes with
//! what its caller presented to the API, and the engine verifies that itself
//! ([`Callers`]): an operation is answered for the tenant the caller acts
//! for and for no other, and a part of the API that has a caller is served
//! to the caller the engine found.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;

use super::wire;
use super::{Engine, Served};
use crate::auth::presented::{Door, Presented};
use crate::auth::{AuthContext, AuthState};

/// Who presented what, as the engine finds it for itself.
#[async_trait::async_trait]
pub trait Callers: Send + Sync {
    /// The caller, or what whoever asked is answered instead.
    async fn verify(&self, presented: &Presented) -> Result<AuthContext, Response>;
}

/// The engine's own verification: what the API's middleware runs, with this
/// process's keys, database and membership store.
pub struct Verification {
    pub api: AuthState,
    pub mcp: AuthState,
}

#[async_trait::async_trait]
impl Callers for Verification {
    async fn verify(&self, presented: &Presented) -> Result<AuthContext, Response> {
        let state = match presented.door {
            Door::Api => &self.api,
            Door::Mcp => &self.mcp,
        };
        // The API key and tenant lookups are the platform's own, as they are
        // in the API's middleware, which runs outside any tenant's scope.
        crate::middleware::auth::verify(state, presented).await
    }
}

/// The largest operation the engine reads. A run's inputs are the bulk of
/// one, and are held to the same size as a request's body.
const OPERATION_LIMIT: usize = 96 * 1024 * 1024;

/// Answers the API role's calls with `engine`.
pub fn router(engine: Arc<dyn Engine>, callers: Arc<dyn Callers>) -> axum::Router {
    axum::Router::new().fallback(move |request: Request| {
        let engine = engine.clone();
        let callers = callers.clone();
        async move { answer(engine, callers, request).await }
    })
}

/// Answers what the API hands intake: the requests of `parts`, as they
/// arrived, and nothing else. Intake answers no operation, and serves no
/// part that has a caller, so it verifies none.
pub fn router_for_intake(engine: Arc<dyn Engine>, parts: &'static [Served]) -> axum::Router {
    axum::Router::new().fallback(move |mut request: Request| {
        let engine = engine.clone();
        async move {
            let path = request.uri().path().to_owned();
            let Some(rest) = path.strip_prefix(wire::SERVED) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let (part, own) = rest.split_once('/').unwrap_or((rest, ""));
            let Some(part) = Served::named(part).filter(|part| parts.contains(part)) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            request.headers_mut().remove(wire::PRESENTED);
            let Some(request) = as_it_arrived(request, own, None) else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            engine.serve(part, request).await
        }
    })
}

/// Answers what anyone sends intake itself, on a listener of intake's own:
/// the requests of `parts` at the paths the API serves them at, and nothing
/// else. What arrives here has passed through no other process, so nothing
/// it says of itself is taken: no caller, and no word of the API's.
pub fn router_for_anyone(engine: Arc<dyn Engine>, parts: &'static [Served]) -> axum::Router {
    axum::Router::new().fallback(move |request: Request| {
        let engine = engine.clone();
        async move {
            // A served tenant's segment is taken out of the path and
            // recorded, as the API's listener does before it routes.
            let mut request = crate::middleware::tenant_path::strip_tenant_segment(request);
            // The path as this router first saw it still has that segment,
            // and is what a part would otherwise be routed by.
            request
                .extensions_mut()
                .remove::<axum::extract::OriginalUri>();
            for said_by_the_api in [wire::PRESENTED, wire::REQUESTED_TENANT, wire::EVENT_SOURCE] {
                request.headers_mut().remove(said_by_the_api);
            }
            request.extensions_mut().remove::<AuthContext>();
            request
                .extensions_mut()
                .remove::<crate::product_events::EventSource>();
            let path = request.uri().path();
            let named = request
                .extensions()
                .get::<crate::middleware::tenant_path::RequestedTenant>()
                .is_some();
            let part = if path.starts_with("/api/runtime/events/") {
                if !named
                    && crate::middleware::tenant_path::is_an_event_of_no_tenant_among_several(path)
                {
                    return StatusCode::NOT_FOUND.into_response();
                }
                Served::Events
            } else if path.starts_with("/api/oauth/") && !named {
                Served::OAuthCallbacks
            } else {
                return StatusCode::NOT_FOUND.into_response();
            };
            if !parts.contains(&part) {
                return StatusCode::NOT_FOUND.into_response();
            }
            engine.serve(part, request).await
        }
    })
}

/// The caller of a call, verified. `Ok(None)` if the call came with nothing
/// presented; `Err` is what is answered instead of the call.
async fn caller_of(
    callers: &dyn Callers,
    headers: &mut HeaderMap,
) -> Result<Option<AuthContext>, Response> {
    let Some(value) = headers.remove(wire::PRESENTED) else {
        return Ok(None);
    };
    let presented: Presented = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok())
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    callers.verify(&presented).await.map(Some)
}

fn refused(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": code, "message": message })),
    )
        .into_response()
}

/// Who a request came from, by the certificate its connection presented. A
/// request that came over no such connection (a test in one process) is the
/// API's.
fn peer_of(request: &Request) -> wire::Peer {
    match request
        .extensions()
        .get::<runtara_executor::tls::PeerName>()
    {
        Some(name) if name.0 == runtara_executor::protocol::INTAKE_NAME => wire::Peer::Intake,
        _ => wire::Peer::Api,
    }
}

async fn answer(engine: Arc<dyn Engine>, callers: Arc<dyn Callers>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let peer = peer_of(&request);
    let mut request = request;
    if let Some(name) = path.strip_prefix(wire::OPERATIONS) {
        if request.method() != Method::POST {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let caller = match caller_of(callers.as_ref(), request.headers_mut()).await {
            Ok(caller) => caller,
            Err(response) => return response,
        };
        let Ok(body) = axum::body::to_bytes(request.into_body(), OPERATION_LIMIT).await else {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        };
        return match wire::answer(engine.as_ref(), name, &body, caller.as_ref(), peer).await {
            None => StatusCode::NOT_FOUND.into_response(),
            Some(Err(wire::Refused::Unreadable(reason))) => {
                (StatusCode::BAD_REQUEST, reason).into_response()
            }
            Some(Err(wire::Refused::NoCaller)) => refused(
                StatusCode::UNAUTHORIZED,
                "NO_CALLER",
                "an operation for a tenant comes with what its caller presented",
            ),
            Some(Err(wire::Refused::NotTheCallers)) => refused(
                StatusCode::FORBIDDEN,
                "NOT_THE_CALLERS_TENANT",
                "the caller does not act for the tenant the operation names",
            ),
            Some(Err(wire::Refused::NotThePeers)) => refused(
                StatusCode::FORBIDDEN,
                "NOT_THIS_PEERS",
                "this operation is not one this peer may ask",
            ),
            Some(Err(wire::Refused::NotPermitted)) => refused(
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "the caller's role or credential does not allow the operation",
            ),
            Some(Ok(answer)) => (
                [(header::CONTENT_TYPE, "application/json")],
                Body::from(answer),
            )
                .into_response(),
        };
    }
    if let Some(rest) = path.strip_prefix(wire::SERVED) {
        // Intake hands the engine nothing as it arrived: it asks operations.
        if peer == wire::Peer::Intake {
            return refused(
                StatusCode::FORBIDDEN,
                "NOT_THIS_PEERS",
                "intake is served no part of the API",
            );
        }
        let (part, own) = rest.split_once('/').unwrap_or((rest, ""));
        let Some(part) = Served::named(part) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        // The engine receives nothing anyone may send. A part with no caller
        // (a webhook, a provider's redirect) is intake's to receive, which
        // asks the engine what it needs of it as an operation; handed over
        // as it arrived, it is refused.
        if !part.has_a_caller() {
            return refused(
                StatusCode::FORBIDDEN,
                "NOT_FOR_THE_ENGINE",
                "what anyone may send is received by intake, not by the engine",
            );
        }
        // A part that has a caller is served to the one the engine found, and
        // to nobody if it found none.
        let caller = match caller_of(callers.as_ref(), request.headers_mut()).await {
            Ok(Some(caller)) => Some(caller),
            Ok(None) => {
                return refused(
                    StatusCode::UNAUTHORIZED,
                    "NO_CALLER",
                    "this part of the API is served to a caller",
                );
            }
            Err(response) => return response,
        };
        let Some(request) = as_it_arrived(request, own, caller) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        return engine.serve(part, request).await;
    }
    StatusCode::NOT_FOUND.into_response()
}

/// The request the API was sent, from the one it handed over: its own path,
/// the caller the engine found, and what the API said of how it arrived put
/// back where this process's handlers look for it.
fn as_it_arrived(
    mut request: Request,
    own_path: &str,
    caller: Option<AuthContext>,
) -> Option<Request> {
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
    // The tenant a path named is the one the caller acts for, which the
    // engine has verified; without a caller it is where a webhook is looked
    // for, as it is when anyone sends one.
    let requested_tenant = match &caller {
        Some(caller) => carried
            .requested_tenant
            .filter(|named| *named == caller.org_id),
        None => carried.requested_tenant,
    };
    if let Some(caller) = caller {
        request.extensions_mut().insert(caller);
    }
    if let Some(tenant) = requested_tenant {
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
    requested_tenant: Option<String>,
    event_source: Option<crate::product_events::EventSource>,
}

/// Takes what the API said of a request out of its headers. `None` if a
/// header that is there cannot be read: a request is not answered as if it
/// said less.
fn carried(headers: &mut HeaderMap) -> Option<Carried> {
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
        requested_tenant,
        event_source,
    })
}
