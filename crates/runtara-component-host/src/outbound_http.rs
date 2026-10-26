//! Outbound HTTP contract and native service injection. No network transport here.
use std::sync::Arc;
use std::time::Duration;

mod bindings {
    wasmtime::component::bindgen!({
        path: "../runtara-wit/wit/host",
        world: "http-client",
        imports: { default: async | trappable },
    });
}

pub use bindings::runtara::host::http::{
    ConnectionDestination, Destination, OutboundError, RequestOptions, Response,
};

pub const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(120);

/// Identity supplied by the execution host, never by guest request headers.
#[derive(Clone, Debug)]
pub struct OutboundContext {
    pub tenant_id: String,
    pub instance_id: Option<String>,
}

#[async_trait::async_trait]
pub trait OutboundHttpHost: Send + Sync {
    async fn request(
        &self,
        context: &OutboundContext,
        request: RequestOptions,
    ) -> Result<Response, OutboundError>;
}

/// One run's outbound HTTP: the service and the run's identity.
pub struct RunOutboundHttp {
    pub backend: Arc<dyn OutboundHttpHost>,
    pub context: OutboundContext,
}

/// Outbound HTTP for a run of `tenant`, both host-supplied.
pub fn for_run(
    backend: Option<&Arc<dyn OutboundHttpHost>>,
    tenant: Option<&str>,
    instance: Option<&str>,
) -> Result<Arc<RunOutboundHttp>, String> {
    Ok(Arc::new(RunOutboundHttp {
        backend: Arc::clone(backend.ok_or("native outbound HTTP service is not configured")?),
        context: OutboundContext {
            tenant_id: tenant
                .filter(|s| !s.trim().is_empty())
                .ok_or("authoritative tenant is not configured")?
                .to_owned(),
            instance_id: instance.map(str::to_owned),
        },
    }))
}

pub fn error(code: &str, message: &str) -> OutboundError {
    OutboundError {
        code: code.into(),
        message: message.into(),
        status: None,
        body: Vec::new(),
        retry_after_ms: None,
    }
}

pub fn timeout(requested_ms: Option<u64>) -> Result<Duration, OutboundError> {
    match requested_ms {
        Some(0) => Err(error(
            "HTTP_INVALID_REQUEST",
            "HTTP timeout must be positive",
        )),
        Some(ms) => Ok(Duration::from_millis(ms).min(MAX_TIMEOUT)),
        None => Ok(DEFAULT_TIMEOUT),
    }
}

pub fn response_limit(requested: Option<u64>) -> Result<usize, OutboundError> {
    match requested {
        Some(0) => Err(error(
            "HTTP_INVALID_REQUEST",
            "Response byte limit must be positive",
        )),
        Some(n) => Ok(n.min(MAX_RESPONSE_BYTES as u64) as usize),
        None => Ok(MAX_RESPONSE_BYTES),
    }
}

fn too_large() -> OutboundError {
    error(
        "HTTP_TOO_LARGE",
        "HTTP request or response exceeds its byte limit",
    )
}

fn charge(remaining: &mut usize, size: usize) -> Result<(), OutboundError> {
    *remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
    Ok(())
}

fn check_response(response: &Response, limit: usize) -> Result<(), OutboundError> {
    let mut remaining = limit;
    charge(&mut remaining, response.body.len().saturating_add(2))?;
    for (name, value) in &response.headers {
        charge(
            &mut remaining,
            name.len().saturating_add(value.len()).saturating_add(16),
        )?;
    }
    Ok(())
}

/// Send `request` through `host` the way `POST /v1/http` does:
/// within the request's own timeout (capped) and the run's active deadline,
/// with the response checked against its byte limit.
pub async fn send_bounded(
    host: &RunOutboundHttp,
    request: RequestOptions,
    run_deadline: Option<tokio::time::Instant>,
) -> Result<Response, OutboundError> {
    let started = tokio::time::Instant::now();
    let requested_deadline = started + timeout(request.timeout_ms)?;
    let deadline = run_deadline.map_or(requested_deadline, |active| active.min(requested_deadline));
    let limit = response_limit(request.max_response_bytes)?;
    bounded(host, request, deadline, limit).await
}

async fn bounded(
    host: &RunOutboundHttp,
    request: RequestOptions,
    deadline: tokio::time::Instant,
    limit: usize,
) -> Result<Response, OutboundError> {
    if deadline <= tokio::time::Instant::now() {
        return Err(error(
            "HTTP_DEADLINE_EXCEEDED",
            "HTTP request timeout: deadline elapsed",
        ));
    }
    match tokio::time::timeout_at(deadline, host.backend.request(&host.context, request)).await {
        Err(_) => Err(error(
            "HTTP_DEADLINE_EXCEEDED",
            "HTTP request timeout: deadline elapsed",
        )),
        Ok(Ok(response)) => check_response(&response, limit).map(|()| response),
        Ok(Err(err)) => {
            if err
                .body
                .len()
                .saturating_add(err.message.len())
                .saturating_add(err.code.len())
                > limit
            {
                Err(too_large())
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_deadline_and_response_limits_are_bounded() {
        assert_eq!(timeout(None).unwrap(), Duration::from_secs(30));
        assert_eq!(timeout(Some(u64::MAX)).unwrap(), Duration::from_secs(120));
        assert_eq!(timeout(Some(1)).unwrap(), Duration::from_millis(1));
        assert!(timeout(Some(0)).is_err());
        assert_eq!(response_limit(None).unwrap(), 8 * 1024 * 1024);
        assert_eq!(response_limit(Some(u64::MAX)).unwrap(), MAX_RESPONSE_BYTES);
        assert!(response_limit(Some(0)).is_err());
    }

    #[test]
    fn response_budget_counts_raw_body_and_header_metadata() {
        let response = Response {
            status: 200,
            headers: vec![("a".into(), "b".into())],
            body: vec![0, 255],
        };
        assert!(check_response(&response, 22).is_ok());
        assert_eq!(
            check_response(&response, 21).unwrap_err().code,
            "HTTP_TOO_LARGE"
        );
    }

    #[test]
    fn missing_backend_has_no_fallback() {
        assert!(for_run(None, Some("tenant"), None).is_err());
    }
}
