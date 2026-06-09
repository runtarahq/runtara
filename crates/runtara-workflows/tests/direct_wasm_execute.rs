//! Direct Wasm execution smoke test.
//!
//! Gated by `RUNTARA_RUN_DIRECT_WASM_E2E=1` because it needs prebuilt shared
//! workflow components, `wac`, and `wasmtime`.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use runtara_workflows::direct_wasm::{
    DIRECT_SHARED_COMPONENT_REQUIREMENTS, DirectArtifactMetadata, DirectCompilationInput,
    compile_direct_workflow, compile_direct_workflow_composed, compose_direct_workflow,
};
use runtara_workflows::{
    CompilationInput, DirectWorkflowCompileOptions, ExecutionGraph, WorkflowCompilerMode,
    compile_workflow_direct,
};
use serde_json::Value;

const SIMPLE_PASSTHROUGH: &str = include_str!("fixtures/simple_passthrough.json");
const CONDITIONAL_WORKFLOW: &str = include_str!("fixtures/conditional_workflow.json");
const CONDITIONAL_NESTED: &str = include_str!("fixtures/conditional_nested.json");
const FILTER_SIMPLE: &str = include_str!("fixtures/filter_simple.json");
const SWITCH_VALUE_SIMPLE: &str = include_str!("fixtures/switch_value_simple.json");
const SWITCH_ROUTING_SIMPLE: &str = include_str!("fixtures/switch_routing_simple.json");
const GROUP_BY_SIMPLE: &str = include_str!("fixtures/group_by_simple.json");
const DELAY_DYNAMIC: &str = include_str!("fixtures/delay_dynamic.json");
const LOG_ALL_LEVELS: &str = include_str!("fixtures/log_all_levels.json");
const ERROR_DIRECT_SIMPLE: &str = include_str!("fixtures/error_direct_simple.json");
const EDGE_CONDITION_PRIORITY: &str = include_str!("fixtures/edge_condition_priority.json");
const WHILE_DIRECT_INDEX_ONLY: &str = include_str!("fixtures/while_direct_index_only.json");
const WHILE_TIMEOUT: &str = include_str!("fixtures/while_timeout.json");
const SPLIT_TIMEOUT: &str = include_str!("fixtures/split_timeout.json");
const AGENT_CACHED_REPLAY: &str = r#"{
  "durable": true,
  "steps": {
    "agent": {
      "stepType": "Agent",
      "id": "agent",
      "name": "Return Cached Value",
      "agentId": "utils",
      "capabilityId": "return-input",
      "maxRetries": 0,
      "inputMapping": {
        "value": { "valueType": "reference", "value": "data.value" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "result": { "valueType": "reference", "value": "steps.agent.outputs" }
      }
    }
  },
  "entryPoint": "agent",
  "executionPlan": [
    { "fromStep": "agent", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// Resolves `data.*` and `variables.*` references in a single Finish step. The
/// canonical input envelope is `{"data": {...}, "variables": {...}}`; `data.tpl`
/// must resolve against the inner `data`, declared variables must resolve to
/// their VALUE (not the `{type, value}` declaration struct), and runtime
/// `variables` must override the declared default.
const ENVELOPE_DATA_AND_VARS: &str = r#"{
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "d":          { "valueType": "reference", "value": "data.tpl" },
        "v_override": { "valueType": "reference", "value": "variables.greeting" },
        "v_default":  { "valueType": "reference", "value": "variables.mood" }
      }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {
    "greeting": { "type": "string", "value": "DEFAULT" },
    "mood":     { "type": "string", "value": "happy" }
  },
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// A single Agent step with no Finish and no edges — the agent is both entry
/// point and terminal. Compiles via an implicit finish; the workflow output is
/// `null` (matching the generated compiler).
const SINGLE_AGENT_NO_FINISH: &str = r#"{
  "steps": {
    "agent": {
      "stepType": "Agent",
      "id": "agent",
      "name": "Random Double",
      "agentId": "utils",
      "capabilityId": "random-double",
      "maxRetries": 1,
      "retryDelay": 1000
    }
  },
  "entryPoint": "agent",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// A chain of two Agent steps with no Finish: the first flows into the second
/// (`next` edge) and the second is terminal. Both agents run; with no Finish the
/// workflow completes with a `null` output via the implicit finish.
const AGENT_CHAIN_NO_FINISH: &str = r#"{
  "steps": {
    "first": {
      "stepType": "Agent",
      "id": "first",
      "name": "Random Double",
      "agentId": "utils",
      "capabilityId": "random-double",
      "maxRetries": 1,
      "retryDelay": 1000
    },
    "second": {
      "stepType": "Agent",
      "id": "second",
      "name": "Random Double Again",
      "agentId": "utils",
      "capabilityId": "random-double",
      "maxRetries": 1,
      "retryDelay": 1000
    }
  },
  "entryPoint": "first",
  "executionPlan": [
    { "fromStep": "first", "toStep": "second", "label": "next" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// Unconditional fan-out that re-converges at a single terminal merge step with
/// no Finish: `start` fans out to `left` and `right`, both flow into `join`, and
/// `join` is terminal. All four agents run; the merge completes the workflow with
/// a `null` output via the implicit finish.
const FANOUT_DIAMOND_NO_FINISH: &str = r#"{
  "steps": {
    "start": {
      "stepType": "Agent", "id": "start", "name": "Start",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 1, "retryDelay": 1000
    },
    "left": {
      "stepType": "Agent", "id": "left", "name": "Left",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 1, "retryDelay": 1000
    },
    "right": {
      "stepType": "Agent", "id": "right", "name": "Right",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 1, "retryDelay": 1000
    },
    "join": {
      "stepType": "Agent", "id": "join", "name": "Join",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 1, "retryDelay": 1000
    }
  },
  "entryPoint": "start",
  "executionPlan": [
    { "fromStep": "start", "toStep": "left" },
    { "fromStep": "start", "toStep": "right" },
    { "fromStep": "left", "toStep": "join" },
    { "fromStep": "right", "toStep": "join" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

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
struct SleepRequest {
    checkpoint_id: String,
    duration_ms: u64,
    state: Vec<u8>,
}

#[derive(Debug)]
struct CheckpointRequest {
    checkpoint_id: String,
    state: Vec<u8>,
}

#[derive(Debug)]
struct DirectRunOutput {
    output_json: Value,
    events: Vec<RuntimeEvent>,
    sleeps: Vec<SleepRequest>,
    checkpoints: Vec<CheckpointRequest>,
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
    sleeps: Vec<SleepRequest>,
    checkpoints: Vec<CheckpointRequest>,
    status_success: bool,
    stderr: String,
}

#[derive(Debug)]
enum CapturedMessage {
    Completed(Completed),
    Failed(Failed),
    Event(RuntimeEvent),
    Sleep(SleepRequest),
    Checkpoint(CheckpointRequest),
}

#[derive(Debug, Default)]
struct ServerState {
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
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
        let stdlib_wasm = dir.join("runtara_workflow_stdlib.wasm");
        let Ok(stdlib_bytes) = std::fs::read(&stdlib_wasm) else {
            eprintln!(
                "SKIP: direct shared workflow stdlib component is not readable: {:?}",
                stdlib_wasm
            );
            return None;
        };
        let required_stdlib_markers: &[&[u8]] = &[
            b"split-cache-key",
            b"embed-workflow-cache-key",
            b"embed-workflow-variables",
            b"embed-workflow-result",
            b"embed-workflow-output-from-result",
            b"embed-workflow-error",
        ];
        if !required_stdlib_markers.iter().all(|marker| {
            stdlib_bytes
                .windows(marker.len())
                .any(|window| window == *marker)
        }) {
            eprintln!(
                "SKIP: direct shared workflow stdlib component is stale: {:?}",
                stdlib_wasm
            );
            return None;
        }
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
    server_state: &ServerState,
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

    let (status, response_json) = route(&method, &path, &body, sink, server_state, workflow_input);
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
    server_state: &ServerState,
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
            ("POST", "checkpoint") => return checkpoint_response(body, sink, server_state),
            ("POST", "sleep") => {
                capture_sleep(body, sink);
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

fn checkpoint_response(
    body: &[u8],
    sink: &mpsc::Sender<CapturedMessage>,
    server_state: &ServerState,
) -> (u16, Value) {
    let Ok(parsed) = serde_json::from_slice::<Value>(body) else {
        return (
            400,
            serde_json::json!({
                "found": false,
                "state": null,
                "signal": null,
                "custom_signal": null,
            }),
        );
    };

    let checkpoint_id = parsed
        .get("checkpoint_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = parsed
        .get("state")
        .and_then(Value::as_str)
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .unwrap_or_default();
    let _ = sink.send(CapturedMessage::Checkpoint(CheckpointRequest {
        checkpoint_id: checkpoint_id.clone(),
        state: state.clone(),
    }));

    let mut checkpoints = server_state
        .checkpoints
        .lock()
        .expect("checkpoint state lock");
    if let Some(existing) = checkpoints.get(&checkpoint_id) {
        return (
            200,
            serde_json::json!({
                "found": true,
                "state": base64::engine::general_purpose::STANDARD.encode(existing),
                "signal": null,
                "custom_signal": null,
            }),
        );
    }

    if !state.is_empty() {
        checkpoints.insert(checkpoint_id, state);
    }

    (
        200,
        serde_json::json!({
            "found": false,
            "state": null,
            "signal": null,
            "custom_signal": null,
        }),
    )
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

fn capture_sleep(body: &[u8], sink: &mpsc::Sender<CapturedMessage>) {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body)
        && let Some(checkpoint_id) = parsed.get("checkpoint_id").and_then(Value::as_str)
    {
        let duration_ms = parsed
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let state = parsed
            .get("state")
            .and_then(Value::as_str)
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
            .unwrap_or_default();
        let _ = sink.send(CapturedMessage::Sleep(SleepRequest {
            checkpoint_id: checkpoint_id.to_string(),
            duration_ms,
            state,
        }));
    }
}

fn serve(
    listener: TcpListener,
    sink: mpsc::Sender<CapturedMessage>,
    server_state: Arc<ServerState>,
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
                let server_state = server_state.clone();
                let workflow_input = workflow_input.clone();
                thread::spawn(move || {
                    while let Ok(true) =
                        handle_request(&mut stream, &sink, &server_state, workflow_input.as_slice())
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
        sleeps: captured.sleeps,
        checkpoints: captured.checkpoints,
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
    run_direct_workflow_capture_with_preloaded_checkpoints(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        track_events,
        Vec::new(),
    )
}

fn run_direct_workflow_capture_with_preloaded_checkpoints(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
) -> CapturedRun {
    let temp = tempfile::tempdir().expect("tempdir");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let compiled = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events,
            agent_catalog: None,
            connection_integration_ids: std::collections::HashMap::new(),
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
    let server_state = Arc::new(ServerState {
        checkpoints: Mutex::new(preloaded_checkpoints.into_iter().collect()),
    });
    let server_handle =
        thread::spawn(move || serve(listener, capture_tx, server_state, stop_rx, workflow_input));

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
    let mut sleeps = Vec::new();
    let mut checkpoints = Vec::new();
    for message in capture_rx.try_iter() {
        match message {
            CapturedMessage::Completed(completed) => output_json = Some(completed.output_json),
            CapturedMessage::Failed(failed) => error_json = Some(failed.error_json),
            CapturedMessage::Event(event) => events.push(event),
            CapturedMessage::Sleep(sleep) => sleeps.push(sleep),
            CapturedMessage::Checkpoint(checkpoint) => checkpoints.push(checkpoint),
        }
    }
    CapturedRun {
        output_json,
        error_json,
        events,
        sleeps,
        checkpoints,
        status_success: output.status.success(),
        stderr: stderr.into_owned(),
    }
}

fn non_durable_graph_json(graph_json: &str) -> String {
    let mut graph: Value = serde_json::from_str(graph_json).expect("fixture parses as json");
    graph["durable"] = Value::Bool(false);
    serde_json::to_string(&graph).expect("graph serializes")
}

#[test]
fn direct_compile_entry_returns_native_result_shape_when_components_available() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let graph: ExecutionGraph = serde_json::from_str(SIMPLE_PASSTHROUGH).expect("fixture parses");
    let compiled = compile_workflow_direct(
        CompilationInput {
            tenant_id: "direct-entry".to_string(),
            workflow_id: "native-result-shape".to_string(),
            version: 9,
            execution_graph: graph,
            track_events: false,
            child_workflows: vec![],
            connection_service_url: None,
            agent_catalog: None,
            progress_callback: None,
            connection_integration_ids: std::collections::HashMap::new(),
        },
        DirectWorkflowCompileOptions {
            output_dir: temp.path().to_path_buf(),
            components_dir,
            source_checksum: Some("source-sha256".to_string()),
        },
    )
    .expect("direct compile entry succeeds");

    assert_eq!(
        compiled.binary_path,
        compiled.build_dir.join("workflow.wasm")
    );
    assert!(compiled.binary_path.exists(), "compiled wasm missing");
    assert_eq!(
        compiled.binary_size as u64,
        fs::metadata(&compiled.binary_path)
            .expect("compiled wasm metadata")
            .len()
    );
    assert_eq!(compiled.binary_checksum.len(), 64);
    assert!(compiled.package_size > 0);
    assert!(!compiled.has_side_effects);
    assert!(compiled.child_dependencies.is_empty());
    assert_eq!(compiled.default_variables, serde_json::json!({}));
    assert_eq!(compiled.compiler_mode, WorkflowCompilerMode::DirectWasm);

    let metadata: DirectArtifactMetadata = serde_json::from_slice(
        &fs::read(compiled.build_dir.join("artifact-metadata.json")).expect("artifact metadata"),
    )
    .expect("metadata parses");
    assert_eq!(metadata.source_checksum.as_deref(), Some("source-sha256"));
    assert!(metadata.composed_wasm.is_some());
}

#[test]
fn direct_compile_measures_json_to_ready_bundle_latency() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    // Time the full direct-emitter path split into its two phases:
    //   1. emit   — JSON string -> parsed graph -> emitted workflow-logic.wasm
    //   2. compose — read shared components + in-process wac-graph composition
    // Set `RUST_LOG=runtara::direct_compile::profile=debug` for the per-substep
    // breakdown inside compose (dep read / parse / resolve / encode+validate).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "runtara::direct_compile::profile=debug".into()),
        )
        .with_test_writer()
        .try_init();

    let parse_start = Instant::now();
    let graph: ExecutionGraph = serde_json::from_str(SIMPLE_PASSTHROUGH).expect("fixture parses");
    let parse_elapsed = parse_start.elapsed();

    let temp = tempfile::tempdir().expect("tempdir");

    let emit_start = Instant::now();
    let mut result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "json-to-bundle-latency".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        connection_integration_ids: std::collections::HashMap::new(),
    })
    .expect("direct emit succeeds");
    let emit_elapsed = emit_start.elapsed();

    let compose_start = Instant::now();
    compose_direct_workflow(&mut result, &components_dir).expect("direct compose succeeds");
    let compose_elapsed = compose_start.elapsed();

    let total_elapsed = parse_elapsed + emit_elapsed + compose_elapsed;

    assert!(result.wasm_path.exists(), "composed wasm missing");
    assert!(result.wasm_size > 0, "composed wasm is empty");

    // Surface the breakdown; `cargo test -- --nocapture` prints it.
    eprintln!(
        "direct compile latency (simple_passthrough): parse={:.3}ms emit={:.3}ms compose={:.3}ms total={:.3}ms -> {} bytes",
        parse_elapsed.as_secs_f64() * 1000.0,
        emit_elapsed.as_secs_f64() * 1000.0,
        compose_elapsed.as_secs_f64() * 1000.0,
        total_elapsed.as_secs_f64() * 1000.0,
        result.wasm_size,
    );
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
fn direct_wasm_execute_single_agent_without_finish_returns_null() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    // The agent runs (random-double), but with no Finish step the workflow
    // completes with a null output, matching the generated compiler.
    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-single-agent-no-finish",
        SINGLE_AGENT_NO_FINISH,
        br#"{}"#,
    );

    assert_eq!(output, Value::Null);
}

#[test]
fn direct_wasm_execute_agent_chain_without_finish_returns_null() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    // Both agents run in sequence; with no Finish step the workflow completes
    // with a null output via the implicit finish, matching the generated
    // compiler's finish-output fallback.
    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-agent-chain-no-finish",
        AGENT_CHAIN_NO_FINISH,
        br#"{}"#,
    );

    assert_eq!(output, Value::Null);
}

#[test]
fn direct_wasm_execute_fanout_diamond_without_finish_returns_null() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    // The fan-out re-converges at `join`; all four agents run and the merge
    // completes the workflow with a null output via the implicit finish. Proves
    // a diamond with no Finish lowers and executes end-to-end (not just that the
    // support gate accepts it).
    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-fanout-diamond-no-finish",
        FANOUT_DIAMOND_NO_FINISH,
        br#"{}"#,
    );

    assert_eq!(output, Value::Null);
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
fn direct_wasm_execute_while_loop_reports_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-while-loop",
        WHILE_DIRECT_INDEX_ONLY,
        br#"{"count":3}"#,
    );

    assert_eq!(
        result.output_json,
        serde_json::json!({
            "iterations": 3,
            "last": {
                "iteration": 2,
                "loopIndex": 2,
                "indices": [2],
                "previous": {
                    "iteration": 1,
                    "loopIndex": 1,
                    "indices": [1],
                    "previous": {
                        "iteration": 0,
                        "loopIndex": 0,
                        "indices": [0],
                        "previous": null
                    }
                }
            }
        })
    );
    assert!(
        result.sleeps.is_empty(),
        "normal While execution should not use durable sleep"
    );
    assert!(
        result.checkpoints.is_empty(),
        "normal While execution should not use durable checkpoints"
    );
}

