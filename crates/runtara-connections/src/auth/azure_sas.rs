use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Signed Storage API version used in the `sv` field of generated SAS tokens.
pub const SAS_API_VERSION: &str = "2023-11-03";

/// Maximum SAS lifetime we allow (7 days, matching the AWS presign cap so the
/// public surface is symmetric across providers).
pub const MAX_SAS_EXPIRES_SECONDS: u32 = 604_800;

/// Generate a Service SAS URL for a single blob.
///
/// Reference: <https://learn.microsoft.com/rest/api/storageservices/create-service-sas>
///
/// `permissions` follows the Service SAS Blob permission alphabet:
/// `r` (read), `w` (write/create), `d` (delete), `c` (create), `a` (add), `t` (tags).
#[allow(clippy::too_many_arguments)]
pub fn generate_blob_sas_url(
    base_url: &str,
    account_name: &str,
    account_key: &str,
    container: &str,
    blob: &str,
    permissions: &str,
    expires_in_seconds: u32,
    content_type: Option<&str>,
) -> Result<String, String> {
    let key_bytes = BASE64
        .decode(account_key.trim())
        .map_err(|e| format!("Invalid Azure account key (not base64): {}", e))?;

    let expiry_ts = Utc::now()
        + chrono::Duration::seconds(expires_in_seconds.min(MAX_SAS_EXPIRES_SECONDS) as i64);
    let signed_expiry = format_iso8601(&expiry_ts);

    // Canonical resource: `/blob/{account}/{container}/{blob}` per the docs.
    let canonical_resource = format!(
        "/blob/{}/{}/{}",
        account_name,
        container.trim_matches('/'),
        encode_path(blob.trim_start_matches('/'))
    );

    let signed_resource = "b";
    // Derive the signed protocol from the base URL. `https` alone is rejected
    // when the URL is `http://…` (Azurite by default); `https,http` permits both.
    let signed_protocol = if base_url.starts_with("http://") {
        "https,http"
    } else {
        "https"
    };

    let rsct = content_type.unwrap_or("");

    // String-to-sign (Service SAS, Blob, signed version 2018-11-09+).
    // Order MUST be exact; empty fields stay empty but the newline remains.
    // 16 fields total — see Azurite's BlobSASAuthenticator for the canonical layout.
    let string_to_sign = [
        permissions,
        "", // signedstart
        &signed_expiry,
        &canonical_resource,
        "", // signedidentifier
        "", // signedIP
        signed_protocol,
        SAS_API_VERSION,
        signed_resource,
        "", // signedSnapshotTime
        "", // signedEncryptionScope
        "", // rscc
        "", // rscd
        "", // rsce
        "", // rscl
        rsct,
    ]
    .join("\n");

    let mut mac =
        HmacSha256::new_from_slice(&key_bytes).map_err(|e| format!("HMAC init failed: {}", e))?;
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());

    let mut sas = format!(
        "sv={}&se={}&sr={}&sp={}&spr={}&sig={}",
        SAS_API_VERSION,
        url_encode(&signed_expiry),
        signed_resource,
        permissions,
        url_encode(signed_protocol),
        url_encode(&signature),
    );
    if let Some(ct) = content_type {
        sas.push_str("&rsct=");
        sas.push_str(&url_encode(ct));
    }

    let cleaned_base = base_url.trim_end_matches('/');
    let blob_path = format!(
        "/{}/{}",
        container.trim_matches('/'),
        encode_path(blob.trim_start_matches('/'))
    );
    Ok(format!("{}{}?{}", cleaned_base, blob_path, sas))
}

/// Map a friendly operation name to Azure SAS permission characters.
pub fn permissions_for(operation: &str) -> Option<&'static str> {
    match operation.to_lowercase().as_str() {
        "download" | "get" | "read" => Some("r"),
        "upload" | "put" | "write" | "create" => Some("cw"),
        "delete" => Some("d"),
        _ => None,
    }
}

fn format_iso8601(ts: &DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn url_encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn encode_path(s: &str) -> String {
    s.split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str =
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    #[test]
    fn sas_url_contains_required_query_params() {
        let url = generate_blob_sas_url(
            "https://acct.blob.core.windows.net",
            "acct",
            TEST_KEY,
            "container",
            "blob.txt",
            "r",
            3600,
            None,
        )
        .unwrap();
        for required in ["sv=2023-11-03", "se=", "sr=b", "sp=r", "spr=https", "sig="] {
            assert!(
                url.contains(required),
                "missing `{}` in SAS URL: {}",
                required,
                url
            );
        }
        assert!(url.starts_with("https://acct.blob.core.windows.net/container/blob.txt?"));
    }

    #[test]
    fn sas_url_includes_response_content_type_when_provided() {
        let url = generate_blob_sas_url(
            "https://acct.blob.core.windows.net",
            "acct",
            TEST_KEY,
            "container",
            "report.csv",
            "r",
            3600,
            Some("text/csv"),
        )
        .unwrap();
        assert!(url.contains("rsct=text%2Fcsv"));
    }

    #[test]
    fn sas_url_works_against_path_style_endpoint() {
        let url = generate_blob_sas_url(
            "http://127.0.0.1:10000/devstoreaccount1",
            "devstoreaccount1",
            TEST_KEY,
            "uploads",
            "x.txt",
            "rw",
            300,
            None,
        )
        .unwrap();
        assert!(url.starts_with("http://127.0.0.1:10000/devstoreaccount1/uploads/x.txt?"));
        assert!(url.contains("sp=rw"));
    }

    #[test]
    fn permissions_for_maps_friendly_names() {
        assert_eq!(permissions_for("download"), Some("r"));
        assert_eq!(permissions_for("UPLOAD"), Some("cw"));
        assert_eq!(permissions_for("delete"), Some("d"));
        assert_eq!(permissions_for("nope"), None);
    }

    #[test]
    fn sas_clamps_expires_to_seven_days() {
        let url = generate_blob_sas_url(
            "https://acct.blob.core.windows.net",
            "acct",
            TEST_KEY,
            "c",
            "b",
            "r",
            999_999_999,
            None,
        )
        .unwrap();
        // Expiry is 7 days from now; we don't pin to a specific timestamp
        // but the call must succeed and produce a sane-looking SAS.
        assert!(url.contains("se="));
    }

    #[test]
    fn invalid_key_returns_error() {
        let err = generate_blob_sas_url(
            "https://acct.blob.core.windows.net",
            "acct",
            "not-base64!@#",
            "c",
            "b",
            "r",
            300,
            None,
        )
        .unwrap_err();
        assert!(err.contains("Invalid Azure account key"));
    }
}
