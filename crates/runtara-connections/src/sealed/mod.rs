//! Sealed credentials: secrets held by seal, each with the rules for using it.
//!
//! seal runs in this process (library mode). The server can still open every
//! credential; what changes is that a secret is stored with the one place it
//! may go, and seal attaches it and sends the request.
//!
//! A sealed connection's parameters hold, for each secret, a marker in place
//! of the value: `{"$sealed": {"envelope": …, "policy": "<sha-256>"}}`. The
//! envelope is seal's; the hash names the policy sealed inside it, so that a
//! later change to the connection that would need another policy is noticed
//! without opening anything.

pub mod keys;
pub(crate) mod oauth;
pub mod policy;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use seal_broker::audit::{AuditFailed, AuditRecord, AuditSink};
use seal_broker::{Broker, BrokerConfig, Environment};
use seal_client::{SealClient, SealedValue, SendRequest};
use seal_format::{Binding, Envelope, FieldName, ItemId, KeyId, KeysetId, Policy, PublicKey};
use seal_keys::{KeySource, Keyring, KeyringConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use oauth::{RotatedSealed, SealedGrant};
pub use policy::Allowances;
pub use seal_client::{ErrorCode as SealErrorCode, SealError};

/// The key of a sealed value's marker.
pub const SEALED_MARKER: &str = "$sealed";

/// A key source that is told which tenant a keyset is for before a key is
/// made for it.
pub trait TenantKeySource: KeySource {
    /// The tenant's keyset.
    fn expect_tenant(&self, tenant_id: &str) -> KeysetId;
}

impl TenantKeySource for keys::PgKeySource {
    fn expect_tenant(&self, tenant_id: &str) -> KeysetId {
        keys::PgKeySource::expect_tenant(self, tenant_id)
    }
}

/// Why a connection's secrets could not be sealed.
#[derive(Debug, thiserror::Error)]
pub enum SealingError {
    /// A secret that was not entered again would need a policy other than the
    /// one it was sealed with: where it goes, or how it is sent, changed.
    #[error(
        "`{field}` must be entered again: the change alters where this credential is sent or how"
    )]
    SecretRequired { field: String },
    /// The parameters hold a secret seal cannot hold, and nothing else may:
    /// why, in words for whoever entered them.
    #[error("this connection's credentials cannot be stored: {reason}")]
    CannotSeal { reason: String },
    /// seal refused or failed. Never includes a value.
    #[error("sealing failed: {0}")]
    Seal(String),
}

/// A response from a request sent with sealed credentials.
pub type SealedResponse = http::Response<seal_client::Body>;

/// A response read whole, within a byte limit.
#[derive(Debug)]
pub struct BoundedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Why a request with sealed credentials got no response.
#[derive(Debug)]
pub enum SendError {
    /// The request could not be formed: its method, URL or a header is not valid.
    InvalidRequest,
    /// seal refused or failed. `code` is seal's error code; the detail has
    /// identifiers and static text only.
    Seal(SealError),
    /// The response is larger than the limit.
    TooLarge,
    /// The response's body failed part way.
    BodyFailed,
}

/// seal, and what this deployment needs to use it for connections.
pub struct SealedCredentials {
    client: Arc<dyn SealClient>,
    keys: Arc<dyn TenantKeySource>,
    allowances: Allowances,
}

/// Writes seal's audit records to the log.
struct LogAudit;

impl AuditSink for LogAudit {
    fn record(&self, record: &AuditRecord) -> Result<(), AuditFailed> {
        let line = serde_json::to_string(record).map_err(|_| AuditFailed)?;
        tracing::info!(target: "seal.audit", audit = %line);
        Ok(())
    }
}

