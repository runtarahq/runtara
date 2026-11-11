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
    /// Claims of a sealed token its policy has this process told, by field:
    /// each a JSON object.
    pub claims: BTreeMap<FieldName, String>,
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
        authorization: Option<String>,
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
        let mut request = http::Request::builder()
            .method("POST")
            .uri(url)
            .header("content-type", content_type)
            .header("accept", "application/json");
        // A client secret this process holds, sent as the provider takes it.
        if let Some(authorization) = authorization {
            request = request.header("authorization", authorization);
        }
        let request = request
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
        let claims = sent.claims;
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
            claims,
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
                    None,
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

/// The tokens a refresh brought back, sealed, as the markers a connection
/// stores.
pub struct RotatedSealed {
    pub access_token: Value,
    /// The next refresh token, if the provider rotated it.
    pub refresh_token: Option<Value>,
    pub token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl SealedCredentials {
    /// The access token of a connection whose refresh token is sealed
    /// (authorization-code grant), as the marker a request presents, with
    /// the pair to store if a refresh was made now.
    ///
    /// The stored access token is used while it is fresh. Otherwise seal
    /// sends the refresh token to the token endpoint, and the pair that
    /// comes back is already sealed. `parameters` hold the client's id and
    /// secret as the token endpoint takes them: a client secret is not
    /// sealed for this grant, and is this process's to send.
    pub(crate) async fn refreshed_access_token(
        &self,
        events: &crate::events::ConnectionEvents,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
    ) -> Result<(Value, Option<RotatedSealed>), AuthResolutionError> {
        let unsupported =
            || AuthResolutionError::permanent("The connection's token refresh cannot be formed");
        let tokens =
            policy::sealed_authorized_tokens(integration_id, parameters).ok_or_else(unsupported)?;

        // The stored token, while it is fresh: no request, and no lock.
        let stored = &parameters[policy::ACCESS_TOKEN_FIELD];
        if super::is_sealed(stored)
            && token_cache::token_is_fresh(token_cache::parse_expiry(
                parameters["token_expires_at"].as_str(),
            ))
        {
            return Ok((stored.clone(), None));
        }

        let config = runtara_agents::registry::find_connection_type(integration_id)
            .and_then(|meta| meta.oauth_config)
            .ok_or_else(unsupported)?;
        let effective =
            crate::auth::provider_auth::resolve_effective_oauth_config(config, parameters);
        let missing = |name: &str| {
            AuthResolutionError::permanent(format!("The connection has no {name} to refresh with"))
        };
        let client_id = parameters["client_id"]
            .as_str()
            .ok_or_else(|| missing("client_id"))?;
        // A sealed client secret is seal's to send, beside the refresh
        // token; a readable one is this process's.
        let sealed_client = policy::sealed_authorized_client(integration_id, parameters);
        let client_secret = match parameters[policy::CLIENT_SECRET_FIELD].as_str() {
            Some(client_secret) => client_secret,
            None if sealed_client => "",
            None => return Err(missing("client_secret")),
        };

        let cache_key = token_cache::build_token_cache_key(
            connection_id,
            &["oauth_refresh", integration_id, &tokens.token_url],
        )
        .sealed();
        let refreshed = token_cache::refreshed_token(&cache_key, || async {
            // The grant, with the client's credentials as the provider takes
            // them. seal adds the refresh token.
            let grant = vec![("grant_type".to_string(), "refresh_token".to_string())];
            let (authorization, body, presented) = if sealed_client {
                (
                    None,
                    client_form(effective.token_endpoint_auth, grant, client_id),
                    &[policy::REFRESH_TOKEN_FIELD, policy::CLIENT_SECRET_FIELD][..],
                )
            } else {
                let (authorization, body) = token_cache::token_request_parts(
                    effective.token_endpoint_auth,
                    grant,
                    client_id,
                    client_secret,
                );
                (authorization, body, &[policy::REFRESH_TOKEN_FIELD][..])
            };
            let outcome = async {
                let reply = self
                    .token_request(
                        tenant_id,
                        connection_id,
                        parameters,
                        presented,
                        &tokens.token_url,
                        "application/x-www-form-urlencoded",
                        authorization,
                        body.into_bytes(),
                    )
                    .await?;
                let refresh_field = FieldName::new(policy::REFRESH_TOKEN_FIELD).map_err(|_| {
                    AuthResolutionError::permanent("The refresh token's field is not valid")
                })?;
                Ok(token_cache::TokenResponse {
                    access_token: sealed_access_token(&reply)?,
                    expires_at: token_cache::token_expiry(
                        &reply.body,
                        token_cache::DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
                    ),
                    refresh_token: reply
                        .sealed
                        .get(&refresh_field)
                        .and_then(|envelope| serde_json::to_string(envelope).ok()),
                })
            }
            .await;
            // Every refresh made is a sign of the connection's health.
            crate::events::emit(
                events,
                Some(tenant_id),
                crate::events::ConnectionLifecycleEvent::TokenRefreshed {
                    connection_id: connection_id.to_string(),
                    integration: integration_id.to_string(),
                    success: outcome.is_ok(),
                },
            );
            outcome
        })
        .await?;

        match refreshed {
            token_cache::Refreshed::Kept(envelope) => Ok((marker_of(&envelope)?, None)),
            token_cache::Refreshed::Now(response) => {
                let access_token = self.stored_token(
                    integration_id,
                    parameters,
                    policy::ACCESS_TOKEN_FIELD,
                    &response.access_token,
                )?;
                let refresh_token = response
                    .refresh_token
                    .as_deref()
                    .map(|envelope| {
                        self.stored_token(
                            integration_id,
                            parameters,
                            policy::REFRESH_TOKEN_FIELD,
                            envelope,
                        )
                    })
                    .transpose()?;
                Ok((
                    access_token.clone(),
                    Some(RotatedSealed {
                        access_token,
                        refresh_token,
                        token_expires_at: response.expires_at,
                    }),
                ))
            }
        }
    }
}

/// What an authorization code was exchanged for: the pair, sealed, as the
/// markers a connection stores.
pub struct SealedGrant {
    pub access_token: Value,
    pub refresh_token: Option<Value>,
    /// The token endpoint's `expires_in`, as it sent it.
    pub expires_in: Value,
    /// The claims of the access token this process is told, if the type's
    /// policy names any.
    pub claims: Option<Value>,
}

impl SealedCredentials {
    /// A sealed token as the marker a connection stores: it names the policy
    /// sealed inside, which is the one the connection's type calls for.
    fn stored_token(
        &self,
        integration_id: &str,
        parameters: &Value,
        field: &str,
        envelope: &str,
    ) -> Result<Value, AuthResolutionError> {
        let unreadable = || AuthResolutionError::transient("A sealed token is not readable");
        let envelope: Envelope = serde_json::from_str(envelope).map_err(|_| unreadable())?;
        let fingerprint = policy::policies(integration_id, parameters, &self.allowances)
            .and_then(|policies| super::policy_fingerprint(policies.get(field)?).ok())
            .ok_or_else(|| {
                AuthResolutionError::permanent("The connection's tokens cannot be sealed")
            })?;
        super::marker(&envelope, &fingerprint).map_err(|_| unreadable())
    }

