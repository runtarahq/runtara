// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-run` as its launcher uses it: the probe workflow compiled by
//! `compile`, run by `run` against a workflow API served on the other end of
//! a socket pair. On Linux the process is sandboxed, as in production;
//! elsewhere it runs `--unsandboxed`, which is all it can do there.
//!
//! Needs the probe component:
//! `cargo build -p runtara-workflow-api-probe --target wasm32-wasip2 --release`
//! (`scripts/build-agent-components.sh` builds it).

#![cfg(unix)]

use std::convert::Infallible;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

const RUNNER: &str = env!("CARGO_BIN_EXE_runtara-run");

fn probe_path() -> PathBuf {
    std::env::var_os("RUNTARA_WORKFLOW_API_PROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release/runtara_workflow_api_probe.wasm")
        })
}

/// The runner started with `input` on descriptor 3 and `output` on 4.
fn runner(subcommand: &str, input: OwnedFd, output: OwnedFd) -> Command {
    let mut command = Command::new(RUNNER);
    command.arg(subcommand).env_clear();
    if !cfg!(target_os = "linux") {
        command.arg("--unsandboxed");
    } else if std::env::var_os("RUNTARA_RUN_TEST_NO_LANDLOCK").is_some() {
        // A kernel without Landlock (some CI runners): seccomp alone.
        command.arg("--no-landlock");
    }
    // SAFETY: only dup, dup2 and close, which are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            // Out of the way first: either may already sit on 3 or 4.
            let input = libc::fcntl(input.as_raw_fd(), libc::F_DUPFD, 10);
            let output = libc::fcntl(output.as_raw_fd(), libc::F_DUPFD, 10);
            if input < 0 || output < 0 || libc::dup2(input, 3) < 0 || libc::dup2(output, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::close(input);
            libc::close(output);
            Ok(())
        });
    }
    command
}

/// The probe, compiled once for every test in this process.
fn compiled_probe() -> PathBuf {
    static COMPILED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    COMPILED
        .get_or_init(|| {
            let probe = probe_path();
            assert!(
                probe.exists(),
                "missing {}: build runtara-workflow-api-probe for wasm32-wasip2",
                probe.display()
            );
            let compiled = std::env::temp_dir()
                .join(format!("runtara-run-probe-{}.cwasm", std::process::id()));
            let output = runner(
                "compile",
                std::fs::File::open(&probe).unwrap().into(),
                std::fs::File::create(&compiled).unwrap().into(),
            )
            .output()
            .unwrap();
            assert!(
                output.status.success(),
                "compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            compiled
        })
        .clone()
}

/// What the workflow API at the launcher's end saw.
#[derive(Default)]
struct Seen {
    /// The outcome posted: its kind and body.
    outcome: Option<(String, Value)>,
    /// Whether any request carried a header a guest may not send.
    reserved_header: bool,
}

async fn serve(
    request: hyper::Request<Incoming>,
    mode: Value,
    seen: Arc<Mutex<Seen>>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    if request.headers().contains_key("authorization") {
        seen.lock().unwrap().reserved_header = true;
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let outcome = request
        .headers()
        .get("runtara-outcome")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/v1/run") => hyper::Response::new(Full::new(Bytes::from(
            json!({
                "instanceId": "instance-1", "tenantId": "tenant-1",
                "launchKind": "start", "debug": false, "apiVersion": "1.0",
            })
            .to_string(),
        ))),
        ("GET", "/v1/run/input") => hyper::Response::builder()
            .header("runtara-checkpoints", "none")
            .header("runtara-lifecycle", "none")
            .body(Full::new(Bytes::from(
                json!({"data": mode, "variables": {}}).to_string(),
            )))
            .unwrap(),
        ("POST", "/v1/run/outcome") => {
            seen.lock().unwrap().outcome = Some((
                outcome.unwrap_or_default(),
                serde_json::from_slice(&body).unwrap_or(Value::Null),
            ));
            hyper::Response::builder()
                .status(204)
                .body(Full::default())
                .unwrap()
        }
        _ => hyper::Response::builder()
            .status(404)
            .body(Full::default())
            .unwrap(),
    };
    Ok(response)
}

struct Ran {
    code: Option<i32>,
    report: Value,
    seen: Seen,
}

