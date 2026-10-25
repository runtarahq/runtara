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

/// `invoke` body returning `Ok(completed("{\"ok\":1}"))`.
const COMPLETED: &str = r#"(i32.store8 (i32.const 2048) (i32.const 0))
      (i32.store8 (i32.const 2056) (i32.const 0))
      (i32.store (i32.const 2060) (i32.const 1088)) (i32.store (i32.const 2064) (i32.const 8))
      (i32.const 2048)"#;

/// A workflow entry that completes — the embedded analogue of exit code 0.
fn run_ok() -> String {
    entry_wat(COMPLETED)
}

/// A workflow entry whose `invoke` spins forever — only stop()/timeout can
/// end it.
fn run_spin() -> String {
    entry_wat("(loop $spin (br $spin)) (i32.const 2048)")
}

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

/// Unlike a looping `invoke`, this never reaches an exported function.
/// Emergency grace must cover instantiation as well as a non-cooperative
/// invocation.
fn initializer_spin() -> String {
    entry_core_wat(
        "(func $init (loop $spin (br $spin))) (start $init)",
        COMPLETED,
    )
}

async fn public_stop_aborts_non_cooperative_guest(wat: &str, grace: u64, peer: bool) {
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
    let peer_delivery = if peer {
        // Seed the existing running launch claim. The fixture's peer shares
        // persistence but has no access to the owner's native task registry.
        let image_id = unique("remote-abort-image");
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1, $2, $1, $3)",
        )
        .bind(&image_id)
        .bind(&handle.tenant_id)
        .bind(wasm.to_string_lossy().as_ref())
        .execute(&h.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO instance_launches (launch_id, instance_id, tenant_id, image_id, kind, state, deadline_at, lease_owner, lease_expires_at, attempt_count) VALUES ($1, $2, $3, $4, 'start', 'running', NOW() + INTERVAL '60 seconds', 'spinning-owner', NOW() + INTERVAL '60 seconds', 1)")
            .bind(&handle.launch_id).bind(&inst_id).bind(&handle.tenant_id).bind(image_id)
            .execute(&h.pool).await.unwrap();
        let pool = h.pool.clone();
        let runner = h.runner.clone();
        let physical = handle.clone();
        Some(tokio::spawn(async move {
            while runner.is_running(&physical).await {
                tokio::time::sleep(Duration::from_millis(250)).await;
                ContainerRegistry::new(pool.clone())
                    .deliver_abort_requests("spinning-owner", runner.as_ref(), 32)
                    .await
                    .unwrap();
            }
        }))
    } else {
        None
    };
    let control_runner: Arc<dyn Runner> = if peer {
        Arc::new(runtara_environment::runner::MockRunner::never_completing())
    } else {
        h.runner.clone()
    };
    let state = EnvironmentHandlerState::new(
        h.pool.clone(),
        h.persistence.clone(),
        control_runner,
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
    let clock_margin = if peer {
        Duration::from_millis(50)
    } else {
        Duration::ZERO
    };
    assert!(before.elapsed() + clock_margin >= Duration::from_secs(grace));
    if let Some(delivery) = peer_delivery {
        tokio::time::timeout(Duration::from_secs(2), delivery)
            .await
            .unwrap()
            .unwrap();
    }
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
    let next_wasm = write_component(h.dir.path(), "next.wasm", &run_ok());
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
    public_stop_aborts_non_cooperative_guest(&run_spin(), 1, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_grace_aborts_infinite_initializer() {
    public_stop_aborts_non_cooperative_guest(&initializer_spin(), 1, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_stop_zero_grace_aborts_spinning_invocation() {
    public_stop_aborts_non_cooperative_guest(&run_spin(), 0, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_stop_grace_aborts_spinning_invocation() {
    public_stop_aborts_non_cooperative_guest(&run_spin(), 1, true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_stop_zero_grace_aborts_infinite_initializer() {
    public_stop_aborts_non_cooperative_guest(&initializer_spin(), 0, true).await;
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
        expected_workflow_checksum: None,
        preparation_attempt: None,
        preparation_deadline: None,
        input: serde_json::Value::Null,
        timeout: Duration::from_secs(30),
        checkpoint_id: None,
        env: HashMap::new(),
        prepersisted_input: None,
        launch_kind: runtara_environment::launch_queue::LaunchKind::Start,
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
    let wasm = write_component(h.dir.path(), "spin.wasm", &run_spin());
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
    let wasm = write_component(h.dir.path(), "spin.wasm", &run_spin());
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
    let wasm = write_component(h.dir.path(), "ok.wasm", &run_ok());
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
    let finished = h.runner.occupancy().unwrap();
    assert_eq!(finished.held, 0);
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
    let wasm = write_component(h.dir.path(), "ok.wasm", &run_ok());
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
    let wasm = write_component(h.dir.path(), "spin.wasm", &run_spin());
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
    let wasm = write_component(h.dir.path(), "gated-spin.wasm", &run_spin());
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

/// A workflow entry whose `invoke` returns `result` (a WAT body that leaves
/// the result area's address on the stack), with no runtime import: the
/// return value is the run's only terminal channel.
fn entry_wat(result: &str) -> String {
    entry_core_wat("", result)
}

/// [`entry_wat`] with `core_items` added to its core module.
fn entry_core_wat(core_items: &str, result: &str) -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (data (i32.const 1024) "BOOM")
    (data (i32.const 1040) "it failed")
    (data (i32.const 1056) "{{\"stepId\":\"s\"}}")
    (data (i32.const 1088) "{{\"ok\":1}}")
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 4096)
    {core_items}
    (func (export "invoke") (param i32 i32 i32 i32) (result i32)
      {result}))
  (core instance $i (instantiate $m))
  (type $error (record (field "code" string) (field "message" string)
    (field "category" string) (field "severity" string) (field "retryable" bool)
    (field "retry-after-ms" (option u64)) (field "attributes" (option string))
    (field "details" (option string))))
  (type $signal (record (field "checkpoint-id" string) (field "deadline-ms" (option u64))))
  (type $wake (variant (case "at" u64) (case "on-signal" $signal) (case "on-resume")
    (case "instances" string)))
  (type $suspension (record (field "wakes" (list $wake)) (field "state" (list u8))))
  (type $outcome (variant (case "completed" (list u8)) (case "suspended" $suspension)))
  (func $invoke async (param "capability-id" string) (param "input" (list u8))
    (result (result $outcome (error $error)))
    (canon lift (core func $i "invoke") (memory $i "memory") (realloc (func $i "realloc"))))
  (instance $entry (export "error-info" (type $error)) (export "signal-wait" (type $signal))
    (export "wake" (type $wake)) (export "suspension" (type $suspension))
    (export "outcome" (type $outcome)) (export "invoke" (func $invoke)))
  (export "runtara:agent-workflow-agent/capabilities@1.0.0" (instance $entry)))"#
    )
}

/// The runner persists a run's terminal result once, from the value its
/// entry returned. A failure records `details` verbatim: before, a guest
/// that returned an error without calling `fail` was later recorded as
/// crashed with no error.
#[tokio::test(flavor = "multi_thread")]
async fn the_returned_result_is_the_persisted_terminal() {
    let h = harness().await;
    let failed = r#"(i32.store8 (i32.const 2048) (i32.const 1))
      (i32.store (i32.const 2056) (i32.const 1024)) (i32.store (i32.const 2060) (i32.const 4))
      (i32.store (i32.const 2064) (i32.const 1040)) (i32.store (i32.const 2068) (i32.const 9))
      (i32.store8 (i32.const 2124) (i32.const 1))
      (i32.store (i32.const 2128) (i32.const 1056)) (i32.store (i32.const 2132) (i32.const 14))
      (i32.const 2048)"#;
    let completed = r#"(i32.store8 (i32.const 2048) (i32.const 0))
      (i32.store8 (i32.const 2056) (i32.const 0))
      (i32.store (i32.const 2060) (i32.const 1088)) (i32.store (i32.const 2064) (i32.const 8))
      (i32.const 2048)"#;
    for (name, body) in [("failed", failed), ("completed", completed)] {
        let inst_id = unique(&format!("inst-terminal-{name}"));
        let wasm = write_component(h.dir.path(), &format!("{name}.wasm"), &entry_wat(body));
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
        let instance = h
            .persistence
            .get_instance(inst_id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert!(instance.termination_reason.is_none(), "{instance:?}");
        if name == "failed" {
            assert_eq!(
                instance.status,
                runtara_core::domain::InstanceStatus::Failed
            );
            assert_eq!(instance.error.as_deref(), Some(r#"{"stepId":"s"}"#));
        } else {
            assert_eq!(
                instance.status,
                runtara_core::domain::InstanceStatus::Completed
            );
            assert_eq!(instance.output.as_deref(), Some(br#"{"ok":1}"#.as_slice()));
        }
    }
}

async fn root_lease_row(pool: &sqlx::PgPool, instance_id: &str) -> Option<(String, i64, bool)> {
    sqlx::query_as("SELECT owner, epoch, active FROM invocation_root_leases WHERE instance_id = $1")
        .bind(instance_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// An ordinary (non-scoped) root run owns its root execution lease while
/// it runs, under its own runner registration, and holds none once it ends.
#[tokio::test(flavor = "multi_thread")]
async fn an_ordinary_root_run_owns_its_root_lease_only_while_running() {
    let h = harness().await;
    let inst_id = unique("inst-root-lease");
    let wasm = write_component(h.dir.path(), "lease-spin.wasm", &run_spin());
    seed_detached_instance(&h, inst_id.as_str()).await;

    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &wasm))
        .await
        .expect("launch");
    tokio::time::timeout(Duration::from_secs(5), async {
        while root_lease_row(&h.pool, &inst_id).await.is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the run must claim its root lease");
    assert_eq!(
        root_lease_row(&h.pool, &inst_id).await,
        Some((handle.handle_id.clone(), 1, true)),
        "the running root must hold an active lease under its own registration"
    );

    h.runner.stop(&handle).await.expect("stop");
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("stopped guest must exit");
    let (_, _, active) = root_lease_row(&h.pool, &inst_id).await.unwrap();
    assert!(!active, "an ended run must not keep its root lease");
}

/// A run promoted by the durable supervisor adopts the lease bound into its
/// start gate instead of claiming another epoch.
#[tokio::test(flavor = "multi_thread")]
async fn a_supervised_run_adopts_the_lease_bound_into_its_gate() {
    use runtara_core::persistence::invocations::InvocationLease;
    let h = harness().await;
    let inst_id = unique("inst-adopted-lease");
    let wasm = write_component(h.dir.path(), "adopted-spin.wasm", &run_spin());
    seed_detached_instance(&h, inst_id.as_str()).await;
    // Stand in for the supervisor's promotion transaction.
    h.persistence
        .update_instance_status(
            &inst_id,
            runtara_core::domain::InstanceStatus::Running,
            None,
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO invocation_root_leases (instance_id, owner, epoch, active) VALUES ($1, 'supervised', 4, TRUE)",
    )
    .bind(&inst_id)
    .execute(&h.pool)
    .await
    .unwrap();
    let gate = StartGate::new(Duration::from_secs(5));
    assert!(gate.bind_root_lease(InvocationLease {
        tenant_id: "embedded-test".into(),
        instance_id: inst_id.clone(),
        owner: "supervised".into(),
        epoch: 4,
    }));
    let mut launch = options(inst_id.as_str(), &wasm);
    launch.start_gate = Some(gate.clone());
    let handle = h.runner.try_launch_detached(&launch).await.expect("launch");
    assert!(gate.open());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.runner.is_running(&handle).await);
    assert_eq!(
        root_lease_row(&h.pool, &inst_id).await,
        Some(("supervised".into(), 4, true)),
        "the run must adopt the supervisor's lease, not claim a new epoch"
    );
    h.runner.stop(&handle).await.expect("stop");
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("stopped guest must exit");
    let (_, epoch, active) = root_lease_row(&h.pool, &inst_id).await.unwrap();
    assert_eq!(epoch, 4);
    assert!(!active, "the run releases the adopted lease when it ends");
}

/// The probe workflow on the workflow API ABI (`wasi:cli/run` over
/// `wasi:http@0.3`), built by `scripts/build-agent-components.sh`.
fn workflow_api_probe() -> PathBuf {
    std::env::var_os("RUNTARA_WORKFLOW_API_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release/runtara_workflow_api_probe.wasm")
        })
}

/// A run on the workflow API ABI reads its input from the instance and its
/// identity from its root lease over the API, and the outcome it posts is the
/// persisted terminal.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_api_run_persists_the_outcome_it_posted() {
    let h = harness().await;
    let probe = workflow_api_probe();
    assert!(
        probe.exists(),
        "missing {}: run scripts/build-agent-components.sh",
        probe.display()
    );
    for mode in ["complete", "fail"] {
        let inst_id = unique(&format!("inst-workflow-api-{mode}"));
        let input = serde_json::to_vec(&serde_json::json!({
            "data": {"mode": mode, "greeting": "hello"},
            "variables": {}
        }))
        .unwrap();
        assert!(
            h.persistence
                .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
                .await
                .expect("register instance")
        );
        let handle = h
            .runner
            .try_launch_detached(&options(inst_id.as_str(), &probe))
            .await
            .expect("launch");
        tokio::time::timeout(
            Duration::from_secs(30),
            h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
        )
        .await
        .expect("wait_for_exit hung");
        let instance = h
            .persistence
            .get_instance(inst_id.as_str())
            .await
            .unwrap()
            .unwrap();
        if mode == "complete" {
            assert_eq!(
                instance.status,
                runtara_core::domain::InstanceStatus::Completed,
                "{instance:?}"
            );
            let output: serde_json::Value =
                serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
            assert_eq!(output["instanceId"], inst_id.as_str());
            assert_eq!(output["tenantId"], "embedded-test");
            assert_eq!(output["echo"]["greeting"], "hello");
        } else {
            assert_eq!(
                instance.status,
                runtara_core::domain::InstanceStatus::Failed,
                "{instance:?}"
            );
            let error = instance.error.as_deref().unwrap_or_default();
            assert!(error.contains(inst_id.as_str()), "{error}");
        }
    }
}

/// A run that never posts an outcome, because it traps or runs out its
/// timeout, stops being served by the workflow API all the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_api_run_that_never_posts_is_released() {
    let h = harness().await;
    let probe = workflow_api_probe();
    for mode in ["trap", "spin"] {
        let inst_id = unique(&format!("inst-workflow-api-{mode}"));
        let input =
            serde_json::to_vec(&serde_json::json!({"data": {"mode": mode}, "variables": {}}))
                .unwrap();
        assert!(
            h.persistence
                .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
                .await
                .expect("register instance")
        );
        let mut launch = options(inst_id.as_str(), &probe);
        launch.timeout = Duration::from_secs(2);
        let handle = h.runner.try_launch_detached(&launch).await.expect("launch");
        tokio::time::timeout(
            Duration::from_secs(30),
            h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
        )
        .await
        .expect("wait_for_exit hung");
        let instance = h
            .persistence
            .get_instance(inst_id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            instance.status,
            runtara_core::domain::InstanceStatus::Completed,
            "{mode}: {instance:?}"
        );
        assert_eq!(h.runner.workflow_api_launches().serving(), 0, "{mode}");
    }
}

/// Run services over the workflow API go through the run's own runtime host:
/// the checkpoint and the event the probe wrote are in persistence.
#[tokio::test(flavor = "multi_thread")]
async fn workflow_api_run_services_reach_persistence() {
    let h = harness().await;
    let probe = workflow_api_probe();
    let inst_id = unique("inst-workflow-api-durable");
    let input =
        serde_json::to_vec(&serde_json::json!({"data": {"mode": "durable"}, "variables": {}}))
            .unwrap();
    assert!(
        h.persistence
            .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
            .await
            .expect("register instance")
    );
    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &probe))
        .await
        .expect("launch");
    tokio::time::timeout(
        Duration::from_secs(30),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("wait_for_exit hung");
    let instance = h
        .persistence
        .get_instance(inst_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        instance.status,
        runtara_core::domain::InstanceStatus::Completed,
        "{instance:?}"
    );
    let output: serde_json::Value =
        serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["firstStored"], true);
    assert_eq!(output["secondStored"], false);
    assert_eq!(output["secondState"], "first");
    assert_eq!(output["read"], "first");
    assert_eq!(output["missing"], true);
    assert!(output["nowMs"].as_u64().unwrap() > 1_700_000_000_000);

    let checkpoint = h
        .persistence
        .load_checkpoint(inst_id.as_str(), "probe/step:1")
        .await
        .unwrap()
        .expect("the probe's checkpoint is persisted");
    assert_eq!(checkpoint.state, b"first");

    let events = h
        .persistence
        .list_events(
            inst_id.as_str(),
            &runtara_core::persistence::ListEventsFilter {
                subtype: Some("probe".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .await
        .unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].payload.as_deref(),
        Some(br#"{"step":1}"#.as_slice())
    );
}

/// Published state over the workflow API goes through the runner's state
/// service: what the probe set is what persistence holds, and a malformed
/// step key is refused.
#[tokio::test(flavor = "multi_thread")]
async fn workflow_api_published_state_reaches_persistence() {
    let h = harness().await;
    let probe = workflow_api_probe();
    let inst_id = unique("inst-workflow-api-state");
    let input =
        serde_json::to_vec(&serde_json::json!({"data": {"mode": "state"}, "variables": {}}))
            .unwrap();
    assert!(
        h.persistence
            .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
            .await
            .expect("register instance")
    );
    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &probe))
        .await
        .expect("launch");
    tokio::time::timeout(
        Duration::from_secs(30),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("wait_for_exit hung");
    let instance = h
        .persistence
        .get_instance(inst_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        instance.status,
        runtara_core::domain::InstanceStatus::Completed,
        "{instance:?}"
    );
    let output: serde_json::Value =
        serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["state"], serde_json::json!({"stage": "probed"}));
    assert_eq!(output["refused"], "invalid");

    let stored = h
        .persistence
        .run_state()
        .expect("postgres persistence keeps run state")
        .get_state("embedded-test", inst_id.as_str())
        .await
        .unwrap()
        .expect("the probe published state");
    assert_eq!(
        stored.state.get("stage"),
        Some(&serde_json::json!("probed")),
        "{stored:?}"
    );
}

/// Managed inputs over the workflow API go through the run's runtime host: a
/// registered input reads open, a replayed registration finds it again, and
/// closing it reports it closed.
#[tokio::test(flavor = "multi_thread")]
async fn workflow_api_managed_inputs_open_and_close() {
    let h = harness().await;
    let probe = workflow_api_probe();
    let inst_id = unique("inst-workflow-api-input");
    let input =
        serde_json::to_vec(&serde_json::json!({"data": {"mode": "input"}, "variables": {}}))
            .unwrap();
    assert!(
        h.persistence
            .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
            .await
            .expect("register instance")
    );
    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &probe))
        .await
        .expect("launch");
    tokio::time::timeout(
        Duration::from_secs(30),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("wait_for_exit hung");
    let instance = h
        .persistence
        .get_instance(inst_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        instance.status,
        runtara_core::domain::InstanceStatus::Completed,
        "{instance:?}"
    );
    let output: serde_json::Value =
        serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["open"], serde_json::json!({"state": "open"}));
    assert_eq!(output["closed"]["state"], "closed", "{output}");
}

/// `PUT /v1/run/deadline` brings the run's deadline forward: a probe that
/// sets 200ms and then spins ends long before its 30s launch timeout.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_api_run_can_shorten_its_deadline() {
    let h = harness().await;
    let probe = workflow_api_probe();
    let inst_id = unique("inst-workflow-api-deadline");
    let input =
        serde_json::to_vec(&serde_json::json!({"data": {"mode": "deadline"}, "variables": {}}))
            .unwrap();
    assert!(
        h.persistence
            .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
            .await
            .expect("register instance")
    );
    let started = std::time::Instant::now();
    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &probe))
        .await
        .expect("launch");
    tokio::time::timeout(
        Duration::from_secs(30),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("wait_for_exit hung");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "ended after {:?}",
        started.elapsed()
    );
    let instance = h
        .persistence
        .get_instance(inst_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        instance.status,
        runtara_core::domain::InstanceStatus::Completed,
        "{instance:?}"
    );
    assert_eq!(h.runner.workflow_api_launches().serving(), 0);
}

