// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The provider-neutral runner over a fake provider whose "runner" is a task
//! calling the run channel as a shim does: a launch is placed, its slot given
//! back once it is placed, its calls served by its credential alone, and it
//! ends on its outcome; a run its provider loses without an outcome is
//! handed to recovery.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use runtara_artifact_store::{ArtifactStore, FilesystemArtifactStore};
use runtara_component_host::api_launches::ApiLaunches;
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::Persistence;
use runtara_core::persistence::memory::InMemoryPersistence;
use runtara_environment::runner::{
    HostServices, LaunchOptions, Provider, ProviderEnd, ProviderRunner, RunChannel, RunExitReport,
    Runner, RunnerParts,
};
use runtara_workflow_api::Binding;
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tower::ServiceExt;

/// What the fake runner does with its launch.
#[derive(Clone, Copy, PartialEq)]
enum Behaviour {
    /// Reads its input and posts a completed outcome echoing it.
    Complete,
    /// Is lost by its provider without posting anything.
    Lose,
}

struct FakeProvider {
    router: axum::Router,
    behaviour: Behaviour,
    ends: Arc<Mutex<HashMap<String, ProviderEnd>>>,
    changed: Arc<Notify>,
    /// Held by the fake runner until the test lets it go on.
    hold: Mutex<Option<oneshot::Receiver<()>>>,
    placed: Arc<Notify>,
}

async fn call(router: &axum::Router, request: Request<Body>) -> (u16, Vec<u8>) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

