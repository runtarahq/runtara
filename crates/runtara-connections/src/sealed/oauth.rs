//! Access tokens of a connection whose OAuth secrets are sealed.
//!
//! seal sends the token request with the sealed secret, and what comes back
//! is the access token already sealed, for the connection's API only. This
//! process learns when it expires, and never what it is.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use seal_client::{SealedValue, SendRequest};
use seal_format::{Envelope, FieldName, ItemId};
use serde_json::{Value, json};

use super::{SEALED_MARKER, SealErrorCode, SealedCredentials, keys, policy, presented};
use crate::auth::token_cache::{
    self, AuthResolutionError, CachedAccessToken, DeferredAuth, TokenRequestBody,
};

/// How long a token endpoint may take, as for a request this process sends.
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest token response read.
const MAX_TOKEN_RESPONSE_BYTES: usize = 256 * 1024;

/// What a token endpoint answered with success: its body with every
/// declared token replaced by a sealed value, and those values.
pub(crate) struct TokenReply {
    pub body: Value,
    pub sealed: BTreeMap<FieldName, Envelope>,
}

impl SealedCredentials {
    /// POSTs a token request through seal, presenting `presented_fields`
    /// (each a parameter of the connection holding a sealed value).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn token_request(
        &self,
        tenant_id: &str,
        connection_id: &str,
        parameters: &Value,
        presented_fields: &[&str],
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<TokenReply, AuthResolutionError> {
        token_cache::check_token_endpoint(url)?;
        let mut sealed: BTreeMap<FieldName, SealedValue> = BTreeMap::new();
        for name in presented_fields {
            let (field, value) = presented(parameters, name).map_err(|error| {
                AuthResolutionError::permanent(format!(
                    "The connection's sealed credential cannot be used: {}",
                    error.detail
                ))
            })?;
            sealed.insert(field, value);
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(url)
            .header("content-type", content_type)
            .header("accept", "application/json")
            .body(
                Full::new(Bytes::from(body))
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .map_err(|_| AuthResolutionError::permanent("The token endpoint's URL is not valid"))?;
        let item = ItemId::new(connection_id).map_err(|_| {
            AuthResolutionError::permanent("The connection's id is not one seal accepts")
        })?;
        let sent = tokio::time::timeout(
            TOKEN_REQUEST_TIMEOUT,
            self.client.send(SendRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item,
                sealed,
                plain: BTreeMap::new(),
                request,
            }),
        )
        .await
        .map_err(|_| AuthResolutionError::transient("Token request timed out"))?
        .map_err(|error| {
            // seal's detail has identifiers and static text only.
            let message = format!(
                "Token request was not sent: {}: {}",
                error.code.as_str(),
                error.detail
            );
            match error.code {
                SealErrorCode::PolicyDenied
                | SealErrorCode::DestinationRefused
                | SealErrorCode::BadRequest
                | SealErrorCode::OpenFailed
                | SealErrorCode::KeysetUnknown
                | SealErrorCode::Conflict => AuthResolutionError::permanent(message),
                _ => AuthResolutionError::transient(message),
            }
        })?;

        let status = sent.response.status().as_u16();
        let mut upstream = sent.response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = tokio::time::timeout(TOKEN_REQUEST_TIMEOUT, upstream.frame())
            .await
            .map_err(|_| AuthResolutionError::transient("Token response timed out"))?
        {
            let frame = frame
                .map_err(|_| AuthResolutionError::transient("Failed to read token response"))?;
            if let Some(chunk) = frame.data_ref() {
                if bytes.len() + chunk.len() > MAX_TOKEN_RESPONSE_BYTES {
                    return Err(AuthResolutionError::transient(
                        "Token response is too large",
                    ));
                }
                bytes.extend_from_slice(chunk);
            }
        }
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|_| AuthResolutionError::transient("Failed to parse token response"))?;
        if !(200..300).contains(&status) {
            return Err(token_endpoint_refusal(status, &body));
        }
        Ok(TokenReply {
            body,
            sealed: sent.sealed_fields,
        })
    }

    /// The access token of a connection whose client secret is sealed
    /// (client-credentials grant), as the marker a request presents: got from
    /// the token endpoint through seal, and kept until it nears its expiry.
    pub(crate) async fn minted_access_token(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
    ) -> Result<Value, AuthResolutionError> {
        let unsupported =
            || AuthResolutionError::permanent("The connection's token request cannot be formed");
        let secret_field = policy::client_secret_field(integration_id).ok_or_else(unsupported)?;
        let (request, _) =
            policy::client_credentials_request(connection_id, integration_id, parameters)
                .ok_or_else(unsupported)?;
        let DeferredAuth::OAuth2ClientCredentials {
            cache_key,
            token_url,
            request_body,
            default_ttl_seconds,
            ..
        } = request
        else {
            return Err(unsupported());
        };
        let envelope = token_cache::resolve_cached_token(&cache_key.sealed(), || async {
            // The request as the type builds it, without the secret: seal
            // puts that where the secret's policy says.
            let (content_type, body) = match request_body {
                TokenRequestBody::Json(mut body) => {
                    if let Some(fields) = body.as_object_mut() {
                        fields.remove("client_secret");
                    }
                    ("application/json", body.to_string().into_bytes())
                }
                TokenRequestBody::FormUrlEncoded(fields) => {
                    let fields: Vec<(String, String)> = fields
                        .into_iter()
                        .filter(|(name, _)| name != "client_secret")
                        .collect();
                    (
                        "application/x-www-form-urlencoded",
                        token_cache::form_urlencoded(&fields).into_bytes(),
                    )
                }
            };
            let reply = self
                .token_request(
                    tenant_id,
                    connection_id,
                    parameters,
                    &[secret_field],
                    &token_url,
                    content_type,
                    body,
                )
                .await?;
            Ok(CachedAccessToken {
                access_token: sealed_access_token(&reply)?,
                expires_at: token_cache::token_expiry(&reply.body, default_ttl_seconds),
            })
        })
        .await?;
        marker_of(&envelope)
    }
}

/// The sealed access token of a reply, as text to keep.
pub(crate) fn sealed_access_token(reply: &TokenReply) -> Result<String, AuthResolutionError> {
    let field = FieldName::new(policy::ACCESS_TOKEN_FIELD)
        .map_err(|_| AuthResolutionError::permanent("The access token's field is not valid"))?;
    let envelope = reply.sealed.get(&field).ok_or_else(|| {
        AuthResolutionError::transient("Token response missing access_token field")
    })?;
    serde_json::to_string(envelope)
        .map_err(|_| AuthResolutionError::transient("The sealed access token cannot be kept"))
}

/// A kept sealed token as the marker a request presents.
pub(crate) fn marker_of(envelope: &str) -> Result<Value, AuthResolutionError> {
    let envelope: Value = serde_json::from_str(envelope)
        .map_err(|_| AuthResolutionError::transient("The kept access token is not readable"))?;
    Ok(json!({ SEALED_MARKER: { "envelope": envelope, "policy": "" } }))
}

/// What a token endpoint's refusal means, as when this process sends the
/// request itself: only the status and the standard OAuth error fields are
/// passed on, never the body.
fn token_endpoint_refusal(status: u16, body: &Value) -> AuthResolutionError {
    let code = body["error"].as_str().unwrap_or("unknown_error");
    let description = body["error_description"].as_str().unwrap_or("");
    let message = format!("Token endpoint returned {status}: {code} {description}")
        .trim()
        .to_string();
    if (400..500).contains(&status) && status != 429 {
        AuthResolutionError::permanent(message)
    } else {
        AuthResolutionError::transient(message)
    }
}
