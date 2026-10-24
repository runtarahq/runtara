use std::sync::Mutex;

use axum::body::to_bytes;
use axum::http::Request as HttpRequest;
use tower::ServiceExt;

use super::*;
use crate::Wake;

#[derive(Default)]
struct FakeRun {
    finished: bool,
    fenced: bool,
    outcomes: Vec<PostedOutcome>,
    deadline: Option<u64>,
}

#[derive(Default)]
struct FakeBackend {
    run: Mutex<FakeRun>,
}

#[async_trait]
impl WorkflowApiBackend for FakeBackend {
    async fn launch_state(&self, _binding: &Binding) -> Result<LaunchState, ApiError> {
        let run = self.run.lock().unwrap();
        Ok(if run.fenced {
            LaunchState::Fenced
        } else if run.finished {
            LaunchState::Finished
        } else {
            LaunchState::Active
        })
    }

    async fn run_info(&self, binding: &Binding) -> Result<RunInfo, ApiError> {
        Ok(RunInfo {
            instance_id: binding.instance_id.clone(),
            tenant_id: binding.tenant_id.clone(),
            workflow_id: Some("wf-1".into()),
            version: Some(3),
            attempt: binding.attempt,
            launch_kind: "fresh".into(),
            debug: false,
            deadline_ms: None,
            api_version: crate::api_version(),
            capability_id: None,
        })
    }

    async fn run_input(&self, _binding: &Binding) -> Result<Vec<u8>, ApiError> {
        Ok(br#"{"data":{"x":1},"variables":{}}"#.to_vec())
    }

    async fn post_outcome(
        &self,
        _binding: &Binding,
        outcome: PostedOutcome,
    ) -> Result<(), ApiError> {
        let mut run = self.run.lock().unwrap();
        if run.finished {
            return Err(ApiError::new(ErrorCode::Finished, "already finished"));
        }
        run.outcomes.push(outcome);
        run.finished = true;
        Ok(())
    }

    async fn set_deadline(&self, _binding: &Binding, abort_after_ms: u64) -> Result<(), ApiError> {
        self.run.lock().unwrap().deadline = Some(abort_after_ms);
        Ok(())
    }
}

fn binding() -> Binding {
    Binding {
        tenant_id: "tenant-a".into(),
        instance_id: "instance-1".into(),
        launch_id: "launch-1".into(),
        attempt: 1,
        root_lease_epoch: 7,
    }
}

fn app(backend: Arc<FakeBackend>) -> Router {
    router(WorkflowApiState {
        backend,
        receipts: Arc::new(MemoryReceiptStore::default()),
        caps: Caps::default(),
    })
}

fn request(method: &str, path: &str) -> axum::http::request::Builder {
    HttpRequest::builder()
        .method(method)
        .uri(path)
        .extension(binding())
}

async fn send(app: &Router, request: HttpRequest<Body>) -> (StatusCode, HeaderMap, Bytes) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, headers, body)
}

fn error_code(body: &Bytes) -> String {
    serde_json::from_slice::<serde_json::Value>(body).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string()
}

fn outcome(kind: &str, key: &str, body: &str) -> HttpRequest<Body> {
    request("POST", "/v1/run/outcome")
        .header(headers::OUTCOME, kind)
        .header(headers::IDEMPOTENCY_KEY, key)
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn a_request_without_a_binding_is_denied() {
    let app = app(Arc::default());
    let request = HttpRequest::builder()
        .uri("/v1/run")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "denied");
}