impl SealedCredentials {
    /// seal in this process, with tenants' keys from `keys`, wrapped by
    /// `wrapping_key` (32 bytes). Needs a Tokio runtime.
    pub fn in_process(
        keys: Arc<dyn TenantKeySource>,
        wrapping_key: &[u8],
        allowances: Allowances,
    ) -> Result<Self, String> {
        let wrap = keys::key_wrap(wrapping_key).map_err(|error| error.to_string())?;
        let source: Arc<dyn KeySource> = keys.clone();
        let keyring = Keyring::new(source, wrap, KeyringConfig::default())
            .map_err(|error| error.to_string())?;
        let mut config = BrokerConfig::default();
        for pem in &allowances.extra_roots {
            use rustls_pki_types::pem::PemObject;
            for certificate in rustls_pki_types::CertificateDer::pem_slice_iter(pem.as_bytes()) {
                config.client.extra_roots.push(
                    certificate.map_err(|_| "an extra root is not a PEM certificate".to_owned())?,
                );
            }
        }
        // A rule names a private destination only when this deployment's
        // allowance listed its host as the secret was sealed.
        config.network.honour_private_rules =
            !allowances.private_hosts.is_empty() || allowances.private_databases;
        // A database with no TLS is reached only on a host this deployment
        // lets be reached in the clear; a password is sealed for one only
        // then.
        config.network.honour_plaintext_postgres_rules = !allowances.plain_http_hosts.is_empty();
        let buffer_cap = config.limits.buffer_cap;
        let environment =
            Environment::production(Arc::new(LogAudit)).map_err(|error| error.to_string())?;
        let broker = Broker::new(config, Arc::new(keyring.clone()), environment)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client: Arc::new(seal_client::in_process(broker, Some(keyring), buffer_cap)),
            keys,
            allowances,
        })
    }

    /// How seal runs: `in_process`, `unix` or `mtls`.
    pub fn mode(&self) -> &'static str {
        self.client.mode().as_str()
    }

    /// The tenant's keyset and its current public key, making the keyset's
    /// first key if it has none.
    async fn sealing_key(
        &self,
        tenant_id: &str,
    ) -> Result<(KeysetId, KeyId, PublicKey), SealError> {
        self.current_key(self.keys.expect_tenant(tenant_id)).await
    }

    /// A keyset's current public key, making its first key if it has none.
    async fn current_key(
        &self,
        keyset: KeysetId,
    ) -> Result<(KeysetId, KeyId, PublicKey), SealError> {
        let current = match self.client.public_keys(&keyset).await {
            Ok(keys) => keys.into_iter().next_back(),
            Err(error) if error.code == SealErrorCode::KeysetUnknown => None,
            Err(error) => return Err(error),
        };
        let current = match current {
            Some(current) => current,
            None => {
                match self.client.generate_keyset(&keyset).await {
                    Ok(_) => {}
                    // Another instance made the first key meanwhile.
                    Err(error) if error.code == SealErrorCode::Conflict => {}
                    Err(error) => return Err(error),
                }
                let keys = self.client.public_keys(&keyset).await?;
                keys.into_iter().next_back().ok_or(SealError {
                    code: SealErrorCode::KeysetUnknown,
                    detail: "the keyset has no key after one was made".to_owned(),
                })?
            }
        };
        let invalid = |what: &str| SealError {
            code: SealErrorCode::Internal,
            detail: format!("seal returned an invalid {what}"),
        };
        Ok((
            keyset,
            KeyId::new(current.kid).map_err(|_| invalid("key id"))?,
            PublicKey::from_base64url(&current.public).map_err(|_| invalid("public key"))?,
        ))
    }

    /// The parameters to store for a connection, with each secret sealed.
    ///
    /// `Ok(None)` means the parameters hold no secret, and are stored as they
    /// are. A secret is stored sealed or not at all: parameters with one seal
    /// cannot hold (see [`policy::policies`]) are refused, with the reason.
    ///
    /// `stored` is what the connection holds now, if it exists. A secret may be
    /// left as its marker only if `stored` has that very marker, and the policy
    /// it was sealed with is still the one these parameters call for. A
    /// connection that was sealed never goes back: if its new parameters cannot
    /// be sealed, its secrets must be entered again.
    pub async fn seal_parameters(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
        stored: Option<&Value>,
    ) -> Result<Option<Value>, SealingError> {
        // What is sealed of a connection may be part of what was entered.
        let prepared = policy::prepared(integration_id, parameters);
        let parameters = &prepared;
        let Some(fields) = parameters.as_object() else {
            return Ok(None);
        };
        let kept: Vec<&String> = fields
            .iter()
            .filter(|(_, value)| is_sealed(value))
            .map(|(name, _)| name)
            .collect();
        let Some(policies) = policy::policies(integration_id, parameters, &self.allowances) else {
            return match kept.first() {
                Some(field) => Err(SealingError::SecretRequired {
                    field: (*field).clone(),
                }),
                None if policy::readable_secrets(integration_id, parameters).is_empty() => Ok(None),
                None => Err(SealingError::CannotSeal {
                    reason: policy::why_unsealable(integration_id, parameters, &self.allowances),
                }),
            };
        };
        // A marker anywhere but a secret's own field is not one this made.
        if let Some(field) = kept.iter().find(|name| !policies.contains_key(**name)) {
            return Err(SealingError::SecretRequired {
                field: (*field).clone(),
            });
        }

        let mut sealed = fields.clone();
        let mut key = None;
        for (field, policy) in &policies {
            let fingerprint = policy_fingerprint(policy)?;
            match fields.get(field) {
                Some(Value::String(secret)) if !secret.is_empty() => {
                    if key.is_none() {
                        key = Some(self.sealing_key(tenant_id).await.map_err(seal_failed)?);
                    }
                    let key = key.as_ref().expect("the key was just set");
                    sealed.insert(
                        field.clone(),
                        seal_one(key, connection_id, field, secret, policy)?,
                    );
                }
                Some(value) if is_sealed(value) => {
                    let unchanged = stored.and_then(|stored| stored.get(field)) == Some(value)
                        && value[SEALED_MARKER]["policy"].as_str() == Some(fingerprint.as_str());
                    if !unchanged {
                        return Err(SealingError::SecretRequired {
                            field: field.clone(),
                        });
                    }
                }
                // Not entered yet, or cleared: nothing to seal.
                _ => {}
            }
        }
        // Each extra header of an MCP connection is a secret of its own,
        // sealed where it is: in the map, under the header's name.
        let held = stored.map(|stored| &stored[policy::EXTRA_HEADERS_FIELD]);
        if integration_id == "mcp"
            && let Some(extras) = sealed
                .get_mut(policy::EXTRA_HEADERS_FIELD)
                .and_then(Value::as_object_mut)
        {
            for (header, value) in extras.iter_mut() {
                let parameter = policy::extra_header_parameter(header);
                let policy = policy::extra_header_policy(parameters, header, &self.allowances)
                    .ok_or_else(|| SealingError::CannotSeal {
                        reason: policy::why_unsealable(
                            integration_id,
                            parameters,
                            &self.allowances,
                        ),
                    })?;
                match value {
                    Value::String(secret) if !secret.is_empty() => {
                        if key.is_none() {
                            key = Some(self.sealing_key(tenant_id).await.map_err(seal_failed)?);
                        }
                        let key = key.as_ref().expect("the key was just set");
                        *value = seal_one(
                            key,
                            connection_id,
                            &policy::extra_header_field(header),
                            secret,
                            &policy,
                        )?;
                    }
                    kept if is_sealed(kept) => {
                        let unchanged = held.map(|held| &held[header.as_str()]) == Some(&*kept)
                            && kept[SEALED_MARKER]["policy"].as_str()
                                == Some(policy_fingerprint(&policy)?.as_str());
                        if !unchanged {
                            return Err(SealingError::SecretRequired { field: parameter });
                        }
                    }
                    _ => {}
                }
            }
        }
        let sealed = Value::Object(sealed);
        // A secret the type's policies do not cover would be stored readable.
        if let Some(field) = policy::readable_secrets(integration_id, &sealed).first() {
            return Err(SealingError::CannotSeal {
                reason: format!("`{field}` is a secret this connection has no use for"),
            });
        }
        Ok(Some(sealed))
    }

    /// Seals a platform OAuth app's client secret, once for every connection
    /// on the app: to the platform's own key, to be sent only to the token
    /// endpoint of the app's connection type, for a code exchange or a
    /// refresh. `parameters` are those of the app's connections that say
    /// where that is, for a type that has several (a Zoho data centre).
    ///
    /// The marker names the app's record beside the envelope, as a request
    /// for a tenant's connection presents it.
    pub async fn seal_app_secret(
        &self,
        app_id: &str,
        integration_id: &str,
        client_id: &str,
        parameters: &Value,
        client_secret: &str,
    ) -> Result<Value, SealingError> {
        let of_app = app_parameters(app_id, client_id, parameters);
        let policy = policy::shared_client_policy(integration_id, &of_app, &self.allowances)
            .ok_or_else(|| SealingError::CannotSeal {
                reason: format!(
                    "the platform OAuth app '{app_id}' names no token endpoint seal can send \
                     its client secret to"
                ),
            })?;
        let key = self
            .current_key(keys::platform_keyset())
            .await
            .map_err(seal_failed)?;
        let record = app_record(app_id);
        let mut marker = seal_one(
            &key,
            &record,
            policy::CLIENT_SECRET_FIELD,
            client_secret,
            &policy,
        )?;
        marker[SEALED_MARKER][SHARED_RECORD] = Value::String(record);
        Ok(marker)
    }

    /// Whether `marker` is a platform OAuth app's client secret this
    /// deployment can open: sealed for `app_id`, to a key of the platform's
    /// that is here. A value sealed for another deployment, or for another
    /// app, is not.
    pub async fn holds_app_secret(&self, app_id: &str, marker: &Value) -> bool {
        let Ok(envelope) =
            serde_json::from_value::<Envelope>(marker[SEALED_MARKER]["envelope"].clone())
        else {
            return false;
        };
        let platform = keys::platform_keyset();
        if !is_shared(marker)
            || marker[SEALED_MARKER][SHARED_RECORD].as_str() != Some(app_record(app_id).as_str())
            || *envelope.keyset() != platform
        {
            return false;
        }
        self.client
            .public_keys(&platform)
            .await
            .is_ok_and(|keys| keys.iter().any(|key| key.kid == envelope.kid().as_str()))
    }

    /// [`Self::with_shared_client`] for `app`.
    pub fn with_app(
        &self,
        integration_id: &str,
        parameters: &Value,
        app: &crate::oauth_apps::OAuthAppCredentials,
    ) -> Result<Value, SealingError> {
        self.with_shared_client(
            integration_id,
            parameters,
            &app.app_id,
            &app.client_id,
            &app.client_secret,
        )
    }

    /// A connection on a platform OAuth app, as seal makes a request to the
    /// provider for it (a code exchange, a refresh, a revocation): with the
    /// app's sealed client secret among its parameters, for seal to send.
    /// For one request: the result is never stored.
    ///
    /// Refuses a connection whose parameters call for another policy than
    /// the secret was sealed with: its tokens would go, or come from,
    /// somewhere the app's secret was not sealed for.
    pub fn with_shared_client(
        &self,
        integration_id: &str,
        parameters: &Value,
        app_id: &str,
        client_id: &str,
        client_secret: &Value,
    ) -> Result<Value, SealingError> {
        let cannot = |reason: String| SealingError::CannotSeal { reason };
        if !is_shared(client_secret) {
            return Err(cannot(format!(
                "the platform OAuth app '{app_id}' has no sealed client secret"
            )));
        }
        let mut own = parameters.clone();
        let fields = own
            .as_object_mut()
            .ok_or_else(|| cannot("the connection has no parameters".to_owned()))?;
        fields.insert("client_id".to_owned(), Value::String(client_id.to_owned()));
        fields.insert(
            policy::CLIENT_SECRET_FIELD.to_owned(),
            client_secret.clone(),
        );
        let current = policy::shared_client_policy(integration_id, &own, &self.allowances)
            .map(|policy| policy_fingerprint(&policy))
            .transpose()?;
        if current.is_none()
            || client_secret[SEALED_MARKER]["policy"].as_str() != current.as_deref()
        {
            return Err(cannot(format!(
                "the platform OAuth app '{app_id}' was sealed for other endpoints than this \
                 connection's: the connection's region differs from the app's, or the server's \
                 settings changed since the app was sealed"
            )));
        }
        Ok(own)
    }

    /// Sends a request with a sealed connection's credentials. seal attaches
    /// them, and refuses a destination their policies do not allow.
    ///
    /// Only the secrets the connection's requests use are presented (see
    /// [`policy::request_fields`]): one that only verifies may go nowhere, and
    /// presenting it would have the whole request refused.
    pub async fn send(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
        request: http::Request<Option<Vec<u8>>>,
    ) -> Result<SealedResponse, SealError> {
        let mut sealed = BTreeMap::new();
        for name in request_fields(integration_id, parameters) {
            let (field, value) = presented(parameters, &name)?;
            sealed.insert(field, value);
        }
        let mut request = request;
        // SigV4 signs the body's hash. This process holds the body, so it
        // says the hash and seal signs with it, without holding the body too.
        if policy::signs_with_sigv4(integration_id, parameters) {
            let hash = hex::encode(Sha256::digest(
                request.body().as_deref().unwrap_or_default(),
            ));
            request.headers_mut().insert(
                "x-amz-content-sha256",
                http::HeaderValue::from_str(&hash)
                    .map_err(|_| invalid_request("the body's hash is not a header value"))?,
            );
        }
        let request = request.map(|body| {
            Full::new(Bytes::from(body.unwrap_or_default()))
                .map_err(|never| match never {})
                .boxed_unsync()
        });
        let response = self
            .client
            .send(SendRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item: ItemId::new(connection_id)
                    .map_err(|_| invalid_request("the connection's id is not an item id"))?,
                sealed,
                plain: plain_parts(integration_id, parameters)?,
                request,
            })
            .await?;
        Ok(response.response)
    }

    /// Opens a session with a sealed Postgres connection's database. seal
    /// logs in with the sealed password and relays the session: speak
    /// Postgres over the stream with no TLS and no password, as the
    /// connection's user and database.
    pub async fn connect_pg(
        &self,
        tenant_id: &str,
        connection_id: &str,
        parameters: &Value,
    ) -> Result<seal_client::PgStream, SealError> {
        let target = policy::postgres_target(parameters)
            .ok_or_else(|| invalid_request("the connection names no database to log in to"))?;
        let (field, value) = presented(parameters, policy::PG_URL_FIELD)?;
        self.client
            .connect_pg(seal_client::ConnectRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item: ItemId::new(connection_id)
                    .map_err(|_| invalid_request("the connection's id is not an item id"))?,
                sealed: BTreeMap::from([(field, value)]),
                plain: BTreeMap::new(),
                host: target.host,
                port: target.port,
            })
            .await
    }

    /// A presigned URL (S3) or a URL with a shared access signature (Azure
    /// Blob) for one object of a sealed storage connection, lasting
    /// `expires_s` seconds. seal refuses an object, a method or a lifetime
    /// the key's policy does not allow.
    ///
    /// Unlike everything else seal returns, the URL is a credential: whoever
    /// holds it can use it until it expires. Never log it.
    #[allow(clippy::too_many_arguments)]
    pub async fn presign(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
        method: &str,
        url: &str,
        expires_s: u32,
    ) -> Result<zeroize::Zeroizing<String>, SealError> {
        let mut sealed = BTreeMap::new();
        for name in request_fields(integration_id, parameters) {
            let (field, value) = presented(parameters, &name)?;
            sealed.insert(field, value);
        }
        let method = http::Method::from_bytes(method.as_bytes())
            .map_err(|_| invalid_request("the method is not valid"))?;
        self.client
            .presign(seal_client::PresignRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item: ItemId::new(connection_id)
                    .map_err(|_| invalid_request("the connection's id is not an item id"))?,
                sealed,
                plain: BTreeMap::new(),
                method,
                url: url.to_owned(),
                expires_s,
            })
            .await
    }

    /// Whether a connection holds sealed OAuth tokens that its parameters no
    /// longer allow: they were sealed for another token endpoint or API, and
    /// only a new authorization replaces them.
    pub fn holds_stale_tokens(&self, integration_id: &str, parameters: &Value) -> bool {
        let policies = policy::policies(integration_id, parameters, &self.allowances);
        [policy::ACCESS_TOKEN_FIELD, policy::REFRESH_TOKEN_FIELD]
            .iter()
            .filter(|field| is_sealed(&parameters[**field]))
            .any(|field| {
                let current = policies
                    .as_ref()
                    .and_then(|policies| policies.get(*field))
                    .and_then(|policy| policy_fingerprint(policy).ok());
                parameters[*field][SEALED_MARKER]["policy"].as_str() != current.as_deref()
            })
    }

    /// Revokes a connection's sealed token at the provider: the refresh
    /// token, or the access token if that is all it holds. The client's own
    /// secret goes with it where the provider wants it, sealed too.
    /// `Ok(false)` if the connection has no sealed token or nowhere to
    /// revoke it.
    pub async fn revoke_token(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
    ) -> Result<bool, String> {
        let Some(url) = policy::authorized_tokens(integration_id, parameters)
            .and_then(|tokens| tokens.revocation_url)
        else {
            return Ok(false);
        };
        let Some(field) = [policy::REFRESH_TOKEN_FIELD, policy::ACCESS_TOKEN_FIELD]
            .into_iter()
            .find(|field| is_sealed(&parameters[*field]))
        else {
            return Ok(false);
        };
        crate::net::validate_credentialed_endpoint(&url)
            .map_err(|reason| format!("refusing to send the revocation request: {reason}"))?;
        let (name, value) = presented(parameters, field).map_err(|error| error.detail)?;
        let mut values = BTreeMap::from([(name, value)]);
        let request = http::Request::builder()
            .method("POST")
            .uri(url)
            .header("content-type", "application/json");
        // The client secret is seal's to send, where the provider has the
        // client say who it is.
        if policy::client_secret_revokes(integration_id, parameters) {
            let (name, value) =
                presented(parameters, policy::CLIENT_SECRET_FIELD).map_err(|error| error.detail)?;
            values.insert(name, value);
        }
        // seal names the token in the body, as its policy says.
        let request = request
            .body(
                Full::new(Bytes::from_static(b"{}"))
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .map_err(|_| "the revocation endpoint's URL is not valid".to_string())?;
        let sent = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.client.send(SendRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item: ItemId::new(connection_id)
                    .map_err(|_| "the connection's id is not an item id".to_string())?,
                sealed: values,
                plain: BTreeMap::new(),
                request,
            }),
        )
        .await
        .map_err(|_| "revocation request timed out".to_string())?
        .map_err(|error| format!("revocation request failed: {error}"))?;
        let status = sent.response.status();
        if !status.is_success() {
            return Err(format!("revocation endpoint returned {status}"));
        }
        Ok(true)
    }

    /// Destroys a tenant's seal keys, for a tenant that is leaving: every
    /// secret sealed for it, in this database and in every copy of its rows,
    /// can no longer be opened. This cannot be undone. A backup of the key
    /// table still opens them while the wrapping key exists.
    pub async fn destroy_tenant_keys(&self, tenant_id: &str) -> Result<(), SealError> {
        match self
            .client
            .destroy_keyset(&keys::keyset_for_tenant(tenant_id))
            .await
        {
            // A tenant with no keys, or whose keys are already gone, has
            // nothing left to open: offboarding can be run again.
            Err(error) if error.code == SealErrorCode::KeysetUnknown => Ok(()),
            outcome => outcome,
        }
    }

    /// Seals the secret token of a webhook of a connection: Telegram's, which
    /// it sends back with each call, registered at `webhook_url`; or, with no
    /// URL, one that is only ever checked. The marker names the connection
    /// too: a trigger moved to another connection holds a token that is not
    /// that connection's.
    pub async fn seal_webhook_secret(
        &self,
        tenant_id: &str,
        connection_id: &str,
        webhook_url: Option<&str>,
        token: &str,
    ) -> Result<Value, SealingError> {
        let policy = policy::telegram_webhook_secret_policy(webhook_url, &self.allowances)
            .ok_or_else(|| SealingError::Seal("the webhook's URL names no rule".to_owned()))?;
        let (keyset, kid, public) = self.sealing_key(tenant_id).await.map_err(seal_failed)?;
        let binding = Binding {
            keyset: &keyset,
            item: &item(connection_id)?,
            field: &field_name(policy::WEBHOOK_SECRET_FIELD)?,
        };
        let envelope = seal_format::seal(&public, &kid, binding, token.as_bytes(), &policy)
            .map_err(|error| SealingError::Seal(error.to_string()))?;
        let mut marker = marker(&envelope, &policy_fingerprint(&policy)?)?;
        marker[SEALED_MARKER]["connection"] = Value::String(connection_id.to_owned());
        Ok(marker)
    }

    /// Whether a sealed webhook secret token is the one a webhook registered
    /// for this connection at `webhook_url` would have: sealed for this
    /// connection, and for that URL (or, with none, only to be checked).
    pub fn webhook_secret_is_current(
        &self,
        connection_id: &str,
        webhook_url: Option<&str>,
        marker: &Value,
    ) -> bool {
        let current = policy::telegram_webhook_secret_policy(webhook_url, &self.allowances)
            .and_then(|policy| policy_fingerprint(&policy).ok());
        marker[SEALED_MARKER]["connection"].as_str() == Some(connection_id)
            && current.is_some()
            && marker[SEALED_MARKER]["policy"].as_str() == current.as_deref()
    }

    /// Whether `presented` is a sealed webhook secret token: the one that
    /// came with a webhook call, checked without reading the sealed one.
    /// seal says only yes or no.
    ///
    /// An error means the check could not be made; treat it as a failed
    /// check.
    pub async fn verify_webhook_secret(
        &self,
        tenant_id: &str,
        connection_id: &str,
        marker: &Value,
        presented: &str,
    ) -> Result<bool, SealError> {
        let envelope: Envelope = serde_json::from_value(marker[SEALED_MARKER]["envelope"].clone())
            .map_err(|_| invalid_request("a stored sealed value is not an envelope"))?;
        self.client
            .verify(seal_client::VerifyRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item: ItemId::new(connection_id)
                    .map_err(|_| invalid_request("the connection's id is not an item id"))?,
                field: FieldName::new(policy::WEBHOOK_SECRET_FIELD)
                    .map_err(|_| invalid_request("the field is not a field name"))?,
                value: SealedValue::from(envelope),
                // A token covers no message.
                message: Bytes::new(),
                mac: presented.as_bytes().to_vec(),
            })
            .await
    }

    /// Whether `mac` is the HMAC of `message` under a sealed signing secret:
    /// a webhook's signature, checked without reading the secret. seal says
    /// only yes or no. The caller builds the message as the provider signs it
    /// and decodes the code to bytes, and checks any timestamp itself.
    ///
    /// An error means the check could not be made; treat it as a failed check.
    pub async fn verify(
        &self,
        tenant_id: &str,
        connection_id: &str,
        parameters: &Value,
        field: &str,
        message: Vec<u8>,
        mac: Vec<u8>,
    ) -> Result<bool, SealError> {
        let (field, value) = presented(parameters, field)?;
        self.client
            .verify(seal_client::VerifyRequest {
                keyset: keys::keyset_for_tenant(tenant_id),
                item: ItemId::new(connection_id)
                    .map_err(|_| invalid_request("the connection's id is not an item id"))?,
                field,
                value,
                message: Bytes::from(message),
                mac,
            })
            .await
    }
}

