use std::collections::HashMap;
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
    checkpoints: HashMap<String, Vec<u8>>,
    retries: Vec<(String, u32, Option<String>)>,
    signal: Option<LifecycleSignal>,
    acked: Vec<(String, String)>,
    custom: Option<Vec<u8>>,
    /// Polls of the custom signal left before it shows up.
    custom_after: u32,
    events: Vec<Event>,
    heartbeats: u32,
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
            attempt: None,
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

    async fn get_checkpoint(&self, _: &Binding, id: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok(self.run.lock().unwrap().checkpoints.get(id).cloned())
    }

    async fn put_checkpoint(
        &self,
        _: &Binding,
        id: &str,
        state: Vec<u8>,
    ) -> Result<CheckpointWrite, ApiError> {
        let mut run = self.run.lock().unwrap();
        let stored = !run.checkpoints.contains_key(id);
        let state = run
            .checkpoints
            .entry(id.to_string())
            .or_insert(state)
            .clone();
        Ok(CheckpointWrite {
            stored,
            state,
            pending_signal: run.signal.clone(),
            custom_signal: run.custom.as_ref().map(|_| CustomSignalNotice {
                signal_id: "sig-1".into(),
                checkpoint_id: id.to_string(),
            }),
        })
    }

    async fn record_retry_attempt(
        &self,
        _: &Binding,
        id: &str,
        attempt: u32,
        error: Option<String>,
    ) -> Result<(), ApiError> {
        self.run
            .lock()
            .unwrap()
            .retries
            .push((id.to_string(), attempt, error));
        Ok(())
    }

    async fn poll_lifecycle_signal(
        &self,
        _: &Binding,
    ) -> Result<Option<LifecycleSignal>, ApiError> {
        Ok(self.run.lock().unwrap().signal.clone())
    }

    async fn ack_lifecycle_signal(
        &self,
        _: &Binding,
        command_id: &str,
        signal_type: &str,
    ) -> Result<bool, ApiError> {
        let mut run = self.run.lock().unwrap();
        run.acked
            .push((command_id.to_string(), signal_type.to_string()));
        Ok(run.signal.take().is_some())
    }

    async fn poll_custom_signal(&self, _: &Binding, _: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let mut run = self.run.lock().unwrap();
        if run.custom_after > 0 {
            run.custom_after -= 1;
            return Ok(None);
        }
        Ok(run.custom.clone())
    }

    async fn now_ms(&self, _: &Binding) -> Result<u64, ApiError> {
        Ok(1_730_000_000_000)
    }

    async fn heartbeat(&self, _: &Binding) -> Result<(), ApiError> {
        self.run.lock().unwrap().heartbeats += 1;
        Ok(())
    }

    async fn append_events(&self, _: &Binding, events: Vec<Event>) -> Result<(), ApiError> {
        self.run.lock().unwrap().events.extend(events);
        Ok(())
    }
}

