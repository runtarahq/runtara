//! Outbound HTTP contract and native service injection. No network transport here.
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::{ComponentType, Lift, Linker, WasmList};
use wasmtime::{AsContext, AsContextMut};
mod text;
use text::BorrowedText;

mod bindings {
    wasmtime::component::bindgen!({
        path: "../runtara-workflow-wit/wit/outbound-http",
        world: "outbound-http-client",
        imports: { default: async | trappable },
    });
}

pub use bindings::runtara::outbound_http::client::{
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

pub(crate) struct RunOutboundHttp {
    pub backend: Arc<dyn OutboundHttpHost>,
    pub context: OutboundContext,
}

pub(crate) fn for_run(
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

pub(crate) trait OutboundHttpContext {
    fn outbound_http(&self) -> Result<Arc<RunOutboundHttp>, String>;
    fn outbound_deadline(&self) -> Option<tokio::time::Instant>;
}

impl OutboundHttpContext for crate::host_state::HostState {
    fn outbound_http(&self) -> Result<Arc<RunOutboundHttp>, String> {
        if self.restricted {
            return Err("Outbound HTTP is disabled in trusted execution".into());
        }
        self.outbound_http.clone()
    }
    fn outbound_deadline(&self) -> Option<tokio::time::Instant> {
        self.http_deadline
    }
}

impl OutboundHttpContext for crate::workflow::WorkflowState {
    fn outbound_http(&self) -> Result<Arc<RunOutboundHttp>, String> {
        self.outbound_http.clone()
    }
    fn outbound_deadline(&self) -> Option<tokio::time::Instant> {
        Some(self.database_deadline())
    }
}

// Borrow guest data until authority and byte budgets have been checked. The
// generated owned records above remain the service contract; linker typechecking
// checks these borrowed lift shapes against the same canonical WIT records.
#[derive(ComponentType, Lift)]
#[component(record)]
struct BorrowedConnection {
    #[component(name = "connection-id")]
    connection_id: BorrowedText,
    url: BorrowedText,
    endpoint: Option<BorrowedText>,
    #[component(name = "endpoint-ref")]
    endpoint_ref: Option<BorrowedText>,
    #[component(name = "ai-provider")]
    ai_provider: Option<BorrowedText>,
    #[component(name = "aws-service")]
    aws_service: Option<BorrowedText>,
}

#[derive(ComponentType, Lift)]
#[component(variant)]
// Keep canonical ABI lift fields inline: borrowing guest strings must not allocate.
#[allow(clippy::large_enum_variant)]
enum BorrowedDestination {
    #[component(name = "connection")]
    Connection(BorrowedConnection),
    #[component(name = "public")]
    Public(BorrowedText),
}

#[derive(ComponentType, Lift)]
#[component(record)]
struct BorrowedRequest {
    destination: BorrowedDestination,
    method: BorrowedText,
    headers: WasmList<(BorrowedText, BorrowedText)>,
    body: Option<WasmList<u8>>,
    #[component(name = "timeout-ms")]
    timeout_ms: Option<u64>,
    #[component(name = "max-response-bytes")]
    max_response_bytes: Option<u64>,
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

fn text<T: 'static>(
    value: &BorrowedText,
    context: impl AsContext<Data = T>,
    remaining: &mut usize,
) -> Result<String, OutboundError> {
    charge(remaining, value.utf8_len)?;
    let value = value
        .value
        .to_str(context.as_context())
        .map_err(|_| error("HTTP_INVALID_REQUEST", "Invalid HTTP text encoding"))?;
    Ok(value.into_owned())
}

fn optional_text<T: 'static>(
    value: &Option<BorrowedText>,
    context: impl AsContext<Data = T>,
    remaining: &mut usize,
) -> Result<Option<String>, OutboundError> {
    value
        .as_ref()
        .map(|s| text(s, context, remaining))
        .transpose()
}

impl BorrowedRequest {
    fn own<T: 'static>(
        self,
        mut context: impl AsContextMut<Data = T>,
    ) -> Result<RequestOptions, OutboundError> {
        let mut remaining = MAX_REQUEST_BYTES;
        charge(&mut remaining, 64)?;
        charge(&mut remaining, self.body.as_ref().map_or(0, WasmList::len))?;
        charge(&mut remaining, self.headers.len().saturating_mul(16))?;
        let method = text(&self.method, &context, &mut remaining)?;
        let destination = match self.destination {
            BorrowedDestination::Public(url) => {
                Destination::Public(text(&url, &context, &mut remaining)?)
            }
            BorrowedDestination::Connection(c) => {
                let connection_id = text(&c.connection_id, &context, &mut remaining)?;
                if connection_id.trim().is_empty() {
                    return Err(error(
                        "HTTP_INVALID_REQUEST",
                        "Connection ID must not be empty",
                    ));
                }
                Destination::Connection(ConnectionDestination {
                    connection_id,
                    url: text(&c.url, &context, &mut remaining)?,
                    endpoint: optional_text(&c.endpoint, &context, &mut remaining)?,
                    endpoint_ref: optional_text(&c.endpoint_ref, &context, &mut remaining)?,
                    ai_provider: optional_text(&c.ai_provider, &context, &mut remaining)?,
                    aws_service: optional_text(&c.aws_service, &context, &mut remaining)?,
                })
            }
        };
        let mut headers = Vec::with_capacity(self.headers.len());
        for i in 0..self.headers.len() {
            let (name, value) = self
                .headers
                .get(context.as_context_mut(), i)
                .expect("index inside checked header list")
                .map_err(|_| error("HTTP_INVALID_REQUEST", "Invalid HTTP headers"))?;
            headers.push((
                text(&name, &context, &mut remaining)?,
                text(&value, &context, &mut remaining)?,
            ));
        }
        Ok(RequestOptions {
            destination,
            method,
            headers,
            body: self
                .body
                .map(|body| body.as_le_slice(context.as_context()).to_vec()),
            timeout_ms: self.timeout_ms,
            max_response_bytes: self.max_response_bytes,
        })
    }
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

pub(crate) fn add_to_linker<T: OutboundHttpContext + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    linker
        .instance(runtara_workflow_wit::OUTBOUND_HTTP_INTERFACE_NAME)?
        .func_wrap_concurrent("request", |accessor, (request,): (BorrowedRequest,)| {
            let started = tokio::time::Instant::now();
            let ready = accessor.with(|mut access| {
                let state = access.get();
                let deadline = state.outbound_deadline();
                let host = state
                    .outbound_http()
                    .map_err(|message| error("HTTP_UNAVAILABLE", &message))?;
                let request = request.own(access.as_context_mut())?;
                let requested_deadline = started + timeout(request.timeout_ms)?;
                let deadline =
                    deadline.map_or(requested_deadline, |active| active.min(requested_deadline));
                let limit = response_limit(request.max_response_bytes)?;
                Ok::<_, OutboundError>((host, request, deadline, limit))
            });
            Box::pin(async move {
                let result = match ready {
                    Err(error) => Err(error),
                    Ok((host, request, deadline, limit)) => {
                        if deadline <= tokio::time::Instant::now() {
                            Err(error(
                                "HTTP_DEADLINE_EXCEEDED",
                                "HTTP request timeout: deadline elapsed",
                            ))
                        } else {
                            match tokio::time::timeout_at(
                                deadline,
                                host.backend.request(&host.context, request),
                            )
                            .await
                            {
                                Err(_) => Err(error(
                                    "HTTP_DEADLINE_EXCEEDED",
                                    "HTTP request timeout: deadline elapsed",
                                )),
                                Ok(Ok(response)) => {
                                    check_response(&response, limit).map(|()| response)
                                }
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
                    }
                };
                Ok((result,))
            })
        })?;
    Ok(())
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
    fn missing_backend_and_restricted_instances_have_no_fallback() {
        assert!(for_run(None, Some("tenant"), None).is_err());
        assert!(crate::HostState::restricted().outbound_http().is_err());
        let metadata =
            crate::HostState::new(Arc::new(crate::CallContext::placeholder_for_metadata()));
        assert!(metadata.outbound_http().is_err());
    }
}
