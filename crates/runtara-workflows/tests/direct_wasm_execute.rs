//! Direct Wasm execution smoke test.
//!
//! Gated by `RUNTARA_RUN_DIRECT_WASM_E2E=1` because it needs prebuilt shared
//! workflow components, `wac`, and `wasmtime`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use base64::Engine;
use runtara_workflows::ExecutionGraph;
use runtara_workflows::direct_wasm::{
    DIRECT_SHARED_COMPONENT_REQUIREMENTS, DirectCompilationInput, compile_direct_workflow_composed,
};
use serde_json::Value;

const SIMPLE_PASSTHROUGH: &str = include_str!("fixtures/simple_passthrough.json");
const CONDITIONAL_WORKFLOW: &str = include_str!("fixtures/conditional_workflow.json");
const CONDITIONAL_NESTED: &str = include_str!("fixtures/conditional_nested.json");
const FILTER_SIMPLE: &str = include_str!("fixtures/filter_simple.json");
const SWITCH_VALUE_SIMPLE: &str = include_str!("fixtures/switch_value_simple.json");
const SWITCH_ROUTING_SIMPLE: &str = include_str!("fixtures/switch_routing_simple.json");
const GROUP_BY_SIMPLE: &str = include_str!("fixtures/group_by_simple.json");
const LOG_ALL_LEVELS: &str = include_str!("fixtures/log_all_levels.json");
const ERROR_DIRECT_SIMPLE: &str = include_str!("fixtures/error_direct_simple.json");
const EDGE_CONDITION_PRIORITY: &str = include_str!("fixtures/edge_condition_priority.json");

#[derive(Debug)]
struct Completed {
    output_json: Value,
}

#[derive(Debug)]
struct Failed {
    error_json: Value,
}

#[derive(Debug)]
struct RuntimeEvent {
    subtype: String,
    payload_json: Value,
}

#[derive(Debug)]
struct DirectRunOutput {
    output_json: Value,
    events: Vec<RuntimeEvent>,
}

#[derive(Debug)]
struct DirectFailureOutput {
    error_json: Value,
    events: Vec<RuntimeEvent>,
}

#[derive(Debug)]
struct CapturedRun {
    output_json: Option<Value>,
    error_json: Option<Value>,
    events: Vec<RuntimeEvent>,
    status_success: bool,
    stderr: String,
}

#[derive(Debug)]
enum CapturedMessage {
    Completed(Completed),
    Failed(Failed),
    Event(RuntimeEvent),
}

fn e2e_enabled() -> bool {
    std::env::var("RUNTARA_RUN_DIRECT_WASM_E2E").as_deref() == Ok("1")
}

fn tool_installed(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn shared_components_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target/wasm32-wasip2/release"));
    let missing: Vec<_> = DIRECT_SHARED_COMPONENT_REQUIREMENTS
        .iter()
        .filter_map(|component| {
            let wasm = dir.join(component.bundle_wasm_filename);
            (!wasm.exists()).then_some(wasm)
        })
        .collect();
    if missing.is_empty() {
        Some(dir)
    } else {
        eprintln!(
            "SKIP: direct shared workflow components are not staged: {:?}",
            missing
        );
        None
    }
}

/// Mirror `WasmRunner::from_env`: honor `WASMTIME_PATH`, then
/// `~/.wasmtime/bin/wasmtime`, then PATH.
fn wasmtime_binary() -> PathBuf {
    if let Ok(path) = std::env::var("WASMTIME_PATH") {
        return PathBuf::from(path);
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
        .is_ok_and(|output| output.status.success())
}

fn handle_request(
    stream: &mut std::net::TcpStream,
    sink: &mpsc::Sender<CapturedMessage>,
    workflow_input: &[u8],
) -> std::io::Result<bool> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(false);
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
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(false);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
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

    let (status, response_json) = route(&method, &path, &body, sink, workflow_input);
    let response_bytes = response_json.to_string();
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: keep-alive\r\n\r\n{body}",
        len = response_bytes.len(),
        body = response_bytes,
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;

    Ok(!connection_close)
}

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

