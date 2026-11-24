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

/// A connection's parameters as a caller sends them: each readable secret
/// replaced by the marker of one sealed where it was entered, and the
/// secrets, by what the request calls them. A Postgres URL is split as a
/// caller splits it: the password is the secret, and the URL without it goes
/// beside it (`database_target`), for the service to check and store.
pub fn enter(
    integration_id: &str,
    parameters: &Value,
) -> (Value, std::collections::BTreeMap<String, String>) {
    let mut parameters = parameters.clone();
    let mut secrets = std::collections::BTreeMap::new();
    if integration_id == "postgres"
        && let Some((target, password)) = parameters[super::policy::PG_URL_FIELD]
            .as_str()
            .and_then(super::policy::without_password)
    {
        parameters[super::policy::PG_URL_FIELD] = super::entered();
        parameters[super::policy::PG_TARGET_FIELD] = Value::String(target);
        secrets.insert(super::policy::PG_URL_FIELD.to_owned(), password);
    }
    for name in super::policy::readable_secrets(integration_id, &parameters) {
        let slot = match super::policy::extra_header_of(&name) {
            Some(header) => parameters[super::policy::EXTRA_HEADERS_FIELD].get_mut(header),
            None => parameters.get_mut(&name),
        };
        if let Some(slot) = slot
            && let Some(secret) = slot.as_str().map(str::to_owned)
        {
            *slot = super::entered();
            secrets.insert(name, secret);
        }
    }
    (parameters, secrets)
}

/// The intake envelope of `secrets`, sealed under an offer as a caller seals
/// them: `offer` and `fields` are those of the answer to an offer request.
pub fn seal_under(
    offer: &Value,
    fields: &std::collections::BTreeMap<String, String>,
    secrets: &std::collections::BTreeMap<String, String>,
) -> Value {
    let offer = seal_format::Offer::from_json(offer.to_string().as_bytes())
        .expect("the offer is one seal made");
    let members = secrets
        .iter()
        .map(|(name, secret)| {
            let field = fields.get(name).unwrap_or(name);
            (
                seal_format::FieldName::new(field).expect("a field name"),
                seal_format::intake::Value::Text(secret),
            )
        })
        .collect();
    let envelope = offer
        .seal(&seal_format::intake::object(&members))
        .expect("the secrets seal under the offer");
    serde_json::to_value(envelope).expect("an envelope is JSON")
}

/// Saves a connection over HTTP as a caller does, for a test that has the
/// secrets readable in `body` (a create request for `POST`, an update for
/// `PUT`): marks each as entered, asks for an offer of the same request,
/// seals the secrets under it, and saves. `send` makes one request and
/// answers with its status and JSON body.
///
/// A refusal of the offer is returned as it came: it is what the caller
/// would be told.
pub async fn save_as_a_caller<F, Fut>(
    send: F,
    method: &str,
    uri: &str,
    mut body: Value,
) -> (u16, Value)
where
    F: Fn(String, String, Option<Value>) -> Fut,
    Fut: std::future::Future<Output = (u16, Value)>,
{
    let create = method.eq_ignore_ascii_case("POST");
    let secrets = if create {
        let integration_id = body["integrationId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if body["connectionParameters"].is_object() {
            let (parameters, secrets) = enter(&integration_id, &body["connectionParameters"]);
            body["connectionParameters"] = parameters;
            secrets
        } else {
            Default::default()
        }
    } else {
        let (_, held) = send("GET".to_owned(), uri.to_owned(), None).await;
        let integration_id = held["connection"]["integrationId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        // What the patch writes and sets, entered as one set of parameters,
        // and put back where each came from.
        let patch = &mut body["connectionParameterPatch"];
        let written: serde_json::Map<String, Value> = ["write", "set"]
            .iter()
            .flat_map(|part| patch[*part].as_object().into_iter().flatten())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let (entered, secrets) = enter(&integration_id, &Value::Object(written));
        for (name, value) in entered.as_object().into_iter().flatten() {
            let part = if patch["write"].get(name).is_some() {
                "write"
            } else {
                "set"
            };
            if patch.is_object() {
                if !patch[part].is_object() {
                    patch[part] = Value::Object(Default::default());
                }
                patch[part][name] = value.clone();
            }
        }
        secrets
    };
    if secrets.is_empty() {
        return send(method.to_owned(), uri.to_owned(), Some(body)).await;
    }
    let (status, offered) = send(
        "POST".to_owned(),
        format!("{uri}/offer"),
        Some(body.clone()),
    )
    .await;
    if status != 200 {
        return (status, offered);
    }
    let fields = serde_json::from_value(offered["fields"].clone()).unwrap_or_default();
    body["sealedSecrets"] = seal_under(&offered["offer"], &fields, &secrets);
    if create {
        body["connectionId"] = offered["connectionId"].clone();
    }
    send(method.to_owned(), uri.to_owned(), Some(body)).await
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
