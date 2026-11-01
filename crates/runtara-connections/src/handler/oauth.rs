//! OAuth2 Authorization Code flow handlers
//!
//! Endpoints:
//! - `GET /api/runtime/connections/{id}/oauth/authorize` (JWT-protected) — starts a
//!   flow bound to the browser by a cookie, and answers with the authorization URL
//! - `GET /api/oauth/callback` (public) — the provider's redirect, for connections
//!   whose redirect URI names no tenant: the tenant comes from the flow
//! - `GET /api/oauth/{tenant_id}/callback` (public) — the same, for connections
//!   registered with their tenant's own redirect URI

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_TYPE, COOKIE, LOCATION, REFERRER_POLICY, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::config::ConnectionsState;
use crate::service::oauth::{OAuthError, OAuthService, StartBinding};
use crate::service::oauth_flow;

// ============================================================================
// Authorize endpoint (JWT-protected)
// ============================================================================

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthAuthorizeResponse {
    pub success: bool,
    pub authorization_url: String,
    /// The redirect URI the provider will send the user to: the one to
    /// register with the provider for this connection.
    pub redirect_uri: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    success: bool,
    error: String,
}

/// Generate an OAuth2 authorization URL for a connection.
///
/// The frontend should open this URL in a popup window, in the same browser:
/// the response sets a cookie, for this flow alone, that the callback needs.
/// After user consent, the provider redirects to the connection's redirect URI.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/oauth/authorize",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "OAuth2 authorization URL generated; sets the flow's cookie", body = OAuthAuthorizeResponse),
        (status = 400, description = "Integration does not support OAuth or missing required parameter", body = crate::types::ErrorResponse),
        (status = 404, description = "Connection not found", body = crate::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::types::ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn authorize_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    caller: Option<axum::Extension<crate::tenant::Caller>>,
    State(state): State<ConnectionsState>,
    Path(id): Path<String>,
) -> Result<Response, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let events = state.connection_events.clone();
    let service = OAuthService::from_state(&state);
    // Without browser binding (a console on another origin than the
    // callback), the flow waits for a member's confirmation instead.
    let binding = if state.oauth.browser_binding {
        StartBinding::Browser
    } else {
        StartBinding::Confirm {
            started_by: started_by(caller.as_deref()),
        }
    };

    let flow = service
        .start(&id, &tenant_id, binding)
        .await
        .map_err(error_response)?;
    crate::events::emit(
        &events,
        Some(&tenant_id),
        crate::events::ConnectionLifecycleEvent::OAuthStarted {
            connection_id: id.clone(),
        },
    );
    let mut response = Json(OAuthAuthorizeResponse {
        success: true,
        authorization_url: flow.authorization_url,
        redirect_uri: flow.redirect_uri,
    })
    .into_response();
    if let Some(nonce) = &flow.browser_nonce {
        set_cookie(
            response.headers_mut(),
            &oauth_flow::binding_cookie(&flow.state, nonce, service.secure_cookies()),
        );
    }
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The caller as recorded on a flow it starts.
fn started_by(caller: Option<&crate::tenant::Caller>) -> String {
    caller
        .map(crate::tenant::Caller::label)
        .unwrap_or_else(|| "unknown".to_string())
}