#[test]
fn direct_wasm_execute_while_timeout_fails_with_timeout_error() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    // The 50ms per-iteration body delay drives the loop past its 10ms timeout, so
    // the While step fails with the static WHILE_TIMEOUT payload. Generated Rust
    // parses but does not enforce the timeout; this proves direct mode honors the
    // documented "if exceeded, step fails" behavior at runtime.
    let result = run_direct_workflow_expect_failure(
        &components_dir,
        "direct-wasm-execute-while-timeout",
        WHILE_TIMEOUT,
        br#"{}"#,
    );

    assert_eq!(
        result.error_json,
        serde_json::json!({
            "code": "WHILE_TIMEOUT",
            "message": "While step exceeded its configured timeout",
            "category": "timeout",
            "severity": "error"
        })
    );
}

#[test]
fn direct_wasm_execute_split_timeout_fails_with_timeout_error() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    // The 50ms per-item body delay drives the sequential Split past its 10ms
    // timeout, so the Split fails hard with the static SPLIT_TIMEOUT payload
    // before processing all items.
    let result = run_direct_workflow_expect_failure(
        &components_dir,
        "direct-wasm-execute-split-timeout",
        SPLIT_TIMEOUT,
        br#"{"items":[{"v":1},{"v":2},{"v":3}]}"#,
    );

    assert_eq!(
        result.error_json,
        serde_json::json!({
            "code": "SPLIT_TIMEOUT",
            "message": "Split step exceeded its configured timeout",
            "category": "timeout",
            "severity": "error"
        })
    );
}