mod workflow_api_connections {
    use super::*;
    use runtara_component_host::outbound_http::{
        Destination, OutboundContext, OutboundError, RequestOptions, Response,
    };
    use runtara_database_contract::{
        BatchRequest, BatchResult, DatabaseError, ExecutionResult, QueryRequest, RowSet, Statement,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct Seen {
        http: Mutex<Vec<(String, RequestOptions)>>,
        sql: Mutex<Vec<(String, String, String)>>,
        resolved: Mutex<Vec<(String, String)>>,
    }

    struct Http(Arc<Seen>);

    #[async_trait::async_trait]
    impl runtara_component_host::OutboundHttpHost for Http {
        async fn request(
            &self,
            context: &OutboundContext,
            request: RequestOptions,
        ) -> Result<Response, OutboundError> {
            let public = matches!(request.destination, Destination::Public(_));
            self.0
                .http
                .lock()
                .unwrap()
                .push((context.tenant_id.clone(), request));
            if public {
                Err(OutboundError {
                    code: "HTTP_DESTINATION_DENIED".into(),
                    message: "blocked".into(),
                    status: Some(403),
                    body: b"nope".to_vec(),
                    retry_after_ms: None,
                })
            } else {
                Ok(Response {
                    status: 201,
                    headers: vec![("x-upstream".into(), "1".into())],
                    body: b"pong".to_vec(),
                })
            }
        }
    }

    struct Database(Arc<Seen>);

    #[async_trait::async_trait]
    impl runtara_component_host::DatabaseHost for Database {
        async fn query(
            &self,
            tenant: &str,
            connection_id: &str,
            request: QueryRequest,
        ) -> Result<RowSet, DatabaseError> {
            self.0.sql.lock().unwrap().push((
                tenant.to_string(),
                connection_id.to_string(),
                request.sql,
            ));
            Ok(RowSet {
                columns: Vec::new(),
                rows: Vec::new(),
            })
        }
        async fn execute(
            &self,
            _: &str,
            _: &str,
            _: Statement,
        ) -> Result<ExecutionResult, DatabaseError> {
            Err(DatabaseError::invalid("not in this test"))
        }
        async fn execute_batch(
            &self,
            _: &str,
            _: &str,
            _: BatchRequest,
        ) -> Result<BatchResult, DatabaseError> {
            Err(DatabaseError::invalid("not in this test"))
        }
    }

    struct Resolver(Arc<Seen>);

    #[async_trait::async_trait]
    impl runtara_component_host::ConnectionResolverHost for Resolver {
        async fn describe(&self, tenant: &str, connection_id: String) -> Result<Vec<u8>, String> {
            self.0
                .resolved
                .lock()
                .unwrap()
                .push((tenant.to_string(), connection_id.clone()));
            Ok(
                serde_json::to_vec(&serde_json::json!({"id": connection_id, "tenant": tenant}))
                    .unwrap(),
            )
        }
        async fn resolve_resource(
            &self,
            tenant: &str,
            connection_id: String,
            _request: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            self.0
                .resolved
                .lock()
                .unwrap()
                .push((tenant.to_string(), connection_id));
            Ok(br#"{"items":[1,2]}"#.to_vec())
        }
    }

    /// Connection metadata, resources, SQL and outbound HTTP over the workflow
    /// API reach the runner's connection services with the binding's tenant,
    /// and their failures come back in the API's shapes.
    #[tokio::test(flavor = "multi_thread")]
    async fn workflow_api_connections_reach_the_connection_services() {
        let h = harness().await;
        let seen = Arc::new(Seen::default());
        let runner = EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: h.dir.path().join("connections"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            Arc::clone(&h.persistence),
        )
        .unwrap()
        .with_in_process_precompiler_for_tests()
        .with_outbound_http(Arc::new(Http(seen.clone())))
        .unwrap()
        .with_database(Arc::new(Database(seen.clone())))
        .unwrap()
        .with_connection_resolver(Arc::new(Resolver(seen.clone())))
        .unwrap();
        let probe = workflow_api_probe();
        let inst_id = unique("inst-workflow-api-connections");
        let input = serde_json::to_vec(
            &serde_json::json!({"data": {"mode": "connections"}, "variables": {}}),
        )
        .unwrap();
        assert!(
            h.persistence
                .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
                .await
                .expect("register instance")
        );
        let handle = runner
            .try_launch_detached(&options(inst_id.as_str(), &probe))
            .await
            .expect("launch");
        tokio::time::timeout(
            Duration::from_secs(30),
            runner.wait_for_exit(&handle, Duration::from_millis(50)),
        )
        .await
        .expect("wait_for_exit hung");
        let instance = h
            .persistence
            .get_instance(inst_id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            instance.status,
            runtara_core::domain::InstanceStatus::Completed,
            "{instance:?}"
        );
        let output: serde_json::Value =
            serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
        assert_eq!(
            output["describe"],
            serde_json::json!({"id": "conn-1", "tenant": "embedded-test"})
        );
        assert_eq!(output["resource"], serde_json::json!({"items": [1, 2]}));
        assert_eq!(
            output["rows"],
            serde_json::json!({"columns": [], "rows": []})
        );
        assert_eq!(output["refused"], "database-error");
        assert_eq!(output["reply"]["status"], 201);
        assert_eq!(output["reply"]["body"], "pong");
        assert_eq!(
            output["reply"]["headers"],
            serde_json::json!([["x-upstream", "1"]])
        );
        assert_eq!(output["failed"]["code"], "HTTP_DESTINATION_DENIED");
        assert_eq!(output["failed"]["status"], 403);
        assert_eq!(output["failed"]["body"], "nope");

        let http = seen.http.lock().unwrap();
        assert_eq!(http.len(), 2);
        assert!(http.iter().all(|(tenant, _)| tenant == "embedded-test"));
        assert_eq!(http[0].1.method, "POST");
        assert_eq!(http[0].1.body.as_deref(), Some(b"ping".as_slice()));
        assert_eq!(
            seen.sql.lock().unwrap().as_slice(),
            &[(
                "embedded-test".to_string(),
                "conn-1".to_string(),
                "select 1".to_string()
            )]
        );
        assert!(
            seen.resolved
                .lock()
                .unwrap()
                .iter()
                .all(|(tenant, connection)| tenant == "embedded-test" && connection == "conn-1")
        );
    }
}

/// An operation-scoped call site over the workflow API: entering yields a
/// stable id, a suspension keeps its continuation in persistence, re-entering
/// with `load` reads it back, and a suspension an agent may not make is
/// refused with the suspension caps' error.
#[tokio::test(flavor = "multi_thread")]
async fn workflow_api_operations_keep_their_continuation() {
    let h = harness().await;
    let probe = workflow_api_probe();
    let inst_id = unique("inst-workflow-api-operation");
    let input =
        serde_json::to_vec(&serde_json::json!({"data": {"mode": "operation"}, "variables": {}}))
            .unwrap();
    assert!(
        h.persistence
            .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
            .await
            .expect("register instance")
    );
    let handle = h
        .runner
        .try_launch_detached(&options(inst_id.as_str(), &probe))
        .await
        .expect("launch");
    tokio::time::timeout(
        Duration::from_secs(30),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("wait_for_exit hung");
    let instance = h
        .persistence
        .get_instance(inst_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        instance.status,
        runtara_core::domain::InstanceStatus::Completed,
        "{instance:?}"
    );
    let output: serde_json::Value =
        serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
    assert_eq!(output["sameId"], true);
    assert_eq!(output["first"], serde_json::Value::Null);
    assert_eq!(output["loaded"], "saved");
    assert_eq!(output["refused"], "agent-invalid-suspension");
}

mod workflow_api_control {
    use super::*;
    use runtara_component_host::control_host::{
        CancelRequest, CommandOutcome, CommandResult, ControlAuthority, ControlError,
        ControlErrorCode, ControlHost, InstanceDetail, InstancePage, InstanceStatus,
        InstanceSummary, QueryRequest, StartRequest, StartResult, TerminalResult,
    };
    use std::sync::Mutex;

    /// A control service with today's mutation rules (a mutation needs an
    /// operation; only `child-1` is a child), recording who each call was for.
    #[derive(Default)]
    struct Control {
        calls: Mutex<Vec<(&'static str, ControlAuthority)>>,
        started: Mutex<Option<StartRequest>>,
    }

    fn summary(id: &str) -> InstanceSummary {
        InstanceSummary {
            instance_id: id.into(),
            workflow_id: "child-wf".into(),
            version: Some(1),
            run_label: None,
            parent_instance_id: Some("parent".into()),
            status: InstanceStatus::Running,
            suspension_reason: None,
            termination_reason: None,
            created_at_ms: 1,
            started_at_ms: Some(2),
            finished_at_ms: None,
        }
    }

    impl Control {
        fn record(&self, call: &'static str, authority: &ControlAuthority) {
            self.calls.lock().unwrap().push((call, authority.clone()));
        }
        fn mutation(&self, authority: &ControlAuthority, target: &str) -> Result<(), ControlError> {
            if authority.operation.is_none() {
                return Err(ControlError::new(
                    ControlErrorCode::RequiresOperation,
                    "needs an operation",
                ));
            }
            if target != "child-1" {
                return Err(ControlError::new(ControlErrorCode::NotChild, "not a child"));
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ControlHost for Control {
        async fn start(
            &self,
            authority: &ControlAuthority,
            request: StartRequest,
        ) -> Result<StartResult, ControlError> {
            self.record("start", authority);
            self.mutation(authority, "child-1")?;
            *self.started.lock().unwrap() = Some(request);
            Ok(StartResult {
                instance_id: "child-1".into(),
                workflow_id: "child-wf".into(),
                version: 1,
                run_label: None,
                replayed: false,
            })
        }
        async fn get(
            &self,
            authority: &ControlAuthority,
            instance_id: String,
        ) -> Result<InstanceDetail, ControlError> {
            self.record("get", authority);
            Ok(InstanceDetail {
                instance: summary(&instance_id),
                terminal: TerminalResult {
                    output: None,
                    output_bytes: None,
                    output_omitted: false,
                    error: None,
                    error_omitted: false,
                },
            })
        }
        async fn query(
            &self,
            authority: &ControlAuthority,
            request: QueryRequest,
        ) -> Result<InstancePage, ControlError> {
            self.record("query", authority);
            assert_eq!(request.statuses, vec![InstanceStatus::Running]);
            Ok(InstancePage {
                items: vec![summary("child-1")],
                total: 1,
                next_page_token: None,
            })
        }
        async fn cancel(
            &self,
            authority: &ControlAuthority,
            request: CancelRequest,
        ) -> Result<CommandResult, ControlError> {
            self.record("cancel", authority);
            self.mutation(authority, &request.instance_id)?;
            Ok(CommandResult {
                instance_id: request.instance_id,
                outcome: CommandOutcome::Requested,
                replayed: false,
            })
        }
        async fn pause(
            &self,
            authority: &ControlAuthority,
            instance_id: String,
        ) -> Result<CommandResult, ControlError> {
            self.record("pause", authority);
            self.mutation(authority, &instance_id)?;
            Ok(CommandResult {
                instance_id,
                outcome: CommandOutcome::Applied,
                replayed: false,
            })
        }
    }

    /// Control calls over the workflow API reach the runner's control service
    /// as the run itself: the binding's tenant and instance, and the operation
    /// only when the run entered it. Control refusals keep their codes.
    #[tokio::test(flavor = "multi_thread")]
    async fn workflow_api_control_calls_act_as_the_run() {
        let h = harness().await;
        let control = Arc::new(Control::default());
        let runner = EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: h.dir.path().join("control"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            Arc::clone(&h.persistence),
        )
        .unwrap()
        .with_in_process_precompiler_for_tests()
        .with_control_host(control.clone())
        .unwrap();
        let probe = workflow_api_probe();
        let inst_id = unique("inst-workflow-api-control");
        let input =
            serde_json::to_vec(&serde_json::json!({"data": {"mode": "control"}, "variables": {}}))
                .unwrap();
        assert!(
            h.persistence
                .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
                .await
                .expect("register instance")
        );
        let handle = runner
            .try_launch_detached(&options(inst_id.as_str(), &probe))
            .await
            .expect("launch");
        tokio::time::timeout(
            Duration::from_secs(30),
            runner.wait_for_exit(&handle, Duration::from_millis(50)),
        )
        .await
        .expect("wait_for_exit hung");
        let instance = h
            .persistence
            .get_instance(inst_id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            instance.status,
            runtara_core::domain::InstanceStatus::Completed,
            "{instance:?}"
        );
        let output: serde_json::Value =
            serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
        assert_eq!(output["started"]["instanceId"], "child-1");
        assert_eq!(output["detail"]["instance"]["status"], "running");
        assert_eq!(output["page"]["items"][0]["instanceId"], "child-1");
        assert_eq!(output["page"]["total"], 1);
        assert_eq!(output["outside"], "requires-operation");
        assert_eq!(output["stranger"], "not-found");
        assert_eq!(output["notChild"], "not-child");

        let started = control.started.lock().unwrap().clone().expect("start");
        assert_eq!(started.workflow_id, "child-wf");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&started.input).unwrap(),
            serde_json::json!({"data": {"n": 1}, "variables": {}})
        );
        let calls = control.calls.lock().unwrap();
        let names: Vec<_> = calls.iter().map(|(name, _)| *name).collect();
        // The stranger operation never reaches the service.
        assert_eq!(names, ["start", "get", "query", "pause", "cancel"]);
        let operation = calls[0]
            .1
            .operation
            .clone()
            .expect("start runs in the operation");
        for (name, authority) in calls.iter() {
            assert_eq!(authority.tenant, "embedded-test", "{name}");
            assert_eq!(
                authority.caller.as_deref(),
                Some(inst_id.as_str()),
                "{name}"
            );
        }
        assert_eq!(calls[1].1.operation, None);
        assert_eq!(calls[3].1.operation, None, "pause went without one");
        assert_eq!(calls[4].1.operation.as_ref(), Some(&operation));
    }
}

mod workflow_api_trusted {
    use super::*;
    use runtara_agent_trusted::{TrustedContext, error};
    use runtara_component_host::ComponentDispatcherService;
    use runtara_component_host::trusted::TrustedCredentials;

    #[derive(Default)]
    struct Credentials(AtomicUsize);

    #[async_trait::async_trait]
    impl TrustedCredentials for Credentials {
        async fn resolve(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<TrustedContext, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(error("DENIED", "not in this test"))
        }
    }

    /// A trusted call over the workflow API is decided by the run's artifact
    /// pins like on the old ABI: a run that pins no trusted artifact is
    /// refused before any credential lookup. (The probe cannot pin one; a
    /// pinned success lands with the emitter's move to the new ABI.)
    #[tokio::test(flavor = "multi_thread")]
    async fn workflow_api_trusted_calls_fail_closed_without_a_pin() {
        let h = harness().await;
        let release =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release");
        let bundle = tempfile::tempdir().unwrap();
        for suffix in ["wasm", "meta.json"] {
            let file = format!("runtara_agent_s3_storage.{suffix}");
            std::fs::copy(release.join(&file), bundle.path().join(&file)).unwrap_or_else(|e| {
                panic!("missing {file}: run scripts/build-agent-components.sh ({e})")
            });
        }
        let dispatcher = ComponentDispatcherService::from_dir(bundle.path())
            .await
            .unwrap();
        let credentials = Arc::new(Credentials::default());
        dispatcher
            .trusted_executor()
            .set_credentials(credentials.clone())
            .unwrap();
        let runner = EmbeddedWasmRunner::new(
            WorkflowRunnerConfig {
                data_dir: h.dir.path().join("trusted"),
                default_timeout: Duration::from_secs(30),
                skip_cert_verification: false,
            },
            Arc::clone(&h.persistence),
        )
        .unwrap()
        .with_in_process_precompiler_for_tests()
        .with_trusted_executor(dispatcher.trusted_executor())
        .unwrap();
        let probe = workflow_api_probe();
        let inst_id = unique("inst-workflow-api-trusted");
        let input =
            serde_json::to_vec(&serde_json::json!({"data": {"mode": "trusted"}, "variables": {}}))
                .unwrap();
        assert!(
            h.persistence
                .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
                .await
                .expect("register instance")
        );
        let handle = runner
            .try_launch_detached(&options(inst_id.as_str(), &probe))
            .await
            .expect("launch");
        tokio::time::timeout(
            Duration::from_secs(30),
            runner.wait_for_exit(&handle, Duration::from_millis(50)),
        )
        .await
        .expect("wait_for_exit hung");
        let instance = h
            .persistence
            .get_instance(inst_id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            instance.status,
            runtara_core::domain::InstanceStatus::Completed,
            "{instance:?}"
        );
        let output: serde_json::Value =
            serde_json::from_slice(instance.output.as_deref().unwrap()).unwrap();
        assert_eq!(
            output["refusal"],
            serde_json::json!({"code": "trusted-error", "envelope": "TRUSTED_VERSION_REQUIRED"})
        );
        assert_eq!(credentials.0.load(Ordering::SeqCst), 0);
    }
}

/// Real workflows compiled for the workflow API: the logic imports
/// `runtara:api`, composed with the `runtara:api` component and the run shim,
/// and the artifact imports only WASI. Each case must end with its expected
/// status and output: an agent step's checkpoint with
/// debug events on, a While loop
/// with Log steps (signal checks at loop boundaries), a durable Delay that
/// parks the run (a suspended outcome with its wake), and published state
/// written by SetState and read back by GetState.
#[tokio::test(flavor = "multi_thread")]
async fn workflows_compiled_for_the_workflow_api_end_as_expected() {
    use runtara_workflows::direct_wasm::{
        DirectCompilationInput, compile_direct_workflow, compose_direct_workflow,
    };
    let h = harness().await;
    let components =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-wasip2/release");
    let fixture = |name: &str| -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../runtara-workflows/tests/fixtures")
                    .join(name),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let echo = serde_json::json!({
        "durable": true,
        "steps": {
            "agent": {
                "stepType": "Agent", "id": "agent", "name": "Echo",
                "agentId": "utils", "capabilityId": "return-input", "maxRetries": 0,
                "inputMapping": {"value": {"valueType": "reference", "value": "data.value"}}
            },
            "finish": {
                "stepType": "Finish", "id": "finish",
                "inputMapping": {"result": {"valueType": "reference", "value": "steps.agent.outputs"}}
            }
        },
        "entryPoint": "agent",
        "executionPlan": [{"fromStep": "agent", "toStep": "finish"}],
        "variables": {},
        "inputSchema": {"value": {"type": "string"}},
        "outputSchema": {}
    });
    let delay = serde_json::json!({
        "durable": true,
        "steps": {
            "delay": {
                "stepType": "Delay", "id": "delay", "name": "Wait",
                "durationMs": {"valueType": "immediate", "value": 3_600_000}
            },
            "finish": {
                "stepType": "Finish", "id": "finish",
                "inputMapping": {"done": {"valueType": "immediate", "value": true}}
            }
        },
        "entryPoint": "delay",
        "executionPlan": [{"fromStep": "delay", "toStep": "finish"}],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    });
    let state = serde_json::json!({
        "durable": true,
        "stateSchema": {"stage": {"type": "string"}, "n": {"type": "integer"}},
        "steps": {
            "s1": {"id": "s1", "stepType": "SetState", "values": {
                "stage": {"valueType": "immediate", "value": "before"},
                "n": {"valueType": "reference", "value": "data.n"}}},
            "s2": {"id": "s2", "stepType": "SetState", "values": {
                "stage": {"valueType": "immediate", "value": "after"}}},
            "g": {"id": "g", "stepType": "GetState"},
            "done": {"id": "done", "stepType": "Finish", "inputMapping": {
                "value": {"valueType": "reference", "value": "steps.g.outputs"}}}
        },
        "entryPoint": "s1",
        "executionPlan": [
            {"fromStep": "s1", "toStep": "s2"},
            {"fromStep": "s2", "toStep": "g"},
            {"fromStep": "g", "toStep": "done"}
        ],
        "variables": {},
        "inputSchema": {"n": {"type": "integer"}},
        "outputSchema": {}
    });
    use runtara_core::domain::InstanceStatus::{Completed, Suspended};
    let cases = [
        (
            "echo",
            echo,
            serde_json::json!({"value": "hello"}),
            true,
            Completed,
        ),
        (
            "log-in-loop",
            fixture("log_in_loop.json"),
            serde_json::json!({"count": 3}),
            true,
            Completed,
        ),
        ("delay", delay, serde_json::json!({}), false, Suspended),
        (
            "state",
            state,
            serde_json::json!({"n": 7}),
            false,
            Completed,
        ),
    ];
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
        ["utils", "transform"]
            .iter()
            .map(|agent| {
                serde_json::from_slice(
                    &std::fs::read(components.join(format!("runtara_agent_{agent}.meta.json")))
                        .unwrap(),
                )
                .unwrap()
            })
            .collect(),
    ));

    for (case, graph, data, track_events, status) in cases {
        let mut runs = Vec::new();
        for abi in ["workflow-api"] {
            let mut compiled = compile_direct_workflow(DirectCompilationInput {
                workflow_id: format!("workflow-api-{case}"),
                version: 1,
                source_checksum: None,
                execution_graph: serde_json::from_value(graph.clone()).unwrap(),
                child_workflows: vec![],
                output_dir: h.dir.path().join(format!("workflow-api-{case}-{abi}")),
                track_events,
                agent_catalog: Some(Arc::clone(&catalog)),
                agent_slug: None,
            })
            .unwrap();
            let wasm = compose_direct_workflow(&mut compiled, &components).unwrap();

            let inst_id = unique(&format!("inst-workflow-api-{case}"));
            let input =
                serde_json::to_vec(&serde_json::json!({"data": data, "variables": {}})).unwrap();
            assert!(
                h.persistence
                    .try_register_instance(inst_id.as_str(), "embedded-test", Some(&input))
                    .await
                    .expect("register instance")
            );
            let handle = h
                .runner
                .try_launch_detached(&options(inst_id.as_str(), &wasm))
                .await
                .expect("launch");
            tokio::time::timeout(
                Duration::from_secs(60),
                h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
            )
            .await
            .expect("wait_for_exit hung");
            let instance = h
                .persistence
                .get_instance(inst_id.as_str())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(instance.status, status, "{case} {abi}: {instance:?}");
            let output: Option<serde_json::Value> = instance
                .output
                .as_deref()
                .map(|output| serde_json::from_slice(output).unwrap());
            let mut events: Vec<(String, Option<String>)> = h
                .persistence
                .list_events(
                    inst_id.as_str(),
                    &runtara_core::persistence::ListEventsFilter::default(),
                    10_000,
                    0,
                )
                .await
                .unwrap()
                .into_iter()
                .map(|event| (format!("{:?}", event.event_type), event.subtype))
                .collect();
            events.sort();
            runs.push((output, events));
        }
        if case == "echo" {
            assert_eq!(runs[0].0, Some(serde_json::json!({"result": "hello"})));
        }
        if case == "state" {
            assert_eq!(
                runs[0].0.as_ref().map(|output| output["value"].clone()),
                Some(serde_json::json!({"stage": "after", "n": 7})),
                "the published state"
            );
        }
    }
    assert_eq!(h.runner.workflow_api_launches().serving(), 0);
}
