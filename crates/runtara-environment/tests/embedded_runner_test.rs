// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! EmbeddedWasmRunner integration tests.
//!
//! Hermetic: components are authored in WAT (no SDK, no HTTP), persistence is
//! a real SQLite store in a temp dir. What the SDK would normally report to
//! runtara-core is pre-seeded so `run()`'s output/error mapping is exercised
//! end to end without a server stack.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use runtara_core::persistence::{CompleteInstanceParams, Persistence, SqlitePersistence};
use runtara_environment::runner::{
    EmbeddedWasmRunner, LaunchOptions, Runner, RunnerError, WorkflowRunnerConfig,
};

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

/// `wasi:cli/run` returning err — the embedded analogue of exit code 1.
const RUN_ERR_WAT: &str = r#"
    (component
        (core module $m
            (func (export "run") (result i32) (i32.const 1))
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
    runner: EmbeddedWasmRunner,
    persistence: Arc<SqlitePersistence>,
    dir: tempfile::TempDir,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let persistence = Arc::new(
        SqlitePersistence::from_path(dir.path().join("core.sqlite"))
            .await
            .expect("sqlite persistence"),
    );
    let config = WorkflowRunnerConfig {
        data_dir: dir.path().join("data"),
        default_timeout: Duration::from_secs(30),
        skip_cert_verification: false,
        connection_service_url: None,
    };
    let runner = EmbeddedWasmRunner::new(config, persistence.clone() as Arc<dyn Persistence>)
        .expect("embedded runner");
    Harness {
        runner,
        persistence,
        dir,
    }
}

fn write_component(dir: &Path, name: &str, wat: &str) -> PathBuf {
    let path = dir.join(name);
    let bytes = wat::parse_str(wat).expect("compile WAT component");
    std::fs::write(&path, bytes).expect("write component");
    path
}

fn options(instance_id: &str, wasm_path: &Path) -> LaunchOptions {
    LaunchOptions {
        instance_id: instance_id.to_string(),
        tenant_id: "embedded-test".to_string(),
        bundle_path: wasm_path.to_path_buf(),
        input: serde_json::Value::Null,
        timeout: Duration::from_secs(30),
        runtara_core_addr: "127.0.0.1:1".to_string(),
        checkpoint_id: None,
        env: HashMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_maps_completed_guest_to_persisted_output() {
    let h = harness().await;
    let wasm = write_component(h.dir.path(), "ok.wasm", RUN_OK_WAT);

    // Seed what the SDK would have reported during execution.
    h.persistence
        .register_instance("inst-ok", "embedded-test")
        .await
        .expect("register");
    h.persistence
        .complete_instance(
            CompleteInstanceParams::new("inst-ok", "completed").with_output(br#"{"answer":42}"#),
        )
        .await
        .expect("complete");

    let result = h
        .runner
        .run(&options("inst-ok", &wasm), None)
        .await
        .expect("run");
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(result.output, Some(serde_json::json!({"answer": 42})));
    assert!(result.metrics.memory_peak_bytes.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn run_maps_guest_error_to_persisted_error_message() {
    let h = harness().await;
    let wasm = write_component(h.dir.path(), "err.wasm", RUN_ERR_WAT);

    h.persistence
        .register_instance("inst-err", "embedded-test")
        .await
        .expect("register");
    h.persistence
        .complete_instance(
            CompleteInstanceParams::new("inst-err", "failed").with_error("boom from sdk"),
        )
        .await
        .expect("complete");

    let result = h
        .runner
        .run(&options("inst-err", &wasm), None)
        .await
        .expect("run");
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("boom from sdk"));
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_detached_completes_and_clears_registry() {
    let h = harness().await;
    let wasm = write_component(h.dir.path(), "ok.wasm", RUN_OK_WAT);

    let handle = h
        .runner
        .launch_detached(&options("inst-detached", &wasm))
        .await
        .expect("launch");
    assert_eq!(handle.spawned_pid, None);
    assert!(handle.child.is_none());

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
async fn stop_cancels_spinning_instance() {
    let h = harness().await;
    let wasm = write_component(h.dir.path(), "spin.wasm", RUN_SPIN_WAT);

    let handle = h
        .runner
        .launch_detached(&options("inst-spin", &wasm))
        .await
        .expect("launch");

    // Give the run a moment to actually enter the guest loop.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        h.runner.is_running(&handle).await,
        "guest should be spinning"
    );

    h.runner.stop(&handle).await.expect("stop");
    tokio::time::timeout(
        Duration::from_secs(10),
        h.runner.wait_for_exit(&handle, Duration::from_millis(50)),
    )
    .await
    .expect("cancel did not end the spinning guest");
    assert!(!h.runner.is_running(&handle).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_component_is_binary_not_found() {
    let h = harness().await;
    let missing = h.dir.path().join("nope.wasm");
    let err = h
        .runner
        .launch_detached(&options("inst-missing", &missing))
        .await
        .expect_err("must fail");
    assert!(matches!(err, RunnerError::BinaryNotFound(_)));
}
