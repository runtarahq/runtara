use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap};

type HmacSha256 = Hmac<Sha256>;

/// Azure Storage Shared Key signing parameters.
pub struct AzureSigningParams {
    pub account_name: String,
    /// Base64-encoded account key (primary or secondary).
    pub account_key: String,
}

/// Azure Storage REST API version sent via `x-ms-version`.
pub const AZURE_STORAGE_API_VERSION: &str = "2023-11-03";

/// Sign an HTTP request for Azure Storage using Shared Key authorization.
///
/// Sets `x-ms-date`, `x-ms-version`, and `Authorization` headers on `headers`.
/// Reference: <https://learn.microsoft.com/rest/api/storageservices/authorize-with-shared-key>
pub fn sign_request_shared_key(
    method: &str,
    url: &url::Url,
    headers: &mut HashMap<String, String>,
    body: &[u8],
    account_name: &str,
    account_key: &str,
) -> Result<(), String> {
    let key_bytes = BASE64
        .decode(account_key.trim())
        .map_err(|e| format!("Invalid Azure account key (not base64): {}", e))?;

    let amz_date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    headers.insert("x-ms-date".to_string(), amz_date);
    headers
        .entry("x-ms-version".to_string())
        .or_insert_with(|| AZURE_STORAGE_API_VERSION.to_string());

    apply_shared_key_signature(method, url, headers, body, account_name, &key_bytes)
}

fn apply_shared_key_signature(
    method: &str,
    url: &url::Url,
    headers: &mut HashMap<String, String>,
    body: &[u8],
    account_name: &str,
    key_bytes: &[u8],
) -> Result<(), String> {
    let method_upper = method.to_uppercase();

    // Normalize headers to a lowercase-keyed lookup with trimmed values.
    let lower_headers: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.trim().to_string()))
        .collect();
    let lookup = |name: &str| lower_headers.get(name).cloned().unwrap_or_default();

    // Per spec: Content-Length is empty for GET/HEAD/DELETE and when 0.
    let content_length = match method_upper.as_str() {
        "GET" | "HEAD" | "DELETE" => String::new(),
        _ if body.is_empty() => String::new(),
        _ => body.len().to_string(),
    };

    // CanonicalizedHeaders: all x-ms-* headers, sorted lexicographically by name.
    let canonical_headers: String = lower_headers
        .iter()
        .filter(|(k, _)| k.starts_with("x-ms-"))
        .collect::<BTreeMap<_, _>>()
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k, v))
        .collect();

    // CanonicalizedResource: /{account}/{path-without-account} + sorted query params.
    let canonical_resource = canonical_resource_for(url, account_name);

    let string_to_sign = format!(
        "{verb}\n{ce}\n{cl}\n{clen}\n{cmd5}\n{ct}\n{date}\n{ims}\n{im}\n{inm}\n{ius}\n{range}\n{headers}{resource}",
        verb = method_upper,
        ce = lookup("content-encoding"),
        cl = lookup("content-language"),
        clen = content_length,
        cmd5 = lookup("content-md5"),
        ct = lookup("content-type"),
        date = "",
        ims = lookup("if-modified-since"),
        im = lookup("if-match"),
        inm = lookup("if-none-match"),
        ius = lookup("if-unmodified-since"),
        range = lookup("range"),
        headers = canonical_headers,
        resource = canonical_resource,
    );

    let mut mac =
        HmacSha256::new_from_slice(key_bytes).map_err(|e| format!("HMAC init failed: {}", e))?;
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());

    headers.insert(
        "Authorization".to_string(),
        format!("SharedKey {}:{}", account_name, signature),
    );
    Ok(())
}