fn route(
    method: &str,
    path: &str,
    body: &[u8],
    sink: &mpsc::Sender<CapturedMessage>,
    workflow_input: &[u8],
) -> (u16, Value) {
    let path = path.split('?').next().unwrap_or(path);

    if method == "GET" && path == "/health" {
        return (200, serde_json::json!({"ok": true}));
    }

    if let Some(rest) = path.strip_prefix("/api/v1/instances/") {
        let mut iter = rest.splitn(2, '/');
        let _instance_id = iter.next().unwrap_or("");
        let endpoint = iter.next().unwrap_or("");

        match (method, endpoint) {
            ("POST", "register") => return (200, serde_json::json!({"success": true})),
            ("GET", "input") => {
                let input = base64::engine::general_purpose::STANDARD.encode(workflow_input);
                return (200, serde_json::json!({ "input": input }));
            }
            ("POST", "completed") => {
                capture_completed(body, sink);
                return (200, serde_json::json!({"success": true}));
            }
            ("POST", "events") => {
                capture_event(body, sink);
                return (200, serde_json::json!({"success": true}));
            }
            ("POST", "failed") => {
                capture_failed(body, sink);
                return (200, serde_json::json!({"success": true}));
            }
            _ => {}
        }
    }

    (200, serde_json::json!({"success": true}))
}

fn capture_completed(body: &[u8], sink: &mpsc::Sender<CapturedMessage>) {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body)
        && let Some(b64) = parsed.get("output").and_then(Value::as_str)
        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
        && let Ok(output_json) = serde_json::from_slice::<Value>(&bytes)
    {
        let _ = sink.send(CapturedMessage::Completed(Completed { output_json }));
    }
}

fn capture_failed(body: &[u8], sink: &mpsc::Sender<CapturedMessage>) {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body)
        && let Some(error) = parsed.get("error").and_then(Value::as_str)
    {
        let error_json =
            serde_json::from_str::<Value>(error).unwrap_or_else(|_| Value::String(error.into()));
        let _ = sink.send(CapturedMessage::Failed(Failed { error_json }));
    }
}

fn capture_event(body: &[u8], sink: &mpsc::Sender<CapturedMessage>) {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body)
        && parsed.get("event_type").and_then(Value::as_str) == Some("custom")
        && let Some(subtype) = parsed.get("subtype").and_then(Value::as_str)
        && let Some(b64) = parsed.get("payload").and_then(Value::as_str)
        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
        && let Ok(payload_json) = serde_json::from_slice::<Value>(&bytes)
    {
        let _ = sink.send(CapturedMessage::Event(RuntimeEvent {
            subtype: subtype.to_string(),
            payload_json,
        }));
    }
}

