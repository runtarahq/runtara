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
    State(state): State<ConnectionsState>,
    Path(id): Path<String>,
) -> Result<Response, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let events = state.connection_events.clone();
    let service = OAuthService::from_state(&state);

    match service.start(&id, &tenant_id, StartBinding::Browser).await {
        Ok(flow) => {
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
        Err(OAuthError::ConnectionNotFound) => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                success: false,
                error: "Connection not found".to_string(),
            }),
        )),
        Err(OAuthError::NotOAuthIntegration(id)) => Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                success: false,
                error: format!("Integration '{}' does not support OAuth", id),
            }),
        )),
        Err(OAuthError::MissingParameter(p)) => Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                success: false,
                error: format!(
                    "Missing connection parameter '{}'. Save the connection first.",
                    p
                ),
            }),
        )),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                success: false,
                error: e.to_string(),
            }),
        )),
    }
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

    let oauth_state = match params.state {
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
    let cookie_name = oauth_flow::cookie_name(&oauth_state, secure);
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
            &oauth_state,
            &code,
            &callback_params,
            browser_nonce.as_deref(),
        )
        .await
    {
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