/// Runs the probe in `mode` with `lifetime_seconds`, its API served here.
/// With `abandon`, the launcher's end of the socket is closed once the guest
/// has read its input.
async fn run_probe(mode: Value, lifetime_seconds: u64, abandon: bool) -> Ran {
    let compiled = compiled_probe();
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    ours.set_nonblocking(true).unwrap();
    let ours = tokio::net::UnixStream::from_std(ours).unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let read_input = Arc::new(tokio::sync::Notify::new());

    let service = {
        let seen = seen.clone();
        let read_input = read_input.clone();
        hyper::service::service_fn(move |request: hyper::Request<Incoming>| {
            let (mode, seen, read_input) = (mode.clone(), seen.clone(), read_input.clone());
            async move {
                let input = request.uri().path() == "/v1/run/input";
                let response = serve(request, mode, seen).await;
                if input {
                    read_input.notify_one();
                }
                response
            }
        })
    };
    let server = tokio::spawn(async move {
        let _ = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
            .serve_connection(hyper_util::rt::TokioIo::new(ours), service)
            .await;
    });

    let mut child = tokio::process::Command::from(runner(
        "run",
        std::fs::File::open(&compiled).unwrap().into(),
        theirs.into(),
    ))
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let spec = json!({
        "wasm_memory_bytes": 64 * 1024 * 1024,
        "lifetime_seconds": lifetime_seconds,
    });
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(spec.to_string().as_bytes()).await.unwrap();
    drop(stdin);
    if abandon {
        tokio::time::timeout(std::time::Duration::from_secs(60), read_input.notified())
            .await
            .expect("the guest did not read its input");
        server.abort();
        let _ = server.await;
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        child.wait_with_output(),
    )
    .await
    .expect("the runner did not end")
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report = stdout
        .lines()
        .last()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or(Value::Null);
    if report.is_null() && !abandon {
        eprintln!("runner stderr: {}", String::from_utf8_lossy(&output.stderr));
    }
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    Ran {
        code: output.status.code(),
        report,
        seen,
    }
}

#[tokio::test]
async fn a_run_that_completes_posts_its_outcome_and_returns() {
    let ran = run_probe(json!({"mode": "complete", "n": 7}), 60, false).await;
    assert_eq!(ran.report["ended"], "returned", "{:?}", ran.report);
    assert_eq!(ran.code, Some(0));
    let (kind, body) = ran.seen.outcome.expect("an outcome");
    assert_eq!(kind, "completed");
    assert_eq!(body["instanceId"], "instance-1");
    assert_eq!(body["echo"]["n"], 7);
    assert!(!ran.seen.reserved_header);
}

#[tokio::test]
async fn a_run_that_fails_posts_the_failure_itself() {
    let ran = run_probe(json!({"mode": "fail"}), 60, false).await;
    assert_eq!(ran.report["ended"], "returned", "{:?}", ran.report);
    let (kind, body) = ran.seen.outcome.expect("an outcome");
    assert_eq!(kind, "failed");
    assert_eq!(body["code"], "PROBE_FAILED");
}

#[tokio::test]
async fn a_guest_that_traps_is_reported_and_posts_nothing() {
    let ran = run_probe(json!({"mode": "trap"}), 60, false).await;
    assert_eq!(ran.report["ended"], "trapped", "{:?}", ran.report);
    assert!(ran.report["reason"].is_string());
    assert_eq!(ran.code, Some(1));
    assert!(ran.seen.outcome.is_none());
}

#[tokio::test]
async fn a_guest_that_only_computes_is_ended_at_its_lifetime() {
    let started = std::time::Instant::now();
    let ran = run_probe(json!({"mode": "spin"}), 1, false).await;
    assert_eq!(ran.report["ended"], "timeout", "{:?}", ran.report);
    assert_eq!(ran.code, Some(1));
    assert!(ran.seen.outcome.is_none());
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[tokio::test]
async fn a_run_whose_launcher_closed_the_socket_ends() {
    let ran = run_probe(json!({"mode": "spin"}), 60, true).await;
    assert_eq!(ran.code, Some(3), "{:?}", ran.report);
    assert!(ran.report.is_null());
}

#[test]
fn a_file_that_is_not_a_workflow_is_not_compiled() {
    let garbage = std::env::temp_dir().join(format!("runtara-run-garbage-{}", std::process::id()));
    std::fs::write(&garbage, b"not a component").unwrap();
    let compiled = garbage.with_extension("cwasm");
    let output = runner(
        "compile",
        std::fs::File::open(&garbage).unwrap().into(),
        std::fs::File::create(&compiled).unwrap().into(),
    )
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::metadata(&compiled).unwrap().len(), 0);
}

#[test]
fn a_spec_out_of_range_is_refused_before_the_guest_starts() {
    let compiled = compiled_probe();
    let (_ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut child = runner(
        "run",
        std::fs::File::open(&compiled).unwrap().into(),
        theirs.into(),
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"wasm_memory_bytes": 1, "lifetime_seconds": 5}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

/// The sandbox refuses what it must, on the kernel these tests run on.
#[cfg(target_os = "linux")]
#[test]
fn the_sandbox_holds_here() {
    let mut command = Command::new(RUNNER);
    command.arg("selftest");
    if std::env::var_os("RUNTARA_RUN_TEST_NO_LANDLOCK").is_some() {
        command.arg("--no-landlock");
    }
    let output = command.output().unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    assert_eq!(
        report["holds"],
        true,
        "{report} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(0));
}