fn serve(
    listener: TcpListener,
    sink: mpsc::Sender<CapturedMessage>,
    stop: mpsc::Receiver<()>,
    workflow_input: Arc<Vec<u8>>,
) {
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
                let workflow_input = workflow_input.clone();
                thread::spawn(move || {
                    while let Ok(true) =
                        handle_request(&mut stream, &sink, workflow_input.as_slice())
                    {
                        // Keep serving the same connection while the SDK reuses it.
                    }
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

fn direct_e2e_components_dir() -> Option<PathBuf> {
    if !e2e_enabled() {
        eprintln!(
            "SKIP: direct_wasm_execute — set RUNTARA_RUN_DIRECT_WASM_E2E=1 to run \
             (needs wac, wasmtime, and staged direct workflow components)."
        );
        return None;
    }
    if !tool_installed("wac") {
        eprintln!("SKIP: wac not installed.");
        return None;
    }
    if !wasmtime_installed() {
        eprintln!("SKIP: wasmtime not installed.");
        return None;
    }
    let components_dir = shared_components_dir()?;

    Some(components_dir)
}

fn run_direct_workflow(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
) -> Value {
    run_direct_workflow_with_events(components_dir, workflow_id, graph_json, workflow_input)
        .output_json
}

fn run_direct_workflow_with_events(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
) -> DirectRunOutput {
    run_direct_workflow_with_events_and_tracking(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        false,
    )
}

fn run_direct_workflow_with_events_and_tracking(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
) -> DirectRunOutput {
    let captured = run_direct_workflow_capture(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        track_events,
    );
    assert!(
        captured.status_success,
        "wasmtime exited non-zero:\n--- stderr ---\n{}",
        captured.stderr
    );
    let output_json = captured.output_json.unwrap_or_else(|| {
        panic!(
            "direct workflow exited but never POSTed /completed.\n--- stderr ---\n{}",
            captured.stderr
        )
    });
    DirectRunOutput {
        output_json,
        events: captured.events,
    }
}

fn run_direct_workflow_expect_failure(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
) -> DirectFailureOutput {
    let captured = run_direct_workflow_capture(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        false,
    );
    assert!(
        !captured.status_success,
        "direct Error workflow should return a failed wasi:cli/run result"
    );
    assert!(
        captured.output_json.is_none(),
        "direct Error workflow should not POST /completed"
    );
    let error_json = captured.error_json.unwrap_or_else(|| {
        panic!(
            "direct workflow exited but never POSTed /failed.\n--- stderr ---\n{}",
            captured.stderr
        )
    });
    DirectFailureOutput {
        error_json,
        events: captured.events,
    }
}

fn run_direct_workflow_capture(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
) -> CapturedRun {
    let temp = tempfile::tempdir().expect("tempdir");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let compiled = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            execution_graph: graph,
            output_dir: temp.path().to_path_buf(),
            track_events,
        },
        components_dir,
    )
    .expect("direct composed compile");
    assert_eq!(compiled.wasm_path, compiled.build_dir.join("workflow.wasm"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let (capture_tx, capture_rx) = mpsc::channel::<CapturedMessage>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let workflow_input = Arc::new(workflow_input.to_vec());
    let server_handle = thread::spawn(move || serve(listener, capture_tx, stop_rx, workflow_input));

    let output = Command::new(wasmtime_binary())
        .arg("run")
        .arg("--wasi")
        .arg("http")
        .arg("--wasi")
        .arg("inherit-network")
        .arg("--env")
        .arg(format!("RUNTARA_HTTP_URL=http://{addr}"))
        .arg("--env")
        .arg(format!("RUNTARA_SERVER_ADDR={addr}"))
        .arg("--env")
        .arg(format!("RUNTARA_INSTANCE_ID={workflow_id}"))
        .arg("--env")
        .arg("RUNTARA_TENANT_ID=direct-wasm-execute")
        .arg("--env")
        .arg("RUST_LOG=warn")
        .arg(&compiled.wasm_path)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .output()
        .expect("spawn wasmtime");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = stop_tx.send(());
    let _ = server_handle.join();

    let mut output_json = None;
    let mut error_json = None;
    let mut events = Vec::new();
    for message in capture_rx.try_iter() {
        match message {
            CapturedMessage::Completed(completed) => output_json = Some(completed.output_json),
            CapturedMessage::Failed(failed) => error_json = Some(failed.error_json),
            CapturedMessage::Event(event) => events.push(event),
        }
    }
    CapturedRun {
        output_json,
        error_json,
        events,
        status_success: output.status.success(),
        stderr: stderr.into_owned(),
    }
}

#[test]
fn direct_wasm_execute_finish_passthrough_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-finish-passthrough",
        SIMPLE_PASSTHROUGH,
        br#"{"input":"direct-finish"}"#,
    );

    assert_eq!(output, serde_json::json!({ "result": "direct-finish" }));
}

#[test]
fn direct_wasm_execute_finish_passthrough_track_events_emits_step_debug_events() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let result = run_direct_workflow_with_events_and_tracking(
        &components_dir,
        "direct-wasm-execute-finish-track-events",
        SIMPLE_PASSTHROUGH,
        br#"{"input":"direct-finish"}"#,
        true,
    );

    assert_eq!(
        result.output_json,
        serde_json::json!({ "result": "direct-finish" })
    );
    assert_eq!(result.events.len(), 2);

    let start = &result.events[0];
    assert_eq!(start.subtype, "step_debug_start");
    assert_eq!(start.payload_json["step_id"], "finish");
    assert_eq!(start.payload_json["step_name"], Value::Null);
    assert_eq!(start.payload_json["step_type"], "Finish");
    assert_eq!(start.payload_json["scope_id"], Value::Null);
    assert_eq!(start.payload_json["parent_scope_id"], Value::Null);
    assert_eq!(start.payload_json["loop_indices"], serde_json::json!([]));
    assert_eq!(
        start.payload_json["inputs"],
        serde_json::json!({ "finishing": true })
    );
    assert_eq!(
        start.payload_json["input_mapping"],
        serde_json::json!({
            "result": {
                "valueType": "reference",
                "value": "data.input"
            }
        })
    );
    assert!(
        start.payload_json["timestamp_ms"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );

    let end = &result.events[1];
    assert_eq!(end.subtype, "step_debug_end");
    assert_eq!(end.payload_json["step_id"], "finish");
    assert_eq!(
        end.payload_json["outputs"],
        serde_json::json!({
            "stepId": "finish",
            "stepName": "Finish",
            "stepType": "Finish",
            "outputs": {
                "result": "direct-finish"
            }
        })
    );
    assert!(
        end.payload_json["duration_ms"]
            .as_i64()
            .is_some_and(|value| value >= 0)
    );
}

#[test]
fn direct_wasm_execute_conditional_finish_branches_report_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let true_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-conditional-true",
        CONDITIONAL_WORKFLOW,
        br#"{"flag":true}"#,
    );
    assert_eq!(true_output, serde_json::json!({ "result": "yes" }));

    let false_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-conditional-false",
        CONDITIONAL_WORKFLOW,
        br#"{"flag":false}"#,
    );
    assert_eq!(false_output, serde_json::json!({ "result": "no" }));
}

