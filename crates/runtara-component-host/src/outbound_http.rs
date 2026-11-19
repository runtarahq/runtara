//! Outbound HTTP contract and native service injection. No network transport here.
use std::sync::Arc;
use std::time::Duration;

/// A request through a connection: the host applies its credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionDestination {
    pub connection_id: String,
    pub url: String,
    pub endpoint: Option<String>,
    pub endpoint_ref: Option<String>,
    pub ai_provider: Option<String>,
    pub aws_service: Option<String>,
}

/// Where an outbound request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Connection(ConnectionDestination),
    /// A public URL, without credentials.
    Public(String),
}

/// One outbound request, as `POST /v1/http` carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestOptions {
    pub destination: Destination,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: Option<u64>,
    pub max_response_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A response whose body is handed over as it arrives.
pub struct StreamedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: axum::body::Body,
}

impl From<Response> for StreamedResponse {
    fn from(response: Response) -> Self {
        Self {
            status: response.status,
            headers: response.headers,
            body: axum::body::Body::from(response.body),
        }
    }
}

/// A request that failed, with whatever upstream response was received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub body: Vec<u8>,
    pub retry_after_ms: Option<u64>,
}

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

    /// As [`Self::request`], with the response's body handed over as it
    /// arrives. A host that cannot stream answers it whole.
    async fn request_streamed(
        &self,
        context: &OutboundContext,
        request: RequestOptions,
    ) -> Result<StreamedResponse, OutboundError> {
        self.request(context, request)
            .await
            .map(StreamedResponse::from)
    }
}

/// How long a streamed answer may take, judged by whether bytes keep coming
/// and not by one bound on the whole call (`docs/ai-agent-streaming.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamLimits {
    /// Until the first byte of the body: a model may think before it writes.
    pub first_byte: Duration,
    /// Between one byte and the next, once the first has come.
    pub stall: Duration,
    /// In all.
    pub total: Duration,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            first_byte: Duration::from_secs(60),
            stall: Duration::from_secs(30),
            total: Duration::from_secs(10 * 60),
        }
    }
}

impl StreamLimits {
    /// As set (`RUNTARA_STREAM_FIRST_BYTE_SECS`, `RUNTARA_STREAM_STALL_SECS`,
    /// `RUNTARA_STREAM_TOTAL_SECS`), each its default unless set.
    pub fn from_env() -> Self {
        let set = |name: &str, unless: Duration| {
            std::env::var(name)
                .ok()
                .and_then(|secs| secs.trim().parse::<u64>().ok())
                .filter(|secs| *secs > 0)
                .map_or(unless, Duration::from_secs)
        };
        let unless = Self::default();
        Self {
            first_byte: set("RUNTARA_STREAM_FIRST_BYTE_SECS", unless.first_byte),
            stall: set("RUNTARA_STREAM_STALL_SECS", unless.stall),
            total: set("RUNTARA_STREAM_TOTAL_SECS", unless.total),
        }
    }
}

/// A streamed body held to [`StreamLimits`] and a byte limit: it ends with an
/// error when no byte comes in time, or more come than allowed.
struct Watched {
    inner: axum::body::Body,
    limits: StreamLimits,
    started: tokio::time::Instant,
    /// When the last byte came; `None` before the first.
    last: Option<tokio::time::Instant>,
    /// What may still pass.
    remaining: usize,
    wake: std::pin::Pin<Box<tokio::time::Sleep>>,
    /// Never later than this: the run's own deadline.
    run_deadline: Option<tokio::time::Instant>,
    ended: bool,
}

impl Watched {
    fn new(
        inner: axum::body::Body,
        limits: StreamLimits,
        limit: usize,
        started: tokio::time::Instant,
        run_deadline: Option<tokio::time::Instant>,
    ) -> Self {
        let mut watched = Self {
            inner,
            limits,
            started,
            last: None,
            remaining: limit,
            wake: Box::pin(tokio::time::sleep_until(started)),
            run_deadline,
            ended: false,
        };
        let due = watched.due();
        watched.wake.as_mut().reset(due);
        watched
    }

