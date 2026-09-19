//! Bounded, connection-authenticated downloads for workflow agents.
use crate::{HttpClient, HttpResponse};
use std::time::Duration;

/// Existing download ceiling, below the host boundary's 8 MiB response budget.
pub const MAX_DOWNLOAD_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug)]
pub struct DownloadError {
    pub code: &'static str,
    pub message: String,
    pub transient: bool,
    pub retry_after_ms: Option<u64>,
}

impl DownloadError {
    fn permanent(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            transient: false,
            retry_after_ms: None,
        }
    }
}

pub fn byte_limit(requested: Option<usize>) -> Result<usize, DownloadError> {
    let limit = requested.unwrap_or(MAX_DOWNLOAD_BYTES);
    if limit == 0 || limit > MAX_DOWNLOAD_BYTES {
        return Err(DownloadError::permanent(
            "INVALID_DOWNLOAD_LIMIT",
            format!("max_bytes must be between 1 and {MAX_DOWNLOAD_BYTES}"),
        ));
    }
    Ok(limit)
}

/// The caller selects a provider-approved URL and endpoint. The host enforces
/// the connection destination, injects auth, bounds bytes, and refuses redirects.
/// Async so a cancelled Agent call drops the in-flight download.
pub async fn get(
    connection_id: &str,
    url: &str,
    endpoint: Option<&str>,
    accept: &str,
    limit: usize,
) -> Result<HttpResponse, DownloadError> {
    byte_limit(Some(limit))?;
    if connection_id.is_empty() {
        return Err(DownloadError::permanent(
            "MISSING_CONNECTION",
            "Select a connection for this download",
        ));
    }
    let mut request = HttpClient::new()
        .request("GET", url)
        .timeout(Duration::from_secs(30))
        .connection_id(connection_id)
        .max_response_bytes(limit as u64)
        .header("Accept", accept);
    if let Some(endpoint) = endpoint {
        request = request.endpoint(endpoint);
    }
    let response = request
        .call_agent_async()
        .await
        .map_err(|_| DownloadError {
            code: "DOWNLOAD_NETWORK_ERROR",
            message: "Download request failed".into(),
            transient: true,
            retry_after_ms: None,
        })?;
    check_response(response, limit)
}

fn check_response(response: HttpResponse, limit: usize) -> Result<HttpResponse, DownloadError> {
    if response.status == 413 || response.body.len() > limit {
        return Err(DownloadError::permanent(
            "DOWNLOAD_TOO_LARGE",
            "Download exceeds max_bytes",
        ));
    }
    if !(200..300).contains(&response.status) {
        return Err(DownloadError {
            code: "DOWNLOAD_HTTP_ERROR",
            message: format!("Download returned HTTP {}", response.status),
            transient: response.status == 429 || response.status == 408 || response.status >= 500,
            retry_after_ms: response
                .header("retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1000)),
        });
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn errors_are_actionable_and_preserve_retry_after() {
        for (status, transient) in [
            (401, false),
            (403, false),
            (404, false),
            (302, false),
            (429, true),
            (503, true),
        ] {
            let error = check_response(
                HttpResponse {
                    status,
                    body: vec![],
                    headers: HashMap::from([("retry-after".into(), "2".into())]),
                },
                10,
            )
            .err()
            .unwrap();
            assert_eq!(error.transient, transient);
            assert_eq!(error.retry_after_ms, Some(2000));
        }
    }

    #[test]
    fn limits_cover_actual_bytes_and_host_rejections() {
        assert!(byte_limit(Some(0)).is_err());
        assert!(byte_limit(Some(MAX_DOWNLOAD_BYTES + 1)).is_err());
        for (status, body) in [(200, vec![0; 11]), (413, vec![])] {
            assert_eq!(
                check_response(
                    HttpResponse {
                        status,
                        body,
                        headers: HashMap::new()
                    },
                    10
                )
                .err()
                .unwrap()
                .code,
                "DOWNLOAD_TOO_LARGE"
            );
        }
        assert!(
            check_response(
                HttpResponse {
                    status: 200,
                    body: vec![0; 10],
                    headers: HashMap::new()
                },
                10
            )
            .is_ok()
        );
    }
}
