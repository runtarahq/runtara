//! Checks for tests: whether a request a mock provider received is signed as
//! the provider would require, and what a connection stores in the clear.

use std::collections::HashMap;

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// `value` with every sealed value in it replaced by `null`: what is stored
/// in the clear, for a test to search for a secret.
///
/// A sealed value's ciphertext is random base64url, so any short text turns
/// up in it by chance now and then. Searching the whole value for a short
/// secret fails a test at random; searching this does not.
pub fn without_sealed(value: &Value) -> Value {
    match value {
        sealed if super::is_sealed(sealed) => Value::Null,
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), without_sealed(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_sealed).collect()),
        other => other.clone(),
    }
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// What a SigV4 `Authorization` header says of itself.
#[derive(Debug, PartialEq, Eq)]
pub struct SigV4Scope {
    pub access_key_id: String,
    pub region: String,
    pub service: String,
    pub signed_headers: Vec<String>,
}

/// Checks a received request's AWS Signature Version 4 against the secret
/// access key, as AWS does: the signature over the request as received, and
/// the body against the hash that was signed. `path` and `query` are as they
/// were sent. Returns what the signature is for, or why it does not hold.
pub fn check_sigv4(
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
    body: &[u8],
    secret_access_key: &str,
) -> Result<SigV4Scope, String> {
    let authorization = header(headers, "authorization").ok_or("no Authorization header")?;
    let rest = authorization
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or("not an AWS4-HMAC-SHA256 authorization")?;
    let mut parts = HashMap::new();
    for part in rest.split(',') {
        let (name, value) = part.trim().split_once('=').ok_or("a malformed part")?;
        parts.insert(name, value);
    }
    let credential: Vec<&str> = parts
        .get("Credential")
        .ok_or("no Credential")?
        .split('/')
        .collect();
    let [access_key_id, date, region, service, "aws4_request"] = credential[..] else {
        return Err("a malformed credential scope".into());
    };
    let signed_headers = *parts.get("SignedHeaders").ok_or("no SignedHeaders")?;
    let signature = *parts.get("Signature").ok_or("no Signature")?;

    let amz_date = header(headers, "x-amz-date").ok_or("no X-Amz-Date")?;
    if !amz_date.starts_with(date) {
        return Err("the date is not the scope's".into());
    }
    let payload_hash = header(headers, "x-amz-content-sha256").ok_or("no payload hash")?;
    if payload_hash != hex::encode(Sha256::digest(body)) {
        return Err("the payload hash is not the body's".into());
    }

    let mut canonical_headers = String::new();
    for name in signed_headers.split(';') {
        let value = header(headers, name).ok_or_else(|| format!("{name} is signed but absent"))?;
        canonical_headers.push_str(&format!("{name}:{}\n", value.trim()));
    }
    // A parameter with no value is signed as `name=`.
    let mut query: Vec<String> = query
        .unwrap_or("")
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| match parameter.contains('=') {
            true => parameter.to_owned(),
            false => format!("{parameter}="),
        })
        .collect();
    query.sort_unstable();
    let canonical_request = format!(
        "{method}\n{path}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        query.join("&"),
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let mut key = hmac(
        format!("AWS4{secret_access_key}").as_bytes(),
        date.as_bytes(),
    );
    for part in [region, service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    if hex::encode(hmac(&key, string_to_sign.as_bytes())) != signature {
        return Err("the signature does not match".into());
    }
    Ok(SigV4Scope {
        access_key_id: access_key_id.to_owned(),
        region: region.to_owned(),
        service: service.to_owned(),
        signed_headers: signed_headers.split(';').map(str::to_owned).collect(),
    })
}

/// Checks a received request's Azure Shared Key authorization against the
/// account key (base64), with this crate's own signer.
pub fn check_shared_key(
    method: &str,
    url: &url::Url,
    headers: &[(String, String)],
    body: &[u8],
    account_name: &str,
    account_key: &str,
) -> Result<(), String> {
    use base64::Engine;
    let sent = header(headers, "authorization").ok_or("no Authorization header")?;
    header(headers, "x-ms-date").ok_or("no x-ms-date")?;
    let mut signed: HashMap<String, String> = headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        .cloned()
        .collect();
    let key = base64::engine::general_purpose::STANDARD
        .decode(account_key)
        .map_err(|_| "the account key is not base64")?;
    crate::auth::azure_signing::apply_shared_key_signature(
        method,
        url,
        &mut signed,
        body,
        account_name,
        &key,
    )?;
    if signed.get("Authorization").map(String::as_str) == Some(sent) {
        Ok(())
    } else {
        Err("the signature does not match".into())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::without_sealed;
    use crate::sealed::SEALED_MARKER;

    #[test]
    fn only_sealed_values_are_taken_out() {
        let sealed = json!({ SEALED_MARKER: { "envelope": { "ct": "csec" }, "policy": "ab" } });
        let stored = json!({
            "client_id": "cid",
            "client_secret": sealed,
            "pending": { "fields": { "access_token": sealed, "scope": "read" } },
            "list": [sealed, "at-1"],
            // An object with the marker beside other keys is not a sealed value.
            "other": { SEALED_MARKER: "x", "csec": 1 },
        });
        assert_eq!(
            without_sealed(&stored),
            json!({
                "client_id": "cid",
                "client_secret": null,
                "pending": { "fields": { "access_token": null, "scope": "read" } },
                "list": [null, "at-1"],
                "other": { SEALED_MARKER: "x", "csec": 1 },
            })
        );
    }
}