#[tokio::test]
async fn run_info_comes_from_the_binding() {
    let app = app(Arc::default());
    let (status, headers, body) = send(
        &app,
        request("GET", "/v1/run")
            .header(headers::API_VERSION, "1.0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    let info: RunInfo = serde_json::from_slice(&body).unwrap();
    assert_eq!(info.instance_id, "instance-1");
    assert_eq!(info.tenant_id, "tenant-a");
}

#[tokio::test]
async fn an_unknown_major_version_is_unsupported() {
    let app = app(Arc::default());
    let (status, _, body) = send(
        &app,
        request("GET", "/v1/run")
            .header(headers::API_VERSION, "2.0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(error_code(&body), "unsupported");
}

#[tokio::test]
async fn input_is_raw_bytes() {
    let app = app(Arc::default());
    let (status, headers, body) = send(
        &app,
        request("GET", "/v1/run/input").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert_eq!(&body[..], br#"{"data":{"x":1},"variables":{}}"#);
}

#[tokio::test]
async fn writes_need_an_idempotency_key() {
    let app = app(Arc::default());
    let request = request("POST", "/v1/run/outcome")
        .header(headers::OUTCOME, "completed")
        .body(Body::from("{}"))
        .unwrap();
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid");
}

#[tokio::test]
async fn an_outcome_finishes_the_run_and_a_retry_replays_its_receipt() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());

    let (status, _, _) = send(&app, outcome("completed", "k1", r#"{"ok":true}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        backend.run.lock().unwrap().outcomes,
        vec![PostedOutcome::Completed(br#"{"ok":true}"#.to_vec())]
    );

    // A network retry of the same request gets the stored answer, not
    // `finished`, and the backend sees the outcome once.
    let (status, _, _) = send(&app, outcome("completed", "k1", r#"{"ok":true}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(backend.run.lock().unwrap().outcomes.len(), 1);

    // The same key with a different request is a conflict.
    let (status, _, body) = send(&app, outcome("completed", "k1", r#"{"ok":false}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "replay-conflict");

    // After the outcome only `GET /v1/run` is served.
    let (status, _, body) = send(
        &app,
        request("GET", "/v1/run/input").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "finished");
    let (status, _, _) = send(&app, request("GET", "/v1/run").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_fenced_launch_is_refused_everywhere() {
    let backend = Arc::new(FakeBackend::default());
    backend.run.lock().unwrap().fenced = true;
    let app = app(backend.clone());
    for request in [
        request("GET", "/v1/run").body(Body::empty()).unwrap(),
        outcome("completed", "k1", "{}"),
    ] {
        let (status, _, body) = send(&app, request).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error_code(&body), "fenced");
    }
    assert!(backend.run.lock().unwrap().outcomes.is_empty());
}

#[tokio::test]
async fn suspensions_are_checked_against_the_caps() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());

    let (status, _, _) = send(&app, outcome("suspended", "k1", r#"{"wakes":[]}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let many = serde_json::json!({"wakes": vec![serde_json::json!({"at": 1}); 17]});
    let (status, _, body) = send(&app, outcome("suspended", "k2", &many.to_string())).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(&body), "too-large");

    let (status, _, _) = send(
        &app,
        outcome(
            "suspended",
            "k3",
            r#"{"wakes":[{"at":5},{"onResume":true}]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        backend.run.lock().unwrap().outcomes,
        vec![PostedOutcome::Suspended(Suspension {
            wakes: vec![Wake::At(5), Wake::OnResume]
        })]
    );
}

#[tokio::test]
async fn a_failed_outcome_carries_error_info() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());
    let body = r#"{"code":"E1","message":"boom","category":"permanent","severity":"error","retryable":false}"#;
    let (status, _, _) = send(&app, outcome("failed", "k1", body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    match &backend.run.lock().unwrap().outcomes[0] {
        PostedOutcome::Failed(info) => assert_eq!(info.code, "E1"),
        other => panic!("{other:?}"),
    }

    let (status, _, _) = send(&app, outcome("cancelled", "k2", "{}")).await;
    assert_eq!(status, StatusCode::CONFLICT, "finished before kind check");
}

#[tokio::test]
async fn deadline_can_be_set() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());
    let request = request("PUT", "/v1/run/deadline")
        .header(headers::IDEMPOTENCY_KEY, "d1")
        .body(Body::from(r#"{"abortAfterMs":1500}"#))
        .unwrap();
    let (status, _, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(backend.run.lock().unwrap().deadline, Some(1500));
}

#[tokio::test]
async fn endpoints_not_served_yet_are_unsupported() {
    let app = app(Arc::default());
    let (status, _, body) = send(
        &app,
        request("GET", "/v1/checkpoints/x")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(error_code(&body), "unsupported");
}

#[tokio::test]
async fn retryable_failures_are_not_remembered() {
    struct Flaky(Mutex<u32>);
    #[async_trait]
    impl WorkflowApiBackend for Flaky {
        async fn launch_state(&self, _: &Binding) -> Result<LaunchState, ApiError> {
            Ok(LaunchState::Active)
        }
        async fn run_info(&self, _: &Binding) -> Result<RunInfo, ApiError> {
            unreachable!()
        }
        async fn run_input(&self, _: &Binding) -> Result<Vec<u8>, ApiError> {
            unreachable!()
        }
        async fn post_outcome(&self, _: &Binding, _: PostedOutcome) -> Result<(), ApiError> {
            let mut calls = self.0.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                Err(ApiError::new(ErrorCode::Unavailable, "try again"))
            } else {
                Ok(())
            }
        }
        async fn set_deadline(&self, _: &Binding, _: u64) -> Result<(), ApiError> {
            unreachable!()
        }
    }
    let flaky = Arc::new(Flaky(Mutex::new(0)));
    let app = router(WorkflowApiState {
        backend: flaky.clone(),
        receipts: Arc::new(MemoryReceiptStore::default()),
        caps: Caps::default(),
    });
    let (status, _, _) = send(&app, outcome("completed", "k1", "{}")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, _) = send(&app, outcome("completed", "k1", "{}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(*flaky.0.lock().unwrap(), 2);
}