#[test]
fn direct_wasm_execute_nested_conditional_branches_report_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let true_true_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-nested-true-true",
        CONDITIONAL_NESTED,
        br#"{"flag":true,"kind":"a"}"#,
    );
    assert_eq!(
        true_true_output,
        serde_json::json!({ "result": "flag-kind-a" })
    );

    let true_false_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-nested-true-false",
        CONDITIONAL_NESTED,
        br#"{"flag":true,"kind":"b"}"#,
    );
    assert_eq!(
        true_false_output,
        serde_json::json!({ "result": "flag-kind-other" })
    );

    let false_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-nested-false",
        CONDITIONAL_NESTED,
        br#"{"flag":false,"kind":"a"}"#,
    );
    assert_eq!(false_output, serde_json::json!({ "result": "flag-false" }));
}

#[test]
fn direct_wasm_execute_group_by_finish_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-group-by",
        GROUP_BY_SIMPLE,
        br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"inactive"},{"id":3,"status":"active"}]}"#,
    );

    assert_eq!(
        output,
        serde_json::json!({
            "groups": {
                "active": [
                    { "id": 1, "status": "active" },
                    { "id": 3, "status": "active" }
                ],
                "inactive": [
                    { "id": 2, "status": "inactive" }
                ]
            },
            "counts": {
                "active": 2,
                "inactive": 1
            },
            "total_groups": 2
        })
    );
}

#[test]
fn direct_wasm_execute_filter_finish_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-filter",
        FILTER_SIMPLE,
        br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"failed"},{"id":3,"status":"active"}]}"#,
    );

    assert_eq!(
        output,
        serde_json::json!({
            "filtered": [
                { "id": 1, "status": "active" },
                { "id": 3, "status": "active" }
            ],
            "count": 2
        })
    );
}

#[test]
fn direct_wasm_execute_value_switch_finish_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-value-switch",
        SWITCH_VALUE_SIMPLE,
        br#"{"status":"active"}"#,
    );

    assert_eq!(
        output,
        serde_json::json!({
            "bucket": "ready",
            "echo": "active"
        })
    );
}