    /// Exchanges an authorization code for tokens, for a connection whose
    /// client secret is sealed: seal sends the secret with the code, and the
    /// pair comes back already sealed.
    #[allow(clippy::too_many_arguments)]
    pub async fn exchange_code(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<SealedGrant, AuthResolutionError> {
        let unsupported =
            || AuthResolutionError::permanent("The connection's code exchange cannot be formed");
        if !policy::sealed_authorized_client(integration_id, parameters) {
            return Err(unsupported());
        }
        let tokens =
            policy::authorized_tokens(integration_id, parameters).ok_or_else(unsupported)?;
        let config = runtara_agents::registry::find_connection_type(integration_id)
            .and_then(|meta| meta.oauth_config)
            .ok_or_else(unsupported)?;
        let effective =
            crate::auth::provider_auth::resolve_effective_oauth_config(config, parameters);
        let client_id = parameters["client_id"].as_str().ok_or_else(|| {
            AuthResolutionError::permanent("The connection has no client_id to exchange with")
        })?;
        let mut grant = vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), code.to_string()),
            ("redirect_uri".to_string(), redirect_uri.to_string()),
        ];
        if let Some(verifier) = code_verifier {
            grant.push(("code_verifier".to_string(), verifier.to_string()));
        }
        let reply = self
            .token_request(
                tenant_id,
                connection_id,
                parameters,
                &[policy::CLIENT_SECRET_FIELD],
                &tokens.token_url,
                "application/x-www-form-urlencoded",
                None,
                client_form(effective.token_endpoint_auth, grant, client_id).into_bytes(),
            )
            .await?;
        let access_field = FieldName::new(policy::ACCESS_TOKEN_FIELD)
            .map_err(|_| AuthResolutionError::permanent("The access token's field is not valid"))?;
        let refresh_field = FieldName::new(policy::REFRESH_TOKEN_FIELD).map_err(|_| {
            AuthResolutionError::permanent("The refresh token's field is not valid")
        })?;
        let refresh_token = reply
            .sealed
            .get(&refresh_field)
            .and_then(|envelope| serde_json::to_string(envelope).ok())
            .map(|envelope| {
                self.stored_token(
                    integration_id,
                    parameters,
                    policy::REFRESH_TOKEN_FIELD,
                    &envelope,
                )
            })
            .transpose()?;
        Ok(SealedGrant {
            access_token: self.stored_token(
                integration_id,
                parameters,
                policy::ACCESS_TOKEN_FIELD,
                &sealed_access_token(&reply)?,
            )?,
            refresh_token,
            expires_in: reply.body["expires_in"].clone(),
            claims: reply
                .claims
                .get(&access_field)
                .and_then(|claims| serde_json::from_str(claims).ok()),
        })
    }
}

/// A token request's form for a client whose secret seal adds: the grant,
/// and the client's id where the provider takes the client's credentials in
/// the body. Where it takes them in a Basic header, seal makes the header.
fn client_form(
    auth: runtara_dsl::agent_meta::TokenEndpointAuth,
    mut grant: Vec<(String, String)>,
    client_id: &str,
) -> String {
    if matches!(auth, runtara_dsl::agent_meta::TokenEndpointAuth::FormBody) {
        grant.push(("client_id".to_string(), client_id.to_string()));
    }
    token_cache::form_urlencoded(&grant)
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
