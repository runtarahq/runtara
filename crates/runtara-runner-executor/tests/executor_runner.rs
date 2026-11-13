// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The whole path of a run on an executor, with nothing faked but the
//! database: runtara's provider runner places the probe workflow with a real
//! executor over mutual TLS, a real `runtara-run` (sandboxed on Linux) runs
//! it, and its workflow API calls come back over the run channel to the
//! launch's real router.
//!
//! Needs the probe component (`scripts/build-agent-components.sh`) and the
//! runner: `cargo build -p runtara-run`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runtara_artifact_store::{ArtifactStore, FilesystemArtifactStore};
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::Persistence;
use runtara_core::persistence::memory::InMemoryPersistence;
use runtara_environment::runner::{
    HostServices, LaunchOptions, Provider, ProviderEnd, Runner, RunnerParts,
};
use runtara_executor::tls::Identity;
use serde_json::{Value, json};

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

fn run_binary() -> PathBuf {
    let path = std::env::var_os("RUNTARA_EXECUTOR_RUN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
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

struct Deployment {
    runner: Arc<dyn Runner>,
    persistence: Arc<dyn Persistence>,
    workflow: runtara_artifact_store::Digest,
    executor_url: String,
    executor_runs: Arc<runtara_executor::runs::Runs>,
    server_tls: PathBuf,
    _dir: tempfile::TempDir,
}

async fn deployment() -> Deployment {
    let dir = tempfile::tempdir().unwrap();
    let server_tls = dir.path().join("server-tls");
    let executor_tls = dir.path().join("executor-tls");
    runtara_executor::pki::init(&server_tls, &executor_tls, false).unwrap();

    let executor = runtara_executor::Executor::bind(&runtara_executor::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls_dir: executor_tls,
        state_dir: dir.path().join("executor-state"),
        max_runs: 4,
        run_binary: run_binary(),
        no_landlock: std::env::var_os("RUNTARA_RUN_TEST_NO_LANDLOCK").is_some(),
        unsandboxed: !cfg!(target_os = "linux"),
    })
    .await
    .unwrap();
    let executor_url = format!("https://{}", executor.address());
    let executor_runs = executor.runs();
    tokio::spawn(executor.serve());

    // A port for the run channel: taken, read and given back.
    let run_channel = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let factory = runtara_runner_executor::prepare(runtara_runner_executor::Config {
        executor_url: executor_url.clone(),
        tls_dir: server_tls.clone(),
        run_channel_listen: run_channel,
        run_channel_endpoint: format!("https://{run_channel}"),
        runner_limit: 2,
    })
    .await
    .unwrap();

    let persistence: Arc<dyn Persistence> = Arc::new(InMemoryPersistence::new());
    let artifacts = Arc::new(FilesystemArtifactStore::new(dir.path().join("artifacts")));
    let workflow = artifacts.put(&probe()).await.unwrap();
    let runner = factory(RunnerParts {
        persistence: persistence.clone(),
        event_observer: None,
        services: HostServices::default(),
        artifacts,
    })
    .unwrap();
    Deployment {
        runner,
        persistence,
        workflow,
        executor_url,
        executor_runs,
        server_tls,
        _dir: dir,
    }
}

impl Deployment {
    /// Launches the probe in `mode` as `instance` and waits for it to end.
    async fn run(&self, instance: &str, mode: &str, timeout: Duration) {
        let input = json!({"data": {"mode": mode, "from": "executor-test"}, "variables": {}});
        assert!(
            self.persistence
                .try_register_instance(
                    instance,
                    "tenant-x",
                    Some(&serde_json::to_vec(&input).unwrap())
                )
                .await
                .unwrap()
        );
        let handle = self
            .runner
            .try_launch_detached(&LaunchOptions {
                launch_id: format!("launch-{instance}"),
                instance_id: instance.into(),
                tenant_id: "tenant-x".into(),
                workflow_digest: self.workflow.clone(),
                preparation_attempt: None,
                preparation_deadline: None,
                input: Value::Null,
                timeout,
                checkpoint_id: None,
                env: HashMap::new(),
                prepersisted_input: None,
                launch_kind: runtara_environment::launch_queue::LaunchKind::Start,
                start_gate: None,
                memory_limit_bytes: Some(64 * 1024 * 1024),
            })
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(120),
            self.runner
                .wait_for_exit(&handle, Duration::from_millis(50)),
        )
        .await
        .expect("the run ends");
    }

    async fn status(&self, instance: &str) -> (InstanceStatus, Option<Value>) {
        let record = self
            .persistence
            .get_instance(instance)
            .await
            .unwrap()
            .unwrap();
        let output = record
            .output
            .as_deref()
            .and_then(|output| serde_json::from_slice(output).ok());
        (record.status, output)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_runs_on_the_executor_and_completes() {
    let deployment = deployment().await;
    assert_eq!(deployment.runner.runner_type(), "executor");
    // The server composes the stdlib into what it compiles for this runner.
    assert!(!deployment.runner.links_workflow_stdlib());
    deployment
        .run("inst-1", "complete", Duration::from_secs(60))
        .await;
    let (status, output) = deployment.status("inst-1").await;
    assert_eq!(status, InstanceStatus::Completed, "{output:?}");
    let output = output.unwrap();
    assert_eq!(output["instanceId"], "inst-1");
    assert_eq!(output["tenantId"], "tenant-x");
    assert_eq!(output["echo"]["from"], "executor-test");

    // Staged once: a second run of the same workflow places straight away.
    deployment
        .run("inst-2", "complete", Duration::from_secs(60))
        .await;
    assert_eq!(
        deployment.status("inst-2").await.0,
        InstanceStatus::Completed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_that_traps_fails_its_instance() {
    let deployment = deployment().await;
    deployment
        .run("inst-trap", "trap", Duration::from_secs(60))
        .await;
    let (status, _) = deployment.status("inst-trap").await;
    assert_eq!(status, InstanceStatus::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_that_never_ends_is_stopped_at_its_timeout() {
    let deployment = deployment().await;
    // `run` returns only once the runner says the launch ended.
    deployment
        .run("inst-spin", "spin", Duration::from_secs(2))
        .await;
    // Its process is gone from the executor. (What the instance becomes after
    // a timeout is the environment's to record, as for a local run.)
    tokio::time::timeout(Duration::from_secs(20), async {
        while deployment.executor_runs.running() != 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the executor ended the run's process");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_its_executor_does_not_hold_is_lost() {
    let deployment = deployment().await;
    let provider = runtara_runner_executor::ExecutorProvider::new(
        &Identity::load(&deployment.server_tls).unwrap(),
        deployment.executor_url.clone(),
    )
    .unwrap();
    match provider
        .ended("no-such-run", Duration::from_millis(50))
        .await
    {
        Some(ProviderEnd::Lost(_)) => {}
        other => panic!("expected a lost run, got {other:?}"),
    }
}