fn invalid_request(detail: &str) -> SealError {
    SealError {
        code: SealErrorCode::BadRequest,
        detail: detail.to_owned(),
    }
}

/// The parts of a connection's credential that are not secret and that a
/// request names, as seal takes them.
fn plain_parts(
    integration_id: &str,
    parameters: &Value,
) -> Result<BTreeMap<FieldName, String>, SealError> {
    policy::plain_fields(integration_id, parameters)
        .into_iter()
        .map(|(name, value)| {
            FieldName::new(name)
                .map(|name| (name, value))
                .map_err(|_| invalid_request("a plain part's name is not a field name"))
        })
        .collect()
}

/// A connection's sealed parameter, as seal takes it.
fn presented(parameters: &Value, name: &str) -> Result<(FieldName, SealedValue), SealError> {
    // An extra header's value is in the map of them, and sealed under a
    // name made from the header's.
    let header = policy::extra_header_of(name);
    let field = header.map(policy::extra_header_field);
    let name = field.as_deref().unwrap_or(name);
    let value = match header {
        Some(header) => &parameters[policy::EXTRA_HEADERS_FIELD][header],
        None => &parameters[name],
    };
    if !is_sealed(value) {
        return Err(invalid_request(
            "the connection holds no sealed value for this",
        ));
    }
    let envelope: Envelope = serde_json::from_value(value[SEALED_MARKER]["envelope"].clone())
        .map_err(|_| invalid_request("a stored sealed value is not an envelope"))?;
    let field = FieldName::new(name)
        .map_err(|_| invalid_request("a sealed parameter's name is not a field name"))?;
    // A platform's value is bound to its own record, not the connection's.
    let value = match value[SEALED_MARKER][SHARED_RECORD].as_str() {
        Some(record) => SealedValue::from_record(
            envelope,
            ItemId::new(record)
                .map_err(|_| invalid_request("a shared value's record is not an item id"))?,
        ),
        None => SealedValue::from(envelope),
    };
    Ok((field, value))
}

