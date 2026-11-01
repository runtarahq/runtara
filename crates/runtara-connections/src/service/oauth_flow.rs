//! OAuth flows in progress, held in Valkey.
//!
//! Starting a flow writes two keys, both expiring with the flow:
//!
//! - `{tenant}:oauth_state:{sha256(state)}`: the flow's record (connection,
//!   redirect URI, PKCE verifier, binding). It lives under the tenant's prefix,
//!   the key space a tenant-scoped Valkey user is limited to.
//! - `runtara:oauth_state:{sha256(state)}`: the tenant alone, so a callback
//!   whose URL names no tenant can find the record. Listing these keys yields
//!   hashes, from which no state can be recovered, and pointing one at another
//!   tenant leads to a tenant where no record exists.
//!
//! Redeeming a flow is one `GETDEL` on the record, so a state is usable once.
//!
//! How a flow is bound to whoever started it is fixed when it starts and kept
//! in the record ([`FlowBinding`]); the callback never chooses it.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How long a flow may take at the provider.
pub const FLOW_TTL_SECONDS: u64 = 600;

/// How a flow is bound to whoever started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowBinding {
    /// Started in the console: the callback must carry the flow's cookie, whose
    /// nonce hashes to `nonce_hash`.
    Browser { nonce_hash: String },
    /// Started without a browser: the connection waits for a member to
    /// confirm it in the console.
    Confirm { started_by: String },
}

/// A flow's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRecord {
    pub tenant_id: String,
    pub connection_id: String,
    pub integration_id: String,
    pub redirect_uri: String,
    pub code_verifier: Option<String>,
    pub binding: FlowBinding,
}

/// SHA-256 of `value`, hex.
pub fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn index_key(state_hash: &str) -> String {
    format!("runtara:oauth_state:{state_hash}")
}

fn record_key(tenant: &str, state_hash: &str) -> String {
    format!("{tenant}:oauth_state:{state_hash}")
}

/// A 32-byte random nonce, base64url: the browser binding's cookie value.
pub fn new_nonce() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The cookie that binds a browser flow, named from the state alone so the
/// callback finds it, and so two flows open at once get two cookies.
/// `__Host-` needs `Secure`, which not every browser keeps on plain http.
pub fn cookie_name(state: &str, secure: bool) -> String {
    let sid = &sha256_hex(state)[..16];
    if secure {
        format!("__Host-runtara-oauth-{sid}")
    } else {
        format!("runtara-oauth-{sid}")
    }
}

/// `Set-Cookie` for a browser flow's nonce.
pub fn binding_cookie(state: &str, nonce: &str, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{}={nonce}; Path=/; HttpOnly; SameSite=Lax; Max-Age={FLOW_TTL_SECONDS}{secure_attr}",
        cookie_name(state, secure)
    )
}

/// `Set-Cookie` that clears a flow's cookie.
pub fn clearing_cookie(state: &str, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure_attr}",
        cookie_name(state, secure)
    )
}

/// The value of cookie `name` in a `Cookie` header.
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Whether `nonce` is the one a browser binding was made with.
pub fn nonce_matches(binding_hash: &str, nonce: Option<&str>) -> bool {
    nonce.is_some_and(|nonce| {
        let hash = sha256_hex(nonce);
        // Equal length (both hex SHA-256), so this compares every byte.
        hash.bytes()
            .zip(binding_hash.bytes())
            .fold(hash.len() ^ binding_hash.len(), |acc, (a, b)| {
                acc | usize::from(a ^ b)
            })
            == 0
    })
}

/// Flows in progress, in Valkey.
#[derive(Clone)]
pub struct FlowStore {
    valkey: ConnectionManager,
}

impl FlowStore {
    pub fn new(valkey: ConnectionManager) -> Self {
        Self { valkey }
    }

    /// Store `record` as the flow `state` names. Refused if the state is
    /// already in use.
    pub async fn put(&self, state: &str, record: &FlowRecord) -> redis::RedisResult<()> {
        let hash = sha256_hex(state);
        let value = serde_json::to_string(record).map_err(|error| {
            redis::RedisError::from((
                redis::ErrorKind::TypeError,
                "flow record",
                error.to_string(),
            ))
        })?;
        let options = redis::SetOptions::default()
            .conditional_set(redis::ExistenceCheck::NX)
            .with_expiration(redis::SetExpiry::EX(FLOW_TTL_SECONDS));
        let mut valkey = self.valkey.clone();
        let (record_set, index_set): (Option<String>, Option<String>) = redis::pipe()
            .atomic()
            .set_options(record_key(&record.tenant_id, &hash), value, options)
            .set_options(index_key(&hash), &record.tenant_id, options)
            .query_async(&mut valkey)
            .await?;
        if record_set.is_none() || index_set.is_none() {
            return Err(redis::RedisError::from((
                redis::ErrorKind::ResponseError,
                "an OAuth state is already in use",
            )));
        }
        Ok(())
    }