    /// When the body has failed if nothing more has come.
    fn due(&self) -> tokio::time::Instant {
        let by_progress = match self.last {
            None => self.started + self.limits.first_byte,
            Some(last) => last + self.limits.stall,
        };
        let due = by_progress.min(self.started + self.limits.total);
        self.run_deadline.map_or(due, |run| due.min(run))
    }

    fn failed(
        &mut self,
        message: &str,
    ) -> std::task::Poll<Option<Result<http_body::Frame<bytes::Bytes>, axum::Error>>> {
        self.ended = true;
        std::task::Poll::Ready(Some(Err(axum::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            message.to_owned(),
        )))))
    }
}

impl http_body::Body for Watched {
    type Data = bytes::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        use std::task::Poll;
        if self.ended {
            return Poll::Ready(None);
        }
        match std::pin::Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if data.len() > self.remaining {
                        return self.failed("the streamed answer is over its byte limit");
                    }
                    self.remaining -= data.len();
                    if !data.is_empty() {
                        let now = tokio::time::Instant::now();
                        self.last = Some(now);
                        let due = self.due();
                        self.wake.as_mut().reset(due);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(other) => {
                if other.is_none() {
                    self.ended = true;
                }
                Poll::Ready(other)
            }
            Poll::Pending => match self.wake.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    let message = if self.last.is_none() {
                        "no byte of the streamed answer came in time"
                    } else {
                        "the streamed answer stopped coming"
                    };
                    self.failed(message)
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

/// Send `request` through `host` with its answer's body handed over as it
/// arrives: the head within [`StreamLimits::first_byte`] and the run's
/// deadline, and the body held to [`StreamLimits`] and its byte limit. Not
/// held to the request's own timeout, which bounds a whole answer.
pub async fn send_streamed(
    host: &RunOutboundHttp,
    request: RequestOptions,
    run_deadline: Option<tokio::time::Instant>,
    limits: StreamLimits,
) -> Result<StreamedResponse, OutboundError> {
    let started = tokio::time::Instant::now();
    let limit = response_limit(request.max_response_bytes)?;
    let head_by = run_deadline.map_or(started + limits.first_byte, |run| {
        run.min(started + limits.first_byte)
    });
    let response = match tokio::time::timeout_at(
        head_by,
        host.backend.request_streamed(&host.context, request),
    )
    .await
    {
        Err(_) => {
            return Err(error(
                "HTTP_DEADLINE_EXCEEDED",
                "no answer to the streamed request came in time",
            ));
        }
        Ok(Err(err)) => {
            return if err.body.len() > limit {
                Err(too_large())
            } else {
                Err(err)
            };
        }
        Ok(Ok(response)) => response,
    };
    Ok(StreamedResponse {
        status: response.status,
        headers: response.headers,
        body: axum::body::Body::new(Watched::new(
            response.body,
            limits,
            limit,
            started,
            run_deadline,
        )),
    })
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

    /// A host that answers with a body whose pieces come after the given
    /// pauses.
    struct Paced(Vec<(Duration, &'static str)>);

    #[async_trait::async_trait]
    impl OutboundHttpHost for Paced {
        async fn request(
            &self,
            _: &OutboundContext,
            _: RequestOptions,
        ) -> Result<Response, OutboundError> {
            unreachable!("asked to stream")
        }

        async fn request_streamed(
            &self,
            _: &OutboundContext,
            _: RequestOptions,
        ) -> Result<StreamedResponse, OutboundError> {
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(4);
            let pieces = self.0.clone();
            tokio::spawn(async move {
                for (pause, piece) in pieces {
                    tokio::time::sleep(pause).await;
                    if tx.send(Ok(bytes::Bytes::from(piece))).await.is_err() {
                        return;
                    }
                }
            });
            Ok(StreamedResponse {
                status: 200,
                headers: Vec::new(),
                body: axum::body::Body::new(Received(rx)),
            })
        }
    }

    /// A body of what a channel receives.
    struct Received(tokio::sync::mpsc::Receiver<Result<bytes::Bytes, std::io::Error>>);

    impl http_body::Body for Received {
        type Data = bytes::Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            self.0
                .poll_recv(cx)
                .map(|piece| piece.map(|piece| piece.map(http_body::Frame::data)))
        }
    }

    fn host(pieces: Vec<(Duration, &'static str)>) -> RunOutboundHttp {
        RunOutboundHttp {
            backend: Arc::new(Paced(pieces)),
            context: OutboundContext {
                tenant_id: "acme".into(),
                instance_id: Some("run".into()),
            },
        }
    }

    fn asked(max_response_bytes: Option<u64>) -> RequestOptions {
        RequestOptions {
            destination: Destination::Public("https://model.example".into()),
            method: "POST".into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: Some(1_000),
            max_response_bytes,
        }
    }

    /// The pieces, and how the body ended: `None` whole, or the error.
    async fn read(response: StreamedResponse) -> (Vec<(Duration, String)>, Option<String>) {
        use http_body_util::BodyExt;
        let started = tokio::time::Instant::now();
        let mut body = response.body;
        let mut pieces = Vec::new();
        while let Some(frame) = body.frame().await {
            match frame {
                Ok(frame) => {
                    if let Ok(data) = frame.into_data() {
                        pieces.push((
                            started.elapsed(),
                            String::from_utf8_lossy(&data).into_owned(),
                        ));
                    }
                }
                Err(error) => return (pieces, Some(error.to_string())),
            }
        }
        (pieces, None)
    }

    const SECOND: Duration = Duration::from_secs(1);

    /// A streamed answer passes as it comes, however long in all, as long as
    /// bytes keep coming; its request's own bound on a whole answer is not
    /// held to it.
    #[tokio::test(start_paused = true)]
    async fn a_streamed_answer_passes_as_it_comes_while_bytes_keep_coming() {
        let limits = StreamLimits::default();
        // Thinking for 50 s, then a piece every 20 s for two minutes.
        let mut pieces = vec![(50 * SECOND, "first")];
        pieces.extend(std::iter::repeat_n((20 * SECOND, "more"), 6));
        let response = send_streamed(&host(pieces), asked(None), None, limits)
            .await
            .unwrap();
        let (pieces, ended) = read(response).await;
        assert_eq!(ended, None, "{pieces:?}");
        assert_eq!(pieces.len(), 7);
        assert_eq!(pieces[0].0.as_secs(), 50);
        assert_eq!(pieces[6].0.as_secs(), 170, "well past the request's 1 s");
    }

    /// It ends with an error when the first byte is late, when bytes stop
    /// coming, when it has run too long in all, and when too many come.
    #[tokio::test(start_paused = true)]
    async fn a_streamed_answer_ends_when_bytes_stop_coming_or_too_many_come() {
        let limits = StreamLimits::default();
        let ended = |pieces: Vec<(Duration, &'static str)>, limit: Option<u64>| async move {
            let response = send_streamed(&host(pieces), asked(limit), None, limits)
                .await
                .unwrap();
            read(response).await
        };
        let (pieces, error) = ended(vec![(61 * SECOND, "late")], None).await;
        assert!(pieces.is_empty());
        assert!(error.unwrap().contains("no byte"));
        let (pieces, error) = ended(vec![(SECOND, "one"), (31 * SECOND, "late")], None).await;
        assert_eq!(pieces.len(), 1);
        assert!(error.unwrap().contains("stopped coming"));
        // A piece every 25 s, past ten minutes in all.
        let (pieces, error) =
            ended(std::iter::repeat_n((25 * SECOND, "x"), 30).collect(), None).await;
        assert_eq!(pieces.len(), 23, "{}", pieces.len());
        assert!(error.is_some());
        let (pieces, error) = ended(vec![(SECOND, "12345"), (SECOND, "67890")], Some(8)).await;
        assert_eq!(pieces.len(), 1);
        assert!(error.unwrap().contains("byte limit"));
        // And by the run's own deadline.
        let deadline = tokio::time::Instant::now() + 5 * SECOND;
        let response = send_streamed(
            &host(vec![(SECOND, "a"), (10 * SECOND, "b")]),
            asked(None),
            Some(deadline),
            limits,
        )
        .await
        .unwrap();
        let (pieces, error) = read(response).await;
        assert_eq!(pieces.len(), 1);
        assert!(error.is_some());
    }

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