#[test]
fn direct_wasm_execute_durable_delay_reports_sleep_and_completion() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-delay-durable",
        DELAY_DYNAMIC,
        br#"{"waitTime":0}"#,
    );

    assert_eq!(result.output_json, serde_json::json!({ "waited": 0 }));
    assert_eq!(result.sleeps.len(), 1);
    let sleep = &result.sleeps[0];
    assert_eq!(sleep.checkpoint_id, "delay");
    assert_eq!(sleep.duration_ms, 0);
    assert!(sleep.state.is_empty());
    assert!(result.checkpoints.is_empty());
}

#[test]
fn direct_wasm_execute_non_durable_delay_reports_completion_without_sleep() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };
    let graph_json = non_durable_graph_json(DELAY_DYNAMIC);

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-delay-non-durable",
        &graph_json,
        br#"{"waitTime":0}"#,
    );

    assert_eq!(result.output_json, serde_json::json!({ "waited": 0 }));
    assert!(
        result.sleeps.is_empty(),
        "non-durable Delay should not call runtime durable sleep"
    );
    assert!(result.checkpoints.is_empty());
}

#[test]
fn direct_wasm_execute_durable_agent_invokes_and_saves_checkpoint() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };
    let workflow_id = "direct-wasm-execute-agent-fresh-checkpoint";
    let checkpoint_id = format!("{workflow_id}::agent::utils::return-input::agent");

    let result = run_direct_workflow_with_events(
        &components_dir,
        workflow_id,
        AGENT_CACHED_REPLAY,
        br#"{"value":"fresh-agent"}"#,
    );

    assert_eq!(
        result.output_json,
        serde_json::json!({ "result": "fresh-agent" })
    );
    assert_eq!(result.checkpoints.len(), 2);
    let lookup = &result.checkpoints[0];
    assert_eq!(lookup.checkpoint_id, checkpoint_id);
    assert!(
        lookup.state.is_empty(),
        "fresh durable Agent should first perform a read-only checkpoint lookup"
    );
    let save = &result.checkpoints[1];
    assert_eq!(save.checkpoint_id, checkpoint_id);
    assert_eq!(save.state, br#""fresh-agent""#);
    assert!(
        result.sleeps.is_empty(),
        "successful durable Agent should not use durable sleep without retries"
    );
}

