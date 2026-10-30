//! Routes a run's `wasi:http@0.3` requests for the workflow API origin to the
//! host's router, with the run's binding attached.
//!
//! This is the single interception point of the workflow API on the embedded
//! runner: the p3 `send_request` hook. A request for
//! `http://api.runtara.internal` never touches a socket; it is handed to the
//! configured [`WorkflowApiTransport`] with the run's
//! [`runtara_workflow_api::Binding`] as a request extension, which the guest
//! cannot see or set. Every other request is denied, and so is the workflow
//! API itself when the run has no binding.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use runtara_workflow_api::{Binding, Caps, headers};
use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody, WasiHttpHooks};

/// A request the hook hands over, and the response it returns.
pub type WorkflowApiFuture = Pin<Box<dyn Future<Output = http::Response<Bytes>> + Send>>;

/// Serves workflow API requests. Implemented by whoever owns the `/v1` router
/// (runtara-environment for the embedded runner). The request carries the
/// run's [`Binding`] as an extension.
pub trait WorkflowApiTransport: Send + Sync + 'static {
    fn call(&self, request: http::Request<Bytes>) -> WorkflowApiFuture;
}

type HookBody = WasiBody;
type IoFuture = Box<dyn Future<Output = Result<(), HttpError>> + Send>;
type SendFuture =
    Box<dyn Future<Output = Result<(http::Response<HookBody>, IoFuture), HttpError>> + Send>;

/// A deadline the workflow API can bring forward (`PUT /v1/run/deadline`),
/// checked by the executor's epoch callback and watchdog next to the launch
/// deadline. It only ever moves earlier.
#[derive(Clone)]
pub(crate) struct ApiDeadline {
    base: tokio::time::Instant,
    /// Milliseconds after `base`; `u64::MAX` is no deadline.
    at_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl ApiDeadline {
    pub(crate) fn new() -> Self {
        Self {
            base: tokio::time::Instant::now(),
            at_ms: Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
        }
    }

    /// Bring the deadline forward to `after` from now.
    pub(crate) fn shorten(&self, after: std::time::Duration) {
        let target = (tokio::time::Instant::now() + after)
            .saturating_duration_since(self.base)
            .as_millis();
        let target = u64::try_from(target).unwrap_or(u64::MAX - 1);
        self.at_ms
            .fetch_min(target, std::sync::atomic::Ordering::AcqRel);
    }

    pub(crate) fn passed(&self) -> bool {
        let at = self.at_ms.load(std::sync::atomic::Ordering::Acquire);
        at != u64::MAX
            && tokio::time::Instant::now() >= self.base + std::time::Duration::from_millis(at)
    }
}

/// Where a run's binding comes from. Asked at every request: a launch binds
/// its root lease after the Store is built, so the binding can't be captured
/// up front.
pub(crate) type BindingSource = Arc<dyn Fn() -> Option<Binding> + Send + Sync>;

/// The p3 HTTP hooks of one run.
pub(crate) struct WorkflowApiHooks {
    route: Option<(Arc<dyn WorkflowApiTransport>, BindingSource)>,
    caps: Caps,
}

impl WorkflowApiHooks {
    /// Hooks for a run. The workflow API is reachable only with a transport
    /// and a source that currently yields a binding.
    pub(crate) fn for_run(
        transport: Option<Arc<dyn WorkflowApiTransport>>,
        binding: Option<BindingSource>,
    ) -> Self {
        Self {
            route: transport.zip(binding),
            caps: Caps::default(),
        }
    }