fn canonical_resource_for(url: &url::Url, account_name: &str) -> String {
    // Always `/{account}` + URL path, regardless of whether the URL uses
    // subdomain style (`https://acct.blob.core.windows.net/container/blob`)
    // or path style (`http://127.0.0.1:10000/acct/container/blob`).
    // Path-style URLs result in the account appearing twice — this matches
    // how Azurite (and the official Azure SDKs) compute the canonical resource.
    let mut out = format!("/{}{}", account_name, url.path());

    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        grouped
            .entry(k.to_lowercase())
            .or_default()
            .push(v.into_owned());
    }

    for (name, mut values) in grouped {
        values.sort();
        out.push('\n');
        out.push_str(&name);
        out.push(':');
        out.push_str(&values.join(","));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Well-known Azurite emulator account key (publicly documented).
    const AZURITE_KEY: &str =
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    fn signed_headers(
        method: &str,
        url: &str,
        extra_headers: &[(&str, &str)],
        body: &[u8],
        account: &str,
        key: &str,
        x_ms_date: &str,
    ) -> HashMap<String, String> {
        let parsed = url::Url::parse(url).unwrap();
        let mut headers: HashMap<String, String> = extra_headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        headers.insert("x-ms-date".to_string(), x_ms_date.to_string());
        headers.insert(
            "x-ms-version".to_string(),
            AZURE_STORAGE_API_VERSION.to_string(),
        );
        let key_bytes = BASE64.decode(key).unwrap();
        apply_shared_key_signature(method, &parsed, &mut headers, body, account, &key_bytes)
            .unwrap();
        headers
    }

    #[test]
    fn canonical_resource_subdomain_style() {
        let url = url::Url::parse(
            "https://acct.blob.core.windows.net/container/folder/blob.txt?comp=metadata",
        )
        .unwrap();
        assert_eq!(
            canonical_resource_for(&url, "acct"),
            "/acct/container/folder/blob.txt\ncomp:metadata"
        );
    }

    #[test]
    fn canonical_resource_path_style_double_prefixes_account() {
        // Azurite / Azure SDKs always compute `/{account}` + URL path, even
        // when the URL path already starts with the account name. The result
        // is a deliberate double prefix that matches Azurite's StringToSign.
        let url = url::Url::parse(
            "http://127.0.0.1:10000/devstoreaccount1/container/blob.txt?restype=container&comp=list",
        )
        .unwrap();
        assert_eq!(
            canonical_resource_for(&url, "devstoreaccount1"),
            "/devstoreaccount1/devstoreaccount1/container/blob.txt\ncomp:list\nrestype:container"
        );
    }

    #[test]
    fn signature_is_deterministic_for_fixed_date() {
        let h = signed_headers(
            "GET",
            "https://acct.blob.core.windows.net/?comp=list",
            &[],
            b"",
            "acct",
            AZURITE_KEY,
            "Mon, 27 Jul 2026 12:00:00 GMT",
        );
        let auth = h.get("Authorization").expect("authorization header");
        assert!(auth.starts_with("SharedKey acct:"));
        // The same inputs must produce the same signature.
        let h2 = signed_headers(
            "GET",
            "https://acct.blob.core.windows.net/?comp=list",
            &[],
            b"",
            "acct",
            AZURITE_KEY,
            "Mon, 27 Jul 2026 12:00:00 GMT",
        );
        assert_eq!(h.get("Authorization"), h2.get("Authorization"));
    }

    #[test]
    fn put_blob_signs_with_content_length_and_blob_type() {
        let body = b"hello world";
        let h = signed_headers(
            "PUT",
            "https://acct.blob.core.windows.net/container/hello.txt",
            &[
                ("Content-Type", "text/plain"),
                ("x-ms-blob-type", "BlockBlob"),
            ],
            body,
            "acct",
            AZURITE_KEY,
            "Mon, 27 Jul 2026 12:00:00 GMT",
        );
        let auth = h.get("Authorization").expect("authorization header");
        assert!(auth.starts_with("SharedKey acct:"));
        // x-ms-blob-type is part of the canonical headers and must influence the signature.
        let h_without_blob_type = signed_headers(
            "PUT",
            "https://acct.blob.core.windows.net/container/hello.txt",
            &[("Content-Type", "text/plain")],
            body,
            "acct",
            AZURITE_KEY,
            "Mon, 27 Jul 2026 12:00:00 GMT",
        );
        assert_ne!(
            h.get("Authorization"),
            h_without_blob_type.get("Authorization")
        );
    }

    #[test]
    fn invalid_account_key_returns_error() {
        let url = url::Url::parse("https://acct.blob.core.windows.net/").unwrap();
        let mut headers = HashMap::new();
        let err = sign_request_shared_key("GET", &url, &mut headers, b"", "acct", "not-base64!@#")
            .unwrap_err();
        assert!(err.contains("Invalid Azure account key"));
    }
}
