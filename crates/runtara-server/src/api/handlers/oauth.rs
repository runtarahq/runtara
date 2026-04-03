//! OAuth2 Authorization Code flow handlers
//!
//! Two endpoints:
//! - `GET /api/runtime/connections/{id}/oauth/authorize` (JWT-protected) — generates auth URL
//! - `GET /api/oauth/{tenant_id}/callback` (public) — handles provider redirect

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Html;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::api::services::oauth::{OAuthError, OAuthService};

// ============================================================================
// Authorize endpoint (JWT-protected)
// ============================================================================

#[derive(Serialize, Deserialize, ToSchema)]
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
#[utoipa::path(
    get,
    path = "/api/runtime/connections/{id}/oauth/authorize",
    params(("id" = String, Path, description = "Connection ID")),
    responses(
        (status = 200, description = "Authorization URL generated", body = OAuthAuthorizeResponse),
        (status = 404, description = "Connection not found"),
        (status = 400, description = "Integration does not support OAuth"),
    ),
    tag = "connections-controller"
)]
pub async fn authorize_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(id): Path<String>,
) -> Result<Json<OAuthAuthorizeResponse>, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let public_base_url =
        std::env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    let service = OAuthService::new(pool, public_base_url);

    match service.generate_authorization_url(&id, &tenant_id).await {
        Ok(url) => Ok(Json(OAuthAuthorizeResponse {
            success: true,
            authorization_url: url,
        })),
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
pub async fn callback_handler(
    State(pool): State<PgPool>,
    Path(_tenant_id): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
) -> Html<String> {
    // Handle provider errors
    if let Some(error) = params.error {
        let desc = params.error_description.unwrap_or_default();
        return oauth_response_html(None, false, &format!("{}: {}", error, desc));
    }

    let state = match params.state {
        Some(s) if !s.is_empty() => s,
        _ => return oauth_response_html(None, false, "Missing state parameter"),
    };

    let code = match params.code {
        Some(c) if !c.is_empty() => c,
        _ => return oauth_response_html(None, false, "Missing authorization code"),
    };

    let public_base_url =
        std::env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    let service = OAuthService::new(pool, public_base_url);

    match service.handle_callback(&state, &code).await {
        Ok(connection_id) => oauth_response_html(Some(&connection_id), true, ""),
        Err(OAuthError::InvalidState) => oauth_response_html(
            None,
            false,
            "Invalid or expired authorization. Please try again.",
        ),
        Err(OAuthError::TokenExchangeFailed(msg)) => {
            oauth_response_html(None, false, &format!("Token exchange failed: {}", msg))
        }
        Err(e) => oauth_response_html(None, false, &e.to_string()),
    }
}

/// Generate the HTML response for the OAuth callback popup.
/// On success, sends a postMessage to the opener and closes the window.
/// On failure, shows an error message with a close button.
fn oauth_response_html(connection_id: Option<&str>, success: bool, error: &str) -> Html<String> {
    let conn_id_json = connection_id.unwrap_or("");
    let error_json = error.replace('\\', "\\\\").replace('"', "\\\"");

    Html(format!(
        r#"<!DOCTYPE html>
<html><head><title>OAuth Authorization</title>
<style>
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
<script>
  if (window.opener) {{
    window.opener.postMessage({{
      type: 'oauth-complete',
      connectionId: '{conn_id}',
      success: {success_js},
      error: "{error_js}"
    }}, '*');
    {close_js}
  }}
</script>
</body></html>"#,
        content = if success {
            "<h2 class=\"success\">Authorization Successful</h2><p>You can close this window.</p>"
                .to_string()
        } else {
            format!(
                "<h2 class=\"error\">Authorization Failed</h2><p>{}</p><button onclick=\"window.close()\">Close</button>",
                error
            )
        },
        conn_id = conn_id_json,
        success_js = if success { "true" } else { "false" },
        error_js = error_json,
        close_js = if success {
            "setTimeout(() => window.close(), 1000);"
        } else {
            ""
        },
    ))
}