#[async_trait]
impl Provider for FakeProvider {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn spec(&self, _options: &LaunchOptions, component_bytes: u64) -> Value {
        json!({"mode":"command","component_bytes":component_bytes})
    }
    async fn stage(&self, _tenant: &str, bytes: &[u8]) -> Result<String, String> {
        Ok(format!("staged-{}", bytes.len()))
    }
    async fn place(
        &self,
        spec: Value,
        artifact: &str,
        binding: &Binding,
    ) -> Result<String, String> {
        assert_eq!(artifact, "staged-8");
        assert_eq!(spec["run"]["execution"], binding.launch_id.as_str());
        let credential = spec["channel"]["credential"].as_str().unwrap().to_owned();
        let run = format!("run-{}", binding.launch_id);
        let (router, ends, changed, behaviour) = (
            self.router.clone(),
            self.ends.clone(),
            self.changed.clone(),
            self.behaviour,
        );
        let hold = self.hold.lock().unwrap().take();
        let id = run.clone();
        self.placed.notify_one();
        tokio::spawn(async move {
            if let Some(hold) = hold {
                let _ = hold.await;
            }
            let end = match behaviour {
                Behaviour::Complete => {
                    let (status, input) = call(
                        &router,
                        Request::get("/v1/run/input")
                            .header("authorization", format!("Bearer {credential}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await;
                    assert_eq!(status, 200);
                    let (status, _) = call(
                        &router,
                        Request::post("/v1/run/outcome")
                            .header("authorization", format!("Bearer {credential}"))
                            .header("runtara-outcome", "completed")
                            .header("idempotency-key", "fake-outcome")
                            .body(Body::from(input))
                            .unwrap(),
                    )
                    .await;
                    assert_eq!(status, 204);
                    ProviderEnd::Exited(0)
                }
                Behaviour::Lose => ProviderEnd::Lost("lost with its host".into()),
            };
            ends.lock().unwrap().insert(id, end);
            changed.notify_waiters();
        });
        Ok(run)
    }
    async fn stop(&self, _run: &str) {}
    async fn ended(&self, run: &str, timeout: Duration) -> Option<ProviderEnd> {
        let notified = self.changed.notified();
        if let Some(end) = self.ends.lock().unwrap().get(run).cloned() {
            return Some(end);
        }
        let _ = tokio::time::timeout(timeout, notified).await;
        self.ends.lock().unwrap().get(run).cloned()
    }
}

struct Setup {
    runner: ProviderRunner,
    persistence: Arc<dyn Persistence>,
    workflow: runtara_artifact_store::Digest,
    _dir: tempfile::TempDir,
}

async fn setup(
    behaviour: Behaviour,
    hold: Option<oneshot::Receiver<()>>,
    placed: Arc<Notify>,
) -> Setup {
    setup_on(
        Arc::new(InMemoryPersistence::new()),
        behaviour,
        hold,
        placed,
    )
    .await
}

async fn setup_on(
    persistence: Arc<dyn Persistence>,
    behaviour: Behaviour,
    hold: Option<oneshot::Receiver<()>>,
    placed: Arc<Notify>,
) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let artifacts = Arc::new(FilesystemArtifactStore::new(dir.path().join("artifacts")));
    // An empty module: the runner reads only its trusted pins.
    let workflow = artifacts.put(b"\0asm\x01\0\0\0").await.unwrap();
    let launches = Arc::new(ApiLaunches::default());
    let provider = Arc::new(FakeProvider {
        router: launches.credentialed_router(),
        behaviour,
        ends: Arc::default(),
        changed: Arc::new(Notify::new()),
        hold: Mutex::new(hold),
        placed,
    });
    let runner = ProviderRunner::new(
        provider,
        RunnerParts {
            persistence: persistence.clone(),
            event_observer: None,
            services: HostServices::default(),
            artifacts: artifacts.clone(),
        },
        launches,
        1,
    )
    .with_run_channel(RunChannel {
        endpoint: "https://127.0.0.1:1".into(),
        ca_pem: "ca".into(),
        pin: None,
        server_name: None,
    });
    Setup {
        runner,
        persistence,
        workflow,
        _dir: dir,
    }
}

fn options(instance: &str, workflow: runtara_artifact_store::Digest) -> LaunchOptions {
    LaunchOptions {
        launch_id: format!("launch-{instance}"),
        instance_id: instance.into(),
        tenant_id: "tenant-p".into(),
        workflow_digest: workflow,
        preparation_attempt: None,
        preparation_deadline: None,
        input: Value::Null,
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
        env: HashMap::new(),
        prepersisted_input: None,
        launch_kind: runtara_environment::launch_queue::LaunchKind::Start,
        start_gate: None,
        memory_limit_bytes: None,
    }
}

async fn register(persistence: &Arc<dyn Persistence>, instance: &str) {
    let input = json!({"data":{"from":"provider-test"},"variables":{}});
    assert!(
        persistence
            .try_register_instance(
                instance,
                "tenant-p",
                Some(&serde_json::to_vec(&input).unwrap())
            )
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_placed_run_gives_its_slot_back_and_ends_on_its_outcome() {
    let (go, hold) = oneshot::channel();
    let placed = Arc::new(Notify::new());
    let s = setup(Behaviour::Complete, Some(hold), placed.clone()).await;
    register(&s.persistence, "inst-1").await;
    let handle = s
        .runner
        .try_launch_detached(&options("inst-1", s.workflow.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), placed.notified())
        .await
        .unwrap();
    // Placed and started, still running: no slot is held for it.
    tokio::time::timeout(Duration::from_secs(5), async {
        while s.runner.occupancy().unwrap().held != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the slot is given back while the run runs");
    assert!(s.runner.is_running(&handle).await);
    go.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        s.runner.wait_for_exit(&handle, Duration::from_millis(20)),
    )
    .await
    .expect("the run ends on its outcome");
    let record = s.persistence.get_instance("inst-1").await.unwrap().unwrap();
    assert_eq!(record.status, InstanceStatus::Completed, "{record:?}");
    let output: Value = serde_json::from_slice(record.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["data"]["from"], "provider-test");
    assert!(s.runner.take_exit_report(&handle).await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_its_provider_loses_is_handed_to_recovery() {
    let s = setup(Behaviour::Lose, None, Arc::new(Notify::new())).await;
    register(&s.persistence, "inst-2").await;
    let handle = s
        .runner
        .try_launch_detached(&options("inst-2", s.workflow.clone()))
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        s.runner.wait_for_exit(&handle, Duration::from_millis(20)),
    )
    .await
    .expect("the lost run ends");
    match s.runner.take_exit_report(&handle).await {
        Some(RunExitReport::Lost { reason }) => {
            assert!(reason.contains("lost with its host"), "{reason}")
        }
        other => panic!("expected a lost report, got {other:?}"),
    }
}

/// A run placed with a provider ends through the same Core completion as a
/// local one, so the host's completion metrics count it, and count it once.
#[cfg(feature = "db-integration-tests")]
mod completion_metrics {
    use super::*;
    use runtara_core::persistence::{InstanceCompletionMetrics, InstanceMetricsSink};
    use runtara_store_postgres::PostgresPersistence;

    #[derive(Default)]
    struct Counting(Mutex<Vec<(String, InstanceStatus)>>);

    impl InstanceMetricsSink for Counting {
        fn on_terminal(&self, metrics: &InstanceCompletionMetrics) {
            self.0
                .lock()
                .unwrap()
                .push((metrics.tenant_id.clone(), metrics.status));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_external_runners_completion_is_counted_once() {
        let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
            .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
            .expect("db-integration-tests requires an environment database URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        runtara_environment::migrations::run(&pool).await.unwrap();
        let sink = Arc::new(Counting::default());
        let persistence: Arc<dyn Persistence> =
            Arc::new(PostgresPersistence::new(pool).with_metrics_sink(sink.clone()));
        let placed = Arc::new(Notify::new());
        let s = setup_on(persistence, Behaviour::Complete, None, placed).await;
        let instance = format!("provider-metrics-{}", uuid::Uuid::new_v4());
        register(&s.persistence, &instance).await;
        let handle = s
            .runner
            .try_launch_detached(&options(&instance, s.workflow.clone()))
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            s.runner.wait_for_exit(&handle, Duration::from_millis(20)),
        )
        .await
        .expect("the run ends on its outcome");
        let record = s
            .persistence
            .get_instance(&instance)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.status, InstanceStatus::Completed, "{record:?}");
        assert_eq!(
            *sink.0.lock().unwrap(),
            [("tenant-p".to_string(), InstanceStatus::Completed)],
            "one completion, for the run's tenant"
        );
    }
}