/// The sealed secrets a connection's requests present to seal. Empty means a
/// request needs none: the connection's settings use no credential, or its
/// secrets are not sealed.
pub fn request_fields(integration_id: &str, parameters: &Value) -> Vec<String> {
    policy::request_fields(integration_id, parameters)
        .into_iter()
        .filter(|name| match policy::extra_header_of(name) {
            Some(header) => is_sealed(&parameters[policy::EXTRA_HEADERS_FIELD][header]),
            None => is_sealed(&parameters[name.as_str()]),
        })
        .collect()
}

impl SealedCredentials {
    /// Sends a request for the outbound proxy and reads the response whole.
    ///
    /// The agent's own copy of the header seal sets is dropped, as are `Host`
    /// and `Content-Length`, which seal sets from the URL and the body. The
    /// response's headers and body together may not exceed `limit` bytes.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_bounded(
        &self,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        parameters: &Value,
        method: &str,
        url: &str,
        headers: impl IntoIterator<Item = (String, String)>,
        body: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<BoundedResponse, SendError> {
        let injected = policy::injected_headers(integration_id, parameters);
        let mut builder = http::Request::builder().method(method).uri(url);
        for (name, value) in headers {
            let dropped = ["host", "content-length"]
                .iter()
                .any(|set_by_seal| name.eq_ignore_ascii_case(set_by_seal))
                || injected
                    .iter()
                    .any(|injected| name.eq_ignore_ascii_case(injected));
            if !dropped {
                builder = builder.header(name, value);
            }
        }
        let request = builder.body(body).map_err(|_| SendError::InvalidRequest)?;
        let response = self
            .send(
                tenant_id,
                connection_id,
                integration_id,
                parameters,
                request,
            )
            .await
            .map_err(SendError::Seal)?;

        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_owned())))
            .collect();
        let header_bytes = headers.iter().fold(2_usize, |size, (name, value)| {
            size.saturating_add(name.len())
                .saturating_add(value.len())
                .saturating_add(16)
        });
        let body_limit = limit.checked_sub(header_bytes).ok_or(SendError::TooLarge)?;
        let mut upstream = response.into_body();
        let mut body = Vec::new();
        while let Some(frame) = upstream.frame().await {
            let frame = frame.map_err(|_| SendError::BodyFailed)?;
            if let Some(chunk) = frame.data_ref() {
                if chunk.len() > body_limit.saturating_sub(body.len()) {
                    return Err(SendError::TooLarge);
                }
                body.extend_from_slice(chunk);
            }
        }
        Ok(BoundedResponse {
            status,
            headers,
            body,
        })
    }
}