#[test]
fn direct_wasm_execute_non_durable_agent_invokes_without_checkpoint() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };
    let graph_json = non_durable_graph_json(AGENT_CACHED_REPLAY);

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-agent-non-durable",
        &graph_json,
        br#"{"value":"fresh-agent"}"#,
    );

    assert_eq!(
        result.output_json,
        serde_json::json!({ "result": "fresh-agent" })
    );
    assert!(
        result.checkpoints.is_empty(),
        "non-durable Agent should not call runtime checkpoint APIs"
    );
    assert!(
        result.sleeps.is_empty(),
        "non-durable successful Agent should not sleep"
    );
}

#[test]
fn direct_wasm_execute_resolves_data_reference_from_canonical_envelope() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };
    // Regression: the workflow start input is the canonical envelope
    // `{"data": {...}, "variables": {...}}`, stored verbatim as the instance
    // input. A `data.*` reference must resolve against the inner `data` payload
    // and reach the agent. Previously the whole envelope was used as `data`, so
    // `data.value` resolved to null and the agent received null. (The existing
    // agent tests pass BARE data, which is why they never caught this.)
    let graph_json = non_durable_graph_json(AGENT_CACHED_REPLAY);
    let result = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-envelope-data",
        &graph_json,
        br#"{"data":{"value":"enveloped-data"},"variables":{}}"#,
    );
    assert_eq!(result, serde_json::json!({ "result": "enveloped-data" }));
}

