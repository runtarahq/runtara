// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! EmbeddedWasmRunner integration tests.
//!
//! Components are authored in WAT (no SDK, no HTTP) and persistence is the real
//! database. What the SDK would normally report to runtara-core is pre-seeded so
//! `run()`'s output/error mapping is exercised end to end without a server
//! stack.
//!
//! Feature-gated on `db-integration-tests` via `required-features` in
//! Cargo.toml. These are `multi_thread` and not serialised, so every instance id
//! is minted fresh — the database is shared with the rest of the suite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use runtara_core::persistence::Persistence;
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, Result as RunnerResult, Runner, RunnerError, StartGate,
    StartGateConfirmation, WorkflowRunnerConfig,
};
use runtara_store_postgres::PostgresPersistence;
use uuid::Uuid;

/// An instance id no concurrently-running test can also be using.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/// `wasi:cli/run` returning ok — the embedded analogue of exit code 0.
const RUN_OK_WAT: &str = r#"
    (component
        (core module $m
            (func (export "run") (result i32) (i32.const 0))
        )
        (core instance $i (instantiate $m))
        (func $run (result (result)) (canon lift (core func $i "run")))
        (instance $run_iface (export "run" (func $run)))
        (export "wasi:cli/run@0.2.3" (instance $run_iface))
    )
"#;

/// `wasi:cli/run` spinning forever — only stop()/timeout can end it.
const RUN_SPIN_WAT: &str = r#"
    (component
        (core module $m
            (func (export "run") (result i32)
                (loop $spin (br $spin))
                (i32.const 0))
        )
        (core instance $i (instantiate $m))
        (func $run (result (result)) (canon lift (core func $i "run")))
        (instance $run_iface (export "run" (func $run)))
        (export "wasi:cli/run@0.2.3" (instance $run_iface))
    )
"#;

struct Harness {
    runner: Arc<EmbeddedWasmRunner>,
    persistence: Arc<dyn Persistence>,
    pool: sqlx::PgPool,
    dir: tempfile::TempDir,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect(
            "db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL \
             or RUNTARA_ENVIRONMENT_DATABASE_URL",
        );
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("required environment test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("required combined core/environment migrations must succeed");
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool.clone()));
    let config = WorkflowRunnerConfig {
        data_dir: dir.path().join("data"),
        default_timeout: Duration::from_secs(30),
        skip_cert_verification: false,
        connection_service_url: None,
    };
    let runner = EmbeddedWasmRunner::new(config, Arc::clone(&persistence))
        .expect("embedded runner")
        // The integration binary is not `runtara-server`, so it cannot serve
        // the production hidden precompile-child command. Opt in explicitly
        // to the protocol-compatible test helper instead.
        .with_in_process_precompiler_for_tests();
    Harness {
        runner: Arc::new(runner),
        persistence,
        pool,
        dir,
    }
}

/// Unlike a looping `run`, this never reaches an exported function. Emergency
/// grace must cover instantiation as well as a non-cooperative invocation.
const INITIALIZER_SPIN_WAT: &str = r#"
    (component
        (core module $m
            (func $init (loop $spin (br $spin)))
            (start $init)
            (func (export "run") (result i32) (i32.const 0))
        )
        (core instance $i (instantiate $m))
        (func $run (result (result)) (canon lift (core func $i "run")))
        (instance $run_iface (export "run" (func $run)))
        (export "wasi:cli/run@0.2.3" (instance $run_iface))
    )
"#;