/// The key that wraps tenants' seal keys: 32 bytes in base64. If unset, the key
/// is derived from the credential encryption key.
pub const WRAPPING_KEY_ENV: &str = "RUNTARA_SEAL_WRAPPING_KEY";

/// The wrapping key: `wrapping_key` (base64, 32 bytes) if set, else one
/// derived from `encryption_key` (base64, as the credential cipher takes it).
pub fn wrapping_key(
    wrapping_key: Option<&str>,
    encryption_key: Option<&str>,
) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
    use base64::Engine;
    let decode = |name: &str, value: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .map(zeroize::Zeroizing::new)
            .map_err(|_| format!("{name} is not valid base64"))
    };
    fn set(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    if let Some(value) = set(wrapping_key) {
        let bytes = decode(WRAPPING_KEY_ENV, value)?;
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| format!("{WRAPPING_KEY_ENV} must be 32 bytes in base64"))?;
        return Ok(zeroize::Zeroizing::new(key));
    }
    let Some(value) = set(encryption_key) else {
        return Err(format!(
            "there is no key to wrap tenants' seal keys with: \
             set {WRAPPING_KEY_ENV} (openssl rand -base64 32), or {}",
            crate::crypto::ENCRYPTION_KEY_ENV
        ));
    };
    let bytes = decode(crate::crypto::ENCRYPTION_KEY_ENV, value)?;
    Ok(keys::wrapping_key_from_encryption_key(&bytes))
}

