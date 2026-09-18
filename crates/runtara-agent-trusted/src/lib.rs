//! Host-authorized isolated capability execution. No credential access exists
//! on the ordinary invocation path; only the host supplies TrustedContext.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod aws_presign;
pub mod azure_sas;

pub const EXECUTOR_INTERFACE: &str = "runtara:trusted/executor@0.1.0";
pub const EXECUTION_INTERFACE: &str = "runtara:trusted/execution@0.1.0";
pub const WIT: &str = include_str!("../wit/trusted.wit");

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
    let url = match context.integration_id.as_str() {
        "s3_compatible" => aws_presign::presign_url_v4_at(
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
        ),
        "azure_blob_storage" => {
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
            azure_sas::generate_blob_sas_url_at(
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
            .map_err(|_| error("INVALID_CREDENTIALS", "Invalid signing credentials"))?
        }
        _ => {
            return Err(error(
                "TRUSTED_CONNECTION_TYPE",
                "Unsupported connection type",
            ));
        }
    };
    Ok(PresignResult {
        url,
        expires_in_seconds: u64::from(expires),
    })
}

/// Pin to the configured endpoint and base path before signing.
pub fn object_url(base: &str, path: &str) -> Result<url::Url, String> {
    let invalid = || {
        error(
            "INVALID_PATH",
            "Object path must stay within the connection endpoint",
        )
    };
    if !path.starts_with('/') || path.starts_with("//") || path.contains('?') || path.contains('#')
    {
        return Err(invalid());
    }
    let base = url::Url::parse(base).map_err(|_| invalid())?;
    let absolute = url::Url::parse(&format!("{}{path}", base.as_str().trim_end_matches('/')))
        .map_err(|_| invalid())?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(invalid());
    }
    fn normalized(path: &str) -> String {
        let decoded = urlencoding::decode(path).unwrap_or_else(|_| path.into());
        let mut segments = Vec::new();
        for segment in decoded.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                value => segments.push(value),
            }
        }
        format!("/{}", segments.join("/"))
    }
    let base_path = normalized(base.path());
    let final_path = normalized(absolute.path());
    let prefix = base_path.trim_end_matches('/');
    if absolute.origin() != base.origin()
        || !(final_path == prefix || final_path.starts_with(&format!("{prefix}/")))
    {
        return Err(invalid());
    }
    Ok(absolute)
}

/// Deliberately not Debug: the payload belongs only to the isolated invocation.
#[derive(Serialize, Deserialize)]
pub struct TrustedContext {
    pub integration_id: String,
    pub credentials: Value,
    pub now_ms: i64,
}

impl Drop for TrustedContext {
    fn drop(&mut self) {
        fn clear(value: &mut Value) {
            match value {
                Value::String(s) => zeroize::Zeroize::zeroize(s),
                Value::Array(values) => values.iter_mut().for_each(clear),
                Value::Object(values) => values.values_mut().for_each(clear),
                _ => {}
            }
        }
        clear(&mut self.credentials);
    }
}

pub fn error(code: &str, message: &str) -> String {
    serde_json::json!({"code": code, "message": message, "category": "permanent",
        "severity": "error", "retryable": false})
    .to_string()
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({path: "wit", world: "client", async: true});
}

/// Generated ordinary capability wrappers use this path, never the privileged
/// function. Native unit tests must invoke that function with an explicit context.
pub async fn invoke(agent: &str, capability: &str, input: Value) -> Result<Value, String> {
    let connection = input
        .get("_connection")
        .and_then(|c| c.get("connection_id"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| error("TRUSTED_CONNECTION_REQUIRED", "A connection is required"))?;
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = serde_json::to_vec(&input)
            .map_err(|_| error("INVALID_INPUT", "Invalid trusted capability input"))?;
        let output = bindings::runtara::trusted::executor::invoke(
            agent.to_owned(),
            capability.to_owned(),
            connection.to_owned(),
            bytes,
        )
        .await?;
        serde_json::from_slice(&output)
            .map_err(|_| error("INVALID_OUTPUT", "Invalid trusted capability output"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (agent, capability, connection);
        Err(error(
            "TRUSTED_HOST_REQUIRED",
            "Trusted capabilities require a component host",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn object_paths_remain_under_authorized_endpoint() {
        let base = "https://storage.example.test/tenant";
        for path in [
            "/../other/key",
            "/%2e%2e%2fother/key",
            "/%2e%2e/other/key",
            "//evil.test/key",
            "https://evil.test/key",
            "/key?override=1",
            "/key#fragment",
        ] {
            assert!(object_url(base, path).is_err(), "{path}");
        }
        assert_eq!(
            object_url(base, "/bucket/key").unwrap().as_str(),
            "https://storage.example.test/tenant/bucket/key"
        );
        assert!(object_url("https://user:password@storage.example.test", "/key").is_err());
    }
}