async fn public_stop_aborts_non_cooperative_guest(wat: &str, grace: u64) {
    use runtara_core::domain::InstanceStatus;
    use runtara_environment::container_registry::{ContainerInfo, ContainerRegistry};
    use runtara_environment::handlers::{
        EnvironmentHandlerState, StopInstanceRequest, handle_stop_instance,
    };

    let h = harness().await;
    let inst_id = unique("public-stop-spin");
    let wasm = write_component(h.dir.path(), "spin.wasm", wat);
    seed_detached_instance(&h, &inst_id).await;
    let handle = h
        .runner
        .try_launch_detached(&options(&inst_id, &wasm))
        .await
        .unwrap();
    let registry = ContainerRegistry::new(h.pool.clone());
    registry
        .register(&ContainerInfo {
            container_id: handle.handle_id.clone(),
            launch_id: handle.launch_id.clone(),
            instance_id: inst_id.clone(),
            tenant_id: handle.tenant_id.clone(),
            binary_path: wasm.to_string_lossy().into_owned(),
            started_at: handle.started_at,
            timeout_seconds: Some(30),
        })
        .await
        .unwrap();
    let state = EnvironmentHandlerState::new(
        h.pool.clone(),
        h.persistence.clone(),
        h.runner.clone(),
        h.dir.path().into(),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.runner.is_running(&handle).await);
    assert_eq!(
        h.persistence
            .get_instance_meta(&inst_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Running
    );
    let before = tokio::time::Instant::now();
    let response = handle_stop_instance(
        &state,
        StopInstanceRequest {
            instance_id: inst_id.clone(),
            reason: "clicked Cancel".into(),
            grace_period_seconds: grace,
        },
    )
    .await
    .unwrap();
    assert!(response.success, "{:?}", response.error);
    if grace > 0 {
        assert!(
            h.runner.is_running(&handle).await,
            "Stop must return before grace expires"
        );
        assert_eq!(
            h.persistence
                .get_instance_meta(&inst_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            InstanceStatus::Running
        );
        assert!(registry.get(&inst_id).await.unwrap().is_some());
        // A repeated request cannot keep an uncooperative run alive by
        // extending an already accepted cancellation grace period.
        let response = handle_stop_instance(
            &state,
            StopInstanceRequest {
                instance_id: inst_id.clone(),
                reason: "Cancel again".into(),
                grace_period_seconds: 60,
            },
        )
        .await
        .unwrap();
        assert!(response.success, "{:?}", response.error);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await
    .expect("grace must abort before the 30-second execution timeout");
    assert!(before.elapsed() >= Duration::from_secs(grace));
    assert!(!h.runner.is_running(&handle).await);
    let instance = h.persistence.get_instance(&inst_id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Cancelled);
    assert_eq!(instance.termination_reason.as_deref(), Some("aborted"));
    let command = h
        .persistence
        .get_pending_signal(&inst_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        command.acknowledged_at.is_none(),
        "emergency abort must not claim guest cleanup"
    );
    assert_eq!(h.runner.occupancy().unwrap().held, 0);
    assert!(
        !h.runner
            .schedule_abort(&handle, tokio::time::Instant::now())
            .await
            .unwrap()
    );

    // A fresh invocation in this runner remains healthy after forced teardown.
    let next_id = unique("after-public-stop");
    let next_wasm = write_component(h.dir.path(), "next.wasm", RUN_OK_WAT);
    seed_detached_instance(&h, &next_id).await;
    let next = h
        .runner
        .try_launch_detached(&options(&next_id, &next_wasm))
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&next, Duration::from_millis(10)),
    )
    .await
    .unwrap();
    assert_eq!(h.runner.occupancy().unwrap().held, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_grace_aborts_spinning_invocation() {
    public_stop_aborts_non_cooperative_guest(RUN_SPIN_WAT, 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_grace_aborts_infinite_initializer() {
    public_stop_aborts_non_cooperative_guest(INITIALIZER_SPIN_WAT, 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_zero_grace_aborts_spinning_invocation() {
    public_stop_aborts_non_cooperative_guest(RUN_SPIN_WAT, 0).await;
}

fn write_component(dir: &Path, name: &str, wat: &str) -> PathBuf {
    let path = dir.join(name);
    let bytes = wat::parse_str(wat).expect("compile WAT component");
    std::fs::write(&path, bytes).expect("write component");
    path
}

fn options(instance_id: &str, wasm_path: &Path) -> LaunchOptions {
    LaunchOptions {
        launch_id: format!("launch-{instance_id}"),
        instance_id: instance_id.to_string(),
        tenant_id: "embedded-test".to_string(),
        wasm_path: wasm_path.to_path_buf(),
        requires_lifecycle_invoke: false,
        expected_workflow_checksum: None,
        preparation_attempt: None,
        preparation_deadline: None,
        input: serde_json::Value::Null,
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
        env: HashMap::new(),
        prepersisted_input: None,
        start_gate: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_lease_database_cannot_keep_a_physical_guest_running() {
    use runtara_environment::{
        container_registry::{ContainerInfo, ContainerRegistry},
        execution_lease::ExecutionLease,
        handlers::{DrainController, spawn_container_monitor},
        launch_dispatcher::LaunchLifecycleObservers,
    };
    let h = harness().await;
    let id = unique("lease-database-blocked");
    let wasm = write_component(h.dir.path(), "spin.wasm", RUN_SPIN_WAT);
    seed_detached_instance(&h, &id).await;
    let handle = h
        .runner
        .try_launch_detached(&options(&id, &wasm))
        .await
        .unwrap();
    let registry = ContainerRegistry::new(h.pool.clone());
    registry
        .register(&ContainerInfo {
            container_id: handle.handle_id.clone(),
            launch_id: handle.launch_id.clone(),
            instance_id: id.clone(),
            tenant_id: handle.tenant_id.clone(),
            binary_path: wasm.to_string_lossy().into_owned(),
            started_at: handle.started_at,
            timeout_seconds: Some(30),
        })
        .await
        .unwrap();
    // Exhaust only the monitor's private pool. Runner/Core work retains its
    // independent pool, so this specifically blocks renewal, not guest start.
    let blocked_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*h.pool.connect_options()).clone())
        .await
        .unwrap();
    let held_connection = blocked_pool.acquire().await.unwrap();
    let lease_deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    spawn_container_monitor(
        blocked_pool.clone(),
        h.runner.clone(),
        handle.clone(),
        h.persistence.clone(),
        Duration::from_secs(30),
        DrainController::new(),
        LaunchLifecycleObservers::default(),
        None,
        Some(ExecutionLease::new("blocked-owner", 1, lease_deadline)),
    );
    let exited = tokio::time::timeout(
        Duration::from_secs(3),
        h.runner.wait_for_exit(&handle, Duration::from_millis(10)),
    )
    .await;
    drop(held_connection);
    if exited.is_err() {
        h.runner.stop(&handle).await.unwrap();
    }
    assert!(
        exited.is_ok(),
        "a blocked lease query must not outlive the execution's ownership bound"
    );
    assert!(tokio::time::Instant::now() >= lease_deadline);
    assert_eq!(h.runner.occupancy().unwrap().held, 0);
    assert!(
        h.persistence
            .get_pending_signal(&id)
            .await
            .unwrap()
            .is_none()
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while registry.get(&id).await.unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("normal monitor cleanup must resume once the pool is released");
    blocked_pool.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn retired_handle_cannot_control_a_reused_durable_launch() {
    let h = harness().await;
    let id = unique("reused-launch");
    let wasm = write_component(h.dir.path(), "spin.wasm", RUN_SPIN_WAT);
    seed_detached_instance(&h, &id).await;
    let options = options(&id, &wasm);
    let old = h.runner.try_launch_detached(&options).await.unwrap();
    h.runner.stop(&old).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&old, Duration::from_millis(10)),
    )
    .await
    .expect("old physical execution must exit");

    let current = h.runner.try_launch_detached(&options).await.unwrap();
    // Exercise each old-handle operation before asserting, so failure still
    // tears down the controlled spinning guest below.
    let old_looks_live = h.runner.is_running(&old).await;
    let old_armed = h
        .runner
        .schedule_abort(&old, tokio::time::Instant::now())
        .await
        .unwrap();
    h.runner.stop(&old).await.unwrap();
    let old_wait = tokio::time::timeout(
        Duration::from_millis(250),
        h.runner.wait_for_exit(&old, Duration::from_millis(10)),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let current_live = h.runner.is_running(&current).await;
    h.runner.stop(&current).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&current, Duration::from_millis(10)),
    )
    .await
    .expect("current physical execution must exit");
    assert_eq!(old.launch_id, current.launch_id);
    assert_eq!(
        (old_looks_live, old_armed, old_wait.is_ok(), current_live),
        (false, false, true, true),
        "old handle must be retired without observing, waiting for, or aborting its replacement"
    );
    assert_ne!(old.handle_id, current.handle_id);
    assert_eq!(h.runner.occupancy().unwrap().held, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn overlapping_handoffs_keep_separate_physical_handles_and_occupancy() {
    let h = harness().await;
    let id = unique("overlapping-handoff");
    let wasm = write_component(h.dir.path(), "ok.wasm", RUN_OK_WAT);
    seed_detached_instance(&h, &id).await;
    let mut first_options = options(&id, &wasm);
    let first_gate = StartGate::new(Duration::from_secs(30));
    first_options.start_gate = Some(first_gate.clone());
    let first = h.runner.try_launch_detached(&first_options).await.unwrap();
    let mut next_options = first_options.clone();
    let next_gate = StartGate::new(Duration::from_secs(30));
    next_options.start_gate = Some(next_gate.clone());
    let next = h.runner.try_launch_detached(&next_options).await.unwrap();
    let both_held = h.runner.occupancy().unwrap().held;

    // Model a cancelled old handoff unwinding after a replacement has already
    // installed its closed gate. Neither fixture executes guest work.
    h.runner.stop(&first).await.unwrap();
    first_gate.open();
    let first_exited = tokio::time::timeout(
        Duration::from_secs(1),
        h.runner.wait_for_exit(&first, Duration::from_millis(10)),
    )
    .await;
    let next_live = h.runner.is_running(&next).await;
    let occupancy = h.runner.occupancy().unwrap();
    next_gate.cancel();
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&next, Duration::from_millis(10)),
    )
    .await
    .expect("replacement closed gate must retire");

    assert_eq!(first.launch_id, next.launch_id);
    assert_ne!(first.handle_id, next.handle_id);
    assert_eq!(both_held, 2);
    assert!(
        first_exited.is_ok(),
        "old wait must not follow the replacement"
    );
    assert!(next_live);
    assert_eq!(occupancy.held, 1);
    assert_eq!(occupancy.oldest_instance_id.as_deref(), Some(id.as_str()));
    assert!(occupancy.oldest_held_ms.is_some());
    let finished = h.runner.occupancy().unwrap();
    assert_eq!(finished.held, 0);
    assert_eq!(finished.oldest_held_ms, None);
}

/// Durable preparation must read the same canonical input envelope that a
/// production launch persists before it reaches the runner.
async fn seed_detached_instance(harness: &Harness, instance_id: &str) {
    const INPUT: &[u8] = br#"{\"data\":{},\"variables\":{}}"#;

    assert!(
        harness
            .persistence
            .try_register_instance(instance_id, "embedded-test", Some(INPUT))
            .await
            .expect("register detached instance"),
        "the freshly generated test instance id must be claimed"
    );
}

/// A durable confirmation held by the test to prove guest preparation cannot
/// cross a merely opened in-memory start gate.
struct BlockingGateConfirmation {
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl StartGateConfirmation for BlockingGateConfirmation {
    async fn confirm(&self) -> RunnerResult<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn try_launch_detached_completes_and_clears_registry() {
    let h = harness().await;
    let inst_id = unique("inst-detached");
    let wasm = write_component(h.dir.path(), "ok.wasm", RUN_OK_WAT);
    seed_detached_instance(&h, inst_id.as_str()).await;

    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &wasm))
        .await
        .expect("launch");

    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("wait_for_exit hung");

    assert!(!h.runner.is_running(&handle).await);
    let (_output, _stderr, metrics) = h.runner.collect_result(&handle).await;
    assert!(metrics.memory_peak_bytes.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_cancels_spinning_instance_without_faking_cleanup() {
    let h = harness().await;
    let inst_id = unique("inst-spin");
    let wasm = write_component(h.dir.path(), "spin.wasm", RUN_SPIN_WAT);
    seed_detached_instance(&h, inst_id.as_str()).await;

    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &wasm))
        .await
        .expect("launch");

    // Give the run a moment to actually enter the guest loop.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        h.runner.is_running(&handle).await,
        "guest should be spinning"
    );

    // The lifecycle request remains pending while the uncooperative guest is
    // aborted via the runner's existing whole-execution stop mechanism.
    h.persistence
        .insert_signal(
            &inst_id,
            runtara_core::domain::SignalType::Cancel,
            b"request",
        )
        .await
        .unwrap();
    let command = h
        .persistence
        .get_pending_signal(&inst_id)
        .await
        .unwrap()
        .unwrap();
    h.runner.stop(&handle).await.expect("stop");
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("cancel did not end the spinning guest");
    assert!(!h.runner.is_running(&handle).await);
    let instance = h.persistence.get_instance(&inst_id).await.unwrap().unwrap();
    assert_eq!(
        instance.status,
        runtara_core::domain::InstanceStatus::Cancelled
    );
    assert_eq!(instance.termination_reason.as_deref(), Some("aborted"));
    let pending = h
        .persistence
        .get_pending_signal(&inst_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.command_id, command.command_id);
    assert!(pending.acknowledged_at.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_gate_allows_preparation_but_blocks_guest_instantiation() {
    let h = harness().await;
    let inst_id = unique("inst-gated");
    // Preparation is intentionally allowed before the gate. A durable
    // dispatcher has already compiled and linked this component before it
    // reserves a scarce guest run permit; only guest instantiation remains
    // protected by the start gate.
    let wasm = write_component(h.dir.path(), "gated-spin.wasm", RUN_SPIN_WAT);
    seed_detached_instance(&h, inst_id.as_str()).await;
    let confirmation_release = Arc::new(tokio::sync::Notify::new());
    let confirmation_calls = Arc::new(AtomicUsize::new(0));
    let gate = StartGate::new(Duration::from_secs(5)).with_confirmation(Arc::new(
        BlockingGateConfirmation {
            release: Arc::clone(&confirmation_release),
            calls: Arc::clone(&confirmation_calls),
        },
    ));
    let mut launch = options(inst_id.as_str(), &wasm);
    launch.start_gate = Some(gate.clone());

    let handle = h.runner.try_launch_detached(&launch).await.expect("launch");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        h.runner.is_running(&handle).await,
        "the closed gate must keep the prepared task alive without instantiating a guest"
    );
    assert_eq!(
        confirmation_calls.load(Ordering::SeqCst),
        0,
        "preparation must not clear the durable marker or instantiate a guest before gate open"
    );

    assert!(gate.open());
    tokio::time::timeout(Duration::from_secs(1), async {
        while confirmation_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("runner must reach the exact pre-instantiation confirmation boundary");
    assert!(
        h.runner.is_running(&handle).await,
        "a supervisor-opened gate must still wait for runner-owned durable confirmation before guest execution"
    );

    confirmation_release.notify_one();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        h.runner.is_running(&handle).await,
        "the spinning guest must not complete before the test can stop it"
    );
    h.runner.stop(&handle).await.expect("stop spinning guest");
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("stopped gated guest must exit");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_component_fails_during_preparation() {
    let h = harness().await;
    let inst_id = unique("inst-invalid-preparation");
    let invalid = h.dir.path().join("invalid.wasm");
    std::fs::write(&invalid, b"not a wasm component").expect("write invalid artifact");

    let error = h
        .runner
        .try_prepare_launch(&options(inst_id.as_str(), &invalid))
        .await
        .expect_err("invalid artifact must fail before a run permit or gate handoff");
    assert!(
        matches!(error, RunnerError::StartFailed(_)),
        "invalid preparation must be reported as a pre-start failure: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_component_is_binary_not_found() {
    let h = harness().await;
    let inst_id = unique("inst-missing");
    let missing = h.dir.path().join("nope.wasm");
    let err = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &missing))
        .await
        .expect_err("must fail");
    assert!(matches!(err, RunnerError::BinaryNotFound(_)));
}