fn binding() -> Binding {
    Binding {
        tenant_id: "tenant-a".into(),
        instance_id: "instance-1".into(),
        launch_id: "launch-1".into(),
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
        request("GET", "/v1/state/x").body(Body::empty()).unwrap(),
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

#[tokio::test]
async fn wake_ids_are_checked() {
    let app = app(Arc::default());
    let long = "w".repeat(Caps::default().wait_id_bytes + 1);
    let cases = [
        (
            r#"{"wakes":[{"instances":""}]}"#.to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!(r#"{{"wakes":[{{"instances":"{long}"}}]}}"#),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            r#"{"wakes":[{"onSignal":{"checkpointId":""}}]}"#.to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"wakes":[{"at":1}],"extra":true}"#.to_string(),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (n, (body, status)) in cases.into_iter().enumerate() {
        let (got, _, _) = send(&app, outcome("suspended", &format!("k{n}"), &body)).await;
        assert_eq!(got, status, "{body}");
    }
}

fn write(method: &str, path: &str, key: &str, body: &str) -> HttpRequest<Body> {
    request(method, path)
        .header(headers::IDEMPOTENCY_KEY, key)
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn checkpoints_keep_the_first_committed_state() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());
    let key = crate::encode_path_key("step/1:agent");
    let path = format!("/v1/checkpoints/{key}");

    let (status, _, body) = send(&app, request("GET", &path).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "not-found");

    let (status, headers, body) = send(&app, write("PUT", &path, "c1", "first")).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(&body[..], b"first");
    assert!(!headers.contains_key(headers::PENDING_SIGNAL));

    let (status, _, body) = send(&app, write("PUT", &path, "c2", "second")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"first", "the first committed state wins");

    let (status, _, body) = send(&app, request("GET", &path).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"first");
    assert!(
        backend
            .run
            .lock()
            .unwrap()
            .checkpoints
            .contains_key("step/1:agent")
    );

    let (status, _, _) = send(&app, write("PUT", &path, "c3", "")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_checkpoint_announces_pending_signals_without_payloads() {
    let backend = Arc::new(FakeBackend::default());
    {
        let mut run = backend.run.lock().unwrap();
        run.signal = Some(LifecycleSignal {
            signal_type: "cancel".into(),
            command_id: "cmd-1".into(),
            payload: b"secret-ish payload".to_vec(),
            checkpoint_id: None,
        });
        run.custom = Some(b"answer".to_vec());
    }
    let app = app(backend);
    let (_, headers, _) = send(&app, write("PUT", "/v1/checkpoints/c", "k1", "s")).await;
    let pending: serde_json::Value =
        serde_json::from_str(headers[headers::PENDING_SIGNAL].to_str().unwrap()).unwrap();
    assert_eq!(
        pending,
        serde_json::json!({"signalType": "cancel", "commandId": "cmd-1"})
    );
    let custom: serde_json::Value =
        serde_json::from_str(headers[headers::CUSTOM_SIGNAL].to_str().unwrap()).unwrap();
    assert_eq!(
        custom,
        serde_json::json!({"signalId": "sig-1", "checkpointId": "c"})
    );
}

#[tokio::test]
async fn lifecycle_signals_are_polled_then_acknowledged() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());
    let (status, _, _) = send(
        &app,
        request("GET", "/v1/signals/lifecycle")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    backend.run.lock().unwrap().signal = Some(LifecycleSignal {
        signal_type: "pause".into(),
        command_id: "cmd-7".into(),
        payload: b"{}".to_vec(),
        checkpoint_id: Some("c".into()),
    });
    let (status, headers, body) = send(
        &app,
        request("GET", "/v1/signals/lifecycle")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"{}");
    let notice: serde_json::Value =
        serde_json::from_str(headers[headers::SIGNAL].to_str().unwrap()).unwrap();
    assert_eq!(notice["commandId"], "cmd-7");
    assert_eq!(notice["checkpointId"], "c");

    let (status, _, body) = send(
        &app,
        write(
            "POST",
            "/v1/signals/lifecycle/cmd-7/ack",
            "a1",
            r#"{"signalType":"pause"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], br#"{"applied":true}"#);
    assert_eq!(
        backend.run.lock().unwrap().acked,
        vec![("cmd-7".to_string(), "pause".to_string())]
    );
}

#[tokio::test]
async fn a_custom_signal_long_poll_waits_for_it() {
    let backend = Arc::new(FakeBackend::default());
    {
        let mut run = backend.run.lock().unwrap();
        run.custom = Some(b"approved".to_vec());
        run.custom_after = 2;
    }
    let app = app(backend.clone());
    let (status, _, _) = send(
        &app,
        request("GET", "/v1/signals/custom/c")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "no wait, not there yet");

    let (status, _, body) = send(
        &app,
        request("GET", "/v1/signals/custom/c?waitMs=5000")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"approved");
}

#[tokio::test]
async fn time_heartbeat_and_retry_attempts() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());
    let (status, _, body) = send(
        &app,
        request("GET", "/v1/time").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], br#"{"nowMs":1730000000000}"#);

    let (status, _, _) = send(&app, write("POST", "/v1/heartbeat", "h1", "")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(backend.run.lock().unwrap().heartbeats, 1);

    let (status, _, _) = send(
        &app,
        write(
            "POST",
            "/v1/checkpoints/c/retry-attempts",
            "r1",
            r#"{"attemptNumber":2,"errorMessage":"boom"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        backend.run.lock().unwrap().retries,
        vec![("c".to_string(), 2, Some("boom".to_string()))]
    );
}

#[tokio::test]
async fn events_are_ndjson_within_the_caps() {
    let backend = Arc::new(FakeBackend::default());
    let app = app(backend.clone());
    let batch =
        "{\"kind\":\"step_debug_start\",\"payload\":{\"stepId\":\"a\"}}\n\n{\"kind\":\"custom\"}\n";
    let (status, _, _) = send(&app, write("POST", "/v1/events", "e1", batch)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let events = backend.run.lock().unwrap().events.clone();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, "step_debug_start");
    assert_eq!(events[0].payload["stepId"], "a");
    assert_eq!(events[1].payload, serde_json::Value::Null);

    let (status, _, _) = send(&app, write("POST", "/v1/events", "e2", "not json\n")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let small = router(WorkflowApiState {
        backend: backend.clone(),
        receipts: Arc::new(MemoryReceiptStore::default()),
        caps: Caps {
            event_batch_lines: 1,
            ..Caps::default()
        },
    });
    let (status, _, _) = send(&small, write("POST", "/v1/events", "e3", batch)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn services_a_backend_does_not_serve_are_unsupported() {
    struct Minimal;
    #[async_trait]
    impl WorkflowApiBackend for Minimal {
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
            unreachable!()
        }
        async fn set_deadline(&self, _: &Binding, _: u64) -> Result<(), ApiError> {
            unreachable!()
        }
    }
    let app = router(WorkflowApiState {
        backend: Arc::new(Minimal),
        receipts: Arc::new(MemoryReceiptStore::default()),
        caps: Caps::default(),
    });
    let (status, _, body) = send(
        &app,
        request("GET", "/v1/time").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(error_code(&body), "unsupported");
}

#[tokio::test]
async fn a_state_patch_must_be_an_object() {
    let app = app(Arc::default());
    for body in ["[1,2]", "\"x\"", "not json"] {
        let (status, _, response) = send(
            &app,
            write("PATCH", "/v1/state/k", &format!("p-{body}"), body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(error_code(&response), "invalid");
    }
    // An object reaches the backend, which this fake does not serve.
    let (status, _, _) = send(&app, write("PATCH", "/v1/state/k", "p-ok", r#"{"a":1}"#)).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn an_input_registration_names_its_own_input() {
    let app = app(Arc::default());
    let (status, _, body) = send(
        &app,
        write(
            "PUT",
            "/v1/inputs/approve",
            "i1",
            r#"{"descriptor":{"signal_id":"other"}}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "invalid");
    // A matching registration reaches the backend, which this fake does not serve.
    let (status, _, _) = send(
        &app,
        write(
            "PUT",
            "/v1/inputs/approve",
            "i2",
            r#"{"descriptor":{"signal_id":"approve"},"deadlineMs":5}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[test]
fn input_reads_are_tagged_by_state() {
    assert_eq!(
        serde_json::to_value(crate::InputRead::Open).unwrap(),
        serde_json::json!({"state": "open"})
    );
    assert_eq!(
        serde_json::to_value(crate::InputRead::Accepted {
            value: serde_json::json!({"ok": true})
        })
        .unwrap(),
        serde_json::json!({"state": "accepted", "value": {"ok": true}})
    );
    assert_eq!(
        serde_json::to_value(crate::InputRead::Closed {
            reason: "expired".into()
        })
        .unwrap(),
        serde_json::json!({"state": "closed", "reason": "expired"})
    );
}