/// seal for this deployment, from its environment. Tenants' keys are kept in
/// `pool`, wrapped by [`WRAPPING_KEY_ENV`] or, without it, a key derived
/// from the credential encryption key. Needs a Tokio runtime.
pub fn from_env(
    pool: sqlx::PgPool,
    allowances: Allowances,
) -> Result<Arc<SealedCredentials>, String> {
    let key = wrapping_key(
        std::env::var(WRAPPING_KEY_ENV).ok().as_deref(),
        std::env::var(crate::crypto::ENCRYPTION_KEY_ENV)
            .ok()
            .as_deref(),
    )?;
    let sealed = SealedCredentials::in_process(
        Arc::new(keys::PgKeySource::new(pool)),
        key.as_ref(),
        allowances,
    )?;
    Ok(Arc::new(sealed))
}

/// Has tenants' seal keys follow a rotation of the credential encryption key.
///
/// Without [`WRAPPING_KEY_ENV`], the key that wraps them is derived from the
/// encryption key, so it changes when that key is rotated. The keys still
/// wrapped under one derived from a retired encryption key
/// ([`crate::crypto::factory::RETIRED_KEYS_ENV`]) are wrapped again under the current
/// one, so that the retired key can then be dropped as the rotation says.
/// Returns how many were. A deployment with its own wrapping key has nothing
/// to do here.
pub async fn follow_encryption_key_rotation(pool: &sqlx::PgPool) -> Result<usize, String> {
    use base64::Engine;
    let set = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|value| zeroize::Zeroizing::new(value.trim().to_owned()))
            .filter(|value| !value.is_empty())
    };
    if set(WRAPPING_KEY_ENV).is_some() {
        return Ok(0);
    }
    let Some(retired) = set(crate::crypto::factory::RETIRED_KEYS_ENV) else {
        return Ok(0);
    };
    let current = wrapping_key(
        None,
        set(crate::crypto::ENCRYPTION_KEY_ENV)
            .as_ref()
            .map(|key| key.as_str()),
    )?;
    let current = keys::key_wrap(current.as_ref()).map_err(|error| error.to_string())?;
    let mut wraps = Vec::new();
    for entry in retired
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (_, key) = entry.split_once(':').ok_or_else(|| {
            format!(
                "{} entries are kid:base64",
                crate::crypto::factory::RETIRED_KEYS_ENV
            )
        })?;
        let bytes = zeroize::Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(key.trim())
                .map_err(|_| {
                    format!(
                        "{} holds a key that is not base64",
                        crate::crypto::factory::RETIRED_KEYS_ENV
                    )
                })?,
        );
        let derived = keys::wrapping_key_from_encryption_key(&bytes);
        wraps.push(keys::key_wrap(derived.as_ref()).map_err(|error| error.to_string())?);
    }
    keys::rewrap_retired(pool, current.as_ref(), &wraps).await
}

