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
pub mod policy;

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
    pub async fn send(
        &self,
        tenant_id: &str,
        connection_id: &str,
        parameters: &Value,
        request: http::Request<Option<Vec<u8>>>,
    ) -> Result<SealedResponse, SealError> {
        let invalid = |detail: &str| SealError {
            code: SealErrorCode::BadRequest,
            detail: detail.to_owned(),
        };
        let mut sealed = BTreeMap::new();
        for (name, value) in parameters.as_object().into_iter().flatten() {
            if !is_sealed(value) {
                continue;
            }
            let envelope: Envelope =
                serde_json::from_value(value[SEALED_MARKER]["envelope"].clone())
                    .map_err(|_| invalid("a stored sealed value is not an envelope"))?;
            let field = FieldName::new(name.as_str())
                .map_err(|_| invalid("a sealed parameter's name is not a field name"))?;
            sealed.insert(field, SealedValue::from(envelope));
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
                    .map_err(|_| invalid("the connection's id is not an item id"))?,
                sealed,
                plain: BTreeMap::new(),
                request,
            })
            .await?;
        Ok(response.response)
    }
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