/// An [`OAuthError`] as the API answers it.
fn error_response(error: OAuthError) -> (StatusCode, Json<ErrorResponse>) {
    let (status, message) = match error {
        OAuthError::ConnectionNotFound => {
            (StatusCode::NOT_FOUND, "Connection not found".to_string())
        }
        OAuthError::NotOAuthIntegration(id) => (
            StatusCode::BAD_REQUEST,
            format!("Integration '{}' does not support OAuth", id),
        ),
        OAuthError::MissingParameter(p) => (
            StatusCode::BAD_REQUEST,
            format!(
                "Missing connection parameter '{}'. Save the connection first.",
                p
            ),
        ),
        OAuthError::App(message) => (StatusCode::BAD_REQUEST, message),
        OAuthError::NotPending | OAuthError::PendingChanged => {
            (StatusCode::CONFLICT, error.to_string())
        }
        OAuthError::ConfirmationExpired => (StatusCode::GONE, error.to_string()),
        e => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (
        status,
        Json(ErrorResponse {
            success: false,
            error: message,
        }),
    )
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthLinkResponse {
    pub success: bool,
    /// The link to hand the person who is to authorize at the provider.
    pub authorization_url: String,
    pub redirect_uri: String,
    /// After this the link no longer works.
    pub expires_at: String,
}

/// Start an OAuth2 flow without a browser, for an API client or an MCP tool
/// to hand someone a link.
///
/// Whoever opens the link authorizes at the provider; the connection then
/// waits as `PENDING_CONFIRMATION`, unusable, until a member confirms it in
/// the console within 15 minutes.
#[cfg_attr(feature = "utoipa", utoipa::path(
    post,
    path = "/api/runtime/connections/{id}/oauth/link",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "A provider link for a flow that waits for confirmation", body = OAuthLinkResponse),
        (status = 400, description = "Integration does not support OAuth or missing required parameter", body = crate::types::ErrorResponse),
        (status = 404, description = "Connection not found", body = crate::types::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::types::ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn link_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    caller: Option<axum::Extension<crate::tenant::Caller>>,
    State(state): State<ConnectionsState>,
    Path(id): Path<String>,
) -> Result<Json<OAuthLinkResponse>, (StatusCode, Json<ErrorResponse>)> {
    let flow = OAuthService::from_state(&state)
        .start(
            &id,
            &tenant_id,
            StartBinding::Confirm {
                started_by: started_by(caller.as_deref()),
            },
        )
        .await
        .map_err(error_response)?;
    crate::events::emit(
        &state.connection_events,
        Some(&tenant_id),
        crate::events::ConnectionLifecycleEvent::OAuthStarted {
            connection_id: id.clone(),
        },
    );
    let expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(oauth_flow::FLOW_TTL_SECONDS as i64);
    Ok(Json(OAuthLinkResponse {
        success: true,
        authorization_url: flow.authorization_url,
        redirect_uri: flow.redirect_uri,
        expires_at: expires_at.to_rfc3339(),
    }))
}

#[derive(Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthPendingResponse {
    pub success: bool,
    /// The authorization waiting for confirmation, if any.
    pub pending: Option<crate::service::oauth::PendingConfirmation>,
}

/// What a connection's authorization waiting for confirmation connected, and
/// who started it. No secrets.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/oauth/pending",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "The pending authorization, or none", body = OAuthPendingResponse),
        (status = 404, description = "Connection not found", body = crate::types::ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn pending_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    State(state): State<ConnectionsState>,
    Path(id): Path<String>,
) -> Result<Json<OAuthPendingResponse>, (StatusCode, Json<ErrorResponse>)> {
    let pending = OAuthService::from_state(&state)
        .pending_confirmation(&tenant_id, &id)
        .await
        .map_err(error_response)?;
    Ok(Json(OAuthPendingResponse {
        success: true,
        pending,
    }))
}

/// Which pending authorization a confirmation or rejection is for: the `id`
/// `GET .../oauth/pending` showed.
#[derive(Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthPendingDecision {
    pub pending_id: String,
}

/// A confirmation must come from a person at the console.
fn require_member(
    caller: Option<&crate::tenant::Caller>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if caller.is_some_and(crate::tenant::Caller::may_confirm) {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            success: false,
            error: "Only a member signed in to the console can confirm or reject an \
                    authorization; not an API key or an MCP client"
                .to_string(),
        }),
    ))
}

