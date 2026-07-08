//! OAuth2 Authorization Code flow handlers
//!
//! Two endpoints:
//! - `GET /api/runtime/connections/{id}/oauth/authorize` (JWT-protected) — generates auth URL
//! - `GET /api/oauth/{tenant_id}/callback` (public) — handles provider redirect

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::config::ConnectionsState;
use crate::service::oauth::{OAuthError, OAuthService};

// ============================================================================
// Authorize endpoint (JWT-protected)
// ============================================================================

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthAuthorizeResponse {
    pub success: bool,
    pub authorization_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    success: bool,
    error: String,
}

/// Generate an OAuth2 authorization URL for a connection.
///
/// The frontend should open this URL in a popup window.
/// After user consent, the provider redirects to /api/oauth/{tenant_id}/callback.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/oauth/authorize",
    params(
        ("id" = String, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "OAuth2 authorization URL generated", body = OAuthAuthorizeResponse),
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
) -> Result<Json<OAuthAuthorizeResponse>, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let events = state.connection_events.clone();
    let service = OAuthService::new(state.db_pool, state.cipher, state.public_base_url);

    match service.generate_authorization_url(&id, &tenant_id).await {
        Ok(url) => {
            crate::events::emit(
                &events,
                crate::events::ConnectionLifecycleEvent::OAuthStarted {
                    connection_id: id.clone(),
                },
            );
            Ok(Json(OAuthAuthorizeResponse {
                success: true,
                authorization_url: url,
            }))
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

/// Handle the OAuth2 provider callback.
///
/// This is a PUBLIC endpoint (no JWT required) — called by the OAuth provider
/// redirecting the user's browser after consent.
#[cfg_attr(feature = "utoipa", utoipa::path(
    get,
    path = "/api/oauth/{tenant_id}/callback",
    params(
        ("tenant_id" = String, Path, description = "Tenant ID encoded in the OAuth redirect URI"),
        ("code" = Option<String>, Query, description = "Authorization code returned by the provider"),
        ("state" = Option<String>, Query, description = "Opaque state value used for CSRF protection and connection lookup"),
        ("error" = Option<String>, Query, description = "Error code if the provider reports a failure"),
        ("error_description" = Option<String>, Query, description = "Human-readable error description from the provider")
    ),
    responses(
        (status = 200, description = "HTML page that posts the result to window.opener and closes the popup", content_type = "text/html")
    ),
    tag = "oauth-callback"
))]
pub async fn callback_handler(
    State(state): State<ConnectionsState>,
    Path(_tenant_id): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
) -> Response {
    let events = state.connection_events.clone();

    // The full callback query, from which the descriptor's extra_callback_params
    // (e.g. Intuit realmId) are captured in handle_callback.
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
            crate::events::ConnectionLifecycleEvent::OAuthFailed {
                reason: reason.clone(),
            },
        );
        return oauth_response_html(None, false, &reason);
    }

    let oauth_state = match params.state {
        Some(s) if !s.is_empty() => s,
        _ => return oauth_response_html(None, false, "Missing state parameter"),
    };

    let code = match params.code {
        Some(c) if !c.is_empty() => c,
        _ => return oauth_response_html(None, false, "Missing authorization code"),
    };

    let service = OAuthService::new(state.db_pool, state.cipher, state.public_base_url);

    match service
        .handle_callback(&oauth_state, &code, &callback_params)
        .await
    {
        Ok(connection_id) => {
            crate::events::emit(
                &events,
                crate::events::ConnectionLifecycleEvent::OAuthCompleted {
                    connection_id: connection_id.clone(),
                },
            );
            oauth_response_html(Some(&connection_id), true, "")
        }
        Err(OAuthError::InvalidState) => {
            crate::events::emit(
                &events,
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: "invalid_state".to_string(),
                },
            );
            oauth_response_html(
                None,
                false,
                "Invalid or expired authorization. Please try again.",
            )
        }
        Err(OAuthError::TokenExchangeFailed(msg)) => {
            crate::events::emit(
                &events,
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: format!("token_exchange_failed: {}", msg),
                },
            );
            oauth_response_html(None, false, &format!("Token exchange failed: {}", msg))
        }
        Err(e) => {
            crate::events::emit(
                &events,
                crate::events::ConnectionLifecycleEvent::OAuthFailed {
                    reason: e.to_string(),
                },
            );
            oauth_response_html(None, false, &e.to_string())
        }
    }
}