#[cfg(any(test, feature = "test-util"))]
impl SealedCredentials {
    /// seal for a test, as a development deployment has it: in this process,
    /// and allowed to reach loopback, where a test's stand-in for a provider
    /// listens. Needs a Tokio runtime.
    ///
    /// Every seal made here shares one set of keys, as the instances of one
    /// deployment do: what one seals, another opens.
    pub fn for_tests() -> Arc<Self> {
        let loopback = || vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
        Self::for_tests_with(Allowances {
            private_hosts: loopback(),
            plain_http_hosts: loopback(),
            private_databases: true,
            ..Allowances::default()
        })
    }

    /// [`Self::for_tests`], with only what a test lets seal reach.
    pub fn for_tests_with(allowances: Allowances) -> Arc<Self> {
        static KEYS: std::sync::OnceLock<Arc<MemoryKeySource>> = std::sync::OnceLock::new();
        Arc::new(
            Self::in_process(
                KEYS.get_or_init(|| Arc::new(MemoryKeySource::default()))
                    .clone(),
                &[7_u8; 32],
                allowances,
            )
            .expect("seal starts in a test"),
        )
    }
}

/// The marker of `secret`, sealed to a tenant's `key` for one field of one
/// connection, to be used as `policy` says.
fn seal_one(
    key: &(KeysetId, KeyId, PublicKey),
    connection_id: &str,
    field: &str,
    secret: &str,
    policy: &Policy,
) -> Result<Value, SealingError> {
    let (keyset, kid, public) = key;
    let binding = Binding {
        keyset,
        item: &item(connection_id)?,
        field: &field_name(field)?,
    };
    let envelope = seal_format::seal(public, kid, binding, secret.as_bytes(), policy)
        .map_err(|error| SealingError::Seal(error.to_string()))?;
    marker(&envelope, &policy_fingerprint(policy)?)
}

