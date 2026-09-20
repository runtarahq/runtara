use super::azure_sas;
use runtara_agent_trusted::{TrustedContext, error, object_url};
use serde_json::Value;

pub struct PresignResult {
    pub url: String,
    pub expires_in_seconds: u64,
}

/// Pure provider signing. The context is supplied only to a restricted instance;
/// ordinary workflow input cannot become credential context.
pub fn presign(
    context: &TrustedContext,
    method: &str,
    path: &str,
    expires: u64,
    content_type: Option<&str>,
) -> Result<PresignResult, String> {
    if context.integration_id != "azure_blob_storage" {
        return Err(error(
            "TRUSTED_CONNECTION_TYPE",
            "Unsupported connection type",
        ));
    }
    let field = |key: &str| {
        context
            .credentials
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| error("INVALID_CREDENTIALS", "Missing signing credential"))
    };
    let now = chrono::DateTime::from_timestamp_millis(context.now_ms)
        .ok_or_else(|| error("INVALID_CONTEXT", "Invalid signing time"))?;
    let base = field("base_url")?;
    object_url(base, path)?;
    let expires = expires.min(604_800) as u32;
    let (container, blob) = path
        .trim_start_matches('/')
        .split_once('/')
        .filter(|(c, b)| !c.is_empty() && !b.is_empty())
        .ok_or_else(|| error("INVALID_INPUT", "Container and blob are required"))?;
    let permissions = match method {
        "GET" | "HEAD" => "r",
        "PUT" | "POST" => "cw",
        "DELETE" => "d",
        _ => return Err(error("INVALID_INPUT", "Unsupported operation")),
    };
    let url = azure_sas::generate_blob_sas_url_at(
        base,
        field("account_name")?,
        field("account_key")?,
        container,
        blob,
        permissions,
        expires,
        content_type,
        now,
    )
    .map_err(|_| error("INVALID_CREDENTIALS", "Invalid signing credentials"))?;
    Ok(PresignResult {
        url,
        expires_in_seconds: u64::from(expires),
    })
}