/// Confirm the authorization a flow started without a browser obtained: the
/// connection becomes usable. Only a member signed in to the console.
#[cfg_attr(feature = "utoipa", utoipa::path(
    post,
    path = "/api/runtime/connections/{id}/oauth/confirm",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    request_body = OAuthPendingDecision,
    responses(
        (status = 200, description = "Confirmed; the connection is ACTIVE"),
        (status = 403, description = "Not a member signed in to the console", body = crate::types::ErrorResponse),
        (status = 404, description = "Connection not found", body = crate::types::ErrorResponse),
        (status = 409, description = "No authorization is waiting for confirmation, or not the one named", body = crate::types::ErrorResponse),
        (status = 410, description = "It waited too long and was dropped", body = crate::types::ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn confirm_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    caller: Option<axum::Extension<crate::tenant::Caller>>,
    State(state): State<ConnectionsState>,
    Path(id): Path<String>,
    Json(decision): Json<OAuthPendingDecision>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    require_member(caller.as_deref())?;
    OAuthService::from_state(&state)
        .confirm(&tenant_id, &id, &decision.pending_id)
        .await
        .map_err(error_response)?;
    crate::events::emit(
        &state.connection_events,
        Some(&tenant_id),
        crate::events::ConnectionLifecycleEvent::OAuthCompleted {
            connection_id: id.clone(),
        },
    );
    Ok(Json(serde_json::json!({ "success": true })))
}

/// Reject the authorization a flow started without a browser obtained: its
/// tokens are dropped and revoked at the provider. Only a member signed in to
/// the console.
#[cfg_attr(feature = "utoipa", utoipa::path(
    post,
    path = "/api/runtime/connections/{id}/oauth/reject",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    request_body = OAuthPendingDecision,
    responses(
        (status = 200, description = "Rejected; the tokens were dropped"),
        (status = 403, description = "Not a member signed in to the console", body = crate::types::ErrorResponse),
        (status = 404, description = "Connection not found", body = crate::types::ErrorResponse),
        (status = 409, description = "No authorization is waiting for confirmation, or not the one named", body = crate::types::ErrorResponse)
    ),
    tag = "connections-controller"
))]
pub async fn reject_handler(
    crate::tenant::TenantId(tenant_id): crate::tenant::TenantId,
    caller: Option<axum::Extension<crate::tenant::Caller>>,
    State(state): State<ConnectionsState>,
    Path(id): Path<String>,
    Json(decision): Json<OAuthPendingDecision>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    require_member(caller.as_deref())?;
    OAuthService::from_state(&state)
        .reject(&tenant_id, &id, &decision.pending_id)
        .await
        .map_err(error_response)?;
    crate::events::emit(
        &state.connection_events,
        Some(&tenant_id),
        crate::events::ConnectionLifecycleEvent::OAuthFailed {
            reason: "rejected".to_string(),
        },
    );
    Ok(Json(serde_json::json!({ "success": true })))
}

// ============================================================================
// Callback endpoint (public, no JWT)
// ============================================================================

#[derive(Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Handle the OAuth2 provider callback, for a connection whose redirect URI
/// names no tenant.
///
/// This is a PUBLIC endpoint (no JWT required) — called by the OAuth provider
/// redirecting the user's browser after consent. The tenant comes from the
/// flow the state names. A tenant this process does not serve is sent on to
/// its own callback URL, which the edge routes to the process that serves it.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/oauth/callback",
    params(
        ("code" = Option<String>, Query, description = "Authorization code returned by the provider"),
        ("state" = Option<String>, Query, description = "Opaque state value naming the flow"),
        ("error" = Option<String>, Query, description = "Error code if the provider reports a failure"),
        ("error_description" = Option<String>, Query, description = "Human-readable error description from the provider")
    ),
    responses(
        (status = 303, description = "Redirect to the console's OAuth landing page, with the outcome in its query"),
        (status = 307, description = "The flow's tenant is served elsewhere: the same request at its tenant's callback URL"),
        (status = 200, description = "Plain-text outcome, when no console is configured", content_type = "text/plain")
    ),
    tag = "oauth-callback"
))]
pub async fn tenantless_callback_handler(
    State(state): State<ConnectionsState>,
    Query(params): Query<OAuthCallbackQuery>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
    headers: HeaderMap,
) -> Response {
    let flow_state = params.state.clone().filter(|s| !s.is_empty());
    let tenant = match &flow_state {
        Some(flow_state) => OAuthService::from_state(&state).tenant_of(flow_state).await,
        None => Ok(None),
    };
    let tenant = match tenant {
        Ok(Some(tenant)) => tenant,
        // No flow to learn the tenant from: the provider's error, or a state
        // that is unknown or expired. Either way no tenant is verified.
        Ok(None) => return finish_callback(state, None, params, raw_query, headers).await,
        Err(error) => {
            tracing::error!(%error, "looking up an OAuth flow's tenant failed");
            let secure = state.public_base_url.starts_with("https://");
            let mut response = callback_response(
                state.oauth_complete_url.as_ref(),
                CallbackOutcome::Failed("server_error".to_string()),
            );
            if let Some(flow_state) = &flow_state {
                set_cookie(
                    response.headers_mut(),
                    &oauth_flow::clearing_cookie(flow_state, secure),
                );
            }
            return response;
        }
    };
    if !state.oauth.tenants.serves(&tenant) {
        let location = format!(
            "{}/api/oauth/{}/callback?{}",
            state.public_base_url.trim_end_matches('/'),
            urlencoding::encode(&tenant),
            raw_query.unwrap_or_default()
        );
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [
                (LOCATION, location),
                (CACHE_CONTROL, "no-store".to_string()),
                (REFERRER_POLICY, "no-referrer".to_string()),
            ],
        )
            .into_response();
    }
    let host = state.oauth.tenants.clone();
    host.scoped(
        tenant.clone(),
        Box::pin(finish_callback(
            state,
            Some(tenant),
            params,
            raw_query,
            headers,
        )),
    )
    .await
}