/// Whether a parameter's value is a sealed value's marker.
pub fn is_sealed(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key(SEALED_MARKER))
}

/// The key, in a marker, of the record a platform's shared value is bound
/// to. Only [`SealedCredentials::seal_app_secret`] makes such a marker, and a
/// connection never stores one.
const SHARED_RECORD: &str = "record";

/// Whether a value is the marker of a platform's shared value.
pub fn is_shared(value: &Value) -> bool {
    is_sealed(value) && value[SEALED_MARKER][SHARED_RECORD].is_string()
}

/// The record a platform OAuth app's client secret is bound to. An app's id
/// is any text, so the record is named by its hash.
fn app_record(app_id: &str) -> String {
    format!(
        "oauth-app:{}",
        hex::encode(Sha256::digest(app_id.as_bytes()))
    )
}

/// The parameters of a connection on a platform app that its client secret's
/// policy is made from.
fn app_parameters(app_id: &str, client_id: &str, parameters: &Value) -> Value {
    let mut of_app = match parameters {
        Value::Object(_) => parameters.clone(),
        _ => json!({}),
    };
    of_app[crate::oauth_apps::OAUTH_APP_PARAM] = Value::String(app_id.to_owned());
    of_app["client_id"] = Value::String(client_id.to_owned());
    of_app
}

/// Whether any of a connection's parameters is sealed.
pub fn has_sealed_values(parameters: &Value) -> bool {
    parameters.as_object().is_some_and(|fields| {
        fields.values().any(is_sealed)
            // An MCP connection's extra headers are sealed in their map.
            || fields
                .get(policy::EXTRA_HEADERS_FIELD)
                .and_then(Value::as_object)
                .is_some_and(|extras| extras.values().any(is_sealed))
    })
}

fn marker(envelope: &Envelope, fingerprint: &str) -> Result<Value, SealingError> {
    let envelope =
        serde_json::to_value(envelope).map_err(|error| SealingError::Seal(error.to_string()))?;
    Ok(json!({ SEALED_MARKER: { "envelope": envelope, "policy": fingerprint } }))
}

fn policy_fingerprint(policy: &Policy) -> Result<String, SealingError> {
    let canonical = policy
        .canonical_json()
        .map_err(|error| SealingError::Seal(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

fn item(connection_id: &str) -> Result<ItemId, SealingError> {
    ItemId::new(connection_id)
        .map_err(|_| SealingError::Seal("the connection's id is not an item id".to_owned()))
}

fn field_name(field: &str) -> Result<FieldName, SealingError> {
    FieldName::new(field)
        .map_err(|_| SealingError::Seal(format!("`{field}` is not a field name seal accepts")))
}

fn seal_failed(error: SealError) -> SealingError {
    SealingError::Seal(format!("{}: {}", error.code.as_str(), error.detail))
}

/// Wrapped keys held in memory, for tests.
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct MemoryKeySource {
    keys: std::sync::Mutex<Vec<seal_keys::WrappedKey>>,
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait::async_trait]
impl KeySource for MemoryKeySource {
    async fn wrapped_keys(
        &self,
        keyset: &KeysetId,
    ) -> Result<Vec<seal_keys::WrappedKey>, seal_keys::KeyError> {
        let keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        Ok(keys
            .iter()
            .filter(|key| key.keyset == *keyset)
            .cloned()
            .collect())
    }

    async fn store(&self, key: &seal_keys::WrappedKey) -> Result<(), seal_keys::KeyError> {
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(key.clone());
        Ok(())
    }

    async fn delete_keyset(&self, keyset: &KeysetId) -> Result<(), seal_keys::KeyError> {
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|key| key.keyset != *keyset);
        Ok(())
    }
}

#[cfg(any(test, feature = "test-util"))]
impl TenantKeySource for MemoryKeySource {
    fn expect_tenant(&self, tenant_id: &str) -> KeysetId {
        keys::keyset_for_tenant(tenant_id)
    }
}

#[cfg(test)]
mod tests;
