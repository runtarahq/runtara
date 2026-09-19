use super::aws_presign;
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
    _content_type: Option<&str>,
) -> Result<PresignResult, String> {
    if context.integration_id != "s3_compatible" {
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
    let absolute = object_url(base, path)?;
    let expires = expires.min(604_800) as u32;
    let url = aws_presign::presign_url_v4_at(
        method,
        &absolute,
        expires,
        field("access_key_id")?,
        field("secret_access_key")?,
        field("region")?,
        "s3",
        context
            .credentials
            .get("session_token")
            .and_then(Value::as_str),
        now,
    );
    Ok(PresignResult {
        url,
        expires_in_seconds: u64::from(expires),
    })
}
