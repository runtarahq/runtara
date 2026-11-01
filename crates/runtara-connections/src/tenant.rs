use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Tenant identifier extracted from request extensions.
///
/// The host application is responsible for inserting this into the request
/// extensions before the request reaches the connections router (typically
/// via an authentication middleware).
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TenantId {
    type Rejection = TenantIdRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantId>()
            .cloned()
            .ok_or(TenantIdRejection)
    }
}

/// Who is calling, as the host's authentication established it. The host
/// inserts it into request extensions with [`TenantId`]; without it, actions
/// that need a signed-in member are refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The caller's identity (a user id, or the user an API key acts for).
    pub id: String,
    pub channel: CallerChannel,
}

/// How a caller reached the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerChannel {
    /// A member signed in to the console (a user token).
    Member,
    /// An API key.
    ApiKey,
    /// An MCP client, whatever its credential.
    Mcp,
    /// A host that authenticates upstream, or not at all (local mode).
    Trusted,
}

impl Caller {
    /// The caller as recorded on a flow and shown to whoever confirms it.
    pub fn label(&self) -> String {
        let channel = match self.channel {
            CallerChannel::Member => "member",
            CallerChannel::ApiKey => "api_key",
            CallerChannel::Mcp => "mcp",
            CallerChannel::Trusted => "trusted",
        };
        format!("{channel}:{}", self.id)
    }

    /// Whether the caller is a person at the console, who may confirm what a
    /// flow started without a browser connected.
    pub fn may_confirm(&self) -> bool {
        matches!(self.channel, CallerChannel::Member | CallerChannel::Trusted)
    }
}

/// Rejection returned when `TenantId` is missing from request extensions.
pub struct TenantIdRejection;

impl IntoResponse for TenantIdRejection {
    fn into_response(self) -> Response {
        let body = axum::Json(json!({"error": "Missing tenant context"}));
        (StatusCode::UNAUTHORIZED, body).into_response()
    }
}