/// Escape text interpolated into HTML element content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Escape text interpolated into a JavaScript string literal inside an inline
/// `<script>`. Escapes both quote styles (so the result is safe in a single- or
/// double-quoted string), `<` (so a `</script>` in the text can't terminate the
/// element), backslashes, and line terminators (which are illegal inside a JS
/// string literal — U+2028/U+2029 included).
fn js_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '<' => out.push_str("\\u003C"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Generate the HTML response for the OAuth callback popup.
///
/// On success, posts an `oauth-complete` message to `window.opener` and closes
/// the popup; on failure, shows the reason with a Close button.
///
/// The page carries its own Content-Security-Policy. The global security-headers
/// middleware would otherwise apply `default-src 'none'`, which blocks the inline
/// `<script>`/`<style>` this page relies on to hand the result back to the opener
/// (without them the popup renders "Authorization Successful" but never notifies
/// the SPA or closes). A per-response nonce whitelists exactly this page's inline
/// script and style — not `'unsafe-inline'` — so an injected `<script>` (e.g. via
/// a hostile provider `error_description`) still cannot execute. All interpolated
/// text is additionally escaped for its context (HTML body vs. JS string literal).
/// Returns `(html_body, csp_header_value)`. Split from the response wrapper so
/// the page and its scoped CSP can be asserted in a synchronous unit test.
fn oauth_response_page(
    connection_id: Option<&str>,
    success: bool,
    error: &str,
) -> (String, String) {
    // Unguessable per-response nonce for the inline <script>/<style>.
    let nonce = uuid::Uuid::new_v4().simple().to_string();

    let conn_id_js = js_string_escape(connection_id.unwrap_or(""));
    let error_js = js_string_escape(error);
    let error_html = html_escape(error);

    let content = if success {
        "<h2 class=\"success\">Authorization Successful</h2><p>You can close this window.</p>"
            .to_string()
    } else {
        format!(
            "<h2 class=\"error\">Authorization Failed</h2><p>{error_html}</p>\
             <button id=\"oauth-close-btn\" type=\"button\">Close</button>"
        )
    };

    // NB: this is a plain value substituted into the format!() below, so it uses
    // single braces — brace-escaping (`{{`) only applies to the format literal.
    let close_js = if success {
        "setTimeout(function() { window.close(); }, 1000);"
    } else {
        ""
    };

    let body = format!(
        r#"<!DOCTYPE html>
<html><head><title>OAuth Authorization</title>
<style nonce="{nonce}">
  body {{ font-family: -apple-system, BlinkMacSystemFont, sans-serif; display: flex;
         justify-content: center; align-items: center; height: 100vh; margin: 0;
         background: #f5f5f5; color: #333; }}
  .card {{ background: white; padding: 2rem; border-radius: 8px;
           box-shadow: 0 2px 8px rgba(0,0,0,0.1); text-align: center; max-width: 400px; }}
  .success {{ color: #16a34a; }}
  .error {{ color: #dc2626; }}
  button {{ margin-top: 1rem; padding: 0.5rem 1.5rem; border: none; border-radius: 4px;
            background: #2563eb; color: white; cursor: pointer; font-size: 1rem; }}
</style></head>
<body><div class="card">
{content}
</div>
<script nonce="{nonce}">
  (function() {{
    var closeBtn = document.getElementById('oauth-close-btn');
    if (closeBtn) {{ closeBtn.addEventListener('click', function() {{ window.close(); }}); }}
    if (window.opener) {{
      window.opener.postMessage({{
        type: 'oauth-complete',
        connectionId: '{conn_id}',
        success: {success_js},
        error: "{error_js}"
      }}, window.location.origin);
      {close_js}
    }}
  }})();
</script>
</body></html>"#,
        nonce = nonce,
        content = content,
        conn_id = conn_id_js,
        success_js = if success { "true" } else { "false" },
        error_js = error_js,
        close_js = close_js,
    );

    let csp = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; frame-ancestors 'none'"
    );

    (body, csp)
}

/// Wrap the callback page into an axum response carrying its page-scoped CSP.
fn oauth_response_html(connection_id: Option<&str>, success: bool, error: &str) -> Response {
    let (body, csp) = oauth_response_page(connection_id, success, error);
    ([(CONTENT_SECURITY_POLICY, csp)], Html(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract the nonce advertised in the CSP header value.
    fn csp_nonce(csp: &str) -> &str {
        let start = csp
            .find("script-src 'nonce-")
            .expect("script-src nonce present")
            + "script-src 'nonce-".len();
        let rest = &csp[start..];
        &rest[..rest.find('\'').expect("nonce terminator")]
    }

    #[test]
    fn html_escape_neutralizes_markup() {
        assert_eq!(
            html_escape(r#"<script>alert('x')&"</script>"#),
            "&lt;script&gt;alert(&#x27;x&#x27;)&amp;&quot;&lt;/script&gt;"
        );
    }

    #[test]
    fn js_string_escape_prevents_breakout() {
        // `</script>` must not be able to terminate the inline script element,
        // and quotes/backslashes/newlines must not break the JS string literal.
        let out = js_string_escape("a\"b'c\\d</script>\n");
        assert_eq!(out, "a\\\"b\\'c\\\\d\\u003C/script>\\n");
        assert!(!out.contains("</script>"));
    }

    #[test]
    fn success_page_has_nonced_inline_script_and_style() {
        let (body, csp) = oauth_response_page(Some("conn-123"), true, "");
        let nonce = csp_nonce(&csp);

        // CSP must whitelist THIS page's inline script/style by nonce, never
        // fall back to 'unsafe-inline', and keep the strict defaults.
        assert!(csp.contains(&format!("script-src 'nonce-{nonce}'")));
        assert!(csp.contains(&format!("style-src 'nonce-{nonce}'")));
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(!csp.contains("unsafe-inline"));

        // The nonce must actually be stamped on the tags, or the browser blocks them.
        assert!(body.contains(&format!("<style nonce=\"{nonce}\">")));
        assert!(body.contains(&format!("<script nonce=\"{nonce}\">")));

        // Success behavior: message the opener + auto-close.
        assert!(body.contains("'oauth-complete'"));
        assert!(body.contains("connectionId: 'conn-123'"));
        assert!(body.contains("success: true"));
        assert!(body.contains("window.close()"));
        // postMessage is scoped to the SPA (same) origin, never the '*' wildcard,
        // so a foreign opener can't receive the result.
        assert!(body.contains("}, window.location.origin);"));
        assert!(!body.contains("}, '*')"));
        // No inline event handlers — they can't be covered by a nonce.
        assert!(!body.contains("onclick"));
    }

    #[test]
    fn failure_page_escapes_error_in_both_contexts() {
        // A hostile provider error_description carrying markup must not execute:
        // escaped in the HTML body, and unable to break out of the JS string.
        let evil = "boom<img src=x onerror=alert(1)></script>";
        let (body, csp) = oauth_response_page(None, false, evil);
        let nonce = csp_nonce(&csp);

        // Raw markup must never appear verbatim in the body.
        assert!(!body.contains("<img src=x"));
        assert!(body.contains("&lt;img src=x"));
        // The only `</script>` in the document is the real closing tag (exactly one).
        assert_eq!(body.matches("</script>").count(), 1);
        // Failure page reports failure and offers a nonce-wired Close button
        // (no inline onclick).
        assert!(body.contains("success: false"));
        assert!(body.contains("id=\"oauth-close-btn\""));
        assert!(!body.contains("onclick"));
        assert!(body.contains(&format!("<script nonce=\"{nonce}\">")));
        assert!(!csp.contains("unsafe-inline"));
    }
}