    /// The tenant of the flow `state` names, if it is in progress.
    pub async fn tenant_of(&self, state: &str) -> redis::RedisResult<Option<String>> {
        let mut valkey = self.valkey.clone();
        valkey.get(index_key(&sha256_hex(state))).await
    }

    /// Redeem the flow `state` names in `tenant`: its record, removed so the
    /// state cannot be used again. `None` if there is no such flow in
    /// `tenant`, whether it never existed, expired or belongs elsewhere.
    pub async fn take(&self, tenant: &str, state: &str) -> redis::RedisResult<Option<FlowRecord>> {
        let hash = sha256_hex(state);
        let mut valkey = self.valkey.clone();
        let raw: Option<String> = valkey.get_del(record_key(tenant, &hash)).await?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        // The index entry goes with it; it expires on its own if this fails.
        let _: redis::RedisResult<()> = valkey.del(index_key(&hash)).await;
        let record: FlowRecord = serde_json::from_str(&raw).map_err(|error| {
            redis::RedisError::from((
                redis::ErrorKind::TypeError,
                "flow record",
                error.to_string(),
            ))
        })?;
        // A record is only ever stored under its own tenant's prefix.
        Ok((record.tenant_id == tenant).then_some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_hold_the_state_only_as_a_hash() {
        let hash = sha256_hex("s3cret-state");
        assert_eq!(index_key(&hash), format!("runtara:oauth_state:{hash}"));
        assert_eq!(
            record_key("acme", &hash),
            format!("acme:oauth_state:{hash}")
        );
        assert!(!index_key(&hash).contains("s3cret"));
    }

    #[test]
    fn the_cookie_is_named_from_the_state_and_host_prefixed_when_secure() {
        let name = cookie_name("state-1", true);
        assert!(name.starts_with("__Host-runtara-oauth-"));
        assert_eq!(name.len(), "__Host-runtara-oauth-".len() + 16);
        assert_ne!(name, cookie_name("state-2", true));
        assert!(cookie_name("state-1", false).starts_with("runtara-oauth-"));

        let set = binding_cookie("state-1", "nonce", true);
        assert!(set.starts_with(&format!("{name}=nonce;")));
        for attribute in [
            "Path=/",
            "HttpOnly",
            "SameSite=Lax",
            "Max-Age=600",
            "Secure",
        ] {
            assert!(set.contains(attribute), "{set}");
        }
        assert!(!binding_cookie("state-1", "nonce", false).contains("Secure"));
        assert!(clearing_cookie("state-1", true).contains("Max-Age=0"));
    }

    #[test]
    fn a_cookie_is_found_by_name_among_others() {
        let header = "a=1; __Host-runtara-oauth-0123=nonce-x; b=2";
        assert_eq!(
            cookie_value(header, "__Host-runtara-oauth-0123"),
            Some("nonce-x")
        );
        assert_eq!(cookie_value(header, "__Host-runtara-oauth-9999"), None);
    }

    #[test]
    fn only_the_flows_own_nonce_matches() {
        let nonce = new_nonce();
        let hash = sha256_hex(&nonce);
        assert!(nonce_matches(&hash, Some(&nonce)));
        assert!(!nonce_matches(&hash, Some(&new_nonce())));
        assert!(!nonce_matches(&hash, None));
        assert!(!nonce_matches(&hash, Some("")));
    }

    #[test]
    fn a_record_round_trips_with_its_binding() {
        for binding in [
            FlowBinding::Browser {
                nonce_hash: "h".into(),
            },
            FlowBinding::Confirm {
                started_by: "api_key:k1".into(),
            },
        ] {
            let record = FlowRecord {
                tenant_id: "acme".into(),
                connection_id: "c1".into(),
                integration_id: "hubspot_private_app".into(),
                redirect_uri: "https://app.example.com/api/oauth/callback".into(),
                code_verifier: None,
                binding,
            };
            let json = serde_json::to_string(&record).unwrap();
            assert_eq!(serde_json::from_str::<FlowRecord>(&json).unwrap(), record);
        }
    }
}
