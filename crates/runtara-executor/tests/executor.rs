// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! An executor as a server uses it: the probe workflow staged and placed over
//! the placement API, its run in a real `runtara-run` (sandboxed on Linux),
//! its workflow API calls arriving at a run channel served here.
//!
//! Needs the probe component (`scripts/build-agent-components.sh`) and the
//! runner: `cargo build -p runtara-run`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use runtara_executor::protocol::{EXECUTOR_NAME, Placed, RunState, SERVER_NAME};
use runtara_executor::tls::{self, Identity};
use serde_json::{Value, json};
use sha2::Digest;

const CREDENTIAL: &str = "rlc1_test_credential";

fn probe() -> Vec<u8> {
    let path = std::env::var_os("RUNTARA_WORKFLOW_API_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release/runtara_workflow_api_probe.wasm")
        });
    std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing {}: build runtara-workflow-api-probe for wasm32-wasip2",
            path.display()
        )
    })
}

fn runner() -> PathBuf {
    let path = std::env::var_os("RUNTARA_EXECUTOR_RUN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // target/<profile>/deps/<this test> → target/<profile>/runtara-run
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .with_file_name("runtara-run")
        });
    assert!(
        path.exists(),
        "missing {}: cargo build -p runtara-run",
        path.display()
    );
    path
}