/// Handle the OAuth2 provider callback, for a connection registered with its
/// tenant's own redirect URI.
///
/// This is a PUBLIC endpoint (no JWT required) — called by the OAuth provider
/// redirecting the user's browser after consent. The consent popup is then
/// sent on to the console's landing page, which tells the console from the
/// console's own origin.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/oauth/{tenant_id}/callback",
    params(
        ("tenant_id" = String, Path, description = "Tenant ID encoded in the OAuth redirect URI"),
        ("code" = Option<String>, Query, description = "Authorization code returned by the provider"),
        ("state" = Option<String>, Query, description = "Opaque state value naming the flow"),
        ("error" = Option<String>, Query, description = "Error code if the provider reports a failure"),
        ("error_description" = Option<String>, Query, description = "Human-readable error description from the provider")
    ),
    responses(
        (status = 303, description = "Redirect to the console's OAuth landing page, with the outcome in its query"),
        (status = 200, description = "Plain-text outcome, when no console is configured", content_type = "text/plain")
    ),
    tag = "oauth-callback"
))]
pub async fn callback_handler(
    State(state): State<ConnectionsState>,
    Path(tenant_id): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
    headers: HeaderMap,
) -> Response {
    finish_callback(state, Some(tenant_id), params, raw_query, headers).await
}

/// The rest of a callback, in the tenant a flow is to be redeemed in: `None`
/// when no flow could name one, so only a failure remains to report.
async fn finish_callback(
    state: ConnectionsState,
    tenant_id: Option<String>,
    params: OAuthCallbackQuery,
    raw_query: Option<String>,
    headers: HeaderMap,
) -> Response {
    let flow_state = params.state.clone().filter(|s| !s.is_empty());
    let secure = state.public_base_url.starts_with("https://");
    let mut response = callback_outcome(state, tenant_id, params, raw_query, &headers).await;
    // The flow's cookie has done its job whatever the outcome. A callback
    // without a state names no cookie; the others may belong to flows still
    // open, and expire on their own.
    if let Some(flow_state) = &flow_state {
        set_cookie(
            response.headers_mut(),
            &oauth_flow::clearing_cookie(flow_state, secure),
        );
    }
    response
}

