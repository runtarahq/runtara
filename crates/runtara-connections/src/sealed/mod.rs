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
        // A rule names a private destination only when this deployment's
        // allowance listed its host as the secret was sealed.
        config.network.honour_private_rules = !allowances.private_hosts.is_empty();
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
        let keyset = self.keys.expect_tenant(tenant_id);
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
    /// `Ok(None)` means the connection is not sealed (see [`policy::policies`])
    /// and is stored as it always was.
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
                None => Ok(None),
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
                    let (keyset, kid, public) = key.as_ref().expect("the key was just set");
                    let binding = Binding {
                        keyset,
                        item: &item(connection_id)?,
                        field: &field_name(field)?,
                    };
                    let envelope =
                        seal_format::seal(public, kid, binding, secret.as_bytes(), policy)
                            .map_err(|error| SealingError::Seal(error.to_string()))?;
                    sealed.insert(field.clone(), marker(&envelope, &fingerprint)?);
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
        Ok(Some(Value::Object(sealed)))
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
                plain: BTreeMap::new(),
                request,
            })
            .await?;
        Ok(response.response)
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

/// A connection's sealed parameter, as seal takes it.
fn presented(parameters: &Value, name: &str) -> Result<(FieldName, SealedValue), SealError> {
    let value = &parameters[name];
    if !is_sealed(value) {
        return Err(invalid_request(
            "the connection holds no sealed value for this",
        ));
    }
    let envelope: Envelope = serde_json::from_value(value[SEALED_MARKER]["envelope"].clone())
        .map_err(|_| invalid_request("a stored sealed value is not an envelope"))?;
    let field = FieldName::new(name)
        .map_err(|_| invalid_request("a sealed parameter's name is not a field name"))?;
    Ok((field, SealedValue::from(envelope)))
}

/// The sealed secrets a connection's requests present to seal. Empty means a
/// request needs none: the connection's settings use no credential, or its
/// secrets are not sealed.
pub fn request_fields(integration_id: &str, parameters: &Value) -> Vec<String> {
    policy::request_fields(integration_id, parameters)
        .into_iter()
        .filter(|name| is_sealed(&parameters[name.as_str()]))
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

/// Turns sealing on: connections of the types seal handles are sealed as
/// they are saved. Off by default.
pub const SEALED_CREDENTIALS_ENV: &str = "RUNTARA_SEALED_CREDENTIALS";

/// The key that wraps tenants' seal keys: 32 bytes in base64. If unset, the key
/// is derived from the credential encryption key.
pub const WRAPPING_KEY_ENV: &str = "RUNTARA_SEAL_WRAPPING_KEY";

/// Whether [`SEALED_CREDENTIALS_ENV`] turns sealing on.
pub fn sealing_enabled(value: Option<&str>) -> bool {
    matches!(
        value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("true" | "1" | "yes" | "on")
    )
}

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
            "{SEALED_CREDENTIALS_ENV} is on, but there is no key to wrap tenants' seal keys with: \
             set {WRAPPING_KEY_ENV} (openssl rand -base64 32), or {}",
            crate::crypto::ENCRYPTION_KEY_ENV
        ));
    };
    let bytes = decode(crate::crypto::ENCRYPTION_KEY_ENV, value)?;
    Ok(keys::wrapping_key_from_encryption_key(&bytes))
}

/// seal for this deployment, from its environment: `None` unless
/// [`SEALED_CREDENTIALS_ENV`] turns it on. Tenants' keys are kept in `pool`.
/// Needs a Tokio runtime.
pub fn from_env(
    pool: sqlx::PgPool,
    allowances: Allowances,
) -> Result<Option<Arc<SealedCredentials>>, String> {
    if !sealing_enabled(std::env::var(SEALED_CREDENTIALS_ENV).ok().as_deref()) {
        return Ok(None);
    }
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
    Ok(Some(Arc::new(sealed)))
}

/// Whether a parameter's value is a sealed value's marker.
pub fn is_sealed(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key(SEALED_MARKER))
}

/// Whether any of a connection's parameters is sealed.
pub fn has_sealed_values(parameters: &Value) -> bool {
    parameters
        .as_object()
        .is_some_and(|fields| fields.values().any(is_sealed))
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
}

#[cfg(any(test, feature = "test-util"))]
impl TenantKeySource for MemoryKeySource {
    fn expect_tenant(&self, tenant_id: &str) -> KeysetId {
        keys::keyset_for_tenant(tenant_id)
    }
}

#[cfg(test)]
mod tests;
