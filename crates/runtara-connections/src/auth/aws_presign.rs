use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

/// Body hash sentinel used for query-string SigV4. The signed URL is valid for
/// any payload, which is the standard S3 pre-signed URL semantic.
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// Maximum lifetime of a presigned URL allowed by AWS (7 days).
pub const MAX_PRESIGN_EXPIRES_SECONDS: u32 = 604_800;

/// Generate a SigV4 query-string presigned URL.
///
/// Returns the input URL with `X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`,
/// `X-Amz-Expires`, `X-Amz-SignedHeaders`, optional `X-Amz-Security-Token`, and
/// `X-Amz-Signature` query parameters appended.
///
/// The signature only covers the `host` header; the caller can send any payload
/// when consuming the URL.
#[allow(clippy::too_many_arguments)]
pub fn presign_url_v4(
    method: &str,
    url: &url::Url,
    expires_in_seconds: u32,
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
    session_token: Option<&str>,
) -> String {
    let now = Utc::now();
    let date_stamp = now.format("%Y%m%d").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

    let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, region, service);
    let credential = format!("{}/{}", access_key, credential_scope);

    let host = url.host_str().unwrap_or("localhost");
    let host_with_port = match url.port() {
        Some(p) => format!("{}:{}", host, p),
        None => host.to_string(),
    };

    // Build the canonical query string from existing params plus the SigV4 params
    // we're about to add. The `X-Amz-Signature` parameter is intentionally not
    // included because it's the output we're computing.
    let mut query: BTreeMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    query.insert(
        "X-Amz-Algorithm".to_string(),
        "AWS4-HMAC-SHA256".to_string(),
    );
    query.insert("X-Amz-Credential".to_string(), credential.clone());
    query.insert("X-Amz-Date".to_string(), amz_date.clone());
    query.insert(
        "X-Amz-Expires".to_string(),
        expires_in_seconds
            .min(MAX_PRESIGN_EXPIRES_SECONDS)
            .to_string(),
    );
    query.insert("X-Amz-SignedHeaders".to_string(), "host".to_string());
    if let Some(token) = session_token {
        query.insert("X-Amz-Security-Token".to_string(), token.to_string());
    }

    let canonical_querystring = query
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let canonical_uri = canonical_uri(url);
    let canonical_headers = format!("host:{}\n", host_with_port);
    let signed_headers = "host";
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(),
        canonical_uri,
        canonical_querystring,
        canonical_headers,
        signed_headers,
        UNSIGNED_PAYLOAD
    );

    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date,
        credential_scope,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let signed_query = format!("{}&X-Amz-Signature={}", canonical_querystring, signature);

    let mut out = String::new();
    out.push_str(url.scheme());
    out.push_str("://");
    out.push_str(&host_with_port);
    out.push_str(&canonical_uri);
    out.push('?');
    out.push_str(&signed_query);
    out
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

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presigned_url_contains_required_query_params() {
        let url =
            url::Url::parse("https://bucket.s3.us-east-1.amazonaws.com/path/to/file.txt").unwrap();
        let signed = presign_url_v4(
            "GET",
            &url,
            900,
            "AKIA_TEST",
            "secret_test",
            "us-east-1",
            "s3",
            None,
        );
        for required in [
            "X-Amz-Algorithm=AWS4-HMAC-SHA256",
            "X-Amz-Credential=",
            "X-Amz-Date=",
            "X-Amz-Expires=900",
            "X-Amz-SignedHeaders=host",
            "X-Amz-Signature=",
        ] {
            assert!(
                signed.contains(required),
                "missing `{}` in signed URL: {}",
                required,
                signed
            );
        }
    }

    #[test]
    fn presign_clamps_expires_to_seven_days() {
        let url = url::Url::parse("https://bucket.s3.amazonaws.com/x").unwrap();
        let signed = presign_url_v4(
            "GET",
            &url,
            999_999_999,
            "AKIA",
            "secret",
            "us-east-1",
            "s3",
            None,
        );
        assert!(signed.contains(&format!("X-Amz-Expires={}", MAX_PRESIGN_EXPIRES_SECONDS)));
    }

    #[test]
    fn session_token_added_when_present() {
        let url = url::Url::parse("https://bucket.s3.amazonaws.com/x").unwrap();
        let signed = presign_url_v4(
            "GET",
            &url,
            300,
            "AKIA",
            "secret",
            "us-east-1",
            "s3",
            Some("sess_tok"),
        );
        assert!(signed.contains("X-Amz-Security-Token=sess_tok"));
    }
}