async fn callback_outcome(
    state: ConnectionsState,
    tenant_id: Option<String>,
    params: OAuthCallbackQuery,
    raw_query: Option<String>,
    headers: &HeaderMap,
) -> Response {
    let events = state.connection_events.clone();
    // A tenant is trusted only once a flow issued for it has been redeemed;
    // events before that carry no tenant.

    // The full callback query, from which the descriptor's extra_callback_params
    // (e.g. Intuit realmId) are captured when the flow completes.
    let callback_params: std::collections::HashMap<String, String> = raw_query
        .as_deref()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();

    // Handle provider errors
    if let Some(error) = params.error {
        let desc = params.error_description.unwrap_or_default();
        let reason = format!("{}: {}", error, desc);
        crate::events::emit(
            &events,
            None,
            crate::events::ConnectionLifecycleEvent::OAuthFailed { reason },
        );
        return callback_response(
            state.oauth_complete_url.as_ref(),
            CallbackOutcome::Failed(provider_error_code(&error)),
        );
    }

    let flow_state = match params.state {
        Some(s) if !s.is_empty() => s,
        _ => {
            return callback_response(
                state.oauth_complete_url.as_ref(),
                CallbackOutcome::Failed("invalid_request".to_string()),
            );
        }
    };

    let code = match params.code {
        Some(c) if !c.is_empty() => c,
        _ => {
            return callback_response(
                state.oauth_complete_url.as_ref(),
                CallbackOutcome::Failed("invalid_request".to_string()),
            );
        }
    };

    let Some(tenant_id) = tenant_id else {
        crate::events::emit(
            &events,
            None,
            crate::events::ConnectionLifecycleEvent::OAuthFailed {
                reason: "invalid_state".to_string(),
            },
        );
        return callback_response(
            state.oauth_complete_url.as_ref(),
            CallbackOutcome::Failed("invalid_state".to_string()),
        );
    };

    let secure = state.public_base_url.starts_with("https://");
    let cookie_name = oauth_flow::cookie_name(&flow_state, secure);
    let browser_nonce = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|header| oauth_flow::cookie_value(header, &cookie_name))
        .map(str::to_owned);

    let complete_url = state.oauth_complete_url.clone();
    let service = OAuthService::from_state(&state);

    let outcome = match service
        .complete(
            &tenant_id,
            &flow_state,
            &code,
            &callback_params,
            browser_nonce.as_deref(),
        )
        .await
    {
        Ok(completed) if completed.pending_confirmation => {
            crate::events::emit(
                &events,
                Some(&tenant_id),
                crate::events::ConnectionLifecycleEvent::OAuthPendingConfirmation {
                    connection_id: completed.connection_id.clone(),
                },
            );
            CallbackOutcome::Pending(completed.connection_id)
        }
        Ok(completed) => {
            crate::events::emit(
                &events,
                Some(&tenant_id),
                crate::events::ConnectionLifecycleEvent::OAuthCompleted {
                    connection_id: completed.connection_id.clone(),
                },
            );
            CallbackOutcome::Connected(completed.connection_id)
        }
        Err(OAuthError::InvalidState) => {
            crate::events::emit(
                &events,
                None,
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: "invalid_state".to_string(),
                },
            );
            CallbackOutcome::Failed("invalid_state".to_string())
        }
        Err(OAuthError::BrowserMismatch) => {
            // The flow was redeemed, so its tenant is verified.
            crate::events::emit(
                &events,
                Some(&tenant_id),
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: "browser_mismatch".to_string(),
                },
            );
            CallbackOutcome::Failed("browser_mismatch".to_string())
        }
        Err(OAuthError::TokenExchangeFailed(msg)) => {
            crate::events::emit(
                &events,
                Some(&tenant_id),
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: format!("token_exchange_failed: {}", msg),
                },
            );
            CallbackOutcome::Failed("token_exchange_failed".to_string())
        }
        Err(e) => {
            // An internal error may come before the state was matched.
            let verified = !matches!(e, OAuthError::Internal(_));
            crate::events::emit(
                &events,
                verified.then_some(tenant_id.as_str()),
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: e.to_string(),
                },
            );
            CallbackOutcome::Failed("server_error".to_string())
        }
    };
    callback_response(complete_url.as_ref(), outcome)
}

/// Add a `Set-Cookie` header.
fn set_cookie(headers: &mut HeaderMap, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        headers.append(SET_COOKIE, value);
    }
}

/// How a callback ended, as the console's landing page is told it.
#[derive(Debug, PartialEq)]
enum CallbackOutcome {
    /// The connection now holds the provider's tokens.
    Connected(String),
    /// The provider's tokens wait for a member's confirmation.
    Pending(String),
    /// The flow failed: an OAuth error code, the provider's or our own.
    Failed(String),
}

/// The provider's `error`, as a code safe to put in a URL: OAuth error codes
/// are ASCII words, so anything else is dropped.
fn provider_error_code(error: &str) -> String {
    let code: String = error
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect();
    if code.is_empty() {
        "provider_error".to_string()
    } else {
        code
    }
}

