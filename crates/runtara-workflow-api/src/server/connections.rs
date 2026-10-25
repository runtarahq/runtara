//! The connection family: connection metadata and resources, SQL, and
//! outbound HTTP (the `runtara:host` package of the old WIT ABI).

use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use super::{Failure, WorkflowApiState, binding_of};
use crate::{ApiError, ErrorCode, OutboundMeta, SqlOperation, headers};

/// A successful outbound call: the upstream status, headers and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn with_type(body: Vec<u8>, content_type: &'static str, status: StatusCode) -> Response {
    let mut response = (status, Bytes::from(body)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn json_header<T: serde::Serialize>(value: &T) -> Result<HeaderValue, ApiError> {
    let json = serde_json::to_string(value)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?;
    HeaderValue::from_str(&json)
        .map_err(|_| ApiError::new(ErrorCode::Unavailable, "metadata is not a header value"))
}

pub(super) async fn get_connection(
    State(state): State<WorkflowApiState>,
    Path(connection_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let descriptor = state
        .backend
        .describe_connection(&binding, &connection_id)
        .await?;
    Ok(with_type(descriptor, "application/json", StatusCode::OK))
}

pub(super) async fn post_resources(
    State(state): State<WorkflowApiState>,
    Path(connection_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "resource request is over the cap"))?;
    let resolved = state
        .backend
        .resolve_resource(&binding, &connection_id, body.to_vec())
        .await?;
    Ok(with_type(resolved, "application/json", StatusCode::OK))
}

pub(super) async fn post_sql(
    State(state): State<WorkflowApiState>,
    Path((connection_id, operation)): Path<(String, String)>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let operation = SqlOperation::parse(&operation).ok_or_else(|| {
        ApiError::new(
            ErrorCode::NotFound,
            "SQL operations are query, execute and batch",
        )
    })?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "SQL request is over the cap"))?;
    let result = state
        .backend
        .sql(&binding, &connection_id, operation, body.to_vec())
        .await?;
    Ok(with_type(result, "application/json", StatusCode::OK))
}

pub(super) async fn post_http(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let meta = request
        .headers()
        .get(headers::OUTBOUND)
        .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "Runtara-Outbound is required"))?;
    if meta.len() > state.caps.metadata_header_bytes {
        return Err(ApiError::new(ErrorCode::TooLarge, "Runtara-Outbound is over the cap").into());
    }
    let meta: OutboundMeta = serde_json::from_slice(meta.as_bytes())
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("Runtara-Outbound: {e}")))?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "outbound body is over the cap"))?;
    let body = (!body.is_empty()).then(|| body.to_vec());
    match state.backend.http(&binding, meta, body).await? {
        Ok(reply) => {
            let mut response = with_type(reply.body, "application/octet-stream", StatusCode::OK);
            let headers_value = json_header(&reply.headers)?;
            let response_headers = response.headers_mut();
            response_headers.insert(
                headers::UPSTREAM_STATUS,
                HeaderValue::from_str(&reply.status.to_string())
                    .map_err(|_| ApiError::new(ErrorCode::Unavailable, "bad upstream status"))?,
            );
            response_headers.insert(headers::UPSTREAM_HEADERS, headers_value);
            Ok(response)
        }
        Err((failure, upstream_body)) => {
            let mut response = with_type(
                upstream_body,
                "application/octet-stream",
                StatusCode::BAD_GATEWAY,
            );
            response
                .headers_mut()
                .insert(headers::ERROR, json_header(&failure)?);
            Ok(response)
        }
    }
}
