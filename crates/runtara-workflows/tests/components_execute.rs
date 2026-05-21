//! Components-mode execution test.
//!
//! Compiles the `crypto/hash` workflow through the full components pipeline
//! (cargo-component build + wac compose), then *runs* the resulting composed
//! `workflow.wasm` under `wasmtime run` and asserts the workflow reported
//! `sha256("hello") = 2cf24dba...` to its `/completed` endpoint.
//!
//! Counterpart to `components_smoke.rs` which proves compilation succeeds for
//! every DSL shape — this proves the end-to-end pipeline is wired correctly:
//!   1. Codegen → cargo-component → wac → workflow.wasm
//!   2. wasmtime loads it (per-agent imports bound at compose time)
//!   3. SDK HTTP backend connects to RUNTARA_HTTP_URL
//!   4. Workflow executes the crypto/hash step in-process via WIT bindings
//!   5. Workflow POSTs the completion envelope with the expected hash
//!
//! Gated by `RUNTARA_RUN_COMPONENTS_E2E=1`.
//!
//! Implementation note: rolls its own minimal HTTP/1.1 server on
//! `127.0.0.1:0` using `std::net` so this test doesn't pull tokio/axum into
//! `runtara-workflows`'s dev-deps. It only handles five endpoints; everything
//! else returns 200/empty.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use runtara_workflows::{CompilationInput, ExecutionGraph, compile_workflow};
use serde_json::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Gating helpers (same shape as components_smoke.rs)
// ---------------------------------------------------------------------------

fn e2e_enabled() -> bool {
    std::env::var("RUNTARA_RUN_COMPONENTS_E2E").as_deref() == Ok("1")
}

fn tool_installed(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cargo_component_installed() -> bool {
    Command::new("cargo")
        .arg("component")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Mirror what `WasmRunner::from_env` does: honor `WASMTIME_PATH`, then
/// `~/.wasmtime/bin/wasmtime`, then `wasmtime` on `PATH`.
fn wasmtime_binary() -> PathBuf {
    if let Ok(p) = std::env::var("WASMTIME_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home)
            .join(".wasmtime")
            .join("bin")
            .join("wasmtime");
        if home_path.exists() {
            return home_path;
        }
    }
    PathBuf::from("wasmtime")
}

fn wasmtime_installed() -> bool {
    Command::new(wasmtime_binary())
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn agent_wasm_staged() -> bool {
    let dir = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join("target/wasm32-wasip1/release"));
    dir.join("runtara_agent_crypto.wasm").exists()
}

fn setup_data_dir() -> Option<TempDir> {
    if std::env::var_os("DATA_DIR").is_some() {
        return None;
    }
    let temp = TempDir::new().expect("tempdir");
    // SAFETY: cargo test runs each #[test] in its own thread, but env mutation
    // here happens before any concurrent reader.
    unsafe {
        std::env::set_var("DATA_DIR", temp.path());
        // Reuse the smoke test's target dir if it's set so we share the
        // cargo-component dep cache across both tests.
        if std::env::var_os("RUNTARA_COMPONENTS_TARGET_DIR").is_none() {
            std::env::set_var(
                "RUNTARA_COMPONENTS_TARGET_DIR",
                temp.path().join("shared-target"),
            );
        }
    }
    Some(temp)
}

// ---------------------------------------------------------------------------
// Workflow fixture — single agent step (crypto/hash) → Finish.
// ---------------------------------------------------------------------------

const CRYPTO_HASH_WORKFLOW: &str = r#"{
    "name": "components_execute_hash",
    "steps": {
        "h": {
            "stepType": "Agent",
            "id": "h",
            "agentId": "crypto",
            "capabilityId": "hash",
            "inputMapping": {
                "data": {"valueType": "immediate", "value": "hello"},
                "algorithm": {"valueType": "immediate", "value": "sha256"},
                "output_format": {"valueType": "immediate", "value": "hex"}
            }
        },
        "f": {
            "stepType": "Finish",
            "id": "f",
            "inputMapping": {
                "hash": {"valueType": "reference", "value": "steps.h.outputs.hash"}
            }
        }
    },
    "entryPoint": "h",
    "executionPlan": [{"fromStep": "h", "toStep": "f"}],
    "variables": {},
    "inputSchema": {},
    "outputSchema": {}
}"#;

/// sha256("hello"), lowercase hex — what the workflow should report.
const EXPECTED_HASH: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

// ---------------------------------------------------------------------------
// Minimal HTTP/1.1 server — just enough of the SDK protocol for one workflow.
// ---------------------------------------------------------------------------

/// What a single `/completed` POST carried, decoded from base64-JSON.
#[derive(Debug)]
struct Completed {
    output_json: Value,
}

/// Read a request, decide based on method + path, write a fixed response.
/// Only one connection (re-used by `ureq`) is handled per call; we loop in
/// `serve` over many connections.
fn handle_request(
    stream: &mut std::net::TcpStream,
    sink: &mpsc::Sender<Completed>,
) -> std::io::Result<bool> {
    let peer = stream.peer_addr().ok();
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(false); // closed
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Ok(false);
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    let mut content_length = 0usize;
    let mut chunked = false;
    let mut connection_close = false;
    let mut header_lines: Vec<String> = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(false);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        header_lines.push(trimmed.to_string());
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
        if let Some(rest) = lower.strip_prefix("transfer-encoding:")
            && rest.trim() == "chunked"
        {
            chunked = true;
        }
        if lower.starts_with("connection:") && lower.contains("close") {
            connection_close = true;
        }
    }

    let body = if chunked {
        read_chunked_body(&mut reader)?
    } else {
        let mut buf = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut buf)?;
        }
        buf
    };

    if std::env::var_os("RUNTARA_EXECUTE_TRACE_HEADERS").is_some() {
        eprintln!(
            "[fake-sdk] headers for {method} {path} (cl={content_length}, chunked={chunked}, body_len={}):\n  {}",
            body.len(),
            header_lines.join("\n  "),
        );
    }
    let _ = peer; // hush unused

    let (status, response_json) = route(&method, &path, &body, sink);
    if std::env::var_os("RUNTARA_EXECUTE_TRACE").is_some() {
        let preview = std::str::from_utf8(&body).unwrap_or("<binary>");
        eprintln!("[fake-sdk] {method} {path} → {status}  req={preview}  resp={response_json}");
    }
    let response_bytes = response_json.to_string();
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: keep-alive\r\n\r\n{body}",
        status = status,
        len = response_bytes.len(),
        body = response_bytes,
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;

    Ok(!connection_close)
}