#[test]
fn direct_wasm_execute_resolves_variables_from_envelope_and_defaults() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };
    // Regression: `variables.*` references must resolve to the declared
    // variable's VALUE (not the `{type, value}` declaration struct), and the
    // canonical envelope's runtime `variables` must override declared defaults.
    // `data.*` from the same envelope is resolved alongside.
    let result = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-envelope-vars",
        ENVELOPE_DATA_AND_VARS,
        br#"{"data":{"tpl":"DATAVAL"},"variables":{"greeting":"OVERRIDDEN"}}"#,
    );
    assert_eq!(
        result,
        serde_json::json!({
            "d": "DATAVAL",
            "v_override": "OVERRIDDEN",
            "v_default": "happy"
        })
    );
}

#[test]
fn direct_wasm_execute_durable_agent_uses_cached_checkpoint() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };
    let workflow_id = "direct-wasm-execute-agent-cached-replay";
    let checkpoint_id = format!("{workflow_id}::agent::utils::return-input::agent");

    let captured = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        workflow_id,
        AGENT_CACHED_REPLAY,
        br#"{"value":"fresh-agent"}"#,
        false,
        vec![(checkpoint_id.clone(), br#""cached-agent""#.to_vec())],
    );

    assert!(
        captured.status_success,
        "wasmtime exited non-zero:\n--- stderr ---\n{}",
        captured.stderr
    );
    assert_eq!(
        captured
            .output_json
            .expect("direct workflow should complete from cached Agent output"),
        serde_json::json!({ "result": "cached-agent" })
    );
    assert_eq!(captured.checkpoints.len(), 1);
    let checkpoint = &captured.checkpoints[0];
    assert_eq!(checkpoint.checkpoint_id, checkpoint_id);
    assert!(
        checkpoint.state.is_empty(),
        "cached Agent replay should only perform the read-only checkpoint lookup"
    );
    assert!(
        captured.sleeps.is_empty(),
        "cached Agent replay should not use durable sleep"
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

// ===========================================================================
// Tier B — fixture execution smoke battery.
//
// Replaces the behavioral half of the deleted A/B parity suite: every fixture
// listed here is composed and run end-to-end under wasmtime, and we assert it
// reaches its expected terminal outcome (completes / fails / sleeps). Pure
// control-flow fixtures are driven with a minimal input; the exact branch
// taken doesn't matter — only that the workflow reaches the expected terminus.
// Gated on the same prerequisites as the rest of this file
// (`RUNTARA_RUN_DIRECT_WASM_E2E=1` + wac + wasmtime + staged components).
//
// AI-agent, embed/child-workflow, and signal-suspension fixtures are NOT here:
// driving them needs bespoke LLM/child/signal mocks. They are covered
// structurally by the Tier A battery in `fixture_smoke.rs` and, where they
// execute, by the dedicated tests above.
// ===========================================================================

#[derive(Clone, Copy, Debug)]
enum ExpectedOutcome {
    /// Reaches a Finish step and POSTs `/completed`.
    Completes,
    /// Returns a failed `wasi:cli/run` result and POSTs `/failed`.
    Fails,
    /// Durable Delay: POSTs `/sleep` and then completes.
    Sleeps,
}

struct SmokeCase {
    fixture: &'static str,
    input: &'static [u8],
    expect: ExpectedOutcome,
}

const EXECUTION_SMOKE_CASES: &[SmokeCase] = &[
    // --- Completes: pure control flow -------------------------------------
    SmokeCase {
        fixture: "simple_passthrough",
        input: br#"{"input":"x"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "conditional_workflow",
        input: br#"{"flag":true}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "conditional_nested",
        input: br#"{"flag":true,"kind":"a"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "conditional_diamond",
        input: br#"{"flag":true}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "conditional_diamond_asymmetric",
        input: br#"{"flag":true,"urgent":false}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "conditional_length_comparison",
        input: br#"{"description":"hello world this is a long description"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "edge_condition_priority",
        input: br#"{"status":"active","tier":"gold"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "edge_condition_diamond",
        input: br#"{"tier":"gold"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "filter_simple",
        input: br#"{"items":[1,2,3,4,5]}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "filter_complex_condition",
        input: br#"{"users":[{"age":25,"active":true},{"age":17,"active":false}]}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "filter_with_not",
        input: br#"{}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "switch_value_simple",
        input: br#"{"status":"active"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "switch_routing_simple",
        input: br#"{"status":"active"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "group_by_simple",
        input:
            br#"{"items":[{"category":"a","v":1},{"category":"b","v":2},{"category":"a","v":3}]}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "group_by_expected_keys",
        input: br#"{"items":[{"category":"a"},{"category":"b"}]}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "group_by_nested_key",
        input: br#"{"users":[{"profile":{"role":"admin"}},{"profile":{"role":"user"}}]}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "log_no_context",
        input: br#"{}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "log_all_levels",
        input: br#"{"message":"hi"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "while_direct_index_only",
        input: br#"{"count":3}"#,
        expect: ExpectedOutcome::Completes,
    },
    // Transform-agent fixtures (split_*, while_*, log_*, transform_workflow)
    // now execute too — their map-fields input mappings were corrected to the
    // current `source_data` + `mappings` schema. See the section below.
    // --- Fails: explicit error / timeout ----------------------------------
    SmokeCase {
        fixture: "error_direct_simple",
        input: br#"{"requestId":"r1"}"#,
        expect: ExpectedOutcome::Fails,
    },
    // Conditional-routed Error fixtures; inputs steer each to its Error branch
    // (these also exercise the passthrough->return-input composite fix).
    SmokeCase {
        fixture: "error_permanent",
        input: br#"{"resourceId":"res-1","found":false}"#,
        expect: ExpectedOutcome::Fails,
    },
    SmokeCase {
        fixture: "error_transient",
        input: br#"{"success":false}"#,
        expect: ExpectedOutcome::Fails,
    },
    SmokeCase {
        fixture: "error_with_context",
        input: br#"{"orderId":"o-1","amount":5000}"#,
        expect: ExpectedOutcome::Fails,
    },
    SmokeCase {
        fixture: "error_all_categories",
        input: br#"{"errorType":"transient"}"#,
        expect: ExpectedOutcome::Fails,
    },
    SmokeCase {
        fixture: "while_timeout",
        input: br#"{}"#,
        expect: ExpectedOutcome::Fails,
    },
    SmokeCase {
        fixture: "split_timeout",
        input: br#"{"items":[1,2,3],"item":1}"#,
        expect: ExpectedOutcome::Fails,
    },
    // --- Sleeps: durable delay --------------------------------------------
    SmokeCase {
        fixture: "delay_simple",
        input: br#"{}"#,
        expect: ExpectedOutcome::Sleeps,
    },
    SmokeCase {
        fixture: "delay_dynamic",
        input: br#"{"waitTime":5}"#,
        expect: ExpectedOutcome::Sleeps,
    },
    // --- transform-agent fixtures (map-fields), now on the corrected schema --
    // These drive their subgraphs/loops through `transform/map-fields`; with
    // the input mappings fixed to `source_data` + `mappings` they execute.
    SmokeCase {
        fixture: "transform_workflow",
        input: br#"{"input_field":"hello"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "split_workflow",
        input: br#"{"items":[{"value":1},{"value":2},{"value":3}]}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "split_parallel_workflow",
        input: br#"{"items":[{"value":1},{"value":2},{"value":3}]}"#,
        expect: ExpectedOutcome::Completes,
    },
    // NOTE: split_with_schemas / split_with_schemas_failing are Tier-A only.
    // Their per-item input/output schemas make the terminal outcome
    // input-specific (a generic item either traps or passes regardless of the
    // "_failing" intent), so they aren't meaningful as input-agnostic smoke.
    // While loops that terminate via `loop.index` against a bound from input.
    SmokeCase {
        fixture: "while_with_loop_index",
        input: br#"{"maxIterations":3}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "while_with_previous_outputs",
        input: br#"{"items":[1,2],"count":2}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "while_max_iterations",
        input: br#"{"value":0}"#,
        expect: ExpectedOutcome::Completes,
    },
    // While loops whose condition reads a constant `steps.init.outputs.*`;
    // seeded so the guard is already false (zero iterations) — exercises
    // condition eval + clean exit without risking a non-terminating loop.
    SmokeCase {
        fixture: "while_simple",
        input: br#"{"counter":5,"target":3}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "while_workflow",
        input: br#"{"counter":5,"target":3}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "while_break_on_first",
        input: br#"{"counter":0,"target":10}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "log_with_context",
        input: br#"{"value":"v","timestamp":"t"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "log_workflow",
        input: br#"{"value":"v"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "log_error_handling",
        input: br#"{"value":"v"}"#,
        expect: ExpectedOutcome::Completes,
    },
    SmokeCase {
        fixture: "log_in_loop",
        input: br#"{"count":3}"#,
        expect: ExpectedOutcome::Completes,
    },
];

fn smoke_fixture_json(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

fn stderr_tail(stderr: &str) -> String {
    let trimmed = stderr.trim();
    let start = trimmed.len().saturating_sub(400);
    trimmed[start..].replace('\n', " | ")
}

#[test]
fn fixture_execution_smoke_battery() {
    let Some(components_dir) = direct_e2e_components_dir() else {
        return;
    };

    let mut failures: Vec<String> = Vec::new();
    for case in EXECUTION_SMOKE_CASES {
        let json = smoke_fixture_json(case.fixture);
        let captured = run_direct_workflow_capture(
            &components_dir,
            &format!("smoke-{}", case.fixture),
            &json,
            case.input,
            false,
        );
        let verdict = match case.expect {
            ExpectedOutcome::Completes => captured.status_success && captured.output_json.is_some(),
            ExpectedOutcome::Fails => !captured.status_success && captured.error_json.is_some(),
            ExpectedOutcome::Sleeps => captured.status_success && !captured.sleeps.is_empty(),
        };
        if !verdict {
            failures.push(format!(
                "  {} [{:?}]: status_success={}, completed={}, failed={}, sleeps={}\n      stderr: {}",
                case.fixture,
                case.expect,
                captured.status_success,
                captured.output_json.is_some(),
                captured.error_json.is_some(),
                captured.sleeps.len(),
                stderr_tail(&captured.stderr),
            ));
        }
    }

    eprintln!("execution smoke: {} cases run", EXECUTION_SMOKE_CASES.len());
    assert!(
        failures.is_empty(),
        "{} execution smoke case(s) did not reach the expected terminal state:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}
