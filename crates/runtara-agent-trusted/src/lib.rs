//! Host-authorized isolated capability execution. No credential access exists
//! on the ordinary invocation path; only the host supplies TrustedContext.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        // `POST /v1/trusted/{agent}/{capability}`, under the run's pins.
        runtara_workflow_sdk::trusted(agent, capability, connection, &input, None)
            .await
            .map_err(trusted_failure)
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

/// The agent-facing error of a failed trusted call: the executor's own
/// envelope when it answered, the host's refusal as before, and otherwise an
/// envelope whose retryability follows the API's error.
#[cfg(any(target_arch = "wasm32", test))]
fn trusted_failure(failure: runtara_workflow_sdk::Error) -> String {
    use runtara_workflow_api::ErrorCode;
    use runtara_workflow_sdk::Error as Failure;
    match failure {
        Failure::Api(error)
            if error.code.as_str() == "trusted-error" && error.details.is_some() =>
        {
            error
                .details
                .map(|details| details.to_string())
                .unwrap_or_default()
        }
        Failure::Api(error) if error.code == ErrorCode::Denied => error_envelope(
            "TRUSTED_CAPABILITY_DENIED",
            "Trusted execution is not available in this context",
            false,
        ),
        Failure::Api(error) => error_envelope(
            "TRUSTED_UNAVAILABLE",
            &error.message,
            error.code.retryable(),
        ),
        Failure::Transport(message) => error_envelope("TRUSTED_UNAVAILABLE", &message, true),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn error_envelope(code: &str, message: &str, retryable: bool) -> String {
    serde_json::json!({"code": code, "message": message,
        "category": if retryable { "transient" } else { "permanent" },
        "severity": "error", "retryable": retryable})
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trusted_failures_keep_the_executors_envelope() {
        use runtara_workflow_api::{ApiError, ErrorCode};
        use runtara_workflow_sdk::Error;
        let envelope = serde_json::json!({"code": "TRUSTED_VERSION_REQUIRED",
            "message": "Workflow has no matching approved trusted dependency",
            "category": "permanent", "severity": "error", "retryable": false});
        let answered = trusted_failure(Error::Api(
            ApiError::domain("trusted-error", ErrorCode::Denied, "no pin")
                .with_details(envelope.clone()),
        ));
        assert_eq!(serde_json::from_str::<Value>(&answered).unwrap(), envelope);

        let refused = trusted_failure(Error::Api(ApiError::new(ErrorCode::Denied, "no")));
        let refused: Value = serde_json::from_str(&refused).unwrap();
        assert_eq!(refused["code"], "TRUSTED_CAPABILITY_DENIED");
        assert_eq!(refused["retryable"], false);

        for (code, retryable) in [(ErrorCode::Unavailable, true), (ErrorCode::Fenced, false)] {
            let other: Value =
                serde_json::from_str(&trusted_failure(Error::Api(ApiError::new(code, "x"))))
                    .unwrap();
            assert_eq!(other["retryable"], retryable, "{other}");
        }
        let lost: Value =
            serde_json::from_str(&trusted_failure(Error::Transport("reset".into()))).unwrap();
        assert_eq!(lost["retryable"], true);
    }

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