/// Read an HTTP/1.1 chunked body up to the terminating `0\r\n\r\n`.
fn read_chunked_body(reader: &mut BufReader<std::net::TcpStream>) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line)? == 0 {
            break;
        }
        let size_hex = size_line.trim().split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).unwrap_or(0);
        if size == 0 {
            // Drain trailing CRLF (or trailer headers, which we don't care about).
            let mut trailer = String::new();
            while reader.read_line(&mut trailer)? > 0 {
                if trailer.trim().is_empty() {
                    break;
                }
                trailer.clear();
            }
            break;
        }
        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk)?;
        out.extend_from_slice(&chunk);
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
    }
    Ok(out)
}

/// Map (method, path) → (status, body). Captures `/completed` payloads into the
/// channel so the test thread can assert on them after the workflow exits.
fn route(method: &str, path: &str, body: &[u8], sink: &mpsc::Sender<Completed>) -> (u16, Value) {
    // Strip query string if any.
    let path = path.split('?').next().unwrap_or(path);

    if method == "GET" && path == "/health" {
        return (200, serde_json::json!({"ok": true}));
    }

    // /api/v1/instances/{id}/...
    if let Some(rest) = path.strip_prefix("/api/v1/instances/") {
        let mut iter = rest.splitn(2, '/');
        let _instance_id = iter.next().unwrap_or("");
        let endpoint = iter.next().unwrap_or("");

        match (method, endpoint) {
            ("POST", "register") => return (200, serde_json::json!({"success": true})),
            ("GET", "input") => return (200, serde_json::json!({"input": null})),
            ("POST", "checkpoint") => {
                return (
                    200,
                    serde_json::json!({
                        "found": false,
                        "state": null,
                        "signal": null,
                        "custom_signal": null,
                    }),
                );
            }
            ("POST", "completed") => {
                if let Ok(parsed) = serde_json::from_slice::<Value>(body)
                    && let Some(b64) = parsed.get("output").and_then(Value::as_str)
                {
                    use base64::Engine;
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
                        && let Ok(output_json) = serde_json::from_slice::<Value>(&bytes)
                    {
                        let _ = sink.send(Completed { output_json });
                    }
                }
                return (200, serde_json::json!({"success": true}));
            }
            ("POST", "failed") => return (200, serde_json::json!({"success": true})),
            ("POST", "events") => return (200, serde_json::json!({"success": true})),
            _ => {}
        }
    }

    // Default: pretend it worked. Real bugs (wrong URL) will surface as the
    // workflow not posting /completed and the test timing out instead.
    (200, serde_json::json!({"success": true}))
}