    /// Hooks that deny every request.
    pub(crate) fn denied() -> Self {
        Self::for_run(None, None)
    }
}

/// Whether `uri` addresses the workflow API origin exactly: `http`, the
/// reserved host, the default port, no userinfo.
fn is_workflow_api(uri: &http::Uri) -> bool {
    let Some(authority) = uri.authority() else {
        return false;
    };
    uri.scheme_str() == Some(runtara_workflow_api::SCHEME)
        && !authority.as_str().contains('@')
        && authority
            .host()
            .eq_ignore_ascii_case(runtara_workflow_api::HOST)
        && authority.port_u16().is_none_or(|port| port == 80)
}

/// Strip reserved headers and attach the binding. The path and query are kept;
/// scheme and authority are dropped, since the router routes by path only.
fn into_api_request(
    parts: http::request::Parts,
    body: Bytes,
    binding: Binding,
) -> Result<http::Request<Bytes>, HttpError> {
    let path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let mut request = http::Request::builder()
        .method(parts.method)
        .uri(path)
        .body(body)
        .map_err(|_| HttpError::HttpRequestUriInvalid)?;
    for (name, value) in &parts.headers {
        if !headers::is_stripped(name.as_str()) && name != http::header::HOST {
            request.headers_mut().append(name.clone(), value.clone());
        }
    }
    request.extensions_mut().insert(binding);
    Ok(request)
}

fn from_api_response(response: http::Response<Bytes>) -> http::Response<HookBody> {
    response.map(|body| {
        Full::new(body)
            .map_err(|never| match never {})
            .boxed_unsync()
    })
}

fn denied() -> SendFuture {
    Box::new(async { Err(HttpError::HttpRequestDenied) })
}

/// A spawned request, aborted when the guest drops the call.
struct AbortOnDrop<T>(tokio::task::JoinHandle<Result<T, HttpError>>);

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, HttpError>;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|joined| {
            joined.unwrap_or_else(|error| Err(HttpError::InternalError(Some(error.to_string()))))
        })
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl WasiHttpHooks for WorkflowApiHooks {
    fn send_request(
        &mut self,
        request: http::Request<HookBody>,
        _options: Option<RequestOptions>,
        _fut: IoFuture,
    ) -> SendFuture {
        let Some((transport, source)) = self.route.clone() else {
            return denied();
        };
        if !is_workflow_api(request.uri()) {
            return denied();
        }
        let Some(binding) = source() else {
            return denied();
        };
        let limit = self.caps.request_body_bytes;
        // Served on a task of its own, so the request makes progress while
        // the guest runs CPU-bound code: the Store's event loop polls the
        // returned future only when the guest yields to it, and a cleanup
        // alarm must be armed before that. Dropping the call aborts it.
        let served = AbortOnDrop(tokio::spawn(async move {
            let (parts, body) = request.into_parts();
            let body = Limited::new(body, limit)
                .collect()
                .await
                .map_err(|_| HttpError::HttpRequestBodySize(Some(limit as u64)))?
                .to_bytes();
            let request = into_api_request(parts, body, binding)?;
            Ok::<_, HttpError>(transport.call(request).await)
        }));
        Box::new(async move {
            let response = served.await?;
            let io: IoFuture = Box::new(async { Ok(()) });
            Ok((from_api_response(response), io))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<http::Request<Bytes>>>,
    }

    impl WorkflowApiTransport for Recorder {
        fn call(&self, request: http::Request<Bytes>) -> WorkflowApiFuture {
            self.seen.lock().unwrap().push(request);
            Box::pin(async {
                http::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Bytes::from_static(br#"{"ok":true}"#))
                    .unwrap()
            })
        }
    }

    fn binding() -> Binding {
        Binding {
            tenant_id: "tenant-a".into(),
            instance_id: "instance-1".into(),
            launch_id: "launch-1".into(),
            root_lease_epoch: 9,
        }
    }

    fn guest_request(uri: &str) -> http::Request<HookBody> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "api.runtara.internal")
            .header("authorization", "Bearer stolen")
            .header("runtara-binding-tenant", "tenant-b")
            .header("x-forwarded-for", "10.0.0.1")
            .header(headers::IDEMPOTENCY_KEY, "k1")
            .header(headers::OUTCOME, "completed")
            .body(
                Full::new(Bytes::from_static(b"output"))
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .unwrap()
    }

    fn source(binding: Option<Binding>) -> Option<BindingSource> {
        Some(Arc::new(move || binding.clone()))
    }

    fn hooks(recorder: &Arc<Recorder>) -> WorkflowApiHooks {
        WorkflowApiHooks::for_run(Some(recorder.clone()), source(Some(binding())))
    }

    async fn send(
        hooks: &mut WorkflowApiHooks,
        request: http::Request<HookBody>,
    ) -> Result<(u16, Bytes), HttpError> {
        let fut = hooks.send_request(request, None, Box::new(async { Ok(()) }));
        match Box::into_pin(fut).await {
            Ok((response, _io)) => {
                let status = response.status().as_u16();
                let body = response.into_body().collect().await.unwrap().to_bytes();
                Ok((status, body))
            }
            Err(error) => Err(error),
        }
    }

    #[tokio::test]
    async fn the_workflow_api_is_routed_with_the_binding() {
        let recorder = Arc::new(Recorder::default());
        let mut hooks = hooks(&recorder);
        let (status, body) = send(
            &mut hooks,
            guest_request("http://api.runtara.internal/v1/run/outcome?x=1"),
        )
        .await
        .unwrap();
        assert_eq!(status, 200);
        assert_eq!(&body[..], br#"{"ok":true}"#);

        let seen = recorder.seen.lock().unwrap();
        let request = &seen[0];
        assert_eq!(request.method(), "POST");
        assert_eq!(request.uri(), "/v1/run/outcome?x=1");
        assert_eq!(request.body(), &Bytes::from_static(b"output"));
        assert_eq!(request.extensions().get::<Binding>(), Some(&binding()));
        assert_eq!(request.headers()[headers::IDEMPOTENCY_KEY], "k1");
        assert_eq!(request.headers()[headers::OUTCOME], "completed");
        for stripped in [
            "authorization",
            "runtara-binding-tenant",
            "x-forwarded-for",
            "host",
        ] {
            assert!(!request.headers().contains_key(stripped), "{stripped}");
        }
    }

    #[tokio::test]
    async fn every_other_origin_is_denied() {
        let recorder = Arc::new(Recorder::default());
        let mut hooks = hooks(&recorder);
        for uri in [
            "https://api.runtara.internal/v1/run",
            "http://api.runtara.internal:8080/v1/run",
            "http://user@api.runtara.internal/v1/run",
            "http://example.com/v1/run",
            "http://api.runtara.internal.example.com/v1/run",
            "http://169.254.169.254/latest/meta-data",
        ] {
            let result = send(&mut hooks, guest_request(uri)).await;
            assert!(
                matches!(result, Err(HttpError::HttpRequestDenied)),
                "{uri}: {result:?}"
            );
        }
        assert!(recorder.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_explicit_default_port_and_host_case_are_accepted() {
        let recorder = Arc::new(Recorder::default());
        let mut hooks = hooks(&recorder);
        for uri in [
            "http://api.runtara.internal:80/v1/run",
            "http://API.Runtara.Internal/v1/run",
        ] {
            assert!(send(&mut hooks, guest_request(uri)).await.is_ok(), "{uri}");
        }
    }

    #[tokio::test]
    async fn a_run_without_a_binding_or_transport_is_denied() {
        let recorder = Arc::new(Recorder::default());
        for mut hooks in [
            WorkflowApiHooks::for_run(Some(recorder.clone()), None),
            WorkflowApiHooks::for_run(Some(recorder.clone()), source(None)),
            WorkflowApiHooks::for_run(None, source(Some(binding()))),
            WorkflowApiHooks::denied(),
        ] {
            let result = send(
                &mut hooks,
                guest_request("http://api.runtara.internal/v1/run"),
            )
            .await;
            assert!(matches!(result, Err(HttpError::HttpRequestDenied)));
        }
        assert!(recorder.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_before_routing() {
        let recorder = Arc::new(Recorder::default());
        let mut hooks = hooks(&recorder);
        hooks.caps.request_body_bytes = 4;
        let result = send(
            &mut hooks,
            guest_request("http://api.runtara.internal/v1/run/outcome"),
        )
        .await;
        assert!(matches!(result, Err(HttpError::HttpRequestBodySize(_))));
        assert!(recorder.seen.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::ApiDeadline;
    use std::time::Duration;

    #[tokio::test]
    async fn a_deadline_only_moves_earlier() {
        let deadline = ApiDeadline::new();
        assert!(!deadline.passed());
        deadline.shorten(Duration::from_secs(3600));
        deadline.shorten(Duration::from_millis(20));
        deadline.shorten(Duration::from_secs(3600));
        assert!(!deadline.passed());
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(deadline.passed());
    }
}