fn digest(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// An outcome the run channel was posted.
#[derive(Clone, Debug)]
struct Outcome {
    kind: String,
    body: Value,
    authorization: String,
    idempotency_key: String,
}

#[derive(Clone, Default)]
struct RunChannel {
    mode: Arc<Mutex<Value>>,
    outcomes: Arc<Mutex<Vec<Outcome>>>,
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn run_info() -> impl IntoResponse {
    axum::Json(json!({
        "instanceId": "instance-1", "tenantId": "tenant-1",
        "launchKind": "start", "debug": false, "apiVersion": "1.0",
    }))
}

async fn input(State(channel): State<RunChannel>) -> impl IntoResponse {
    let mode = channel.mode.lock().unwrap().clone();
    (
        [
            ("runtara-checkpoints", "none"),
            ("runtara-lifecycle", "none"),
        ],
        json!({"data": mode, "variables": {}}).to_string(),
    )
}

async fn outcome(State(channel): State<RunChannel>, headers: HeaderMap, body: Bytes) -> StatusCode {
    channel.outcomes.lock().unwrap().push(Outcome {
        kind: header(&headers, "runtara-outcome"),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
        authorization: header(&headers, "authorization"),
        idempotency_key: header(&headers, "idempotency-key"),
    });
    StatusCode::NO_CONTENT
}

/// A server's two halves (its run channel and its placement client) and an
/// executor, each with its own certificate of one CA.
struct Deployment {
    channel: RunChannel,
    endpoint: String,
    ca_pem: String,
    executor: String,
    client: tls::Client,
    /// The executor's own certificate, as a client: not the server's.
    impostor: tls::Client,
    _state: tempfile::TempDir,
}

impl Deployment {
    async fn start(max_runs: usize) -> Self {
        let (for_server, for_executor) = runtara_executor::pki::issue().unwrap();
        let server = Identity::from_pem(
            for_server.ca_pem.clone(),
            for_server.certificate_pem.as_bytes(),
            for_server.key_pem.as_bytes(),
        )
        .unwrap();
        let state = tempfile::tempdir().unwrap();
        let tls_dir = state.path().join("tls");
        std::fs::create_dir_all(&tls_dir).unwrap();
        std::fs::write(tls_dir.join("ca.pem"), &for_executor.ca_pem).unwrap();
        std::fs::write(tls_dir.join("tls.pem"), &for_executor.certificate_pem).unwrap();
        std::fs::write(tls_dir.join("tls.key"), &for_executor.key_pem).unwrap();

        let channel = RunChannel::default();
        let router = axum::Router::new()
            .route("/v1/run", get(run_info))
            .route("/v1/run/input", get(input))
            .route("/v1/run/outcome", post(outcome))
            .with_state(channel.clone());
        let listener = tls::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        tokio::spawn(tls::serve(
            listener,
            server.server_config().unwrap(),
            EXECUTOR_NAME,
            router,
        ));

        let executor = runtara_executor::Executor::bind(&runtara_executor::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            tls_dir: tls_dir.clone(),
            state_dir: state.path().join("state"),
            max_runs,
            run_binary: runner(),
            no_landlock: std::env::var_os("RUNTARA_RUN_TEST_NO_LANDLOCK").is_some(),
            unsandboxed: !cfg!(target_os = "linux"),
        })
        .await
        .unwrap();
        let address = format!("https://{}", executor.address());
        tokio::spawn(executor.serve());

        let client = tls::client(
            server.client_config(&for_server.ca_pem, None).unwrap(),
            EXECUTOR_NAME,
        )
        .unwrap();
        let impostor = tls::client(
            Identity::load(&tls_dir)
                .unwrap()
                .client_config(&for_server.ca_pem, None)
                .unwrap(),
            EXECUTOR_NAME,
        )
        .unwrap();
        Self {
            channel,
            endpoint,
            ca_pem: for_server.ca_pem,
            executor: address,
            client,
            impostor,
            _state: state,
        }
    }

    async fn call(&self, method: &str, path: &str, body: Vec<u8>) -> (StatusCode, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.executor))
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))
            .unwrap();
        let response = self.client.request(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn stage(&self, tenant: &str) -> String {
        let probe = probe();
        let digest = digest(&probe);
        let path = format!("/v1/artifacts/{tenant}/{digest}");
        let (status, body) = self.call("PUT", &path, probe).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(
            self.call("GET", &path, Vec::new()).await.0,
            StatusCode::NO_CONTENT
        );
        digest
    }

    fn place_body(&self, tenant: &str, artifact: &str, lifetime_seconds: u64) -> Vec<u8> {
        json!({
            "tenant": tenant,
            "artifact": artifact,
            "spec": {
                "protocol": 1,
                "run": {"execution": "launch-1", "generation": 3, "trace_id": "launch-1"},
                "channel": {
                    "endpoint": self.endpoint,
                    "credential": CREDENTIAL,
                    "trust": {"ca_pem": self.ca_pem, "pin": null, "server_name": SERVER_NAME},
                },
                "wasm_memory_bytes": 64 * 1024 * 1024,
                "lifetime_seconds": lifetime_seconds,
                "env": {},
            },
        })
        .to_string()
        .into_bytes()
    }

    /// Stages the probe and places a run of it in `mode`; the run's id.
    async fn place(&self, mode: Value, lifetime_seconds: u64) -> String {
        *self.channel.mode.lock().unwrap() = mode;
        let artifact = self.stage("tenant-1").await;
        let (status, body) = self
            .call(
                "POST",
                "/v1/runs",
                self.place_body("tenant-1", &artifact, lifetime_seconds),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value::<Placed>(body).unwrap().run
    }

    async fn ended(&self, run: &str) -> RunState {
        for _ in 0..12 {
            let (status, body) = self
                .call("GET", &format!("/v1/runs/{run}?wait_ms=10000"), Vec::new())
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let state: RunState = serde_json::from_value(body).unwrap();
            if state != RunState::Running {
                return state;
            }
        }
        panic!("the run did not end");
    }

    fn outcomes(&self) -> Vec<Outcome> {
        self.channel.outcomes.lock().unwrap().clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_completes_and_its_calls_carry_the_launch_credential() {
    let deployment = Deployment::start(4).await;
    let run = deployment.place(json!({"mode": "complete"}), 60).await;
    assert_eq!(deployment.ended(&run).await, RunState::Exited { code: 0 });
    let outcomes = deployment.outcomes();
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].kind, "completed");
    assert_eq!(outcomes[0].body["instanceId"], "instance-1");
    assert_eq!(outcomes[0].authorization, format!("Bearer {CREDENTIAL}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_traps_has_its_failure_posted_by_the_executor() {
    let deployment = Deployment::start(4).await;
    let run = deployment.place(json!({"mode": "trap"}), 60).await;
    assert_eq!(deployment.ended(&run).await, RunState::Exited { code: 1 });
    let outcomes = deployment.outcomes();
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].kind, "failed");
    assert_eq!(outcomes[0].body["code"], "GUEST_FAILED");
    assert_eq!(outcomes[0].body["category"], "permanent");
    assert_eq!(outcomes[0].authorization, format!("Bearer {CREDENTIAL}"));
    assert_eq!(
        outcomes[0].idempotency_key,
        "runtara-executor-outcome-launch-1-3"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_past_its_lifetime_is_ended_and_reported_as_a_timeout() {
    let deployment = Deployment::start(4).await;
    let run = deployment.place(json!({"mode": "spin"}), 1).await;
    assert_eq!(deployment.ended(&run).await, RunState::Exited { code: 1 });
    let outcomes = deployment.outcomes();
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].body["code"], "LIFETIME_EXCEEDED");
    assert_eq!(outcomes[0].body["category"], "timeout");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_run_ends_without_an_outcome() {
    let deployment = Deployment::start(4).await;
    let run = deployment.place(json!({"mode": "spin"}), 60).await;
    let path = format!("/v1/runs/{run}");
    assert_eq!(
        deployment.call("DELETE", &path, Vec::new()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        deployment.ended(&run).await,
        RunState::Failed {
            reason: "stopped".to_owned()
        }
    );
    assert!(deployment.outcomes().is_empty());
    assert_eq!(
        deployment
            .call("DELETE", "/v1/runs/nope", Vec::new())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_executor_refuses_a_run_for_now() {
    let deployment = Deployment::start(1).await;
    let first = deployment.place(json!({"mode": "spin"}), 60).await;
    let artifact = deployment.stage("tenant-1").await;
    let (status, body) = deployment
        .call(
            "POST",
            "/v1/runs",
            deployment.place_body("tenant-1", &artifact, 60),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "capacity");
    deployment
        .call("DELETE", &format!("/v1/runs/{first}"), Vec::new())
        .await;
    deployment.ended(&first).await;
    // Its slot is free again.
    let (status, _) = deployment
        .call(
            "POST",
            "/v1/runs",
            deployment.place_body("tenant-1", &artifact, 60),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

/// Many runs placed in the same instant all start and all end: starting one
/// process must not hold up starting another.
#[tokio::test(flavor = "multi_thread")]
async fn runs_placed_at_once_all_end() {
    let deployment = Deployment::start(16).await;
    *deployment.channel.mode.lock().unwrap() = json!({"mode": "complete"});
    let artifact = deployment.stage("tenant-1").await;
    for _round in 0..4 {
        let placed = futures_all((0..12).map(|_| {
            deployment.call(
                "POST",
                "/v1/runs",
                deployment.place_body("tenant-1", &artifact, 60),
            )
        }));
        let placed = tokio::time::timeout(Duration::from_secs(60), placed)
            .await
            .expect("every run is placed");
        for (status, body) in placed {
            assert_eq!(status, StatusCode::OK, "{body}");
            let run = serde_json::from_value::<Placed>(body).unwrap().run;
            assert_eq!(deployment.ended(&run).await, RunState::Exited { code: 0 });
        }
    }
    assert_eq!(deployment.outcomes().len(), 48);
}

/// Every future's output, all of them driven at once.
async fn futures_all<F: std::future::Future>(futures: impl Iterator<Item = F>) -> Vec<F::Output> {
    let mut pending: Vec<_> = futures.map(|future| Box::pin(future)).collect();
    let mut done: Vec<Option<F::Output>> = pending.iter().map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut waiting = false;
        for (index, future) in pending.iter_mut().enumerate() {
            if done[index].is_none() {
                match future.as_mut().poll(context) {
                    std::task::Poll::Ready(output) => done[index] = Some(output),
                    std::task::Poll::Pending => waiting = true,
                }
            }
        }
        if waiting {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    done.into_iter().map(|output| output.unwrap()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_is_staged_for_its_tenant_only_and_checked() {
    let deployment = Deployment::start(4).await;
    let artifact = deployment.stage("tenant-1").await;
    // Another tenant has not staged it.
    let (status, body) = deployment
        .call(
            "POST",
            "/v1/runs",
            deployment.place_body("tenant-2", &artifact, 60),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // Bytes that are not the digest's.
    let (status, body) = deployment
        .call(
            "PUT",
            &format!("/v1/artifacts/tenant-1/{}", "0".repeat(64)),
            b"x".to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Bytes that are not a workflow.
    let garbage = b"not a component".to_vec();
    let (status, body) = deployment
        .call(
            "PUT",
            &format!("/v1/artifacts/tenant-1/{}", digest(&garbage)),
            garbage,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "compile");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_servers_certificate_may_place_runs() {
    let deployment = Deployment::start(4).await;
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("{}/v1/runs/any", deployment.executor))
        .body(Full::new(Bytes::new()))
        .unwrap();
    // Signed by the same CA, but not the server's: no request is answered.
    let answered = tokio::time::timeout(
        Duration::from_secs(10),
        deployment.impostor.request(request),
    )
    .await
    .expect("the connection was neither served nor closed");
    assert!(answered.is_err());
}