fn serve(listener: TcpListener, sink: mpsc::Sender<Completed>, stop: mpsc::Receiver<()>) {
    listener
        .set_nonblocking(true)
        .expect("set_nonblocking on listener");
    loop {
        if stop.try_recv().is_ok() {
            return;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let sink = sink.clone();
                thread::spawn(move || {
                    // Loop while the SDK reuses the connection.
                    while let Ok(true) = handle_request(&mut stream, &sink) {}
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn components_execute_crypto_hash_produces_expected_digest() {
    if !e2e_enabled() {
        eprintln!(
            "SKIP: components_execute — set RUNTARA_RUN_COMPONENTS_E2E=1 to run \
             (heavy: cargo-component build + wasmtime run)."
        );
        return;
    }
    if !cargo_component_installed() {
        eprintln!("SKIP: cargo-component not installed.");
        return;
    }
    if !tool_installed("wac") {
        eprintln!("SKIP: wac not installed.");
        return;
    }
    if !wasmtime_installed() {
        eprintln!(
            "SKIP: wasmtime not installed (looked at WASMTIME_PATH, \
             ~/.wasmtime/bin/wasmtime, then PATH)."
        );
        return;
    }
    if !agent_wasm_staged() {
        eprintln!("SKIP: agent components not staged.");
        return;
    }

    let _data = setup_data_dir();

    // Compile the workflow.
    let graph: ExecutionGraph =
        serde_json::from_str(CRYPTO_HASH_WORKFLOW).expect("fixture JSON parses");
    let input = CompilationInput {
        tenant_id: "components-execute".to_string(),
        workflow_id: "components_execute_hash".to_string(),
        version: 1,
        execution_graph: graph,
        track_events: false,
        child_workflows: vec![],
        connection_service_url: None,
        agent_catalog: None,
        progress_callback: None,
    };
    let compiled = compile_workflow(input).expect("compile crypto/hash workflow");
    assert!(compiled.binary_path.exists(), "compiled wasm missing");

    // Stand up the fake SDK server.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let (completion_tx, completion_rx) = mpsc::channel::<Completed>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let server_handle = thread::spawn(move || serve(listener, completion_tx, stop_rx));

    // Run wasmtime.
    let mut cmd = Command::new(wasmtime_binary());
    cmd.arg("run")
        .arg("--wasi")
        .arg("http")
        .arg("--wasi")
        .arg("inherit-network")
        .arg("--env")
        .arg(format!("RUNTARA_HTTP_URL=http://{addr}"))
        .arg("--env")
        .arg(format!("RUNTARA_SERVER_ADDR={addr}"))
        .arg("--env")
        .arg("RUNTARA_INSTANCE_ID=test-instance")
        .arg("--env")
        .arg("RUNTARA_TENANT_ID=components-execute")
        .arg("--env")
        .arg(format!(
            "RUST_LOG={}",
            std::env::var("RUNTARA_EXECUTE_LOG").unwrap_or_else(|_| "warn".to_string())
        ))
        .arg(&compiled.binary_path)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null());

    let output = cmd.output().expect("spawn wasmtime");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Stop the server thread.
    let _ = stop_tx.send(());
    let _ = server_handle.join();

    assert!(
        output.status.success(),
        "wasmtime exited non-zero ({:?}):\n--- stderr ---\n{stderr}",
        output.status.code(),
    );

    // Find the completion record.
    let mut completion: Option<Completed> = None;
    while let Ok(c) = completion_rx.try_recv() {
        completion = Some(c);
    }
    let completion = completion.unwrap_or_else(|| {
        panic!("workflow exited cleanly but never POSTed /completed.\n--- stderr ---\n{stderr}")
    });

    let hash = completion
        .output_json
        .get("hash")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "/completed payload missing `hash` field: {:?}",
                completion.output_json
            )
        });

    assert_eq!(
        hash, EXPECTED_HASH,
        "workflow reported wrong sha256: got {hash}, want {EXPECTED_HASH}"
    );

    eprintln!(
        "✓ components_execute_crypto_hash: sha256(\"hello\") = {hash} (composed wasm = {} bytes)",
        compiled.binary_size,
    );
}
