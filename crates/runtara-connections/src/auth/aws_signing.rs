use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

type HmacSha256 = Hmac<Sha256>;

pub struct AwsSigningParams {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub service: String,
    pub session_token: Option<String>,
}

/// Sign an HTTP request using AWS Signature V4.
///
/// Computes the SigV4 signature and sets the `Authorization`, `X-Amz-Date`,
/// `X-Amz-Content-Sha256`, and optionally `X-Amz-Security-Token` headers.
#[allow(clippy::too_many_arguments)]
pub fn sign_request_v4(
    method: &str,
    url: &url::Url,
    headers: &mut HashMap<String, String>,
    body: &[u8],
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
    session_token: Option<&str>,
) {
    let now = Utc::now();
    let date_stamp = now.format("%Y%m%d").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

    // Payload hash
    let payload_hash = hex::encode(Sha256::digest(body));

    // Host header
    let host = url.host_str().unwrap_or("localhost");
    let host_with_port = if let Some(port) = url.port() {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    let canonical_uri = canonical_uri(url);

    // Canonical query string (sorted)
    let canonical_querystring = {
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        pairs
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    };

    // Build sorted headers map for signing
    let mut headers_map = BTreeMap::new();
    headers_map.insert("host".to_string(), host_with_port.clone());
    headers_map.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
    headers_map.insert("x-amz-date".to_string(), amz_date.clone());

    // Include content-type in signing if present in request headers
    for (k, v) in headers.iter() {
        let lk = k.to_lowercase();
        if lk == "content-type" {
            headers_map.insert(lk, v.trim().to_string());
        }
    }

    if let Some(token) = session_token {
        headers_map.insert("x-amz-security-token".to_string(), token.to_string());
    }

    // Include extra S3 headers (x-amz-copy-source, etc.) in signing
    for (k, v) in headers.iter() {
        let lk = k.to_lowercase();
        if lk.starts_with("x-amz-") && !headers_map.contains_key(&lk) {
            headers_map.insert(lk, v.trim().to_string());
        }
    }

    let signed_headers: Vec<String> = headers_map.keys().cloned().collect();
    let signed_headers_str = signed_headers.join(";");

    let canonical_headers: String = headers_map
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k, v.trim()))
        .collect();

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        canonical_querystring,
        canonical_headers,
        signed_headers_str,
        payload_hash
    );

    let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, region, service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date,
        credential_scope,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    // Calculate signing key
    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        access_key, credential_scope, signed_headers_str, signature
    );

    // Set the signing headers on the outbound request
    headers.insert("Authorization".into(), authorization);
    headers.insert("X-Amz-Date".into(), amz_date);
    headers.insert("X-Amz-Content-Sha256".into(), payload_hash);
    headers.insert("Host".into(), host_with_port);
    if let Some(token) = session_token {
        headers.insert("X-Amz-Security-Token".into(), token.to_string());
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn canonical_uri(url: &url::Url) -> String {
    let path = url.path();
    if path.is_empty() {
        return "/".to_string();
    }

    path.split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_uri_encodes_reserved_chars_inside_segments() {
        let url = url::Url::parse(
            "https://bedrock-runtime.ap-southeast-2.amazonaws.com/model/openai.gpt-oss-120b-1:0/converse",
        )
        .unwrap();

        assert_eq!(
            canonical_uri(&url),
            "/model/openai.gpt-oss-120b-1%3A0/converse"
        );
    }
}