#[test]
fn direct_wasm_execute_routing_switch_finish_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let active_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-routing-switch-active",
        SWITCH_ROUTING_SIMPLE,
        br#"{"status":"active"}"#,
    );
    assert_eq!(
        active_output,
        serde_json::json!({
            "path": "active",
            "bucket": "ready",
            "echo": "active",
            "route": "active"
        })
    );

    let default_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-routing-switch-default",
        SWITCH_ROUTING_SIMPLE,
        br#"{"status":"done"}"#,
    );
    assert_eq!(
        default_output,
        serde_json::json!({
            "path": "default",
            "bucket": "other",
            "route": "default"
        })
    );
}

#[test]
fn direct_wasm_execute_log_finish_emits_events_and_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-log",
        LOG_ALL_LEVELS,
        br#"{"message":"hello"}"#,
    );

    assert_eq!(result.output_json, serde_json::json!({ "logsEmitted": 4 }));
    assert_eq!(result.events.len(), 4);

    let debug = &result.events[0];
    assert_eq!(debug.subtype, "workflow_log");
    assert_eq!(debug.payload_json["step_id"], "log_debug");
    assert_eq!(debug.payload_json["level"], "debug");
    assert_eq!(debug.payload_json["message"], "Debug level message");
    assert_eq!(
        debug.payload_json["context"],
        serde_json::json!({
            "debugData": { "message": "hello" }
        })
    );
    assert!(
        debug.payload_json["timestamp_ms"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );

    assert_eq!(result.events[1].payload_json["level"], "info");
    assert_eq!(
        result.events[1].payload_json["context"],
        serde_json::json!({ "infoData": "hello" })
    );
    assert_eq!(result.events[2].payload_json["level"], "warn");
    assert_eq!(
        result.events[2].payload_json["context"],
        serde_json::json!({ "warningReason": "potential_issue" })
    );
    assert_eq!(result.events[3].payload_json["level"], "error");
    assert_eq!(
        result.events[3].payload_json["context"],
        serde_json::json!({
            "errorCode": "E001",
            "errorDescription": "Sample error for testing"
        })
    );
}

#[test]
fn direct_wasm_execute_error_entry_emits_event_and_reports_failure() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let result = run_direct_workflow_expect_failure(
        &components_dir,
        "direct-wasm-execute-error",
        ERROR_DIRECT_SIMPLE,
        br#"{"requestId":"req-123"}"#,
    );

    assert_eq!(
        result.error_json,
        serde_json::json!({
            "stepId": "fail",
            "stepName": "Fail Fast",
            "category": "permanent",
            "code": "DIRECT_FAILURE",
            "message": "Direct workflow failure",
            "severity": "critical",
            "context": {
                "requestId": "req-123",
                "reason": "fixture"
            }
        })
    );
    assert_eq!(result.events.len(), 1);
    let event = &result.events[0];
    assert_eq!(event.subtype, "workflow_error");
    assert_eq!(event.payload_json["step_id"], "fail");
    assert_eq!(event.payload_json["step_name"], "Fail Fast");
    assert_eq!(event.payload_json["category"], "permanent");
    assert_eq!(event.payload_json["code"], "DIRECT_FAILURE");
    assert_eq!(event.payload_json["message"], "Direct workflow failure");
    assert_eq!(event.payload_json["severity"], "critical");
    assert_eq!(
        event.payload_json["context"],
        serde_json::json!({
            "requestId": "req-123",
            "reason": "fixture"
        })
    );
    assert!(
        event.payload_json["timestamp_ms"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );
}

#[test]
fn direct_wasm_execute_edge_condition_priority_and_default_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let vip_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-edge-condition-vip",
        EDGE_CONDITION_PRIORITY,
        br#"{"status":"active","tier":"vip"}"#,
    );
    assert_eq!(
        vip_output,
        serde_json::json!({ "path": "vip", "status": "active" })
    );

    let active_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-edge-condition-active",
        EDGE_CONDITION_PRIORITY,
        br#"{"status":"active","tier":"basic"}"#,
    );
    assert_eq!(
        active_output,
        serde_json::json!({ "path": "active", "status": "active" })
    );

    let default_output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-edge-condition-default",
        EDGE_CONDITION_PRIORITY,
        br#"{"status":"inactive","tier":"basic"}"#,
    );
    assert_eq!(
        default_output,
        serde_json::json!({ "path": "default", "status": "inactive" })
    );
}