/// Send the consent popup to the console's landing page, which runs on the
/// console's origin and so can tell the console (over `BroadcastChannel` and
/// `window.opener`) wherever the console and this server are hosted.
///
/// Only the connection id and an error code travel in the URL, which lands in
/// browser history and logs; the console reads the connection's state from
/// the API. The detail of a failure is in the server log and the
/// `OAuthFailed` event.
///
/// With no console to go to, a plain-text page reports the outcome and the
/// user closes the window themselves.
fn callback_response(complete_url: Option<&url::Url>, outcome: CallbackOutcome) -> Response {
    let Some(complete_url) = complete_url else {
        return (
            [
                (CONTENT_TYPE, "text/plain; charset=utf-8"),
                (CACHE_CONTROL, "no-store"),
            ],
            outcome_text(&outcome),
        )
            .into_response();
    };
    (
        StatusCode::SEE_OTHER,
        [
            (LOCATION, landing_url(complete_url, &outcome).to_string()),
            (CACHE_CONTROL, "no-store".to_string()),
            (REFERRER_POLICY, "no-referrer".to_string()),
        ],
    )
        .into_response()
}

/// The landing page's URL for `outcome`.
fn landing_url(complete_url: &url::Url, outcome: &CallbackOutcome) -> url::Url {
    let mut url = complete_url.clone();
    {
        let mut query = url.query_pairs_mut();
        match outcome {
            CallbackOutcome::Connected(connection_id) => {
                query
                    .append_pair("result", "ok")
                    .append_pair("connection", connection_id);
            }
            CallbackOutcome::Pending(connection_id) => {
                query
                    .append_pair("result", "pending")
                    .append_pair("connection", connection_id);
            }
            CallbackOutcome::Failed(code) => {
                query
                    .append_pair("result", "error")
                    .append_pair("error", code);
            }
        }
    }
    url
}

fn outcome_text(outcome: &CallbackOutcome) -> String {
    match outcome {
        CallbackOutcome::Connected(_) => {
            "Authorization successful. You can close this window.".to_string()
        }
        CallbackOutcome::Pending(_) => "Authorization received. A member must confirm the \
                                        connection in the console within 15 minutes. You can \
                                        close this window."
            .to_string(),
        CallbackOutcome::Failed(code) => {
            let reason = match code.as_str() {
                "invalid_state" => "Invalid or expired authorization. Please try again.",
                "access_denied" => "Access was denied.",
                "browser_mismatch" => "Finish the authorization in the browser you started it in.",
                "token_exchange_failed" => "The provider did not issue a token.",
                _ => "Authorization failed.",
            };
            format!("{reason} ({code}) You can close this window.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing() -> url::Url {
        url::Url::parse("https://app.example.com/ui/oauth-complete.html").unwrap()
    }

    #[test]
    fn provider_error_code_keeps_only_a_code() {
        assert_eq!(provider_error_code("access_denied"), "access_denied");
        assert_eq!(
            provider_error_code("<script>alert(1)</script>"),
            "scriptalert1script"
        );
        assert_eq!(provider_error_code("&?#"), "provider_error");
        assert_eq!(provider_error_code(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn success_redirects_to_the_landing_page_with_the_connection() {
        let response = callback_response(
            Some(&landing()),
            CallbackOutcome::Connected("conn 1".to_string()),
        );
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[LOCATION],
            "https://app.example.com/ui/oauth-complete.html?result=ok&connection=conn+1"
        );
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[REFERRER_POLICY], "no-referrer");
    }

    #[test]
    fn failure_redirects_with_its_code_only() {
        let response = callback_response(
            Some(&landing()),
            CallbackOutcome::Failed("invalid_state".to_string()),
        );
        assert_eq!(
            response.headers()[LOCATION],
            "https://app.example.com/ui/oauth-complete.html?result=error&error=invalid_state"
        );
    }

    #[test]
    fn without_a_console_the_outcome_is_plain_text() {
        let response = callback_response(None, CallbackOutcome::Failed("invalid_state".into()));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert!(
            outcome_text(&CallbackOutcome::Failed("invalid_state".into()))
                .starts_with("Invalid or expired authorization")
        );
    }
}
